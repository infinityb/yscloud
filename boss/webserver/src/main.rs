use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt::{self, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use std::convert::TryInto;
use std::collections::BTreeMap;

use askama::Template;
use bytes::{Buf, BytesMut};
use byteorder::{BigEndian, ByteOrder};
use clap::{App, Arg};
use hyper::Body;
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Request, Response, Server, StatusCode};
use metrics::counter;
use rand::seq::SliceRandom;
use rand::thread_rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::net::TcpStream;
use tokio_postgres::NoTls;
use tracing::{event, Level};
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

use ksuid::Ksuid;
use webserver::{DirectoryListingEntry, DirectoryListingTemplate, ErrorPageTemplate};
use magnetite_common::TorrentId;
use magnetite_common::proto::{BLOCK_SIZE, BLOCK_SIZE_SHIFT};
use boss_vfs::{FetchContentRootResponse, FetchContentRootQueryProcessor, FetchContentRootQuery, FetchDirectoriesResponse, FetchDirectoriesRequest, FetchDirectoriesQueryProcessor, VfsEntry, VfsEntryData, NoEntityExists};

mod arc;

pub use self::arc::Cache;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

pub const SERVER_NAME: &str = "Magnetite Webserver";
pub const SUBCOMMAND_NAME: &str = "webserver";

#[derive(Clone)]
struct Opts {
    enable_directory_listings: bool,
}

struct Impl {
    roots: BTreeMap<TorrentId, FetchContentRootResponse>,
    inodes: Cache<i64, VfsEntry>,
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let mut my_subscriber_builder = FmtSubscriber::builder();

    let app = App::new(CARGO_PKG_NAME)
        .version(CARGO_PKG_VERSION)
        .author("Stacey Ell <stacey.ell@gmail.com>")
        .about("Magnetite-based webserver")
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        )
        .arg(
            Arg::with_name("bind-address")
                .long("bind-address")
                .value_name("[ADDRESS]")
                .help("The address to bind to")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("enable-directory-listings")
                .long("enable-directory-listings")
                .help("enables directory listings"),
        );

    let matches = app.get_matches();

    let verbosity = matches.occurrences_of("v");
    let should_print_test_logging = 4 < verbosity;

    my_subscriber_builder = my_subscriber_builder.with_max_level(match verbosity {
        0 => TracingLevelFilter::ERROR,
        1 => TracingLevelFilter::WARN,
        2 => TracingLevelFilter::INFO,
        3 => TracingLevelFilter::DEBUG,
        _ => TracingLevelFilter::TRACE,
    });

    tracing::subscriber::set_global_default(my_subscriber_builder.finish())
        .expect("setting tracing default failed");

    if should_print_test_logging {
        print_test_logging();
    }

    let bind_address = matches.value_of("bind-address").unwrap().to_string();

    let opts = Opts {
        enable_directory_listings: matches.is_present("enable-directory-listings"),
    };

    let fs_impl = Arc::new(Mutex::new(Impl {
        roots: Default::default(),
        inodes: Cache::new(256*1024),
    }));

    let instance_id = Uuid::new_v4();
    let make_svc = make_service_fn(move |socket: &AddrStream| {
        let fs_impl = fs_impl.clone();
        let remote_addr = socket.remote_addr();
        let opts = opts.clone();

        async move {
            let service = service_fn(move |req: Request<Body>| {
                let fs_impl = fs_impl.clone();
                let opts = opts.clone();
                async move {
                    let v = service_request(instance_id, fs_impl, remote_addr, opts, req).await;
                    Ok::<_, Infallible>(v)
                }
            });

            Ok::<_, Infallible>(service)
        }
    });

    let sa: SocketAddr = bind_address.parse().unwrap();
    let server = Server::bind(&sa).serve(make_svc);
    event!(Level::INFO, "binding to {}", bind_address);

    server.await?;
    Ok(())
}

struct RequestContext {
    instance_id: Uuid,
    request_id: Ksuid,
    start_instant: Instant,
}

async fn service_request(
    instance_id: Uuid,
    fsi: Arc<Mutex<Impl>>,
    remote_addr: std::net::SocketAddr,
    opts: Opts,
    req: Request<Body>,
) -> Response<Body> {
    let req_ctx = RequestContext {
        instance_id,
        request_id: Ksuid::generate(),
        start_instant: Instant::now(),
    };
    match service_request_helper(&req_ctx, fsi, remote_addr, opts, req).await {
        Ok(resp) => resp,
        Err(err) => {
            if err.downcast_ref::<NoEntityExists>().is_some() {
                return response_http_not_found(&req_ctx);
            }

            if err.downcast_ref::<OutOfRange>().is_some() {
                return repsonse_http_range_not_satisfiable(&req_ctx);
            }

            if err.downcast_ref::<ClientError>().is_some() {
                event!(Level::ERROR, "bad request: {}", err);
                return response_http_bad_request(&req_ctx);
            }

            if err.downcast_ref::<InternalError>().is_some() {
                event!(Level::ERROR, "explicit ISE: {}", err);
                return response_http_internal_server_error(&req_ctx);
            }

            event!(Level::ERROR, "implicit ISE: {}", err);
            response_http_internal_server_error(&req_ctx)
        }
    }
}

fn percent_decode_str(x: &str) -> Result<Cow<str>, failure::Error> {
    use percent_encoding::percent_decode_str;

    let s = percent_decode_str(x).decode_utf8()?;
    Ok(s)
}

async fn service_request_helper(
    req_ctx: &RequestContext,
    fsi: Arc<Mutex<Impl>>,
    _remote_addr: std::net::SocketAddr,
    opts: Opts,
    req: Request<Body>,
) -> Result<Response<Body>, failure::Error>
{
    #[inline]
    pub fn align_floor(v: u32, align_shift: u8) -> u32 {
        let mask: u32 = (1 << align_shift) - 1;
        v & !mask
    }

    let path: Vec<Cow<str>> = req
        .uri()
        .path()
        .split('/')
        .filter(|x| !x.is_empty())
        .map(percent_decode_str)
        .collect::<Result<Vec<Cow<str>>, failure::Error>>()
        ?;

    event!(Level::INFO, "HTTP access {:?}", path.join("/"));
    
    if path.is_empty() {
        return Err(NoEntityExists.into());
    }

    let mut path_ref: Vec<&str> = Vec::with_capacity(path.len());
    for p in &path {
        path_ref.push(p);
    }

    // let (mut client, connection) =
    //     tokio_postgres::connect("host=/run/magnetite-postgresql dbname=magnetite", NoTls).await?;

    let content_key: TorrentId = path[0].parse()?;
    
    let mut path_elements = path[1..].iter();

    let mut path_advance = 1;  // root

    
    let fs = fsi.lock().await;
    let root_resp = if let Some(root_resp) = fs.roots.get(&content_key).cloned() {
        drop(fs);
        root_resp
    } else {
        drop(fs);

        let (mut client, connection) =
            tokio_postgres::connect("host=/tmp/psql dbname=magnetite", NoTls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let tx = client.transaction().await?;

        let root_resp = FetchContentRootQueryProcessor::execute(&tx, &FetchContentRootQuery {
            content_key: &path[0],
        }).await?;


        let mut fs = fsi.lock().await;
        fs.roots.insert(content_key, root_resp.clone());
        root_resp
    };

    let mut fs = fsi.lock().await;

    let mut cur_inode = root_resp.root_inode;
    loop {
        let mut modified = false;
        if let Some(vfs_entry) = fs.inodes.get_refresh(&cur_inode) {
            if let VfsEntryData::Directory(dir_data) = &vfs_entry.data {
                if dir_data.is_complete {
                    if let Some(pe) = path_elements.next() {
                        if let Some(inode) = dir_data.contents.get(&pe[..]) {
                            cur_inode = *inode;
                            path_advance += 1;
                            modified = true;
                        } else {
                            return Err(NoEntityExists.into());
                        }
                    }
                }
            }
        }

        if !modified {
            break;
        }
    }
    let have_final = match fs.inodes.get_norefresh(&cur_inode) {
        Some(fe) => match fe.data {
            VfsEntryData::Directory(ref dir) => dir.is_complete,
            VfsEntryData::Regular(..) => true,
        }
        None => false,
    };
    drop(fs);

    if !have_final {
        let fetch_dir_req = FetchDirectoriesRequest {
            root_inode: cur_inode,
            path: &path_ref[path_advance..],
        };


        let (mut client, connection) =
            tokio_postgres::connect("host=/tmp/psql dbname=magnetite", NoTls).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let tx = client.transaction().await?;
        let fetch_resp: FetchDirectoriesResponse = 
            FetchDirectoriesQueryProcessor::execute(&tx, &fetch_dir_req).await?;
        drop(tx);

        let mut fs = fsi.lock().await;
        for (k, v) in fetch_resp.inodes {
            let mut over_write = true;
            if let Some(v2) = fs.inodes.get_norefresh(&k) {
                if let VfsEntryData::Directory(ref old_dir) = v2.data {
                    if let VfsEntryData::Directory(ref new_dir) = v.data {
                        if old_dir.is_complete && !new_dir.is_complete {
                            // keep old dir - it's complete while the new one 
                            // is not.
                            over_write = false;
                        }
                    }
                }
            }
            if over_write {
                fs.inodes.insert(k, v, None);
            }
        }
        loop {
            let mut modified = false;
            if let Some(vfs_entry) = fs.inodes.get_refresh(&cur_inode) {
                if let VfsEntryData::Directory(dir_data) = &vfs_entry.data {
                    if dir_data.is_complete {
                        if let Some(pe) = path_elements.next() {
                            if let Some(inode) = dir_data.contents.get(&pe[..]) {
                                cur_inode = *inode;
                                path_advance += 1;
                                modified = true;
                            } else {
                                return Err(NoEntityExists.into());
                            }
                        }
                    }
                }
            }
            if !modified {
                break;
            }
        }
    }


    let cur_dir = format!("/{}", path[..].join("/"));

    let fs = fsi.lock().await;
    let fe = match fs.inodes.get_norefresh(&cur_inode) {
        Some(v) => v.clone(),
        None => return Err(NoEntityExists.into()),
    };
    

    let file_length;
    let torrent_global_offset_start;

    match &fe.data {
        VfsEntryData::Directory(dir) => {
            if !req.uri().path().ends_with('/') {
                let uri = format!("{}/", req.uri().path());
                return Ok(response_http_found(req_ctx, &uri));
            }
            if opts.enable_directory_listings {
                let mut dir_data: Vec<(&str, &VfsEntry)> = Vec::new();
                for (key, val) in dir.contents.iter() {
                    if let Some(vfs_entry) = fs.inodes.get_norefresh(val) {
                        dir_data.push((key, vfs_entry));
                    }
                }

                return Ok(response_ok_rendering_directory(
                    &req_ctx, &cur_dir, path_ref[1..].is_empty(), &dir_data[..]));
            } else {
                return Err(NoEntityExists.into());
            }
        }
        VfsEntryData::Regular(ref reg) => {
            file_length = reg.file_length;
            torrent_global_offset_start = reg.global_offset;
        }
    };
    drop(fs);


    // let content_info = fs
    //     .content_info
    //     .get_content_info(&content_key)
    //     .ok_or(InternalError {
    //         msg: "unknown content key",
    //     })?;

    const BOUNDARY_LENGTH: usize = 60;
    const BOUNDARY_CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

    let mut builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .header(hyper::header::ACCEPT_RANGES, "bytes")
        .header(
            hyper::header::LAST_MODIFIED,
            "Wed, 22 Apr 2020 02:08:01 GMT",
        );

    let mut spans: Vec<HttpRangeSpan> = Vec::new();

    let mut boundary = None;
    let mut status_code = StatusCode::OK;
    if let Some(range_data) = req.headers().get(hyper::header::RANGE) {
        status_code = StatusCode::PARTIAL_CONTENT;

        let mut range_count = 0;
        for range_atom in get_ranges(range_data, file_length)? {
            range_count += 1;
            spans.push(range_atom);
        }

        http_span_check_no_overlaps(&spans[..])?;

        if range_count == 0 {
            return Err(ClientError.into());
        } else if range_count > 1 {
            let mut boundary_tmp = [0u8; BOUNDARY_LENGTH];

            let mut rng = thread_rng();
            for v in boundary_tmp.iter_mut() {
                *v = *BOUNDARY_CHARS.choose(&mut rng).unwrap();
            }

            let boundary_string = std::str::from_utf8(&boundary_tmp[..]).unwrap().to_string();
            boundary = Some(boundary_string);
        } else {
            let onespan = &spans[0];
            builder = builder.header(
                hyper::header::CONTENT_RANGE,
                format!(
                    "bytes {}-{}/{}",
                    onespan.start,
                    onespan.start + onespan.length - 1,
                    file_length,
                ),
            );
        }
    } else {
        spans.push(HttpRangeSpan {
            start: 0,
            length: file_length,
        });
    }
    if spans.len() == 1 {
        builder = builder.header(
            hyper::header::CONTENT_LENGTH,
            format!("{}", spans[0].length),
        );
    }

    let mut requested_bytes = 0;
    for span in &spans {
        requested_bytes += span.length;
    }

    counter!("webserver.requested_bytes", requested_bytes as u64);

    let file_mime_type = if req.uri().path().ends_with(".jpg") {
        "image/jpeg"
    } else if req.uri().path().ends_with(".png") {
        "image/png"
    } else if req.uri().path().ends_with(".gif") {
        "image/gif"
    } else if req.uri().path().ends_with(".mp4") {
        "video/mp4"
    } else if req.uri().path().ends_with(".cue") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };

    if let Some(ref b) = boundary {
        builder = builder.header(
            hyper::header::CONTENT_TYPE,
            format!("multipart/byteranges; boundary={}", b),
        );
    } else {
        builder = builder.header(hyper::header::CONTENT_TYPE, file_mime_type);
    }

    event!(
        Level::DEBUG,
        "fetching spans: {:?} -- {:#?}",
        spans,
        req.headers()
    );

    // let mut total_builder = BytesMut::new();
    let (mut tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, failure::Error>>(2);

    let mut socket = TcpStream::connect("127.0.0.1:3102").await?;

    tokio::spawn(async move {
        let mut is_first_boundary = true;
        let mut prefix = BytesMut::new();

        for sp in spans.iter() {
            let global_span_start = torrent_global_offset_start + sp.start;
            let piece_span_start = global_span_start % (1 << root_resp.piece_length_shift);

            let piece_index_cur = match (global_span_start >> root_resp.piece_length_shift).try_into() {
                Ok(piece_index_cur) => piece_index_cur,
                Err(err) => {
                    let send_err = Err(InternalError { msg: "piece index out of range" }.into());
                    if let Err(err2) = tx.send(send_err).await {
                        event!(
                            Level::ERROR,
                            "failed to send error to response handler: {}: {}",
                            err,
                            err2,
                        );
                    }
                    return;
                }
            };
            if let Some(ref b) = boundary {
                if is_first_boundary {
                    is_first_boundary = false;
                } else {
                    write!(&mut prefix, "\r\n").unwrap();
                }
                write!(&mut prefix, "--{}\r\n", b).unwrap();
                write!(
                    &mut prefix,
                    "Content-Range: bytes {}-{}/{}\r\n",
                    sp.start,
                    sp.full_closed_end(),
                    file_length,
                )
                .unwrap();
                write!(&mut prefix, "Content-Type: {}\r\n", file_mime_type).unwrap();
                write!(&mut prefix, "\r\n").unwrap();

                // total_builder.extend_from_slice(&prefix[..]);
                // prefix.clear();
                if let Err(err) = tx.send(Ok(prefix.split().freeze())).await {
                    event!(
                        Level::ERROR,
                        "failed to send data to response handler: {}",
                        err
                    );
                    return;
                }
            }

            let mut piece_index_cur = piece_index_cur;
            let mut piece_span_start = piece_span_start;
            let mut to_send: u64 = sp.length as u64;
            let mut wbuf = BytesMut::new();
            while to_send != 0 {
                #[inline]
                pub fn compute_offset_length(index: u32, atom_length: u32, total_length: u64) -> (u64, u32) {
                    event!(
                        Level::DEBUG,
                        index,
                        atom_length,
                        total_length,
                        "compute_offset_length"
                    );
                    let atom_length = u64::from(atom_length);
                    let index = u64::from(index);

                    let offset_start = atom_length * index;
                    let mut offset_end = atom_length * (index + 1);
                    if total_length < offset_end {
                        offset_end = total_length;
                    }
                    event!(
                        Level::DEBUG,
                        offset_start,
                        offset_end,
                        "compute_offset_length2"
                    );
                    (offset_start, (offset_end - offset_start) as u32)
                }

                // piece timeout check

                // connection check

                let blocks_per_piece = 1 << (root_resp.piece_length_shift - BLOCK_SIZE_SHIFT);
                for i in 0..blocks_per_piece {
                    let prm = PieceRequestMessage {
                        content_key,
                        piece_sha: TorrentId::zero(),
                        content_total_length: root_resp.total_length as u64,
                        piece_length_shift: root_resp.piece_length_shift,
                        piece_index: piece_index_cur,
                        piece_fetch_offset: i * BLOCK_SIZE,
                        piece_fetch_length: BLOCK_SIZE,
                    };

                    let data = prm.to_buf_with_header();
                    wbuf.extend_from_slice(&data);
                }

                while !wbuf.is_empty() {
                    if let Err(err) = socket.write_buf(&mut wbuf).await {
                        event!(Level::ERROR, "write err: {}", err);
                        return;
                    }
                }

                let final_size_max = 1 << root_resp.piece_length_shift;
                let mut piece_data = BytesMut::with_capacity(final_size_max);
                let mut tmp_data = vec![0; BLOCK_SIZE as usize];
                for _ in 0..blocks_per_piece {
                    let mut length_header = [0_u8; 4];
                    if let Err(err) = socket.read_exact(&mut length_header[..]).await {
                        event!(Level::ERROR, "write err: {}", err);
                        return;
                    }
                    let size = BigEndian::read_u32(&mut length_header);
                    if BLOCK_SIZE < size {
                        event!(
                            Level::ERROR,
                            "upstream server returned oversized piece",
                        );
                        return;
                    }

                    let size = size as usize;

                    if let Err(err) = socket.read_exact(&mut tmp_data[..size]).await {
                        event!(Level::ERROR, "write err: {}", err);
                        return;
                    }
                    piece_data.extend_from_slice(&tmp_data[..]);
                }

                piece_index_cur += 1;

                let mut piece_data = piece_data.freeze();

                if piece_span_start != 0 {
                    piece_data.advance(piece_span_start as usize);
                    piece_span_start = 0;
                }


                let bytes_length = piece_data.len() as u64;
                if to_send < bytes_length {
                    piece_data.truncate(to_send as usize);
                }


                to_send -= piece_data.len() as u64;
                if let Err(err) = tx.send(Ok(piece_data)).await {
                    event!(
                        Level::ERROR,
                        "failed to send bytes to response handler: {}",
                        err
                    );
                    return;
                }
            }
        }

        if let Some(ref b) = boundary {
            write!(&mut prefix, "\r\n--{}--\r\n", b).unwrap();
            // total_builder.extend_from_slice(&prefix[..]);
            // prefix.clear();
            if let Err(err) = tx.send(Ok(prefix.split().freeze())).await {
                event!(
                    Level::ERROR,
                    "failed to send data to response handler: {}",
                    err
                );
                return;
            }
        }
    });

    // let body = total_builder[..].to_vec().into();
    // Ok(builder.status(status_code).body(body).unwrap())
    Ok(builder
        .status(status_code)
        .body(Body::wrap_stream(rx))
        .unwrap())
}

fn get_ranges(
    value: &hyper::header::HeaderValue,
    total_size: i64,
) -> Result<Vec<HttpRangeSpan>, failure::Error> {
    let value_str = value.to_str().map_err(|_| ClientError)?;
    if !value_str.starts_with("bytes=") {
        return Err(ClientError.into());
    }

    let mut out = Vec::new();
    for part in value_str[6..].split(", ") {
        let mut part_iter = part.splitn(2, '-');
        let start: i64 = part_iter.next().ok_or(ClientError)?.parse()?;
        if start < 0 {
            return Err(ClientError.into());
        }

        if total_size <= start {
            return Err(OutOfRange.into());
        }
        let end_str = part_iter.next().ok_or(ClientError)?;

        let end = if end_str.is_empty() {
            total_size - 1
        } else {
            end_str.parse()?
        };
        if end <= start {
            return Err(OutOfRange.into());
        }

        out.push(HttpRangeSpan {
            start,
            length: end - start + 1,
        });
    }
    Ok(out)
}

fn http_span_check_no_overlaps(spans: &[HttpRangeSpan]) -> Result<(), failure::Error> {
    if spans.len() > 30 {
        // FIXME
        return Err(ClientError.into());
    }

    Ok(())
}

#[derive(Debug)]
struct HttpRangeSpan {
    start: i64,
    length: i64,
}

impl HttpRangeSpan {
    pub fn full_closed_end(&self) -> i64 {
        self.start + self.length - 1
    }
}

fn response_http_bad_request(req_ctx: &RequestContext) -> Response<Body> {
    let rendered = ErrorPageTemplate {
        status_code: 400,
        status_code_text: "",
        error_title: "",
        error_string: "",
        request_id: req_ctx.request_id,
        fe_instance_id: req_ctx.instance_id,
    }.render().unwrap();

    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .status(StatusCode::BAD_REQUEST);

    builder.body(rendered.into()).unwrap()
}

fn repsonse_http_range_not_satisfiable(req_ctx: &RequestContext) -> Response<Body> {
    let rendered = ErrorPageTemplate {
        status_code: 416,
        status_code_text: "",
        error_title: "",
        error_string: "",
        request_id: req_ctx.request_id,
        fe_instance_id: req_ctx.instance_id,
    }.render().unwrap();

    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .status(StatusCode::RANGE_NOT_SATISFIABLE);

    builder.body(rendered.into()).unwrap()
}

fn response_http_not_found(req_ctx: &RequestContext) -> Response<Body> {
    let rendered = ErrorPageTemplate {
        status_code: 404,
        status_code_text: "",
        error_title: "",
        error_string: "",
        request_id: req_ctx.request_id,
        fe_instance_id: req_ctx.instance_id,
    }.render().unwrap();

    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .status(StatusCode::NOT_FOUND);

    builder.body(rendered.into()).unwrap()
}

fn response_http_internal_server_error(req_ctx: &RequestContext) -> Response<Body> {
    let rendered = ErrorPageTemplate {
        status_code: 500,
        status_code_text: "",
        error_title: "",
        error_string: "",
        request_id: req_ctx.request_id,
        fe_instance_id: req_ctx.instance_id,
    }.render().unwrap();

    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .status(StatusCode::INTERNAL_SERVER_ERROR);

    builder.body(rendered.into()).unwrap()
}

fn response_http_found(req_ctx: &RequestContext, new_path: &str) -> Response<Body> {
    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(hyper::header::LOCATION, new_path)
        .status(StatusCode::FOUND);

    let content = format!("Redirecting you to <a href=\"{0}\">{0}</a>", new_path);

    builder.body(content.into()).unwrap()
}

fn response_ok_rendering_directory(
    req_ctx: &RequestContext,
    current_path: &str,
    is_root: bool,
    dir: &[(&str, &VfsEntry)],
) -> Response<Body> {
    let mut entries: Vec<DirectoryListingEntry> = Vec::new();
    for (name, entry) in dir {
        let file_type;
        let is_directory;
        let file_size_bytes;
        match entry.data {
            VfsEntryData::Regular(ref reg) => {
                is_directory = false;
                file_type = "REG";
                file_size_bytes = reg.file_length as u64;
            }
            VfsEntryData::Directory(..) => {
                is_directory = true;
                file_type = "DIR";
                file_size_bytes = 0;
            }
        }
        entries.push(DirectoryListingEntry {
            is_directory,
            file_type,
            file_name: name,
            file_size_bytes,
        });
    }

    let dlt = DirectoryListingTemplate {
        current_path,
        is_root,
        entries,
        render_time: req_ctx.start_instant.elapsed(),
        request_id: req_ctx.request_id,
        fe_instance_id: req_ctx.instance_id,
    }.render().unwrap();

    let builder = Response::builder()
        .header(hyper::header::SERVER, SERVER_NAME)
        .header(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("text/html; charset=utf-8"),
        );

    builder.body(dlt.into()).unwrap()
}

// --

#[derive(Debug)]
pub struct OutOfRange;

impl fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "OutOfRange")
    }
}

impl std::error::Error for OutOfRange {}

// --

#[derive(Debug)]
pub struct ClientError;

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ClientError")
    }
}

impl std::error::Error for ClientError {}

// --


#[derive(Debug)]
pub struct InternalError {
    pub msg: &'static str,
}

impl fmt::Display for InternalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InternalError: {}", self.msg)
    }
}

impl std::error::Error for InternalError {}

// --


#[allow(clippy::cognitive_complexity)] // macro bug around event!()
fn print_test_logging() {
    event!(Level::TRACE, "logger initialized - trace check");
    event!(Level::DEBUG, "logger initialized - debug check");
    event!(Level::INFO, "logger initialized - info check");
    event!(Level::WARN, "logger initialized - warn check");
    event!(Level::ERROR, "logger initialized - error check");
}

// --


#[derive(Debug, PartialEq, Eq)]
struct PieceRequestMessage {
    content_key: TorrentId,
    piece_sha: TorrentId,
    content_total_length: u64,
    piece_length_shift: u8,
    piece_index: u32,
    piece_fetch_offset: u32,
    piece_fetch_length: u32,
}

impl PieceRequestMessage {
    pub const PACKET_SIZE: usize = 2 * 20 + 8 +  4 * 4;
    pub const TOKEN: u32 = 0x500e1857;

    pub fn from_buf(buf: &[u8]) -> Result<PieceRequestMessage, failure::Error> {
        
        if buf.len() < PieceRequestMessage::PACKET_SIZE {
            return Err(failure::format_err!("bad size - got {}, expected {}", buf.len(), PieceRequestMessage::PACKET_SIZE));
        }

        let content_key = TorrentId::from_slice(&buf[0..20]).unwrap();
        let piece_sha = TorrentId::from_slice(&buf[20..40]).unwrap();
        let content_total_length = BigEndian::read_u64(&buf[40..]);
        let piece_length_shift_aligned = BigEndian::read_u32(&buf[48..]);
        let piece_index = BigEndian::read_u32(&buf[52..]);
        let piece_fetch_offset = BigEndian::read_u32(&buf[56..]);
        let piece_fetch_length = BigEndian::read_u32(&buf[60..]);

        let piece_length_shift: u8 = piece_length_shift_aligned.try_into()?;

        Ok(PieceRequestMessage {
            content_key,
            piece_sha,
            content_total_length,
            piece_length_shift,
            piece_index,
            piece_fetch_offset,
            piece_fetch_length,
        })
    }

    pub fn to_buf_with_header(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(PieceRequestMessage::PACKET_SIZE);
        buf.resize(8, 0);
        BigEndian::write_u32(&mut buf[..], PieceRequestMessage::TOKEN);
        BigEndian::write_u32(&mut buf[4..], PieceRequestMessage::PACKET_SIZE as u32);
        buf.extend(self.content_key.as_bytes());
        buf.extend(self.piece_sha.as_bytes());
        buf.resize(8 + PieceRequestMessage::PACKET_SIZE, 0);
        BigEndian::write_u64(&mut buf[48..], self.content_total_length);
        BigEndian::write_u32(&mut buf[56..], self.piece_length_shift as u32);
        BigEndian::write_u32(&mut buf[60..], self.piece_index);
        BigEndian::write_u32(&mut buf[64..], self.piece_fetch_offset);
        BigEndian::write_u32(&mut buf[68..], self.piece_fetch_length);

        assert_eq!(&Self::from_buf(&buf[8..]).unwrap(), self);

        buf
    }
}