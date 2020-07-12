use std::io;
use std::pin::Pin;
use std::task::{Poll, Context};
use std::mem::MaybeUninit;

use bytes::Buf;
use tokio::io::{AsyncWrite, AsyncRead};
use tokio::net::{UnixStream, TcpStream};

use super::{AsyncWriteClose, Socket};

pub struct DynamicSocket {
    inner: DynamicSocketInner,
}

impl DynamicSocket {
    fn shutdown(&self, how: std::net::Shutdown) -> Result<(), tokio::io::Error> {
        match self.inner {
            DynamicSocketInner::Unix(ref uds) => uds.shutdown(how),
            DynamicSocketInner::Tcp(ref tcp) => tcp.shutdown(how),
        }
    }

    fn split<'a>(
        &'a mut self,
    ) -> (
        Box<dyn AsyncRead + Send + Unpin + 'a>,
        Box<dyn AsyncWriteClose + Send + Unpin + 'a>,
    ) {
        match self.inner {
            DynamicSocketInner::Unix(ref mut uds) => {
                let (rh, wh) = uds.split();
                (Box::new(rh), Box::new(wh))
            }
            DynamicSocketInner::Tcp(ref mut tcp) => {
                let (rh, wh) = tcp.split();
                (Box::new(rh), Box::new(wh))
            }
        }
    }
}

enum DynamicSocketInner {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl From<UnixStream> for DynamicSocket {
    fn from(uds: UnixStream) -> DynamicSocket {
        DynamicSocket {
            inner: DynamicSocketInner::Unix(uds),
        }
    }
}

impl From<TcpStream> for DynamicSocket {
    fn from(tcp: TcpStream) -> DynamicSocket {
        DynamicSocket {
            inner: DynamicSocketInner::Tcp(tcp),
        }
    }
}

impl<'a> Socket<'a> for DynamicSocket {
    type ReadHalf = Box<dyn AsyncRead + Send + Unpin + 'a>;

    type WriteHalf = Box<dyn AsyncWriteClose + Send + Unpin + 'a>;

    fn shutdown_write(&self) -> Result<(), tokio::io::Error> {
        DynamicSocket::shutdown(self, std::net::Shutdown::Write)
    }

    fn shutdown(&self) -> Result<(), tokio::io::Error> {
        DynamicSocket::shutdown(self, std::net::Shutdown::Both)
    }

    fn split(&'a mut self) -> (Self::ReadHalf, Self::WriteHalf) {
        DynamicSocket::split(self)
    }
}

impl AsyncRead for DynamicSocket {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut[MaybeUninit<u8>]) -> bool {
        match self.inner {
            DynamicSocketInner::Unix(ref u) => u.prepare_uninitialized_buffer(buf),
            DynamicSocketInner::Tcp(ref t) => t.prepare_uninitialized_buffer(buf),
        }
    }

    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner {
            DynamicSocketInner::Unix(ref mut u) => Pin::new(u).poll_read(cx, buf),
            DynamicSocketInner::Tcp(ref mut t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for DynamicSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner {
            DynamicSocketInner::Unix(ref mut u) => Pin::new(u).poll_write(cx, buf),
            DynamicSocketInner::Tcp(ref mut t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_write_buf<B: Buf>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<io::Result<usize>> {
        match self.inner {
            DynamicSocketInner::Unix(ref mut u) => Pin::new(u).poll_write_buf(cx, buf),
            DynamicSocketInner::Tcp(ref mut t) => Pin::new(t).poll_write_buf(cx, buf),
        }
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.inner {
            DynamicSocketInner::Unix(ref mut u) => Pin::new(u).poll_flush(cx),
            DynamicSocketInner::Tcp(ref mut t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.inner {
            DynamicSocketInner::Unix(ref mut u) => Pin::new(u).poll_shutdown(cx),
            DynamicSocketInner::Tcp(ref mut t) => Pin::new(t).poll_shutdown(cx),
        }
    }
}
