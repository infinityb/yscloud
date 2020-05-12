use std::fmt::Write;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fmt, str};

use bytes::{Bytes, BytesMut};
use futures::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub fn read_into<'a, R>(
    source: &'a mut R,
    into: &'a mut BytesMut,
) -> impl Future<Output = io::Result<usize>> + 'a + Unpin
where
    R: AsyncRead + Unpin,
{
    struct AsyncReadAny<'a, R>
    where
        R: AsyncRead + Unpin,
    {
        source: Pin<&'a mut R>,
        into: Pin<&'a mut BytesMut>,
    }

    impl<'a, R: AsyncRead + Unpin> Future for AsyncReadAny<'a, R> {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncReadAny {
                ref mut source,
                ref mut into,
            } = *self;
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
    W: AsyncWrite + Unpin,
{
    struct AsyncWriteAny<'a, W>
    where
        W: AsyncWrite + Unpin,
    {
        destination: Pin<&'a mut W>,
        to_write: Pin<&'a mut Bytes>,
    }

    impl<'a, W: AsyncWrite + Unpin> Future for AsyncWriteAny<'a, W> {
        type Output = io::Result<usize>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let AsyncWriteAny {
                ref mut destination,
                ref mut to_write,
            } = *self;

            match AsyncWrite::poll_write(Pin::new(&mut *destination), cx, &to_write) {
                Poll::Ready(Ok(wlen)) => {
                    drop(to_write.split_to(wlen));
                    Poll::Ready(Ok(wlen))
                }
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

pub struct BinStr<'a>(pub &'a [u8]);

impl<'a> fmt::Debug for BinStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const SPECIALS: &[u8; 14] = b"0123456abtnvfr";
        const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

        f.write_char('b')?;
        f.write_char('"')?;

        let mut from = 0;
        for (i, c) in self.0.iter().enumerate() {
            if usize::from(*c) < SPECIALS.len() {
                // guaranteed all-ascii
                f.write_str(str::from_utf8(&self.0[from..i]).unwrap())?;
                let mut special_char_buf = [b'\\', 0];
                special_char_buf[1] = SPECIALS[*c as usize];
                f.write_str(str::from_utf8(&special_char_buf).unwrap())?;
                from = i + 1;
                continue;
            }
            if *c < 32 || 128 <= *c {
                // guaranteed all-ascii
                f.write_str(str::from_utf8(&self.0[from..i]).unwrap())?;
                let mut hex_char_buf = [b'\\', b'x', 0, 0];
                hex_char_buf[2] = HEX_CHARS[(*c >> 4) as usize];
                hex_char_buf[3] = HEX_CHARS[(*c & 0x0F) as usize];
                f.write_str(str::from_utf8(&hex_char_buf).unwrap())?;
                from = i + 1;
                continue;
            }
        }

        // guaranteed all-ascii
        f.write_str(str::from_utf8(&self.0[from..]).unwrap())?;

        f.write_char('"')
    }
}

pub struct BinStrBuf(pub Vec<u8>);

impl fmt::Debug for BinStrBuf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let bin_str = BinStr(&self.0);
        write!(f, "{:?}.to_vec()", bin_str)
    }
}
