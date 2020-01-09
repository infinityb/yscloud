use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ksuid::Ksuid;

use crate::resolver::{BackendSet};
use crate::model::{HaproxyProxyHeaderVersion, NetworkLocationAddress};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ConfigBackend {
    pub use_haproxy_header: Option<HaproxyProxyHeaderVersion>,
    pub locations: Vec<NetworkLocationAddress>,
}

impl From<ConfigBackend> for BackendSet {
    fn from(b: ConfigBackend) -> BackendSet {
        BackendSet {
            haproxy_header_version: b.use_haproxy_header,
            haproxy_header_allow_passthrough: false,
            locations: b.locations.into_iter().map(|v| (Ksuid::generate(), v)).collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigResolverInit {
    pub hostnames: HashMap<String, ConfigBackend>,
}