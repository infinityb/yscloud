// use std::pin::Unpin;

pub mod dynamic;
pub use self::dynamic::{
    DynamicSocket,
};

pub trait Socket<'a> {
    type ReadHalf: tokio::io::AsyncRead + Send + Unpin + 'a;

    type WriteHalf: AsyncWriteClose + Send + Unpin + 'a;

    fn shutdown_write(&self) -> Result<(), tokio::io::Error>;

    fn shutdown(&self) -> Result<(), tokio::io::Error>;

    fn split(&'a mut self) -> (Self::ReadHalf, Self::WriteHalf);
}

pub trait AsyncWriteClose: tokio::io::AsyncWrite {
    fn close_write(&self) -> Result<(), tokio::io::Error>;
}

impl<T: ?Sized + AsyncWriteClose + Unpin> AsyncWriteClose for Box<T> {
    fn close_write(&self) -> Result<(), tokio::io::Error> {
        (**self).close_write()
    }
}

impl<'a> Socket<'a> for tokio::net::UnixStream {
    type ReadHalf = tokio::net::unix::ReadHalf<'a>;

    type WriteHalf = tokio::net::unix::WriteHalf<'a>;

    fn shutdown_write(&self) -> Result<(), tokio::io::Error> {
        tokio::net::UnixStream::shutdown(self, std::net::Shutdown::Write)
    }

    fn shutdown(&self) -> Result<(), tokio::io::Error> {
        tokio::net::UnixStream::shutdown(self, std::net::Shutdown::Both)
    }

    fn split(&'a mut self) -> (Self::ReadHalf, Self::WriteHalf) {
        tokio::net::UnixStream::split(self)
    }
}

impl<'a> Socket<'a> for tokio::net::TcpStream {
    type ReadHalf = tokio::net::tcp::ReadHalf<'a>;

    type WriteHalf = tokio::net::tcp::WriteHalf<'a>;

    fn shutdown_write(&self) -> Result<(), tokio::io::Error> {
        tokio::net::TcpStream::shutdown(self, std::net::Shutdown::Write)
    }

    fn shutdown(&self) -> Result<(), tokio::io::Error> {
        tokio::net::TcpStream::shutdown(self, std::net::Shutdown::Both)
    }

    fn split(&'a mut self) -> (Self::ReadHalf, Self::WriteHalf) {
        tokio::net::TcpStream::split(self)
    }
}

impl<'a> AsyncWriteClose for tokio::net::unix::WriteHalf<'a> {
    fn close_write(&self) -> Result<(), tokio::io::Error> {
        self.as_ref().shutdown(std::net::Shutdown::Write)
    }
}

impl<'a> AsyncWriteClose for tokio::net::tcp::WriteHalf<'a> {
    fn close_write(&self) -> Result<(), tokio::io::Error> {
        self.as_ref().shutdown(std::net::Shutdown::Write)
    }
}