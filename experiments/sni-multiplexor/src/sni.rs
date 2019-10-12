use std::fmt::{self, Write as _Write};
use std::sync::Arc;
use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use copy_arena::Arena;
use futures::future;
use futures::prelude::{Sink, Stream};
use futures::stream::StreamExt;
use ksuid::Ksuid;
use log::{debug, info, warn};
use tokio::codec::{Decoder, Encoder};
use tokio::future::FutureExt;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::TcpStream;
use tokio::net::unix::UnixStream;
use tokio::sync::Mutex;
use tokio::codec::{FramedRead, FramedWrite};

use tls::{
    extract_record, ByteIterRead, ClientHello, Extension, ExtensionServerName, Handshake,
    RECORD_CONTENT_TYPE_HANDSHAKE,
};

use crate::abortable_stream::{
    abortable_stream_pair, AbortTryStreamError, AbortableStreamFactory, AbortableTryStream,
};
use crate::erased::NetworkStream;
use crate::resolver::{BackendManager, NetworkLocationAddress, Resolver2};
use crate::state_track::{Session, SessionManager};

const DEFAULT_SNI_DETECTOR_MAX_LEN: usize = 20480;

pub const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};
pub const ALERT_UNRECOGNIZED_NAME: AlertError = AlertError {
    alert_description: 112,
};

#[derive(Debug)]
enum Direction {
    BackendToClient,
    ClientToBackend,
}

#[derive(Debug, Copy, Clone)]
pub struct AlertError {
    alert_description: u8,
}

impl std::error::Error for AlertError {}

impl fmt::Display for AlertError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "alert #{}", self.alert_description)
    }
}

#[derive(Debug)]
pub enum SniDetectRecord {
    SniHostname(String),
    PassThrough(Bytes),
}

impl SniDetectRecord {
    pub fn rec_byte_size(&self) -> u64 {
        match *self {
            SniDetectRecord::SniHostname(..) => 0,
            SniDetectRecord::PassThrough(ref b) => b.len() as u64,
        }
    }
}

pub struct SniPassCodec;

impl Encoder for SniPassCodec {
    type Item = SniDetectRecord;

    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            SniDetectRecord::SniHostname(_internal_only) => (),
            SniDetectRecord::PassThrough(bytes) => {
                dst.extend_from_slice(&bytes);
            }
        }
        Ok(())
    }
}

impl Decoder for SniPassCodec {
    type Item = SniDetectRecord;

    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        Ok(Some(SniDetectRecord::PassThrough(src.take().freeze())))
    }
}

pub struct SniDetectorCodec {
    max_length: usize,
    emitted_sni: bool,
    arena: Arena,
}

impl SniDetectorCodec {
    pub fn new() -> SniDetectorCodec {
        SniDetectorCodec {
            max_length: DEFAULT_SNI_DETECTOR_MAX_LEN,
            emitted_sni: false,
            arena: Arena::with_capacity(2048),
        }
    }
}

impl Encoder for SniDetectorCodec {
    type Item = SniDetectRecord;

    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            SniDetectRecord::SniHostname(_internal_only) => (),
            SniDetectRecord::PassThrough(bytes) => {
                dst.extend_from_slice(&bytes);
            }
        }
        Ok(())
    }
}

impl Decoder for SniDetectorCodec {
    type Item = SniDetectRecord;

    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        use tls::{Error as TlsError, ErrorKind as TlsErrorKind};
        fn fixup_err(tls: TlsError) -> io::Error {
            match tls.kind() {
                TlsErrorKind::ProtocolViolation => {
                    io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR)
                }
                TlsErrorKind::Truncated => {
                    warn!("I think this shouldn't happen: {}", tls);
                    io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR)
                }
                TlsErrorKind::Other => {
                    warn!("got Other error: {}", tls);
                    io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR)
                }
            }
        }

        if self.emitted_sni {
            if src.is_empty() {
                return Ok(None);
            }

            return Ok(Some(SniDetectRecord::PassThrough(src.take().freeze())));
        }

        if self.max_length < src.len() {
            warn!("Handshake too large: {} < {}", self.max_length, src.len());
            return Err(io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR));
        }

        let mut allocator = self.arena.allocator();
        let mut dst_iter = src.iter();

        let mut data_size = 0;

        loop {
            let record = match extract_record(&mut dst_iter) {
                Ok(Some(rec)) => rec,
                Ok(None) => break,
                Err(err) => {
                    return Err(fixup_err(err));
                }
            };
            if record.content_type != RECORD_CONTENT_TYPE_HANDSHAKE {
                continue;
            }
            data_size += record.data.len();
        }

        let unframed_data = allocator.alloc_slice_default(data_size);
        let mut unframed_data_write = &mut unframed_data[..];

        let mut dst_iter = src.iter();
        while let Some(record) = extract_record(&mut dst_iter).unwrap() {
            if record.content_type != RECORD_CONTENT_TYPE_HANDSHAKE {
                continue;
            }

            let (to_write, rest) = unframed_data_write.split_at_mut(record.data.len());
            to_write.copy_from_slice(record.data);
            unframed_data_write = rest;
        }
        assert_eq!(unframed_data_write.len(), 0);

        let server_names =
            match Handshake::read_byte_iter(&mut allocator, &mut unframed_data.iter())
                .map_err(fixup_err)?
            {
                Handshake::ClientHello(hello) => get_server_names(hello)?,
                Handshake::Unknown(..) => {
                    return Err(io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR));
                }
            };

        if server_names.0.len() != 1 {
            return Err(io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR));
        }
        let server_name = server_names.0[0].0.to_string();
        drop(allocator);

        debug!(
            "arena had capacity {} after SNI detection",
            self.arena.capacity()
        );

        self.emitted_sni = true;

        Ok(Some(SniDetectRecord::SniHostname(server_name)))
    }
}

fn get_server_names<'arena>(
    hello: &ClientHello<'arena>,
) -> Result<&'arena ExtensionServerName<'arena>, io::Error> {
    for ext in hello.extensions.0 {
        match *ext {
            Extension::ServerName(name_ext) => return Ok(name_ext),
            Extension::Unknown(..) => (),
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR))
}

#[derive(Clone)]
pub struct SocketAddrPairV4 {
    pub local_addr: SocketAddrV4,
    pub peer_addr: SocketAddrV4,
}

#[derive(Clone)]
pub struct SocketAddrPairV6 {
    pub local_addr: SocketAddrV6,
    pub peer_addr: SocketAddrV6,
}

#[derive(Clone)]
pub enum SocketAddrPair {
    V4(SocketAddrPairV4),
    V6(SocketAddrPairV6),
}

impl SocketAddrPair {
    pub fn peer_addr(&self) -> SocketAddr {
        match *self {
            SocketAddrPair::V4(ref v4) => SocketAddr::V4(v4.peer_addr),
            SocketAddrPair::V6(ref v6) => SocketAddr::V6(v6.peer_addr),
        }
    }

    pub fn from_pair(
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<SocketAddrPair, io::Error> {
        match (local_addr, peer_addr) {
            (SocketAddr::V4(local_addr), SocketAddr::V4(peer_addr)) => {
                Ok(SocketAddrPair::V4(SocketAddrPairV4 {
                    local_addr,
                    peer_addr,
                }))
            }
            (SocketAddr::V6(local_addr), SocketAddr::V6(peer_addr)) => {
                Ok(SocketAddrPair::V6(SocketAddrPairV6 {
                    local_addr,
                    peer_addr,
                }))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Other,
                "address families must match",
            )),
        }
    }
}

struct ClientHandle {
    start_time: Instant,
    session_id: Ksuid,
    aborter: AbortableStreamFactory,
}

async fn register_sessman(
    sessman: &Arc<Mutex<SessionManager>>,
    client_addr: &SocketAddrPair,
) -> ClientHandle {
    let start_time = Instant::now();
    let session_id = Ksuid::generate();
    let (handle, aborter) = abortable_stream_pair();

    {
        let mut sessman = sessman.lock().await;
        sessman.add_session(Session::new(
            session_id,
            Instant::now(),
            client_addr.clone(),
            handle,
        ));
    }

    ClientHandle {
        start_time,
        session_id,
        aborter,
    }
}


pub async fn start_client<ClientRead, ClientWrite>(
    sessman: Arc<Mutex<SessionManager>>,
    backend_man: Arc<Mutex<BackendManager>>,
    client_addr: SocketAddrPair,
    client_reader: ClientRead,
    client_writer: ClientWrite,
) where
    ClientRead: AsyncRead + Unpin,
    ClientWrite: AsyncWrite + Unpin,
{
    let backend_man: BackendManager = {
        let backend_man = backend_man.lock().await;
        BackendManager::clone(&*backend_man)
    };

    let handle = register_sessman(&sessman, &client_addr).await;
    let session_id = handle.session_id;

    match start_client_helper(handle, Arc::clone(&sessman), backend_man, client_addr, client_reader, client_writer).await {
        Ok(()) => info!("{} terminated OK", session_id.fmt_base62()),
        Err(()) => info!("{} terminated with error status", session_id.fmt_base62()),
    }
    {
        let mut sessman = sessman.lock().await;
        sessman.mark_shutdown(&session_id);
    }
}


async fn start_client_helper<ClientRead, ClientWrite>(
    handle: ClientHandle,
    sessman: Arc<Mutex<SessionManager>>,
    backend_man: BackendManager,
    client_addr: SocketAddrPair,
    client_reader: ClientRead,
    client_writer: ClientWrite,
) -> Result<(), ()>
where
    ClientRead: AsyncRead + Unpin,
    ClientWrite: AsyncWrite + Unpin,
{
    info!(
        "{}: started connection from {}",
        handle.session_id.fmt_base62(),
        client_addr.peer_addr()
    );

    let client_stream = FramedRead::new(client_reader, SniDetectorCodec::new());
    let client_sink = FramedWrite::new(client_writer, SniDetectorCodec::new());

    let client_stream = handle.aborter.with_try_stream(client_stream);

    let mut proxy_buf = BytesMut::with_capacity(300);

    #[allow(clippy::write_with_newline)]
    // The protocol dictates the use of carriage return, which writeln!() might
    // not do on unix-based systems, so we prefer specifying this manually.
    match client_addr {
        SocketAddrPair::V4(ref v4) => {
            write!(
                &mut proxy_buf,
                "PROXY TCP4 {} {} {} {}\r\n",
                v4.peer_addr.ip(),
                v4.local_addr.ip(),
                v4.peer_addr.port(),
                v4.local_addr.port(),
            )
            .unwrap();
        }
        SocketAddrPair::V6(ref v6) => {
            write!(
                &mut proxy_buf,
                "PROXY TCP6 {} {} {} {}\r\n",
                v6.peer_addr.ip(),
                v6.local_addr.ip(),
                v6.peer_addr.port(),
                v6.local_addr.port(),
            )
            .unwrap();
        }
    }

    let frame_res = client_stream
        .into_future()
        .timeout(Duration::from_secs(3))
        .await;

    let (first_frame, client_stream) = match frame_res {
        Ok(pair) => pair,
        Err(err) => {
            warn!(
                "{} timed out waiting for handshake: {}",
                handle.session_id.fmt_base62(),
                err,
            );
            return Err(());
        }
    };

    let first_frame = match first_frame {
        Some(Ok(frame)) => frame,
        Some(Err(AbortTryStreamError::Err(err))) => {
            warn!("{} error: {}", handle.session_id.fmt_base62(), err);
            return Err(());
        }
        Some(Err(AbortTryStreamError::Aborted(..))) => {
            warn!("{} was aborted", handle.session_id.fmt_base62());
            return Err(());
        }
        None => {
            warn!("{} error: got EOF", handle.session_id.fmt_base62());
            return Err(());
        }
    };

    let hostname;
    match first_frame {
        SniDetectRecord::SniHostname(h) => {
            hostname = h;
        }
        _ => {
            warn!(
                "{} error: got something that isn't a hostname: {:?}",
                handle.session_id.fmt_base62(),
                first_frame
            );
            return Err(());
        }
    }

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_resolving(&handle.session_id, &hostname);
    }

    info!(
        "{} requesting a connection to {:?}",
        handle.session_id.fmt_base62(),
        hostname,
    );

    let bset = match backend_man.resolve(&hostname).await {
        Ok(bset) => bset,
        Err(err) => {
            warn!("unimplemented - sending TLS error: {:?}", err);
            return Err(());
        }
    };
    assert_eq!(bset.locations.len(), 1);
    let backend = &bset.locations[0];

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_connecting(&handle.session_id);
    }

    if !backend.use_haproxy_header() {
        // clears the whole thing.
        proxy_buf.take();
    }

    let mut backend_sock = match backend.address {
        NetworkLocationAddress::Unix(ref addr) => {
            match UnixStream::connect(addr)
                .timeout(Duration::from_secs(3))
                .await
            {
                Ok(Ok(conn)) => Box::new(conn) as NetworkStream,
                Ok(Err(err)) => {
                    warn!(
                        "{}: backend connection error: {}",
                        handle.session_id.fmt_base62(),
                        err,
                    );
                    warn!("unimplemented - sending TLS error: {:?}", err);
                    return Err(());
                }
                Err(err) => {
                    warn!("unimplemented - sending TLS error for timeout: {:?}", err);
                    warn!(
                        "{}: failed to connect to backend within allotted time: {}",
                        handle.session_id.fmt_base62(),
                        err,
                    );                    
                    return Err(());
                }
            }
        }
        NetworkLocationAddress::Tcp(ref addr) => {
            match TcpStream::connect(addr)
                .timeout(Duration::from_secs(3))
                .await
            {
                Ok(Ok(conn)) => Box::new(conn) as NetworkStream,
                Ok(Err(err)) => {
                    warn!(
                        "{}: backend connection error: {}",
                        handle.session_id.fmt_base62(),
                        err,
                    );
                    warn!("unimplemented - sending TLS error: {:?}", err);                    
                    return Err(());
                }
                Err(err) => {
                    warn!("unimplemented - sending TLS error for timeout: {:?}", err);
                    warn!(
                        "{}: failed to connect to backend within allotted time: {}",
                        handle.session_id.fmt_base62(),
                        err,
                    );                    
                    return Err(());
                }
            }
        }
    };

    if !proxy_buf.is_empty() {
        if let Err(err) = backend_sock.write_all(&proxy_buf[..]).await {
            warn!(
                "{}: failed to write proxy header to backend: {}",
                handle.session_id.fmt_base62(),
                err,
            );
            
            return Err(());
        }
    }

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_connected(&handle.session_id);
    }

    info!(
        "{}: connected to backend after {}ms",
        handle.session_id.fmt_base62(),
        duration_milliseconds(handle.start_time.elapsed())
    );

    let (backend_sink, backend_stream) = SniPassCodec.framed(backend_sock).split();
    let backend_stream = handle.aborter.with_try_stream(backend_stream);

    let s2c_sessman = sessman.clone();
    let server_to_client = async {
        let res = unidirectional_copy(
            &handle,
            &s2c_sessman,
            client_sink,
            backend_stream,
            Direction::BackendToClient,
        )
        .await;

        let mut sessman = s2c_sessman.lock().await;
        sessman.mark_shutdown_read(&handle.session_id);

        if let Err(err) = res {
            warn!("{}: s2c error {}", handle.session_id.fmt_base62(), err);
            // sends the cancellation to the other copy
            let _ = sessman.destroy(&handle.session_id);
        }
        
        info!(
            "{} closed s2c after {:?}",
            handle.session_id.fmt_base62(),
            handle.start_time.elapsed()
        );
    };

    let c2s_sessman = sessman.clone();
    let client_to_server = async {
        let res = unidirectional_copy(
            &handle,
            &c2s_sessman,
            backend_sink,
            client_stream,
            Direction::ClientToBackend,
        )
        .await;

        let mut sessman = c2s_sessman.lock().await;
        sessman.mark_shutdown_write(&handle.session_id);

        if let Err(err) = res {
            warn!("{}: c2s error {}", handle.session_id.fmt_base62(), err);
            // sends the cancellation to the other copy
            let _ = sessman.destroy(&handle.session_id);
        }

        info!(
            "{} closed c2s after {:?}",
            handle.session_id.fmt_base62(),
            handle.start_time.elapsed()
        );
    };

    let _: ((), ()) = future::join(server_to_client, client_to_server).await;

    Ok(())
}

#[allow(clippy::needless_lifetimes)]
async fn unidirectional_copy<Si, St>(
    handle: &ClientHandle,
    sessman: &Arc<Mutex<SessionManager>>,
    mut sink: Si,
    mut stream: AbortableTryStream<St, SniDetectRecord, io::Error>,
    direction: Direction,
) -> Result<(), String>
where
    Si: Sink<SniDetectRecord> + Unpin,
    St: Stream<Item = Result<SniDetectRecord, io::Error>> + Unpin,
    Si::Error: std::error::Error,
{
    use futures::sink::SinkExt;

    let mut res = Ok(());

    loop {
        let (value, stream_tail) = stream.into_future().await;
        stream = stream_tail;

        let rec = match value {
            Some(Ok(v)) => v,
            Some(Err(AbortTryStreamError::Err(err))) => {
                warn!(
                    "{} {:?} encountered an error: {}",
                    handle.session_id.fmt_base62(),
                    direction,
                    err
                );
                break;
            }
            Some(Err(AbortTryStreamError::Aborted(..))) => {
                warn!("{} was aborted", handle.session_id.fmt_base62());
                break;
            }
            None => {
                break;
            }
        };

        let bytes = rec.rec_byte_size();

        {
            let mut sessman = sessman.lock().await;
            match direction {
                Direction::BackendToClient => {
                    sessman.handle_xmit_backend_to_client(&handle.session_id, bytes);
                }
                Direction::ClientToBackend => {
                    sessman.handle_xmit_client_to_backend(&handle.session_id, bytes);
                }
            };
        }

        if let Err(err) = sink.send(rec).await {
            warn!(
                "{}: aborting copy - sink send error {}",
                handle.session_id.fmt_base62(),
                err
            );

            res = Err("sink send error".into());

            break;
        }
    }

    crate::abortable_stream::SinkCloser::new(sink)
        .await
        .map_err(|e| format!("{}", e))?;

    res
}

fn duration_milliseconds(dur: Duration) -> u64 {
    let mut out = 0;
    out += u64::from(dur.subsec_nanos()) / 1_000_000;
    out += dur.as_secs() * 1_000;
    out
}
