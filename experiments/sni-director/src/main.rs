use std::fmt;
use std::io;
use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use copy_arena::Arena;
use log::{debug, info, log, warn};
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::{future, stream, Future, Stream};

use tls::{
    extract_record, ByteIterRead, ClientHello, Extension, ExtensionServerName, Handshake,
    RECORD_CONTENT_TYPE_HANDSHAKE,
};

#[derive(Debug, Copy, Clone)]
struct AlertError {
    alert_description: u8,
}

const DEFAULT_SNI_DETECTOR_MAX_LEN: usize = 20480;
const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};

impl std::error::Error for AlertError {}

impl fmt::Display for AlertError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "alert #{}", self.alert_description)
    }
}

enum SniDetectRecord {
    SniHostname(String),
    PassThrough(Bytes),
}

struct SniPassCodec;

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

struct SniDetectorCodec {
    max_length: usize,
    emitted_sni: bool,
    arena: Arena,
}

impl SniDetectorCodec {
    pub fn new() -> SniDetectorCodec {
        SniDetectorCodec {
            max_length: DEFAULT_SNI_DETECTOR_MAX_LEN,
            emitted_sni: false,
            arena: Arena::new(),
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

fn start_client<A>(
    connector: (),
    client: Framed<A, SniDetectorCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (client_sink, client_stream) = client.split();
    Box::new(
        client_stream
            .into_future()
            .map_err(|(e, _)| {
                info!("error on stream: {}", e);
            })
            .and_then(
                move |(sni_data, client_stream)| -> Box<dyn Future<Item = (), Error = ()> + Send> {
                    match sni_data {
                        Some(SniDetectRecord::SniHostname(hostname)) => connect_hostname(
                            connector,
                            hostname,
                            client_sink.reunite(client_stream).unwrap(),
                        ),
                        Some(..) | None => Box::new(future::err(())),
                    }
                },
            ),
    )
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
    _connector: (),
    hostname: String,
    client: Framed<A, SniDetectorCodec>,
) -> Box<dyn Future<Item = (), Error = ()> + Send + 'static>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (client_sink, client_stream) = client.split();
    match &hostname[..] {
        "staceyell.com" => {
            let addr = match "45.79.89.177:443".parse::<SocketAddr>() {
                Ok(v) => v,
                Err(err) => {
                    warn!("error: {}", err);
                    return Box::new(future::err(()));
                }
            };
            let copy = TcpStream::connect(&addr)
                .map_err(|err| {
                    warn!("error: {}", err);
                })
                .and_then(|stream| {
                    println!("connected to server");
                    bidirectional_copy(
                        client_sink.reunite(client_stream).unwrap(),
                        SniPassCodec.framed(stream),
                    )
                });
            Box::new(copy)
        }
        "google.com" => {
            let addr = match "216.58.193.78:443".parse::<SocketAddr>() {
                Ok(v) => v,
                Err(err) => {
                    warn!("error: {}", err);
                    return Box::new(future::err(()));
                }
            };
            let copy = TcpStream::connect(&addr)
                .map_err(|err| {
                    warn!("error: {}", err);
                })
                .and_then(|stream| {
                    println!("connected to server");
                    bidirectional_copy(
                        client_sink.reunite(client_stream).unwrap(),
                        SniPassCodec.framed(stream),
                    )
                });
            Box::new(copy)
        }
        _ => Box::new(future::err(())),
    }
}

fn main() {
    env_logger::init();

    let addr = "127.0.0.1:6142".parse().unwrap();
    let listener = TcpListener::bind(&addr).unwrap();

    let server = listener
        .incoming()
        .for_each(|socket| {
            let framed = SniDetectorCodec::new().framed(socket);
            tokio::spawn(start_client((), framed));
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    println!("server running on localhost:6142");

    tokio::run(server);
}
