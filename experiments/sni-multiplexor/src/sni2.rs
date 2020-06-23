use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::BytesMut;
use copy_arena::{Allocator, Arena};
use futures::future::{self, Future, FutureExt};
use futures::stream::StreamExt;
use ppp::{parse_v1_header, parse_v2_header};
use tokio::io::AsyncRead;
use tokio::net::{TcpStream, UnixStream};
use ksuid::Ksuid;
use socket_traits::{AsyncWriteClose, DynamicSocket, Socket};
use tls::decode_client_hello;
use tokio::sync::Mutex;
use tracing::{event, Level};

use crate::context;
use crate::ioutil::{read_into, write_from, BinStr};
use crate::model::{ClientCtx, HaproxyProxyHeaderVersion, NetworkLocationAddress, SocketAddrPair};
use crate::resolver::{BackendManager, Resolver2};
use crate::sni_base::get_server_names;
use crate::state_track::{Session, SessionManager};

enum SniDetectionError {
    Truncated,
}

fn detect_sni_name(scratch: &mut Allocator, data: &[u8], eof: bool) -> io::Result<Option<String>> {
    let client_hello = match decode_client_hello(scratch, data) {
        Ok(hello) => hello,
        Err(err) => {
            if err.is_truncated() && !eof {
                return Ok(None);
            }
            return Err(io::Error::new(io::ErrorKind::Other, "ALERT_INTERNAL_ERROR"));
            // return Err(ALERT_INTERNAL_ERROR);
        }
    };

    event!(Level::DEBUG, "got hello: {:#?}", client_hello);

    let server_names = match get_server_names(&client_hello) {
        Ok(snames) => snames,
        Err(err) => {
            event!(Level::WARN, "encountered {:?} while detecting SNI name", err);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "ALERT_UNRECOGNIZED_NAME",
            ));
            // return Err(ALERT_UNRECOGNIZED_NAME);
        }
    };

    if server_names.0.len() != 1 {
        return Err(io::Error::new(io::ErrorKind::Other, "ALERT_INTERNAL_ERROR"));
        // return Err(ALERT_INTERNAL_ERROR);
    }

    let server_name = server_names.0[0].0.to_string();

    Ok(Some(server_name))
}

fn detect_haproxy_header(
    proxy_header_version: HaproxyProxyHeaderVersion,
    sdata: &[u8],
    eof: bool,
) -> io::Result<Option<(usize, ppp::model::Header)>> {
    use ppp::error::ParseError;

    let res = match proxy_header_version {
        HaproxyProxyHeaderVersion::Version1 => parse_v1_header(sdata),
        HaproxyProxyHeaderVersion::Version2 => parse_v2_header(sdata),
    };
    match res {
        Ok((rest, header)) => Ok(Some((sdata.len() - rest.len(), header))),
        Err(ParseError::Incomplete) if eof => {
            Err(io::Error::new(io::ErrorKind::Other, "incomplete handshake"))
        }
        Err(ParseError::Incomplete) => Ok(None),
        Err(ParseError::Failure) => Err(io::Error::new(io::ErrorKind::Other, "invalid handshake")),
    }
}

fn read_client_hello_sni<'a, R>(
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<String>> + 'a + Unpin
where
    R: AsyncRead + Unpin,
{
    struct AsyncReadClientHello<'a, R>
    where
        R: AsyncRead + Unpin,
    {
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
        arena: Arena,
        encountered_eof: bool,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadClientHello<'a, R> {
        type Output = io::Result<String>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadClientHello {
                ref mut source,
                ref mut into,
                ref mut arena,
                ref mut encountered_eof,
            } = *self;

            let into: &mut BytesMut = &mut *into;
            loop {
                let mut allocator = arena.allocator();

                match detect_sni_name(&mut allocator, &into[..], *encountered_eof)? {
                    Some(v) => return Poll::Ready(Ok(v)),
                    None => (), // try reading.
                }

                match AsyncRead::poll_read_buf(Pin::new(&mut *source), cx, into) {
                    Poll::Ready(Ok(wlen)) => {
                        if wlen == 0 {
                            *encountered_eof = true;
                        }
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
    }


    let source = Pin::new(source);
    let into = Pin::new(into);
    AsyncReadClientHello {
        source,
        into,
        arena: Arena::new(),
        encountered_eof: false,
    }
}

struct HaproxyHeader {
    raw_data: Vec<u8>,
    parsed: ppp::model::Header,
}

fn read_haproxy_header<'a, R>(
    proxy_header_version: HaproxyProxyHeaderVersion,
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<HaproxyHeader>> + 'a + Unpin
where
    R: AsyncRead + Unpin,
{
    struct AsyncReadHaproxyHeader<'a, R>
    where
        R: AsyncRead + Unpin,
    {
        proxy_header_version: HaproxyProxyHeaderVersion,
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
        encountered_eof: bool,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadHaproxyHeader<'a, R> {
        type Output = io::Result<HaproxyHeader>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadHaproxyHeader {
                proxy_header_version,
                ref mut source,
                ref mut into,
                ref mut encountered_eof,
            } = *self;

            let into: &mut BytesMut = &mut *into;
            loop {
                match detect_haproxy_header(proxy_header_version, &into[..], *encountered_eof)? {
                    Some((hlen, parsed)) => {
                        return Poll::Ready(Ok(HaproxyHeader {
                            raw_data: into.split_to(hlen).freeze().as_ref().to_vec(),
                            parsed,
                        }))
                    }
                    None => (), // try reading.
                }

                match AsyncRead::poll_read_buf(Pin::new(&mut *source), cx, into) {
                    Poll::Ready(Ok(wlen)) => {
                        if wlen == 0 {
                            *encountered_eof = true;
                        }
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
    }

    let source = Pin::new(source);
    let into = Pin::new(into);
    AsyncReadHaproxyHeader {
        proxy_header_version,
        source,
        into,
        encountered_eof: false,
    }
}

async fn dial_backend(addr: &NetworkLocationAddress) -> io::Result<DynamicSocket> {
    match *addr {
        NetworkLocationAddress::Unix(ref path) => {
            let sock = UnixStream::connect(path).await?;
            Ok(sock.into())
        }
        NetworkLocationAddress::Tcp(ref addr) => {
            let sock = TcpStream::connect(addr).await?;
            Ok(sock.into())
        }
    }
}

pub async fn sni_connect_and_copy(
    sessman: Arc<Mutex<SessionManager>>,
    backend_man: Arc<Mutex<BackendManager>>,
    mut client_addr: SocketAddrPair,
    mut client: TcpStream,
    client_ctx: ClientCtx,
    // canceler: Done,
) -> io::Result<()> {
    let backend_man: BackendManager = {
        let backend_man = backend_man.lock().await;
        BackendManager::clone(&*backend_man)
    };

    use tokio::time::{timeout, Duration};
    let client_hello_timeout = Duration::new(4, 0);

    let session_id = Ksuid::generate();

    let mut header_timeout = futures::stream::once(tokio::time::delay_for(client_hello_timeout));
    let (holder, mut canceler) = context::channel();

    let mut haproxy_passthrough_header: Option<HaproxyHeader> = None;
    let mut sni_hostname: Option<String> = None;
    let mut client_to_backend_bytes: u64 = 0;
    let mut backend_to_client_bytes: u64 = 0;
    let mut backend_write_buf = BytesMut::with_capacity(64 * 1024);
    let mut client_write_buf = BytesMut::with_capacity(64 * 1024);

    if let Some(haproxy_v) = client_ctx.proxy_header_version {
        let res = future::select(
            header_timeout.next(),
            read_haproxy_header(haproxy_v, &mut client, &mut backend_write_buf),
        )
        .await;

        match res {
            futures::future::Either::Left((timeout, _next_fut)) => {
                event!(Level::WARN, "HaproxyHeader timeout error: {:?}", timeout);

                let mut sessman = sessman.lock().await;
                sessman.mark_shutdown(&session_id);

                return Ok(());
            }
            futures::future::Either::Right((Ok(haproxy_header), _timeout_fut)) => {
                client_addr = haproxy_header.parsed.addresses.clone().into();
                haproxy_passthrough_header = Some(haproxy_header);
            }
            futures::future::Either::Right((Err(err), _timeout_fut)) => {
                event!(Level::WARN, "HaproxyHeader read error: {:?}", err);

                let mut sessman = sessman.lock().await;
                sessman.mark_shutdown(&session_id);

                return Ok(());
            }
        }
    }

    {
        let mut sessman = sessman.lock().await;
        sessman.add_session(Session::new(
            session_id,
            Instant::now(),
            client_addr.clone(),
            holder,
        ));
    }

    // let handshake_fut = async {
    //     if let Some(haproxy_v) = client_ctx.proxy_header_version {
    //         match read_haproxy_header(haproxy_v, &mut client, &mut backend_write_buf).await {
    //             Ok(haproxy_header) => {
    //                 let haproxy_header: Header = haproxy_header;

    //                 //
    //             }
    //             Err(err) => {
    //                 event!(Level::WARN, "HaproxyHeader read error: {:?}", err);
    //                 return false;
    //             }
    //         }
    //     }

    //     match read_client_hello_sni(&mut client, &mut backend_write_buf).await {
    //         Ok(hostname) => sni_hostname = hostname,
    //         Err(err) => {
    //             event!(Level::WARN, "ClientHello read error: {:?}", err);
    //             return false;
    //         }
    //     }

    //     true
    // };
    // if timeout_and_context_done(handshake_fut).await {
    //     // replace all these with a guard?
    //     let mut sessman = sessman.lock().await;
    //     sessman.mark_shutdown(&session_id);

    //     return Ok(())
    // }
    match Pin::new(&mut canceler)
        .await_with(timeout(
            client_hello_timeout,
            read_client_hello_sni(&mut client, &mut backend_write_buf),
        ))
        .await
    {
        Ok(Ok(Ok(hostname))) => sni_hostname = Some(hostname),
        Ok(Ok(Err(err))) => {
            event!(Level::WARN, "ClientHello read error: {:?}", err);

            // replace all these with a guard.
            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
        Ok(Err(..)) => {
            event!(Level::WARN, "ClientHello read timed out - terminating");

            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
        Err(()) => {
            event!(Level::WARN, "connection destroyed");

            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
    };

    let sni_hostname = sni_hostname.unwrap();

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_resolving(&session_id, &sni_hostname);
    }

    event!(Level::INFO, "looking up SNI name: {:?}", sni_hostname);
    let bset = match backend_man.resolve(&sni_hostname).await {
        Ok(bset) => bset,
        Err(err) => {
            event!(Level::WARN, "unimplemented - sending TLS error: {:?}", err);

            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
    };

    assert_eq!(bset.locations.len(), 1);

    if let Some(haproxy_v) = bset.haproxy_header_version {
        let tls_handshake = backend_write_buf.split().freeze();

        if client_ctx.proxy_header_version.is_some() && bset.haproxy_header_allow_passthrough {
            let header = haproxy_passthrough_header.as_ref().unwrap();
            write_haproxy_header_from_parsed(&mut backend_write_buf, haproxy_v, &header.parsed);
        } else {
            write_haproxy_header_from_socketaddr(&mut backend_write_buf, haproxy_v, &client_addr);
        }

        backend_write_buf.extend_from_slice(&tls_handshake[..])
    }

    let backend_info = &bset.locations.iter().next().unwrap().1;
    let mut backend = dial_backend(*backend_info).await?;

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_connecting(&session_id);
    }

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_connected(&session_id);
    }

    let (mut client_read, mut client_write) = client.split();
    let (mut backend_read, mut backend_write) = backend.split();

    let client_to_backend = async {
        loop {
            let length;
            // skip reading in the first iteration, if we have data
            if backend_write_buf.len() == 0 {
                length = read_into(&mut client_read, &mut backend_write_buf).await?;
            } else {
                length = backend_write_buf.len();
            }

            let mut to_write = backend_write_buf.split().freeze();

            if to_write.is_empty() {
                // EOF was reached.
                break;
            }
            while !to_write.is_empty() {
                write_from(&mut backend_write, &mut to_write).await?;
            }

            let mut sessman = sessman.lock().await;
            sessman.handle_xmit_client_to_backend(&session_id, length as u64);
        }

        backend_write.close_write()?;

        let mut sessman = sessman.lock().await;
        sessman.mark_shutdown_write(&session_id);

        io::Result::Ok(())
    };

    let backend_to_client = async {
        loop {
            let length;
            // skip reading in the first iteration, if we have data
            if client_write_buf.len() == 0 {
                length = read_into(&mut backend_read, &mut client_write_buf).await?;
            } else {
                length = client_write_buf.len();
            }
            let mut to_write = client_write_buf.split().freeze();

            if to_write.is_empty() {
                // EOF was reached.
                break;
            }
            while !to_write.is_empty() {
                write_from(&mut client_write, &mut to_write).await?;
            }

            let mut sessman = sessman.lock().await;
            sessman.handle_xmit_backend_to_client(&session_id, length as u64);
        }

        io::Result::Ok(())
    };

    let mut force_destroy = false;
    let completion_fut = future::join(client_to_backend, backend_to_client).boxed();
    match Pin::new(&mut canceler).await_with(completion_fut).await {
        Ok((res1, res2)) => {
            if let Err(err) = res1 {
                event!(Level::INFO, "client-to-backend error: {:?}", err);
                force_destroy = true;
            }

            if let Err(err) = res2 {
                event!(Level::INFO, "backend-to-client error: {:?}", err);
                force_destroy = true;
            }
        }
        Err(()) => force_destroy = true,
    }

    let mut sessman = sessman.lock().await;
    if force_destroy {
        sessman.destroy(&session_id);
    }
    sessman.mark_shutdown(&session_id);

    Ok(())
}

fn write_haproxy_header_from_parsed(
    dst: &mut BytesMut,
    v: HaproxyProxyHeaderVersion,
    h: &ppp::model::Header,
) {
    use ppp::model::Version;

    let mut new_h = h.clone();
    new_h.version = match v {
        HaproxyProxyHeaderVersion::Version1 => Version::One,
        HaproxyProxyHeaderVersion::Version2 => Version::Two,
    };

    match v {
        HaproxyProxyHeaderVersion::Version1 => {
            let header_string = ppp::to_string(new_h).expect("foobar");
            dst.extend_from_slice(header_string.as_bytes());
        }
        HaproxyProxyHeaderVersion::Version2 => {
            let bytes = ppp::to_bytes(new_h).expect("foobar");
            dst.extend_from_slice(&bytes)
        }
    }
}

fn write_haproxy_header_from_socketaddr(
    dst: &mut BytesMut,
    v: HaproxyProxyHeaderVersion,
    ap: &SocketAddrPair,
) {
    use ppp::model::{Command, Header, Protocol, Version};

    let version = match v {
        HaproxyProxyHeaderVersion::Version1 => Version::One,
        HaproxyProxyHeaderVersion::Version2 => Version::Two,
    };

    let header = Header::new(
        version,
        Command::Proxy,
        Protocol::Stream,
        Vec::new(),
        ap.clone().into(),
    );

    match v {
        HaproxyProxyHeaderVersion::Version1 => {
            let header_string = ppp::to_string(header).expect("foobar");
            dst.extend_from_slice(header_string.as_bytes());
        }
        HaproxyProxyHeaderVersion::Version2 => {
            let bytes = ppp::to_bytes(header).expect("foobar");
            dst.extend_from_slice(&bytes)
        }
    }
}
