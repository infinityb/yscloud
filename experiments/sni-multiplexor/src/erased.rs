use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::TcpStream;
use tokio::net::unix::UnixStream;

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl AsyncReadWrite for UnixStream {}

impl AsyncReadWrite for TcpStream {}

pub type NetworkStream = Box<dyn AsyncReadWrite + Send + Sync + Unpin>;
