use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::unix::io::AsRawFd;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::buf::{Buf, BufMut};
use mio::unix::EventedFd;
use mio::{IoVec, Poll as MioPoll, PollOpt, Ready as MioReady, Token};
use socket2::Socket;
use tokio::io::PollEvented;
use tokio::io::{AsyncRead, AsyncWrite};

macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

struct StreamSocketInner(Socket);

impl io::Read for StreamSocketInner {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }
}

impl io::Write for StreamSocketInner {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }

    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }
}

impl mio::event::Evented for StreamSocketInner {
    fn register(
        &self,
        poll: &MioPoll,
        token: Token,
        interest: MioReady,
        opts: PollOpt,
    ) -> io::Result<()> {
        let raw_fd = self.0.as_raw_fd();
        EventedFd(&raw_fd).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &MioPoll,
        token: Token,
        interest: MioReady,
        opts: PollOpt,
    ) -> io::Result<()> {
        let raw_fd = self.0.as_raw_fd();
        EventedFd(&raw_fd).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &MioPoll) -> io::Result<()> {
        let raw_fd = self.0.as_raw_fd();
        EventedFd(&raw_fd).deregister(poll)
    }
}

struct StreamSocket {
    io: PollEvented<StreamSocketInner>,
}

impl StreamSocket {
    // == Poll IO functions that takes `&self` ==
    //
    // They are not public because (taken from the doc of `PollEvented`):
    //
    // While `PollEvented` is `Sync` (if the underlying I/O type is `Sync`), the
    // caller must ensure that there are at most two tasks that use a
    // `PollEvented` instance concurrently. One for reading and one for writing.
    // While violating this requirement is "safe" from a Rust memory model point
    // of view, it will result in unexpected behavior in the form of lost
    // notifications and tasks hanging.

    fn poll_read_priv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        ready!(self.io.poll_read_ready(cx, mio::Ready::readable()))?;

        match self.io.get_mut().0.read(buf) {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.io.clear_read_ready(cx, mio::Ready::readable())?;
                Poll::Pending
            }
            x => Poll::Ready(x),
        }
    }

    fn poll_read_buf_priv<B: BufMut>(
        &self,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<io::Result<usize>> {
        ready!(self.io.poll_read_ready(cx, mio::Ready::readable()))?;

        let r = unsafe {
            // The `IoVec` type can't have a 0-length size, so we create a bunch
            // of dummy versions on the stack with 1 length which we'll quickly
            // overwrite.
            let b1: &mut [u8] = &mut [0];
            let b2: &mut [u8] = &mut [0];
            let b3: &mut [u8] = &mut [0];
            let b4: &mut [u8] = &mut [0];
            let b5: &mut [u8] = &mut [0];
            let b6: &mut [u8] = &mut [0];
            let b7: &mut [u8] = &mut [0];
            let b8: &mut [u8] = &mut [0];
            let b9: &mut [u8] = &mut [0];
            let b10: &mut [u8] = &mut [0];
            let b11: &mut [u8] = &mut [0];
            let b12: &mut [u8] = &mut [0];
            let b13: &mut [u8] = &mut [0];
            let b14: &mut [u8] = &mut [0];
            let b15: &mut [u8] = &mut [0];
            let b16: &mut [u8] = &mut [0];
            let mut bufs: [&mut IoVec; 16] = [
                b1.into(),
                b2.into(),
                b3.into(),
                b4.into(),
                b5.into(),
                b6.into(),
                b7.into(),
                b8.into(),
                b9.into(),
                b10.into(),
                b11.into(),
                b12.into(),
                b13.into(),
                b14.into(),
                b15.into(),
                b16.into(),
            ];
            let n = buf.bytes_vec_mut(&mut bufs);
            self.io.get_mut().read_bufs(&mut bufs[..n])
        };

        match r {
            Ok(n) => {
                unsafe {
                    buf.advance_mut(n);
                }
                Poll::Ready(Ok(n))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.io.clear_read_ready(cx, mio::Ready::readable())?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_write_priv(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        ready!(self.io.poll_write_ready(cx))?;

        match self.io.get_mut().write(buf) {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.io.clear_write_ready(cx)?;
                Poll::Pending
            }
            x => Poll::Ready(x),
        }
    }

    fn poll_write_buf_priv<B: Buf>(
        &self,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<io::Result<usize>> {
        ready!(self.io.poll_write_ready(cx))?;

        let r = {
            // The `IoVec` type can't have a zero-length size, so create a dummy
            // version from a 1-length slice which we'll overwrite with the
            // `bytes_vec` method.
            static DUMMY: &[u8] = &[0];
            let iovec = <&IoVec>::from(DUMMY);
            let mut bufs = [iovec; 64];
            let n = buf.bytes_vec(&mut bufs);
            self.io.get_mut().write_bufs(&bufs[..n])
        };
        match r {
            Ok(n) => {
                buf.advance(n);
                Poll::Ready(Ok(n))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.io.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncRead for StreamSocket {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [MaybeUninit<u8>]) -> bool {
        false
    }

    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_priv(cx, buf)
    }

    fn poll_read_buf<B: BufMut>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<io::Result<usize>> {
        self.poll_read_buf_priv(cx, buf)
    }
}

impl AsyncWrite for StreamSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_priv(cx, buf)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // tcp flush is a no-op
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut socket = PollEvented::get_mut(&mut self.io);
        if let Err(e) = socket.0.shutdown(std::net::Shutdown::Write) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(()))
    }

    fn poll_write_buf<B: Buf>(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut B,
    ) -> Poll<io::Result<usize>> {
        self.poll_write_buf_priv(cx, buf)
    }
}
