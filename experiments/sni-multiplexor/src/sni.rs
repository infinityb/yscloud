use std::fmt::{self, Write as _Write};
use std::fs::{remove_file, File};
use std::io::{self, Write};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use copy_arena::Arena;
use futures01::prelude::{Future as LegacyFuture};
use futures::compat::{Compat01As03, Compat01As03Sink};
use futures::future;
use futures::prelude::{Sink, SinkExt, Stream, StreamExt};
use ksuid::Ksuid;
use log::{debug, info, warn};
use tokio::codec::{Framed, Decoder, Encoder};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::FutureExt;
use tokio::prelude::stream::{SplitStream, SplitSink};
use tokio::sync::mpsc::Sender;


use tls::{
    extract_record, ByteIterRead, ClientHello, Extension, ExtensionServerName, Handshake,
    RECORD_CONTENT_TYPE_HANDSHAKE,
};

use crate::config::Resolver;
use crate::state_track::{SessionCommand, SessionCommandData};

const DEFAULT_SNI_DETECTOR_MAX_LEN: usize = 20480;

pub const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};
pub const ALERT_UNRECOGNIZED_NAME: AlertError = AlertError {
    alert_description: 112,
};


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
    connection_id: Ksuid,
    max_length: usize,
    emitted_sni: bool,
    arena: Arena,
}

impl SniDetectorCodec {
    pub fn new(connection_id: Ksuid) -> SniDetectorCodec {
        SniDetectorCodec {
            connection_id,
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
                    write_handshake_sample(&self.connection_id, &src[..]);
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
                    write_handshake_sample(&self.connection_id, &src[..]);
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

pub struct ClientMetadata {
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub stats_tx_impl: Sender<SessionCommand>,
    pub stats_tx: Compat01As03Sink<Sender<SessionCommand>, SessionCommand>,
}

impl Clone for ClientMetadata {
    fn clone(&self) -> Self {
        ClientMetadata {
            session_id: self.session_id,
            start_time: self.start_time,
            client_conn: self.client_conn.clone(),
            stats_tx_impl: self.stats_tx_impl.clone(),
            stats_tx: Compat01As03Sink::new(self.stats_tx_impl.clone()),
        }
    }
}

pub async fn start_client<A>(
    connector: Arc<dyn Resolver + Send + Sync + Unpin>,
    mut client_meta: ClientMetadata,
    client: Framed<A, SniDetectorCodec>,
) -> Result<(), io::Error>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    use futures01::prelude::Stream;

    info!(
        "{}: started connection from {}",
        client_meta.session_id.fmt_base62(),
        client_meta.client_conn.peer_addr()
    );

    // TODO/XXX: we need a timeout thing.
    let (client_sink, client_stream) = client.split();
    let client_sink = Compat01As03Sink::new(client_sink);

    let mut proxy_buf = BytesMut::with_capacity(107);

    #[allow(clippy::write_with_newline)]
    // The protocol dictates the use of carriage return, which writeln!() might
    // not do on unix-based systems, so we prefer specifying this manually.
    match client_meta.client_conn {
        SocketAddrPair::V4(ref v4) => {
            write!(
                &mut proxy_buf,
                "PROXY TCP4 {} {} {} {}\r\n",
                v4.local_addr.ip(),
                v4.peer_addr.ip(),
                v4.local_addr.port(),
                v4.peer_addr.port(),
            )
            .unwrap();
        }
        SocketAddrPair::V6(ref v6) => {
            write!(
                &mut proxy_buf,
                "PROXY TCP6 {} {} {} {}\r\n",
                v6.local_addr.ip(),
                v6.peer_addr.ip(),
                v6.local_addr.port(),
                v6.peer_addr.port(),
            )
            .unwrap();
        }
    }

    let hanshake_client_meta = client_meta.clone();
    let client_fut = client_stream
        .into_future()
        .timeout(Duration::from_secs(3))
        .map_err(move |e| {
            if let Some((inner, _stream)) = e.into_inner() {
                warn!(
                    "error with ClientHello for {}: {}",
                    hanshake_client_meta.client_conn.peer_addr(),
                    inner
                );
            } else {
                warn!(
                    "{} failed to send ClientHello within allotted time",
                    hanshake_client_meta.client_conn.peer_addr()
                );
            }
        });
    
    let (first_frame, client_stream_tail) = Compat01As03::new(client_fut).await
        .map_err(|()| io::Error::new(io::ErrorKind::Other, format!("{}:{}", file!(), line!())))
        ?;

    let hostname;
    match first_frame {
        Some(SniDetectRecord::SniHostname(h)) => {
            hostname = h;
        }
        Some(..) | None => {
            return Err(io::Error::new(io::ErrorKind::Other, "something that isn't a hostname"));
        }
    }

    let client_stream = Compat01As03::new(client_stream_tail);
    // if !connector.use_haproxy_header(&hostname) {
    //     // clears the whole thing.
    //     proxy_buf.take();
    // }
    let result = connect_hostname(client_meta.clone(), connector, hostname, proxy_buf.freeze(), client_sink, client_stream).await;

    client_meta.stats_tx
        .send(SessionCommand {
            session_id: client_meta.session_id,
            data: SessionCommandData::Destroy,
        }).await.map_err(|e| {
            io::Error::new(io::ErrorKind::Other, e)
        })?;

    result
}

#[allow(clippy::needless_lifetimes)]
async fn unidirectional_copy<Si, St>(
    mut client_meta: ClientMetadata,
    mut sink: Si,
    mut stream: St,
    direction: Direction,
) -> Result<(), io::Error>
where
    Si: Sink<SniDetectRecord> + Unpin,
    St: Stream<Item=Result<SniDetectRecord, io::Error>> + Unpin,
    Si::SinkError: std::error::Error + Sync + Send + 'static,
{
    loop {
        let (stream_next, stream_tail) = stream.into_future().await;
        stream = stream_tail;
        match stream_next {
            Some(rec) => {
                let rec = rec.map_err(|err| {
                    warn!("unidirectional_copy::{}: {}", line!(), err);
                    err
                })?;
                let bytes = rec.rec_byte_size();

                let data = match direction {
                    Direction::BackendToClient => SessionCommandData::XmitBackendToClient(bytes),
                    Direction::ClientToBackend => SessionCommandData::XmitClientToBackend(bytes),
                };

                client_meta.stats_tx.send(SessionCommand {
                    session_id: client_meta.session_id,
                    data,
                }).await.map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, e)
                }).map_err(|err| {
                    warn!("unidirectional_copy::{}: {}", line!(), err);
                    err
                })?;

                sink.send(rec).await.map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, e)
                }).map_err(|err| {
                    warn!("unidirectional_copy::{}: {}", line!(), err);
                    err
                })?;
            }
            None => break,
        }
    }

    warn!("closing sink!");
    sink.close().await.map_err(|e| {
        io::Error::new(io::ErrorKind::Other, e)
    })?;
    warn!("closed sink!");
    drop(sink);

    let sess_cmd = SessionCommand {
        session_id: client_meta.session_id,
        data: match direction {
            Direction::BackendToClient => SessionCommandData::ShutdownRead,
            Direction::ClientToBackend => SessionCommandData::ShutdownWrite,
        },
    };
    client_meta.stats_tx.send(sess_cmd).await.map_err(|e| {
        io::Error::new(io::ErrorKind::Other, e)
    })?;
    Ok(())
}

pub async fn bidirectional_copy<A, B>(
    meta: ClientMetadata,
    client_stream: Compat01As03<SplitStream<Framed<A, SniDetectorCodec>>>,
    client_sink: Compat01As03Sink<SplitSink<Framed<A, SniDetectorCodec>>, SniDetectRecord>,
    server_stream: Compat01As03<SplitStream<Framed<B, SniPassCodec>>>,
    server_sink: Compat01As03Sink<SplitSink<Framed<B, SniPassCodec>>, SniDetectRecord>,
) -> Result<(), io::Error>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
    B: AsyncRead + AsyncWrite + Send + 'static,
{
    let server_to_client = unidirectional_copy(
        meta.clone(), client_sink, server_stream,
        Direction::BackendToClient);

    let client_to_server = unidirectional_copy(
        meta.clone(), server_sink, client_stream,
        Direction::ClientToBackend);

    let (s2c, c2s) = future::join(
        server_to_client,
        client_to_server,
    ).await;

    s2c?;
    c2s?;

    Ok(())
}

fn duration_milliseconds(dur: Duration) -> u64 {
    let mut out = 0;
    out += u64::from(dur.subsec_nanos()) / 1_000_000;
    out += dur.as_secs() * 1_000;
    out
}

async fn connect_hostname_helper<A>(
    mut client_meta: ClientMetadata,
    resolver: Arc<dyn Resolver + Send + Sync + Unpin>,
    hostname: String,
    proxy_line: Bytes,
    client_sink: Compat01As03Sink<SplitSink<Framed<A, SniDetectorCodec>>, SniDetectRecord>,
    client_stream: Compat01As03<SplitStream<Framed<A, SniDetectorCodec>>>,
) -> Result<(), io::Error>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    use futures01::stream::Stream;

    let client_meta_copy = client_meta.clone();
    let stream_fut = resolver.resolve(&hostname)
        .timeout(Duration::from_secs(3))
        .map_err(move |e| {
            if let Some(inner) = e.into_inner() {
                inner
            } else {
                warn!(
                    "{}: failed to connect to backend within allotted time",
                    client_meta_copy.session_id.fmt_base62(),
                );
                io::Error::new(io::ErrorKind::Other, "backend connection timeout")
            }
        });

    let (sockaddr_pair, socket) = Compat01As03::new(stream_fut).await?;
    info!(
        "{}: connected to backend after {}ms",
        client_meta.session_id.fmt_base62(),
        duration_milliseconds(client_meta.start_time.elapsed())
    );

    let (backend_sink, backend_stream) = SniPassCodec.framed(socket).split();
    let mut backend_sink = Compat01As03Sink::new(backend_sink);
    let backend_stream = Compat01As03::new(backend_stream);

    if let Some(pair) = sockaddr_pair {
        warn!("{} connection", client_meta.session_id.fmt_base62());
        let send_res = client_meta.stats_tx.send(SessionCommand {
            session_id: client_meta.session_id,
            data: SessionCommandData::Connected(
                hostname,
                pair,
                client_meta.start_time.elapsed(),
            ),
        }).await;

        if let Err(err) = send_res {
            info!(
                "{}: failed to send to stats collector: {}",
                client_meta.session_id.fmt_base62(),
                err,
            );

            return Err(io::Error::new(io::ErrorKind::Other, err))
        }
    }

    backend_sink.send(SniDetectRecord::PassThrough(proxy_line)).await.map_err(|e| {
        io::Error::new(io::ErrorKind::Other, e)
    })?;
    
    bidirectional_copy(client_meta.clone(),
        client_stream, client_sink, backend_stream, backend_sink).await?;

    Ok(())
}

async fn connect_hostname<A>(
    client_meta: ClientMetadata,
    resolver: Arc<dyn Resolver + Send + Sync + Unpin>,
    hostname: String,
    proxy_line: Bytes,
    client_sink: Compat01As03Sink<SplitSink<Framed<A, SniDetectorCodec>>, SniDetectRecord>,
    client_stream: Compat01As03<SplitStream<Framed<A, SniDetectorCodec>>>,
) -> Result<(), io::Error>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let connection_id = client_meta.session_id;
    let connect_time = client_meta.start_time;

    info!(
        "{}: requested to connect to backend {}",
        connection_id.fmt_base62(),
        hostname
    );

    let result = connect_hostname_helper(client_meta, resolver, hostname, proxy_line, client_sink, client_stream).await;

    match result {
        Ok(()) => {
            info!(
                "{}: finished without error after {}ms",
                connection_id.fmt_base62(),
                duration_milliseconds(connect_time.elapsed()),
            );
        },
        Err(ref err) => {
            info!(
                "{}: finished with error after {}ms: {}",
                connection_id.fmt_base62(),
                duration_milliseconds(connect_time.elapsed()),
                err,
            );
        }
    };

    result
}

fn write_handshake_sample(connection_id: &Ksuid, handshake: &[u8]) {
    let conn_id_str = connection_id.fmt_base62();
    info!("logging bad handshake sample: {}", conn_id_str);
    let filename = format!("bad-handshake-{}.bin", conn_id_str);
    match File::create(&filename) {
        Ok(mut file) => {
            if let Err(err) = file.write_all(handshake) {
                let _ = remove_file(&filename);
                warn!("failed to write bad handshake sample: {}", err);
            }
        }
        Err(err) => {
            warn!("failed to create bad handshake sample: {}", err);
        }
    }
}
