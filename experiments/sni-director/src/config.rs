use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{self, TcpStream};
use tokio::net::unix::{self, UnixStream};
use tokio::prelude::{Async, Future, Poll};

use crate::sni::ALERT_UNRECOGNIZED_NAME;

/// Future returned by `Dail::dial` which will resolve to a
/// `NetworkStream` when the stream is connected.
#[derive(Debug)]
pub struct ConnectFuture {
    inner: State,
}

#[derive(Debug)]
enum State {
    Tcp(tcp::ConnectFuture),
    Unix(unix::ConnectFuture),
    Error(io::Error),
    Empty,
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl AsyncReadWrite for UnixStream {}

impl AsyncReadWrite for TcpStream {}

type NetworkStream = Box<dyn AsyncReadWrite + Send + Sync>;

impl Future for ConnectFuture {
    type Item = NetworkStream;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<NetworkStream, io::Error> {
        use std::mem;

        match self.inner {
            State::Tcp(ref mut fut) => match fut.poll()? {
                Async::Ready(x) => Ok(Async::Ready(Box::new(x))),
                Async::NotReady => Ok(Async::NotReady),
            },
            State::Unix(ref mut fut) => match fut.poll()? {
                Async::Ready(x) => Ok(Async::Ready(Box::new(x))),
                Async::NotReady => Ok(Async::NotReady),
            },
            State::Error(_) => {
                let e = match mem::replace(&mut self.inner, State::Empty) {
                    State::Error(e) => e,
                    _ => unreachable!(),
                };

                Err(e)
            }
            State::Empty => panic!("can't poll stream twice"),
        }
    }
}

pub trait Resolver {
    fn use_haproxy_header(&self, hostname: &str) -> bool;

    fn resolve(&self, hostname: &str) -> ConnectFuture;
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum NetworkLocation {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub struct Backend {
    use_haproxy_header: bool,
    location: NetworkLocation,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MemoryResolver {
    hostnames: HashMap<String, Backend>,
}

impl Resolver for MemoryResolver {
    fn use_haproxy_header(&self, hostname: &str) -> bool {
        match self.hostnames.get(hostname) {
            Some(b) => b.use_haproxy_header,
            None => false,
        }
    }

    fn resolve(&self, hostname: &str) -> ConnectFuture {
        let addr = match self.hostnames.get(hostname) {
            Some(v) => v,
            None => {
                let err = io::Error::new(io::ErrorKind::Other, ALERT_UNRECOGNIZED_NAME);
                return ConnectFuture {
                    inner: State::Error(err),
                };
            }
        };
        connect(&addr.location)
    }
}

pub fn connect(location: &NetworkLocation) -> ConnectFuture {
    match *location {
        NetworkLocation::Unix(ref addr) => ConnectFuture {
            inner: State::Unix(UnixStream::connect(addr)),
        },
        NetworkLocation::Tcp(ref addr) => ConnectFuture {
            inner: State::Tcp(TcpStream::connect(addr)),
        },
    }
}

#[test]
fn test() {
    let mut hostnames: HashMap<String, Backend> = HashMap::new();
    hostnames.insert(
        "irc2.yshi.org".into(),
        Backend {
            use_haproxy_header: true,
            location: NetworkLocation::Tcp("45.79.89.177:443".parse::<SocketAddr>().unwrap()),
        },
    );
    hostnames.insert(
        "staceyell.com".into(),
        Backend {
            use_haproxy_header: true,
            location: NetworkLocation::Unix("/var/run/https.staceyell.com".into()),
        },
    );

    let resolver = MemoryResolver { hostnames };
    println!("{}", serde_json::to_string_pretty(&resolver).unwrap());
    panic!("{:?}", resolver);
}
