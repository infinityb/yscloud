use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::net::UnixStream;

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl AsyncReadWrite for UnixStream {}

impl AsyncReadWrite for TcpStream {}

pub type NetworkStream = Box<dyn AsyncReadWrite + Send + Sync + Unpin>;
