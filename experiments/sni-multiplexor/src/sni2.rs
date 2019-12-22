use std::io;
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use copy_arena::{Allocator, Arena};
use tokio::io::{AsyncRead, AsyncWrite};
use log::{debug, warn};
use std::time::Duration;
use futures::future;
use futures::task::Poll;
use futures::future::Future;
use futures::task::Context;

use socket_traits::{Socket, DynamicSocket, AsyncWriteClose};
use tls::decode_client_hello;

use super::sni::{AlertError, get_server_names, ALERT_UNRECOGNIZED_NAME, ALERT_INTERNAL_ERROR};

enum SniDetectionError {
    Truncated,
}

fn detect_sni_name(scratch: &mut Allocator, data: &[u8], eof: bool) -> Result<Option<String>, AlertError> {
    let client_hello = match decode_client_hello(scratch, data) {
        Ok(hello) => hello,
        Err(err) => {
            if err.is_truncated() && !eof {
                return Ok(None);
            }
            return Err(ALERT_INTERNAL_ERROR);
        }
    };

    let server_names = match get_server_names(&client_hello) {
        Ok(snames) => snames,
        Err(err) => {
            warn!("encountered {:?} while detecting SNI name", err);
            return Err(ALERT_UNRECOGNIZED_NAME);
        }
    };

    if server_names.0.len() != 1 {
        return Err(ALERT_INTERNAL_ERROR);
    }

    let server_name = server_names.0[0].0.to_string();

    Ok(Some(server_name))
}

async fn read_client_hello_sni<R>(
    mut reader: &mut R,
    buffer: &mut BytesMut,
    canceler: ()
) -> io::Result<String> where R: AsyncRead + Unpin
{
    let mut arena = Arena::new();
    let mut encountered_eof = false;
    loop {
        let length = read_into(&mut reader, buffer).await?;
        if length == 0 {
            encountered_eof = true;
        }

        let mut allocator = arena.allocator();

        match detect_sni_name(&mut allocator, &buffer[..], encountered_eof) {
            Ok(Some(name)) => {
                return Ok(name);
            },
            Ok(None) => (),
            Err(err) => {
                debug!(
                    "failed to detect SNI name - bailing badly: {}",
                    err
                );
                return Err(io::Error::new(io::ErrorKind::Other, "bad SNI handshake - terminated without alert"));
            }
        }
    }
}

async fn dial_hostname(dialer_ctx: (), hostname: &str) -> io::Result<DynamicSocket> {
    unimplemented!();
}

async fn read_into<'a, R>(source: &mut R, into: &mut BytesMut) -> io::Result<usize> where R: AsyncRead + Unpin {
    struct AsyncReadAny<'a, R> where R: AsyncRead + Unpin {
        source: &'a mut R,
        into: &'a mut BytesMut,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadAny<'a, R>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            
            AsyncRead::poll_read_buf(Pin::new(&mut *self.source), cx, self.into)
        }
    }


    AsyncReadAny { source, into }.await
}

async fn write_from<W>(destination: &mut W, to_write: &mut Bytes) -> io::Result<usize> where W: AsyncWrite + Unpin {
    struct AsyncWriteAny<'a, W> where W: AsyncWrite + Unpin {
        destination: &'a mut W,
        to_write: &'a mut Bytes,
    }

    impl<'a, W: AsyncWrite + Unpin> Future for AsyncWriteAny<'a, W>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            AsyncWrite::poll_write(Pin::new(&mut *self.destination), cx, &self.to_write[..])
        }
    }

    AsyncWriteAny { destination, to_write }.await
}


async fn sni_connect_and_copy<'sock, 'split, S>(client: &'sock mut S, canceler: ()) -> io::Result<()>
    where S: Socket<'split>, 'sock: 'split 
{
    use tokio::timer::delay_for;

    let dialer_ctx = ();

    let local_timer = delay_for(Duration::new(4, 0));

    let mut client_to_backend_bytes: u64 = 0;
    let mut backend_to_client_bytes: u64 = 0;

    let (mut client_read, mut client_write) = client.split();
    let mut backend_write_buf = BytesMut::with_capacity(64 * 1024);

    let sni_hostname = read_client_hello_sni(&mut client_read, &mut backend_write_buf, canceler).await?;
    debug!("want to dial {:?}", sni_hostname);
    let mut backend = dial_hostname(dialer_ctx, &sni_hostname).await?;
    let (mut backend_read, mut backend_write) = backend.split();

    let client_to_backend = async {
        loop {
            // to skip reading in the first iteration
            if backend_write_buf.len() == 0 {
                read_into(&mut Pin::new(&mut client_read), &mut backend_write_buf).await?;
            }

            let mut to_write = backend_write_buf.freeze();
            
            if to_write.is_empty() {
                // EOF was reached.
                break;
            }

            while !to_write.is_empty() {
                let length = write_from(&mut backend_write, &mut to_write).await?;
                drop(to_write.split_to(length));
            }

            backend_write_buf = to_write.try_mut().unwrap();
            backend_write_buf.clear();
        }

        backend_write.close_write()?;

        io::Result::Ok(())
    };

    let backend_to_client = async {
        let mut client_write_buf = BytesMut::with_capacity(64 * 1024);
        loop {
            read_into(&mut Pin::new(&mut backend_read), &mut client_write_buf).await?;
            
            let mut to_write = client_write_buf.freeze();

            if to_write.is_empty() {
                // EOF was reached.
                break;
            }

            while !to_write.is_empty() {
                let length = write_from(&mut client_write, &mut to_write).await?;
                drop(to_write.split_to(length));
            }

            client_write_buf = to_write.try_mut().unwrap();
            client_write_buf.clear();
        }

        client_write.close_write()?;

        io::Result::Ok(())
    };

    let (res1, res2) = future::join(client_to_backend, backend_to_client).await;
    
    res1?;
    res2?;

    Ok(())
}