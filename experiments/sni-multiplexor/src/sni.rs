use std::fmt::{self, Write};
use std::fs::{remove_file, File};
use std::io::{self, Write as IoWrite};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use copy_arena::Arena;
use ksuid::Ksuid;
use log::{debug, info, warn};
use std::time::Duration;
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::{future, Future, FutureExt, Sink, Stream};
use tokio::sync::mpsc::Sender;

use tls::{
    extract_record, ByteIterRead, ClientHello, Extension, ExtensionServerName, Handshake,
    RECORD_CONTENT_TYPE_HANDSHAKE,
};

use crate::config::Resolver;
use crate::state_track::{SessionCommand, SessionCommandData};

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
            warn!("Handshake too large: {} < {}", self.max_length, src.len());
            return Err(TokError::new(TokErrorKind::Other, ALERT_INTERNAL_ERROR));
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
    // pub fn local_addr(&self) -> SocketAddr {
    //     match *self {
    //         SocketAddrPair::V4(ref v4) => SocketAddr::V4(v4.local_addr),
    //         SocketAddrPair::V6(ref v6) => SocketAddr::V6(v6.local_addr),
    //     }
    // }

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
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub stats_tx: Sender<SessionCommand>,
}

pub fn start_client<A>(
    connector: Arc<Resolver + Send + Sync>,
    client_info: ClientMetadata,
    client: Framed<A, SniDetectorCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    info!(
        "{}: started connection from {}",
        client_info.session_id.fmt_base62(),
        client_info.client_conn.peer_addr()
    );

    // TODO/XXX: we need a timeout thing.
    let (client_sink, client_stream) = client.split();
    let mut proxy_buf = BytesMut::with_capacity(107);
    let closer_client_info2 = client_info.clone();

    #[allow(clippy::write_with_newline)]
    // The protocol dictates the use of carriage return, which writeln!() might
    // not do on unix-based systems, so we prefer specifying this manually.
    match client_info.client_conn {
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
            if let Some((inner, _stream)) = e.into_inner() {
                warn!(
                    "error with ClientHello for {}: {}",
                    hanshake_client_info.client_conn.peer_addr(),
                    inner
                );
            } else {
                warn!(
                    "{} failed to send ClientHello within allotted time",
                    hanshake_client_info.client_conn.peer_addr()
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
        ).or_else(move |err| {
            info!(
                "{}: finished with error after {}ms: ??",
                closer_client_info2.session_id.fmt_base62(),
                duration_milliseconds(closer_client_info2.start_time.elapsed()),
            );

            closer_client_info2.stats_tx
                .send(SessionCommand {
                    session_id: closer_client_info2.session_id,
                    data: SessionCommandData::Destroy,
                }).map(|_| ()).map_err(|_| ())
        });
    Box::new(client_fut)
}

fn bidirectional_copy<A, B>(
    meta: ClientMetadata,
    client: Framed<A, SniDetectorCodec>,
    server: Framed<B, SniPassCodec>,
) -> Box<dyn Future<Item = (), Error = io::Error> + Send + 'static>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
    B: AsyncRead + AsyncWrite + Send + 'static,
{
    let session2 = meta.clone();
    let session3 = meta.clone();
    let session4 = meta.clone();
    let session5 = meta.clone();
    let session6 = meta.clone();

    let (sink, stream) = server.split();
    let (client_sink, client_stream) = client.split();

    let s2c = stream.and_then(move |rec| {
        let bytes = rec.rec_byte_size();
        session6.clone().stats_tx
            .send(SessionCommand {
                session_id: session6.session_id,
                data: SessionCommandData::XmitBackendToClient(bytes),
            })
            .map(|_| rec)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }).forward(client_sink).map(move |_| {
        session2
            .stats_tx
            .send(SessionCommand {
                session_id: session2.session_id,
                data: SessionCommandData::ShutdownRead,
            })
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    });

    let c2s = client_stream.and_then(move |rec| {
        let bytes = rec.rec_byte_size();
        session5.clone().stats_tx
            .send(SessionCommand {
                session_id: session5.session_id,
                data: SessionCommandData::XmitClientToBackend(bytes),
            })
            .map(|_| rec)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }).forward(sink).map(move |_| {
        session3
            .stats_tx
            .send(SessionCommand {
                session_id: session3.session_id,
                data: SessionCommandData::ShutdownWrite,
            })
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    });
    let out = s2c.join(c2s).map(|_| ()).then(move |rr| {
        session4
            .stats_tx
            .send(SessionCommand {
                session_id: session4.session_id,
                data: SessionCommandData::Destroy,
            }).then(move |_| rr)
    });

    Box::new(out)
}

fn duration_milliseconds(dur: Duration) -> u64 {
    let mut out = 0;
    out += u64::from(dur.subsec_nanos()) / 1_000_000;
    out += dur.as_secs() * 1_000;
    out
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

    let session = client_info.clone();
    let connection_id = client_info.session_id;
    let connect_time = session.start_time;

    info!(
        "{}: requested to connect to backend {}",
        connection_id.fmt_base62(),
        hostname
    );

    let closer_client_info2 = client_info.clone();
    let hostname2 = hostname.clone();
    let copy = resolver
        .resolve(&hostname)
        .timeout(Duration::from_secs(3))
        .map_err(move |e| {
            if let Some(inner) = e.into_inner() {
                inner
            } else {
                warn!(
                    "{}: failed to connect to backend within allotted time",
                    connection_id.fmt_base62(),
                );
                io::Error::new(io::ErrorKind::Other, "backend connection timeout")
            }
        })
        .and_then(
            move |(pair, socket)| -> Box<dyn Future<Item = _, Error = io::Error> + Send + 'static> {
                let connection_id = connection_id;
                if let Some(pair) = pair {
                    // FIXME/XXX: this is currently wrong because
                    let fut = client_info
                        .stats_tx
                        .send(SessionCommand {
                            session_id: client_info.session_id,
                            data: SessionCommandData::Connected(
                                hostname2,
                                pair,
                                client_info.start_time.elapsed(),
                            ),
                        })
                        .map(move |_stats_tx| socket)
                        .map_err(move |e| {
                            info!(
                                "{}: failed to send to stats collector: {}",
                                connection_id.fmt_base62(),
                                e
                            );
                            io::Error::new(io::ErrorKind::Other, "stats-send failure")
                        });

                    Box::new(fut)
                } else {
                    Box::new(future::ok(socket))
                }
            },
        )
        .and_then(move |socket| {
            let framed_stream = SniPassCodec.framed(socket);
            framed_stream.send(SniDetectRecord::PassThrough(proxy_line))
        })
        .and_then(move |stream| {
            let client_stream = client_sink.reunite(client_stream).unwrap();
            info!(
                "{}: connected to backend after {}ms",
                connection_id.fmt_base62(),
                duration_milliseconds(connect_time.elapsed())
            );

            bidirectional_copy(session.clone(), client_stream, stream)
        })
        .map(move |()| {
            info!(
                "{}: finished without error after {}ms",
                connection_id.fmt_base62(),
                duration_milliseconds(connect_time.elapsed()),
            );
        })
        .or_else(move |err| {
            info!(
                "{}: finished with error after {}ms: {}",
                connection_id.fmt_base62(),
                duration_milliseconds(connect_time.elapsed()),
                err,
            );

            closer_client_info2.stats_tx
                .send(SessionCommand {
                    session_id: closer_client_info2.session_id,
                    data: SessionCommandData::Destroy,
                }).map(|_| ()).map_err(|_| ())
        });
    Box::new(copy)
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
