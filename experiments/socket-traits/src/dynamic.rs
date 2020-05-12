use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::net::UnixStream;

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
