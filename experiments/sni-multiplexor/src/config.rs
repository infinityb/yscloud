use std::pin::Pin;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::future::Future;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::TcpStream;
use tokio::net::unix::UnixStream;

use crate::sni::{SocketAddrPair, ALERT_UNRECOGNIZED_NAME};

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl AsyncReadWrite for UnixStream {}

impl AsyncReadWrite for TcpStream {}

type NetworkStream = Box<dyn AsyncReadWrite + Send + Sync>;

// //

// impl Future for ConnectFuture {
//     type Item = (Option<SocketAddrPair>, NetworkStream);
//     type Error = io::Error;

//     fn poll(&mut self) -> Poll<Self::Item, io::Error> {
//         use std::mem;

//         match self.inner {
//             State::Tcp(ref mut fut) => match fut.poll()? {
//                 Async::Ready(x) => {
//                     let addresses = SocketAddrPair::from_pair(x.local_addr()?, x.peer_addr()?)?;
//                     Ok(Async::Ready((Some(addresses), Box::new(x))))
//                 }
//                 Async::NotReady => Ok(Async::NotReady),
//             },
//             State::Unix(ref mut fut) => match fut.poll()? {
//                 Async::Ready(x) => Ok(Async::Ready((None, Box::new(x)))),
//                 Async::NotReady => Ok(Async::NotReady),
//             },
//             State::Error(_) => {
//                 let e = match mem::replace(&mut self.inner, State::Empty) {
//                     State::Error(e) => e,
//                     _ => unreachable!(),
//                 };

//                 Err(e)
//             }
//             State::Empty => panic!("can't poll stream twice"),
//         }
//     }
// }

pub trait Resolver {
    // type Fut: Future<Output=io::Result<(Option<SocketAddrPair>, NetworkStream)>>;

    fn use_haproxy_header(&self, hostname: &str) -> bool;

    fn resolve(&self, hostname: &str) -> Pin<Box<dyn Future<Output=io::Result<(Option<SocketAddrPair>, NetworkStream)>>>>;
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

    fn resolve(&self, hostname: &str) -> Pin<Box<dyn Future<Output=io::Result<(Option<SocketAddrPair>, NetworkStream)>>>> {
        async {
            let addr = self.hostnames.get(hostname)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, ALERT_UNRECOGNIZED_NAME)
                })?;
            connect(&addr.location).await
        }.boxed()
    }
}

pub async fn connect(location: &NetworkLocation) -> impl Future<Output=io::Result<NetworkStream>> {
    match *location {
        NetworkLocation::Unix(ref addr) => {
            let connected = UnixStream::connect(addr).await?;
            Ok(Box::new(connected) as NetworkStream)
        },
        NetworkLocation::Tcp(ref addr) => {
            let connected = TcpStream::connect(addr).await?;
            Ok(Box::new(connected) as NetworkStream)
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
