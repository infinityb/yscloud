use std::task::{Poll, Context};
use std::pin::Pin;
use std::io;

use bytes::{Bytes, BytesMut};
use futures::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn read_into<'a, R>(source: &mut R, into: &mut BytesMut) -> io::Result<usize> where R: AsyncRead + Unpin {
    struct AsyncReadAny<'a, R> where R: AsyncRead + Unpin {
        source: &'a mut R,
        into: &'a mut BytesMut,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadAny<'a, R>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadAny { ref mut source, ref mut into } = *self;
            AsyncRead::poll_read_buf(Pin::new(source), cx, into)
        }
    }

    AsyncReadAny { source, into }.await
}

// Writes from `to_write` into `destination`, consuming the data in `to_write`
pub async fn write_from<W>(destination: &mut W, to_write: &mut Bytes) -> io::Result<usize> where W: AsyncWrite + Unpin {
    struct AsyncWriteAny<'a, W> where W: AsyncWrite + Unpin {
        destination: &'a mut W,
        to_write: &'a [u8],
    }

    impl<'a, W: AsyncWrite + Unpin> Future for AsyncWriteAny<'a, W>
    {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncWriteAny { ref mut destination, ref mut to_write } = *self;
            AsyncWrite::poll_write(Pin::new(destination), cx, to_write)
        }
    }

    let length = AsyncWriteAny {
        destination,
        to_write: &to_write[..],
    }.await?;

    drop(to_write.split_to(length));
    Ok(length)
}