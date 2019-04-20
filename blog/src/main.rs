use std::env;
use std::error::Error as StdError;
use std::fs;
use std::io::{self, Read};
use std::sync::Arc;

// mod statics;
mod vfs;

use chrono::naive::{NaiveDate, NaiveDateTime, NaiveTime};
use chrono::{
    offset::{Local as LocalTz, TimeZone},
    DateTime,
};
use http::Method;
use hyper_staticfile::{Resolve, ResolveFuture, ResolveResult, ThinMetadata};
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

use futures::{future, Future, Poll};
use http::Request;
use hyper::Body;
use hyper_staticfile::{Static, StaticFuture};

/// Hyper `Service` implementation that serves all requests.
#[derive(Clone)]
struct MainService {
    static_: Static<ZipResolver>,
}

// impl MainService {
//     fn new(resolver: ZipResolver) -> MainService {
//         MainService {
//             static_: Static::new(resolver),
//         }
//     }
// }

impl hyper::service::Service for MainService {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Box<StdError + Send + Sync>;
    type Future = StaticFuture<Body>;

    fn call(&mut self, req: Request<Body>) -> StaticFuture<Body> {
        self.static_.serve(req)
    }
}

/// Application entry point.
fn main() {
    let file = fs::File::open(env::args_os().nth(1).unwrap()).unwrap();
    let zr: ZipResolver = ZipResolver::load(file).unwrap();
    println!("zr = {:#?}", zr.archive);
}

struct SubsliceReader<B>
where
    io::Cursor<B>: io::Read,
{
    metadata: ThinMetadata,
    underlying: io::Take<io::Cursor<B>>,
}

#[derive(Debug)]
struct SubsliceReaderOpts {
    modified: Option<DateTime<LocalTz>>,
}

impl<B> io::Read for SubsliceReader<B>
where
    io::Cursor<B>: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.underlying.read(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.underlying.read_exact(buf)
    }
}

impl<B> SubsliceReader<B>
where
    io::Cursor<B>: io::Read,
{
    fn new(data: B, offset: u64, limit: u64, opts: SubsliceReaderOpts) -> SubsliceReader<B> {
        let mut cursor = io::Cursor::new(data);
        cursor.set_position(offset);
        let mut metadata = ThinMetadata::new(limit);
        metadata.set_modified(opts.modified);
        SubsliceReader {
            metadata,
            underlying: io::Read::take(cursor, limit),
        }
    }
}

struct ZipFuture {
    reader: SubsliceReader<Arc<[u8]>>,
}

impl Future for ZipFuture {
    type Item = ResolveResult;
    type Error = Box<StdError + Send + Sync>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        // use std::fs::Metadata;
        // use tokio::fs::File;
        // return Ok(Ready(ResolveResult::Found(file, metadata)));
        unimplemented!();
    }
}

#[derive(Clone)]
struct ZipResolver {
    /// Resolved filesystem path. An option, because we take ownership later.
    // full_path: Option<PathBuf>,
    /// Whether this is a directory request. (Request path ends with a slash.)
    // is_dir_request: bool,
    raw_data: Arc<[u8]>,
    archive: ZipArchive<io::Cursor<Arc<[u8]>>>,
}

impl ZipResolver {
    fn load(mut file: fs::File) -> io::Result<ZipResolver> {
        let mut vec = Vec::new();
        file.read_to_end(&mut vec)?;
        drop(file);

        let raw_data: Arc<[u8]> = vec.into_boxed_slice().into();
        let cursor = io::Cursor::new(Arc::clone(&raw_data));
        let archive = ZipArchive::new(cursor)?;

        Ok(ZipResolver { raw_data, archive })
    }
}

impl Resolve for ZipResolver {
    fn resolve<B>(&self, req: &Request<B>) -> ResolveFuture {
        // Handle only `GET`/`HEAD` and absolute paths.
        println!("{}", req.method());

        match *req.method() {
            Method::HEAD | Method::GET => {}
            _ => {
                return ResolveFuture::from_future(future::ok(ResolveResult::MethodNotMatched));
            }
        }

        // Handle only simple path requests.
        if req.uri().scheme_part().is_some() || req.uri().host().is_some() {
            return ResolveFuture::from_future(future::ok(ResolveResult::UriNotMatched));
        }

        let path = req.uri().path();
        if path.is_empty() {
            println!("x");
            return ResolveFuture::from_future(future::ok(ResolveResult::IsDirectory));
        }

        let mut archive = self.archive.clone();
        println!("2x");
        let reader = match archive.by_name(&path[1..]) {
            Ok(reader) => reader,
            Err(err) => {
                println!("3x {}", path);
                return map_zip_error(err);
            }
        };
        println!("4x {}", path);

        if reader.compression() != CompressionMethod::Stored {
            println!("5x {}", path);
            let err = "Unsupported archive: compressed file".into();
            return ResolveFuture::from_future(future::err(err));
        }

        let offset = reader.data_start();
        if (usize::max_value() as u64) < offset {
            println!("6x {}", path);
            let err = "Unsupported archive".into();
            return ResolveFuture::from_future(future::err(err));
        }

        let limit = reader.compressed_size();
        if (usize::max_value() as u64) < limit {
            println!("7x {}", path);
            let err = "Unsupported archive".into();
            return ResolveFuture::from_future(future::err(err));
        }

        println!("8x {}", path);
        let last_mod = reader.last_modified();
        let last_mod = LocalTz
            .from_local_datetime(&NaiveDateTime::new(
                NaiveDate::from_ymd(
                    i32::from(last_mod.year()),
                    u32::from(last_mod.month()),
                    u32::from(last_mod.day()),
                ),
                NaiveTime::from_hms(
                    u32::from(last_mod.hour()),
                    u32::from(last_mod.minute()),
                    u32::from(last_mod.second()),
                ),
            ))
            .unwrap();

        drop(reader);

        let opts = SubsliceReaderOpts {
            modified: Some(last_mod),
        };

        println!("SSR {:?}", (offset, limit, &opts));
        ResolveFuture::from_future(ZipFuture {
            reader: SubsliceReader::new(
                // Save an Arc increment/decrement.
                archive.into_inner().into_inner(),
                offset,
                limit,
                opts,
            ),
        })
    }
}

fn map_zip_error(err: ZipError) -> ResolveFuture {
    match err {
        ZipError::FileNotFound => ResolveFuture::from_future(future::ok(ResolveResult::NotFound)),
        ZipError::Io(err) => ResolveFuture::from_future(future::err(format!("{}", err).into())),
        ZipError::InvalidArchive(msg) => {
            let msg = format!("Invalid archive: {}", msg);
            ResolveFuture::from_future(future::err(msg.into()))
        }
        ZipError::UnsupportedArchive(msg) => {
            let msg = format!("Unsupported archive: {}", msg);
            ResolveFuture::from_future(future::err(msg.into()))
        }
    }
}
