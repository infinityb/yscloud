use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::resolver::NetworkLocationAddress;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct Backend {
    pub use_haproxy_header: bool,
    pub location: NetworkLocationAddress,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResolverInit {
    pub hostnames: HashMap<String, Backend>,
}

// impl Resolver for MemoryResolver {
//     fn use_haproxy_header(&self, hostname: &str) -> bool {
//         match self.hostnames.get(hostname) {
//             Some(b) => b.use_haproxy_header,
//             None => false,
//         }
//     }

//     fn resolve(
//         &self,
//         hostname: &str,
//     ) -> Pin<Box<dyn Future<Output = io::Result<NetworkStream>> + Send>> {
//         use futures::future::FutureExt;

//         let addr_res = self.hostnames.get(hostname).cloned();
//         async {
//             let addr = match addr_res {
//                 Some(addr) => addr,
//                 None => {
//                     return Err(io::Error::new(
//                         io::ErrorKind::Other,
//                         ALERT_UNRECOGNIZED_NAME,
//                     ));
//                 }
//             };
//             connect(&addr.location).await
//         }
//             .boxed()
//     }
// }

// pub async fn connect(location: &NetworkLocation) -> io::Result<NetworkStream> {
//     match *location {
//         NetworkLocation::Unix(ref addr) => {
//             let connected = UnixStream::connect(addr).await?;
//             Ok(Box::new(connected) as NetworkStream)
//         }
//         NetworkLocation::Tcp(ref addr) => {
//             let connected = TcpStream::connect(addr).await?;
//             Ok(Box::new(connected) as NetworkStream)
//         }
//     }
// }

// #[test]
// fn test() {
//     let mut hostnames: HashMap<String, Backend> = HashMap::new();
//     hostnames.insert(
//         "irc2.yshi.org".into(),
//         Backend {
//             use_haproxy_header: true,
//             location: NetworkLocation::Tcp("45.79.89.177:443".parse::<SocketAddr>().unwrap()),
//         },
//     );
//     hostnames.insert(
//         "staceyell.com".into(),
//         Backend {
//             use_haproxy_header: true,
//             location: NetworkLocation::Unix("/var/run/https.staceyell.com".into()),
//         },
//     );

//     let resolver = MemoryResolver { hostnames };
//     println!("{}", serde_json::to_string_pretty(&resolver).unwrap());
//     panic!("{:?}", resolver);
// }
