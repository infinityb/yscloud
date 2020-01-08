use std::collections::{btree_map, BTreeMap};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use log::info;
use futures::future;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use ksuid::Ksuid;


use crate::sni_base::ALERT_UNRECOGNIZED_NAME;

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
    pub locations: BTreeMap<Ksuid, NetworkLocation>,
}

impl BackendSet {
    pub fn from_list(loc_list: Vec<NetworkLocation>) -> BackendSet {
        let mut locations = BTreeMap::new();

        for v in loc_list {
            locations.insert(Ksuid::generate(), v);
        }

        BackendSet { locations }
    }
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
    pub fn add_backend(&mut self, hostname: &str, nl: NetworkLocation) -> Ksuid {
        let mut backends = BTreeMap::clone(&*self.backends);

        info!("adding {} backend with: {:?}", hostname, nl);
        let backend_id = Ksuid::generate();

        match backends.entry(hostname.to_string()) {
            btree_map::Entry::Occupied(mut occ) => {
                let backend_set = occ.get_mut();
                backend_set.locations.insert(backend_id, nl);
                println!("backends: {:#?}", backend_set.locations);
            }
            btree_map::Entry::Vacant(vac) => {
                let mut locations = BTreeMap::new();
                locations.insert(backend_id, nl);
                vac.insert(BackendSet { locations });
            }
        }

        self.backends = Arc::new(backends);

        backend_id
    }

    pub fn remove_backend(&mut self, hostname: &str, nl: Ksuid) {
        let mut backends = BTreeMap::clone(&*self.backends);

        info!("removing a backend from {}: {:?}", hostname, nl);

        if let btree_map::Entry::Occupied(mut occ) = backends.entry(hostname.to_string()) {
            let backend_set = occ.get_mut();

            backend_set.locations.remove(&nl);
            let backend_location_count = backend_set.locations.len();
            drop(backend_set);

            if backend_location_count == 0 {
                occ.remove_entry();
            }
        }

        self.backends = Arc::new(backends);
    }

    pub fn replace_backend(&mut self, hostname: &str, nl: NetworkLocation) {
        let mut backends = BTreeMap::clone(&*self.backends);

        info!("replacing {} backend with: {:?}", hostname, nl);

        backends.insert(
            hostname.to_string(),
            BackendSet::from_list(vec![nl]),
        );
        self.backends = Arc::new(backends);
    }

    pub fn remove_backends(&mut self, hostname: &str) {
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
    let vv: Vec<_> = bs.locations.iter().map(|v| v.1.clone()).collect();
    BackendSet::from_list(vv.choose_multiple(&mut rng, 1).cloned().collect())
}
