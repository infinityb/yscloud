use std::collections::{BTreeMap, btree_map};
use std::collections::hash_map::RandomState;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::net::{Ipv4Addr, Ipv6Addr};

use futures::future;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use ksuid::Ksuid;
use smallvec::{self, SmallVec};
use tokio::sync::Mutex;

use crate::model::{NetworkLocationAddress, HaproxyProxyHeaderVersion};
use crate::error::tls::ALERT_UNRECOGNIZED_NAME;

const BACKEND_CANDIDATE_STACK_COUNT: usize = 8;
const DEFAULT_ATTEMPT_SCALING_FACTOR: Duration = Duration::from_secs(1);
const DEFAULT_STARTING_LAST_ATTEMPT_AGO: Duration = Duration::from_secs(600);

pub trait Resolver2 {
    type ResolveFuture: Future<Output = io::Result<BackendAddress>>;

    fn resolve(&self, hostname: &str) -> Self::ResolveFuture;
}

#[derive(Clone)]
pub struct BackendManager {
    data: Arc<BackendManagerData>,
}

struct BackendManagerData {
    unix_socket_hasher1: RandomState,
    unix_socket_hasher2: RandomState,
    backends: Mutex<BTreeMap<String, Backend>>,
    stats_table: Mutex<BTreeMap<BackendKeyInternal, Box<BackendStatistics>>>,
}


fn create_backend_key_internal(
    unix_socket_hasher1: &RandomState,
    unix_socket_hasher2: &RandomState,
    key: &BackendAddress,
) -> BackendKeyInternal {
    use std::hash::{BuildHasher, Hash, Hasher};

    match *key {
        BackendAddress::UnixSocket(ref usi) => {
            let mut hasher1 = unix_socket_hasher1.build_hasher();
            usi.hash(&mut hasher1);

            let mut hasher2 = unix_socket_hasher2.build_hasher();
            usi.hash(&mut hasher2);

            BackendKeyInternal::UnixSocket {
                unix_socket_hash1: hasher1.finish(),
                unix_socket_hash2: hasher2.finish(),
            }
        },
        BackendAddress::TcpV4(v) => BackendKeyInternal::TcpV4(v),
        BackendAddress::TcpV6(v) => BackendKeyInternal::TcpV6(v),
    }
}

/// You should randomize candidate list before passing it to this function.
fn backend_manager_lookup_best_backends<'backend>(
    candidates: &'backend [BackendAddress],
    unix_socket_hasher1: &RandomState,
    unix_socket_hasher2: &RandomState,
    stats: &BTreeMap<BackendKeyInternal, Box<BackendStatistics>>,
    into: &mut SmallVec<[&'backend BackendAddress; BACKEND_CANDIDATE_STACK_COUNT]>,
    limit: usize,
) {
    let now = Instant::now();

    for c in candidates {
        if limit <= into.len() {
            return;
        }

        let key_internal = create_backend_key_internal(
            unix_socket_hasher1, unix_socket_hasher2, c);

        if let Some(be_stat) = stats.get(&key_internal) {
            if be_stat.next_allowed_attempt <= now {
                into.push(c);
            }
        } else {
            into.push(c);
        }
    }
}

pub struct Backend {
    use_haproxy_header_v: HaproxyProxyHeaderVersion,
    address_resolvable: BackendAddressResolvable,
    
    next_address_update: Instant,
    cached_addresses: SmallVec<[BackendAddress; 16]>,
}

enum BackendAddressResolvable {
    Hostname(BackendAddressResolvableHostname),
    InMemoryList(Vec<BackendAddress>),
}

#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy, Ord, PartialOrd)]
enum BackendKeyInternal {
    UnixSocket {
        unix_socket_hash1: u64,
        unix_socket_hash2: u64,
    },
    TcpV4(Ipv4Addr),
    TcpV6(Ipv6Addr),
}

#[derive(Debug)]
enum BackendAddress {
    UnixSocket(PathBuf),
    TcpV4(Ipv4Addr),
    TcpV6(Ipv6Addr),
}

struct BackendAddressResolvableHostname {
    hostname: String,
    mode: BackendAddressResolvableHostnameMode,
}

enum BackendAddressResolvableHostnameMode {
    Ip { port: u16 },
    // Srv, // for later.
}

impl BackendManagerData {
    fn create_backend_key_internal(&self, key: &BackendAddress) -> BackendKeyInternal {
        use std::hash::{BuildHasher, Hash, Hasher};

        match *key {
            BackendAddress::UnixSocket(ref usi) => {
                let mut hasher1 = self.unix_socket_hasher1.build_hasher();
                usi.hash(&mut hasher1);

                let mut hasher2 = self.unix_socket_hasher2.build_hasher();
                usi.hash(&mut hasher2);

                BackendKeyInternal::UnixSocket {
                    unix_socket_hash1: hasher1.finish(),
                    unix_socket_hash2: hasher2.finish(),
                }
            },
            BackendAddress::TcpV4(v) => BackendKeyInternal::TcpV4(v),
            BackendAddress::TcpV6(v) => BackendKeyInternal::TcpV6(v),
        }
    }
}

struct BackendStatistics {
    // saturating failure count.
    failure_count: u32,
    attempt_scaling_factor: Duration,
    last_attempt: Instant,
    next_allowed_attempt: Instant,
}


impl Default for BackendStatistics {
    fn default() -> BackendStatistics {
        let now = Instant::now();
        BackendStatistics {
            failure_count: 0,
            attempt_scaling_factor: DEFAULT_ATTEMPT_SCALING_FACTOR,
            last_attempt: now - DEFAULT_STARTING_LAST_ATTEMPT_AGO,
            next_allowed_attempt: now - DEFAULT_STARTING_LAST_ATTEMPT_AGO,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BackendSet {
    pub locations: BTreeMap<Ksuid, NetworkLocation>,
}


#[test]
fn synchronous_in_memory_resolver() {
    let backend_paths = vec![
        BackendAddress::UnixSocket(PathBuf::from("/foobar1")),
        BackendAddress::UnixSocket(PathBuf::from("/foobar2")),
        BackendAddress::UnixSocket(PathBuf::from("/foobar3")),
    ];

    let mut candidates = SmallVec::new();

    let unix_socket_hasher1 = Default::default();
    let unix_socket_hasher2 = Default::default();
    let mut stats: BTreeMap<BackendKeyInternal, Box<BackendStatistics>> = Default::default();

    let backend0_ikey = create_backend_key_internal(
        &unix_socket_hasher1,
        &unix_socket_hasher2,
        &backend_paths[0],
    );

    stats.insert(backend0_ikey, Box::new(BackendStatistics {
        failure_count: 10,
        next_allowed_attempt: Instant::now() + Duration::new(1, 0),
        ..Default::default()
    }));

    backend_manager_lookup_best_backends(
        &backend_paths[..],
        &unix_socket_hasher1,
        &unix_socket_hasher2,
        &stats,
        &mut candidates,
        10);


    assert_eq!(candidates.len(), 2);
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

impl NetworkLocation {
    pub fn use_haproxy_header(&self) -> bool {
        self.use_haproxy_header_v
    }
}

impl BackendManager {}

fn backend_set_prune(bs: &BackendSet) -> BackendSet {
    let mut rng = &mut rand::thread_rng();
    let vv: Vec<_> = bs.locations.iter().map(|v| v.1.clone()).collect();
    BackendSet::from_list(vv.choose_multiple(&mut rng, 1).cloned().collect())
}
