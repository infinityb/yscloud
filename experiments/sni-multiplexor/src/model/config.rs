use std::collections::HashMap;
use std::net::SocketAddr;

use ksuid::Ksuid;
use serde::{Deserialize, Serialize};

use crate::model::{HaproxyProxyHeaderVersion, NetworkLocationAddress};
use crate::resolver::BackendSet;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ConfigBackend {
    pub use_haproxy_header: Option<HaproxyProxyHeaderVersion>,
    pub use_haproxy_passthrough: bool,
    pub locations: Vec<NetworkLocationAddress>,
}

impl From<ConfigBackend> for BackendSet {
    fn from(b: ConfigBackend) -> BackendSet {
        BackendSet {
            haproxy_header_version: b.use_haproxy_header,
            haproxy_header_allow_passthrough: b.use_haproxy_passthrough,
            locations: b
                .locations
                .into_iter()
                .map(|v| (Ksuid::generate(), v))
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigResolverInit {
    pub hostnames: HashMap<String, ConfigBackend>,
    pub upstream_dns: SocketAddr,
}
