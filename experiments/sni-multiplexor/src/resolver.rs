use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::sni::ALERT_UNRECOGNIZED_NAME;

pub trait Resolver2 {
    type ResolveFuture: Future<Output = io::Result<BackendSet>>;

    fn resolve(&self, hostname: &str) -> Self::ResolveFuture;
}

#[derive(Clone)]
pub struct BackendManager {
    pub backends: Arc<BTreeMap<String, BackendSet>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetworkLocation {
    pub use_haproxy_header_v: bool,
    pub address: NetworkLocationAddress,
    pub stats: (),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum NetworkLocationAddress {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BackendSet {
    pub locations: Vec<NetworkLocation>,
}

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[serde(rename_all = "snake_case")]
// pub struct ConnStats {
//     //
// }

impl std::str::FromStr for NetworkLocationAddress {
    type Err = Box<dyn std::error::Error>;

    fn from_str(from: &str) -> Result<Self, Self::Err> {
        // FIXME: only handle unix paths for now.
        Ok(NetworkLocationAddress::Unix(PathBuf::from(from)))
    }
}

impl NetworkLocation {
    pub fn use_haproxy_header(&self) -> bool {
        self.use_haproxy_header_v
    }
}

impl BackendManager {
    pub fn replace_backend(&mut self, hostname: &str, nl: NetworkLocation) {
        let mut backends = BTreeMap::clone(&*self.backends);
        backends.insert(
            hostname.to_string(),
            BackendSet {
                locations: vec![nl],
            },
        );
        self.backends = Arc::new(backends);
    }

    pub fn remove_backend(&mut self, hostname: &str) {
        let mut backends = BTreeMap::clone(&*self.backends);
        backends.remove(hostname);
        self.backends = Arc::new(backends);
    }

    fn sync_resolve(&self, hostname: &str) -> io::Result<BackendSet> {
        let backend_set = self
            .backends
            .get(hostname)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, ALERT_UNRECOGNIZED_NAME))?;

        Ok(backend_set_prune(backend_set))
    }
}

impl Resolver2 for BackendManager {
    type ResolveFuture = future::Ready<io::Result<BackendSet>>;

    fn resolve(&self, hostname: &str) -> Self::ResolveFuture {
        future::ready(self.sync_resolve(hostname))
    }
}

fn backend_set_prune(bs: &BackendSet) -> BackendSet {
    let mut rng = &mut rand::thread_rng();
    BackendSet {
        locations: bs.locations.choose_multiple(&mut rng, 1).cloned().collect(),
    }
}
