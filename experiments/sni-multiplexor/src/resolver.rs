use std::collections::{btree_map, BTreeMap};
use std::future::Future;
use std::sync::Arc;
use std::str::FromStr;

use log::info;
use futures::future;
use serde::{Deserialize, Serialize};
use ksuid::Ksuid;
use failure::{Error, Fail};
use rand::thread_rng;

use crate::model::{
    HaproxyProxyHeaderVersion,
    BackendArgs,
    BackendArgsFlags,
    NetworkLocationAddress,
};
use crate::error::tls::ALERT_UNRECOGNIZED_NAME;

pub trait Resolver2 {
    type ResolveFuture: Future<Output = Result<BackendSet, Error>>;

    fn resolve(&self, hostname: &str) -> Self::ResolveFuture;
}

#[derive(Clone)]
pub struct BackendManager {
    pub backends: Arc<BTreeMap<String, BackendSet>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BackendSet {
    // #[serde(with="crate::helpers::haproxy_proxy_header_version")]
    pub haproxy_header_version: Option<HaproxyProxyHeaderVersion>,
    pub haproxy_header_allow_passthrough: bool,
    pub locations: BTreeMap<Ksuid, NetworkLocationAddress>,
}


#[derive(Debug, Fail)]
#[fail(display = "haproxy header version must match other backends")]
struct HaproxyProxyHeaderVersionMismatch;

impl BackendManager {
    pub fn add_backend(&mut self, args: &BackendArgs) -> Result<Ksuid, Error> {
        let mut backends = BTreeMap::clone(&*self.backends);

        let mut haproxy_header_version = None;
        for flag in &args.flags {
            match *flag {
                BackendArgsFlags::UseHaproxy(v) => {
                    if haproxy_header_version.is_some() {
                        return Err(failure::format_err!("duplicate haproxy version argument"));
                    }
                    haproxy_header_version = Some(v);
                }
            }
        }

        let backend_id = Ksuid::generate();
        let nla = args.target_address.clone();

        match backends.entry(args.hostname.clone()) {
            btree_map::Entry::Occupied(mut occ) => {
                let backend_set = occ.get_mut();

                if backend_set.haproxy_header_version != haproxy_header_version {
                    return Err(HaproxyProxyHeaderVersionMismatch.into());
                }

                backend_set.locations.insert(backend_id, nla);
            }
            btree_map::Entry::Vacant(vac) => {
                let mut locations = BTreeMap::new();
                locations.insert(backend_id, nla);
                vac.insert(BackendSet {
                    haproxy_header_version,
                    haproxy_header_allow_passthrough: false,
                    locations: locations,
                });
            }
        }

        self.backends = Arc::new(backends);

        Ok(backend_id)
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

    pub fn replace_backend(&mut self, args: &BackendArgs) -> Result<Ksuid, Error> {
        let mut backends = BTreeMap::clone(&*self.backends);

        let mut haproxy_header_version = None;
        for flag in &args.flags {
            match *flag {
                BackendArgsFlags::UseHaproxy(v) => {
                    if haproxy_header_version.is_some() {
                        return Err(failure::format_err!("duplicate haproxy version argument"));
                    }
                    haproxy_header_version = Some(v);
                }
            }
        }

        let backend_id = Ksuid::generate();
        let nla = args.target_address.clone();

        match backends.entry(args.hostname.clone()) {
            btree_map::Entry::Occupied(mut occ) => {
                let backend_set = occ.get_mut();
                backend_set.haproxy_header_version = haproxy_header_version;
                backend_set.locations.clear();
                backend_set.locations.insert(backend_id, nla);
            }
            btree_map::Entry::Vacant(vac) => {
                let mut locations = BTreeMap::new();
                locations.insert(backend_id, nla);
                vac.insert(BackendSet {
                    haproxy_header_version,
                    haproxy_header_allow_passthrough: false,
                    locations,
                });
            }
        }

        self.backends = Arc::new(backends);

        Ok(backend_id)
    }

    pub fn remove_backends(&mut self, hostname: &str) {
        let mut backends = BTreeMap::clone(&*self.backends);
        backends.remove(hostname);
        self.backends = Arc::new(backends);
    }


    fn sync_resolve(&self, hostname: &str) -> Result<BackendSet, Error> {
        let backend_set = self
            .backends
            .get(hostname)
            .ok_or(ALERT_UNRECOGNIZED_NAME)?;

        Ok(backend_set_prune(backend_set))
    }
}

impl Resolver2 for BackendManager {
    type ResolveFuture = future::Ready<Result<BackendSet, Error>>;

    fn resolve(&self, hostname: &str) -> Self::ResolveFuture {
        future::ready(self.sync_resolve(hostname))
    }
}

fn backend_set_prune(bs: &BackendSet) -> BackendSet {
    use rand::seq::SliceRandom;

    let mut rng = thread_rng();

    let vv: Vec<(&Ksuid, &NetworkLocationAddress)> = bs.locations.iter().collect();
    let reduced_locations: BTreeMap<Ksuid, NetworkLocationAddress> = vv
        .choose_multiple(&mut rng, 1)
        .map(|(k, v)| (Clone::clone(*k), Clone::clone(*v)))
        .collect();

    BackendSet {
        haproxy_header_version: bs.haproxy_header_version,
        haproxy_header_allow_passthrough: bs.haproxy_header_allow_passthrough,
        locations: reduced_locations,
    }
}
