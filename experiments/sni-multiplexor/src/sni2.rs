use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use std::task::{Poll, Context};

use ppp::{model::Header, parse_v1_header, parse_v2_header};
use bytes::{Bytes, BytesMut};
use copy_arena::{Allocator, Arena};
use futures::future::{self, Future, FutureExt};
use futures::stream::{StreamExt};
use log::{debug, info, warn};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use socket_traits::{Socket, DynamicSocket, AsyncWriteClose};
use tls::decode_client_hello;
use tokio::sync::Mutex;
use ksuid::Ksuid;

use crate::resolver::BackendManager;
use crate::resolver2::HaproxyProxyVersion;
use crate::sni_base::{SocketAddrPair, AlertError, get_server_names, ALERT_UNRECOGNIZED_NAME, ALERT_INTERNAL_ERROR};
use crate::context::{self, Done};
use crate::state_track::{Session, SessionManager};


macro_rules! canceler_check {
    ($canceler:expr, $otherfut:expr, $canceled:expr) => (
        match Pin::new($canceler).await_with($otherfut).await {
            std::result::Result::Ok(val) => val,
            std::result::Result::Err(()) => $canceled,
        }
    );
}

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

    let server_names = match get_server_names(&client_hello) {
        Ok(snames) => snames,
        Err(err) => {
            warn!("encountered {:?} while detecting SNI name", err);
            return Err(io::Error::new(io::ErrorKind::Other, "ALERT_UNRECOGNIZED_NAME"));
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

fn detect_haproxy_header(proxy_header_version: HaproxyProxyVersion, sdata: &[u8], eof: bool) -> io::Result<Option<(usize, ppp::model::Header)>> {
    use ppp::error::ParseError;

    let res = match proxy_header_version {
        HaproxyProxyVersion::Version1 => parse_v1_header(sdata),
        HaproxyProxyVersion::Version2 => parse_v2_header(sdata),
    };
    match res {
        Ok((rest, header)) => {
            Ok(Some((sdata.len() - rest.len(), header)))
        }
        Err(ParseError::Incomplete) if eof => {
            Err(io::Error::new(io::ErrorKind::Other, "incomplete handshake"))
        }
        Err(ParseError::Incomplete) => {
            Ok(None)
        }
        Err(ParseError::Failure) => {
            Err(io::Error::new(io::ErrorKind::Other, "invalid handshake"))
        }
    }
}


pub fn read_into<'a, R>(
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<usize>> + 'a + Unpin
where
    R: AsyncRead + Unpin
{
    struct AsyncReadAny<'a, R> where R: AsyncRead + Unpin {
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadAny<'a, R>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadAny { ref mut source, ref mut into } = *self;
            let into: &mut BytesMut = &mut *into;
            AsyncRead::poll_read_buf(Pin::new(&mut *source), cx, into)
        }
    }

    let source = Pin::new(source);
    let into = Pin::new(into);
    AsyncReadAny { source, into }
}

// Writes from `to_write` into `destination`, consuming the data in `to_write`
pub fn write_from<'a, W>(
    destination: &'a mut W,
    to_write: &'a mut Bytes,
) -> impl Future<Output = io::Result<usize>> + 'a + Unpin
where
    W: AsyncWrite + Unpin
{
    use std::task::Poll;

    struct AsyncWriteAny<'a, W> where W: AsyncWrite + Unpin {
        destination: Pin<&'a mut W>,
        to_write: Pin<&'a mut Bytes>,
    }

    impl<'a, W: AsyncWrite + Unpin> Future for AsyncWriteAny<'a, W>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncWriteAny { ref mut destination, ref mut to_write } = *self;

            match AsyncWrite::poll_write(Pin::new(&mut *destination), cx, &to_write) {
                Poll::Ready(Ok(wlen)) => {
                    drop(to_write.split_to(wlen));
                    Poll::Ready(Ok(wlen))
                },
                Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    AsyncWriteAny {
        destination: Pin::new(destination),
        to_write: Pin::new(to_write),
    }
}

fn read_client_hello_sni<'a, R>(
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<String>> + 'a + Unpin
where
    R: AsyncRead + Unpin
{
    struct AsyncReadClientHello<'a, R> where R: AsyncRead + Unpin {
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
        arena: Arena,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadClientHello<'a, R>
    {
        type Output = io::Result<String>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadClientHello { ref mut source, ref mut into, ref mut arena } = *self;
            let into: &mut BytesMut = &mut *into;
            
            loop {
                return match AsyncRead::poll_read_buf(Pin::new(&mut *source), cx, into) {
                    Poll::Ready(Ok(wlen)) => {
                        let mut allocator = arena.allocator();
                        match detect_sni_name(&mut allocator, &into[..], wlen == 0)? {
                            Some(v) => Poll::Ready(Ok(v)),
                            None => continue, // re-read, which should block and re-register us
                        }
                    },
                    Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                    Poll::Pending => Poll::Pending,
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
    }
}

fn read_haproxy_header<'a, R>(
    proxy_header_version: HaproxyProxyVersion,
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<ppp::model::Header>> + 'a + Unpin
where
    R: AsyncRead + Unpin
{
    struct AsyncReadHaproxyHeader<'a, R> where R: AsyncRead + Unpin {
        proxy_header_version: HaproxyProxyVersion,
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadHaproxyHeader<'a, R>
    {
        type Output = io::Result<ppp::model::Header>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadHaproxyHeader { proxy_header_version, ref mut source, ref mut into } = *self;
            let into: &mut BytesMut = &mut *into;
            loop {
                return match AsyncRead::poll_read_buf(Pin::new(&mut *source), cx, into) {
                    Poll::Ready(Ok(wlen)) => {
                        match detect_haproxy_header(proxy_header_version, &into[..], wlen == 0)? {
                            Some((sz, h)) => {
                                drop(into.split_to(sz));
                                Poll::Ready(Ok(h))
                            }
                            None => continue, // re-read, which should block and re-register us
                        }
                    },
                    Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                    Poll::Pending => Poll::Pending,
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
    }
}

async fn dial_hostname(dialer_ctx: (), hostname: &str) -> io::Result<DynamicSocket> {
    let xx = TcpStream::connect("172.105.96.16:443").await?;
    Ok(xx.into())
}

pub struct ClientCtx {
    pub proxy_header_version: Option<HaproxyProxyVersion>,
}

impl ClientCtx {
}

pub async fn sni_connect_and_copy(
    sessman: Arc<Mutex<SessionManager>>,
    backend_man: Arc<Mutex<BackendManager>>,
    mut client_addr: SocketAddrPair,
    mut client: TcpStream,
    client_ctx: ClientCtx,
    // canceler: Done,
) -> io::Result<()>
{
    use tokio::time::{timeout, Duration};
    let client_hello_timeout = Duration::new(4, 0);

    let session_id = Ksuid::generate();

    let mut header_timeout = futures::stream::once(tokio::time::delay_for(client_hello_timeout));
    let (holder, mut canceler) = context::channel();

    {
        let mut sessman = sessman.lock().await;
        sessman.add_session(Session::new(
            session_id,
            Instant::now(),
            client_addr.clone(),
            holder,
        ));
    }

    let dialer_ctx = ();
    let mut client_to_backend_bytes: u64 = 0;
    let mut backend_to_client_bytes: u64 = 0;
    let mut backend_write_buf = BytesMut::with_capacity(64 * 1024);

    // let handshake_fut = async {
    //     if let Some(haproxy_v) = client_ctx.proxy_header_version {
    //         match read_haproxy_header(haproxy_v, &mut client, &mut backend_write_buf).await {
    //             Ok(haproxy_header) => {
    //                 let haproxy_header: Header = haproxy_header;

    //                 //
    //             }
    //             Err(err) => {
    //                 warn!("HaproxyHeader read error: {:?}", err);
    //                 return false;
    //             }
    //         }
    //     }

    //     match read_client_hello_sni(&mut client, &mut backend_write_buf).await {
    //         Ok(hostname) => sni_hostname = hostname,
    //         Err(err) => {
    //             warn!("ClientHello read error: {:?}", err);
    //             return false;
    //         }
    //     }

    //     true
    // };
    // if timeout(handshake_fut).await {
    //     // replace all these with a guard?
    //     let mut sessman = sessman.lock().await;
    //     sessman.mark_shutdown(&session_id);

    //     return Ok(())
    // }

    if let Some(haproxy_v) = client_ctx.proxy_header_version {
        let res = future::select(
            header_timeout.next(),
            read_haproxy_header(haproxy_v, &mut client, &mut backend_write_buf)).await;

        match res {
            futures::future::Either::Left((timeout, _next_fut)) => {
                warn!("HaproxyHeader timeout error: {:?}", timeout);

                let mut sessman = sessman.lock().await;
                sessman.mark_shutdown(&session_id);

                return Ok(());
            },
            futures::future::Either::Right((Ok(haproxy_header), _timeout_fut)) => {
                let haproxy_header: Header = haproxy_header;

                //
            },
            futures::future::Either::Right((Err(err), _timeout_fut)) => {
                warn!("HaproxyHeader read error: {:?}", err);

                let mut sessman = sessman.lock().await;
                sessman.mark_shutdown(&session_id);

                return Ok(());
            },
        }
    }

    let sni_hostname: String;
    match Pin::new(&mut canceler).await_with(timeout(client_hello_timeout, read_client_hello_sni(&mut client, &mut backend_write_buf))).await {
        Ok(Ok(Ok(hostname))) => sni_hostname = hostname,
        Ok(Ok(Err(err))) => {
            warn!("ClientHello read error: {:?}", err);

            // replace all these with a guard.
            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
        Ok(Err(..)) => {
            warn!("ClientHello read timed out - terminating");

            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
        Err(()) => {
            warn!("connection destroyed");

            let mut sessman = sessman.lock().await;
            sessman.mark_shutdown(&session_id);

            return Ok(());
        }
    };
    
    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_connecting(&session_id);
    }

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_backend_resolving(&session_id, &sni_hostname);
    }

    debug!("want to dial {:?}", sni_hostname);
    let mut backend = dial_hostname(dialer_ctx, &sni_hostname).await?;

    {
        let mut sessman = sessman.lock().await;
        sessman.mark_connected(&session_id);
    }

    let (mut client_read, mut client_write) = client.split();
    let (mut backend_read, mut backend_write) = backend.split();

    let canceler_copy = canceler.clone();
    let client_to_backend = async {
        let mut canceler = canceler_copy;
        'taskloop: loop {
            // to skip reading in the first iteration
            let length;
            if backend_write_buf.len() == 0 {
                let read_fut = read_into(&mut client_read, &mut backend_write_buf);
                length = canceler_check!(&mut canceler, read_fut, break 'taskloop)?;
            } else {
                length = backend_write_buf.len();
            }

            let mut to_write = backend_write_buf.split().freeze();
            
            if to_write.is_empty() {
                // EOF was reached.
                break;
            }

            while !to_write.is_empty() {
                let write_fut = write_from(&mut backend_write, &mut to_write);
                let _length = canceler_check!(&mut canceler, write_fut, break 'taskloop)?;
            }

            let mut sessman = sessman.lock().await;
            sessman.handle_xmit_client_to_backend(&session_id, length as u64);
        }

        backend_write.close_write()?;

        let mut sessman = sessman.lock().await;
        sessman.mark_shutdown_write(&session_id);

        io::Result::Ok(())
    };

    let canceler_copy = canceler.clone();
    let backend_to_client = async {
        let mut canceler = canceler_copy;
        let mut client_write_buf = BytesMut::with_capacity(64 * 1024);
        'taskloop: loop {
            let read_fut = read_into(&mut backend_read, &mut client_write_buf);
            let length = canceler_check!(&mut canceler, read_fut, break 'taskloop)?;

            let mut to_write = client_write_buf.split().freeze();

            if to_write.is_empty() {
                // EOF was reached.
                break;
            }

            while !to_write.is_empty() {
                let write_fut = write_from(&mut client_write, &mut to_write);
                let length = canceler_check!(&mut canceler, write_fut, break 'taskloop)?;
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
                info!("client-to-backend error: {:?}", err);
                force_destroy = true;
            }

            if let Err(err) = res2 {
                info!("backend-to-client error: {:?}", err);
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