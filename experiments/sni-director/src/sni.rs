use std::fmt::{self, Write};
use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use copy_arena::Arena;
use log::{debug, info, log, warn};
use std::time::Duration;
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};

use tokio::prelude::{future, Future, FutureExt, Sink, Stream};

use tls::{
    extract_record, ByteIterRead, ClientHello, Extension, ExtensionServerName, Handshake,
    RECORD_CONTENT_TYPE_HANDSHAKE,
};

use crate::config::Resolver;

#[derive(Debug, Copy, Clone)]
pub struct AlertError {
    alert_description: u8,
}

const DEFAULT_SNI_DETECTOR_MAX_LEN: usize = 20480;

pub const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};
pub const ALERT_UNRECOGNIZED_NAME: AlertError = AlertError {
    alert_description: 112,
};

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
        use tokio::io::{Error as TokError, ErrorKind as TokErrorKind};

        fn fixup_err(tls: TlsError) -> TokError {
            match tls.kind() {
                TlsErrorKind::ProtocolViolation => {
                    TokError::new(TokErrorKind::Other, ALERT_INTERNAL_ERROR)
                }
                TlsErrorKind::Truncated => {
                    warn!("I think this shouldn't happen: {}", tls);
                    TokError::new(TokErrorKind::Other, ALERT_INTERNAL_ERROR)
                }
                TlsErrorKind::Other => {
                    warn!("got Other error: {}", tls);
                    TokError::new(TokErrorKind::Other, ALERT_INTERNAL_ERROR)
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
            return Err(TokError::new(TokErrorKind::Other, ALERT_INTERNAL_ERROR));
        }

        let mut allocator = self.arena.allocator();
        let mut dst_iter = src.iter();

        let mut data_size = 0;
        while let Some(record) = extract_record(&mut dst_iter).map_err(fixup_err)? {
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

        debug!("got connection request for {}", server_name);
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
    local_addr: SocketAddrV4,
    peer_addr: SocketAddrV4,
}

#[derive(Clone)]
pub struct SocketAddrPairV6 {
    local_addr: SocketAddrV6,
    peer_addr: SocketAddrV6,
}

#[derive(Clone)]
pub enum SocketAddrPair {
    V4(SocketAddrPairV4),
    V6(SocketAddrPairV6),
}

impl SocketAddrPair {
    pub fn local_addr(&self) -> SocketAddr {
        match *self {
            SocketAddrPair::V4(ref v4) => SocketAddr::V4(v4.local_addr),
            SocketAddrPair::V6(ref v6) => SocketAddr::V6(v6.local_addr),
        }
    }

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

#[derive(Clone)]
pub struct ClientMetadata {
    pub addresses: SocketAddrPair,
}

pub fn start_client<A>(
    connector: Arc<Resolver + Send + Sync>,
    client_info: ClientMetadata,
    client: Framed<A, SniDetectorCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    // TODO/XXX: we need a timeout thing.
    let (client_sink, client_stream) = client.split();
    let mut proxy_buf = BytesMut::with_capacity(107);

    #[allow(clippy::write_with_newline)]
    // The protocol dictates the use of carriage return, which writeln!() might
    // not do on unix-based systems, so we prefer specifying this manually.
    match client_info.addresses {
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

    let hanshake_client_info = client_info.clone();
    let client_fut = client_stream
        .into_future()
        .timeout(Duration::from_secs(3))
        .map_err(move |e| {
            if let Some((inner, _)) = e.into_inner() {
                warn!(
                    "error with ClientHello for {}: {}",
                    hanshake_client_info.addresses.peer_addr(),
                    inner
                );
            } else {
                warn!(
                    "{} failed to send ClientHello within allotted time",
                    hanshake_client_info.addresses.peer_addr()
                );
            }
        })
        .and_then(
            move |(sni_data, client_stream)| -> Box<dyn Future<Item = (), Error = ()> + Send> {
                match sni_data {
                    Some(SniDetectRecord::SniHostname(hostname)) => {
                        if !connector.use_haproxy_header(&hostname) {
                            proxy_buf.take();
                        }
                        connect_hostname(
                            client_info,
                            connector,
                            hostname,
                            proxy_buf.freeze(),
                            client_sink.reunite(client_stream).unwrap(),
                        )
                    }
                    Some(..) | None => Box::new(future::err(())),
                }
            },
        );
    Box::new(client_fut)
}

fn bidirectional_copy<A, B>(
    client: Framed<A, SniDetectorCodec>,
    server: Framed<B, SniPassCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send + 'static>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
    B: AsyncRead + AsyncWrite + Send + 'static,
{
    let (sink, stream) = server.split();
    let (client_sink, client_stream) = client.split();
    Box::new(
        stream
            .forward(client_sink)
            .join(client_stream.forward(sink))
            .map(|_| ())
            .map_err(|_| ()),
    )
}

fn connect_hostname<A>(
    client_info: ClientMetadata,
    resolver: Arc<Resolver + Send + Sync>,
    hostname: String,
    proxy_line: Bytes,
    client: Framed<A, SniDetectorCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send + 'static>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (client_sink, client_stream) = client.split();

    let client_info = Arc::new(client_info);
    let client_info2 = Arc::clone(&client_info);
    let client_info3 = Arc::clone(&client_info);
    let client_info4 = Arc::clone(&client_info);
    let hostname = Arc::new(hostname);
    let hostname2 = Arc::clone(&hostname);
    let hostname3 = Arc::clone(&hostname);
    let hostname4 = Arc::clone(&hostname);
    let copy = resolver
        .resolve(&hostname)
        .timeout(Duration::from_secs(3))
        .map_err(move |e| {
            if let Some(inner) = e.into_inner() {
                inner
            } else {
                warn!(
                    "{}: failed to connect to backend for {} within allotted time",
                    client_info.addresses.peer_addr(),
                    hostname
                );
                io::Error::new(io::ErrorKind::Other, "backend connection timeout")
            }
        })
        .and_then(|stream| {
            let framed_stream = SniPassCodec.framed(stream);
            framed_stream.send(SniDetectRecord::PassThrough(proxy_line))
        })
        .map_err(|err| {
            warn!("error: {}", err);
        })
        .and_then(move |stream| {
            info!(
                "{}: connected to server for {}",
                client_info2.addresses.peer_addr(),
                hostname2
            );
            bidirectional_copy(client_sink.reunite(client_stream).unwrap(), stream)
        })
        .map(move |()| {
            info!(
                "{}: finished connection to {} successfully",
                client_info3.addresses.peer_addr(),
                hostname3
            );
        })
        .map_err(move |()| {
            info!(
                "{}: finished connection to {} with error",
                client_info4.addresses.peer_addr(),
                hostname4
            );
        });
    Box::new(copy)
}
