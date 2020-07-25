use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::path::PathBuf;

use failure::{Error, Fail};
use serde::{Deserialize, Serialize};

pub mod config;

#[derive(Clone, Debug)]
pub struct BackendArgs {
    pub hostname: String,
    pub target_address: NetworkLocationAddress,
    pub flags: Vec<BackendArgsFlags>,
}

#[derive(Debug, Clone)]
pub enum BackendArgsFlags {
    UseHaproxy(HaproxyProxyHeaderVersion),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HaproxyProxyHeaderVersion {
    Version1,
    Version2,
}

#[derive(Debug)]
pub struct ClientCtx {
    pub proxy_header_version: Option<HaproxyProxyHeaderVersion>,
}

pub struct BackendCtx {
    pub proxy_header_version: Option<HaproxyProxyHeaderVersion>,
    pub proxy_header_passthrough: bool,
}

#[derive(Clone)]
pub struct SocketAddrPairV4 {
    pub local_addr: SocketAddrV4,
    pub peer_addr: SocketAddrV4,
}

#[derive(Clone)]
pub struct SocketAddrPairV6 {
    pub local_addr: SocketAddrV6,
    pub peer_addr: SocketAddrV6,
}

#[derive(Clone)]
pub enum SocketAddrPair {
    V4(SocketAddrPairV4),
    V6(SocketAddrPairV6),
    Unix,
    Unknown,
}

impl From<ppp::model::Addresses> for SocketAddrPair {
    fn from(h: ppp::model::Addresses) -> SocketAddrPair {
        use ppp::model::Addresses;

        match h {
            Addresses::IPv4 {
                source_address,
                destination_address,
                source_port,
                destination_port,
            } => SocketAddrPair::V4(SocketAddrPairV4 {
                local_addr: SocketAddrV4::new(
                    destination_address.into(),
                    destination_port.unwrap_or(0),
                ),
                peer_addr: SocketAddrV4::new(source_address.into(), source_port.unwrap_or(0)),
            }),
            Addresses::IPv6 {
                source_address,
                destination_address,
                source_port,
                destination_port,
            } => SocketAddrPair::V6(SocketAddrPairV6 {
                local_addr: SocketAddrV6::new(
                    destination_address.into(),
                    destination_port.unwrap_or(0),
                    0,
                    0,
                ),
                peer_addr: SocketAddrV6::new(source_address.into(), source_port.unwrap_or(0), 0, 0),
            }),
            Addresses::Unix { .. } => SocketAddrPair::Unix,
            Addresses::None => SocketAddrPair::Unknown,
        }
    }
}

impl Into<ppp::model::Addresses> for SocketAddrPair {
    fn into(self) -> ppp::model::Addresses {
        use ppp::model::Addresses;

        match self {
            SocketAddrPair::V4(ref v4) => {
                let source_address = v4.peer_addr.ip().octets();
                let destination_address = v4.local_addr.ip().octets();
                let source_port = Some(v4.peer_addr.port());
                let destination_port = Some(v4.local_addr.port());

                Addresses::IPv4 {
                    source_address,
                    destination_address,
                    source_port,
                    destination_port,
                }
            }
            SocketAddrPair::V6(ref v6) => {
                let source_address = v6.peer_addr.ip().segments();
                let destination_address = v6.local_addr.ip().segments();
                let source_port = Some(v6.peer_addr.port());
                let destination_port = Some(v6.local_addr.port());

                Addresses::IPv6 {
                    source_address,
                    destination_address,
                    source_port,
                    destination_port,
                }
            }
            SocketAddrPair::Unix => Addresses::None,
            SocketAddrPair::Unknown => Addresses::None,
        }
    }
}

impl SocketAddrPair {
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match *self {
            SocketAddrPair::V4(ref v4) => Some(SocketAddr::V4(v4.peer_addr)),
            SocketAddrPair::V6(ref v6) => Some(SocketAddr::V6(v6.peer_addr)),
            _ => None,
        }
    }

    pub fn from_pair(
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<SocketAddrPair, io::Error> {
        match (local_addr, peer_addr) {
            (SocketAddr::V4(local_addr), SocketAddr::V4(peer_addr)) => {
                Ok(SocketAddrPair::V4(SocketAddrPairV4 {
                    local_addr,
                    peer_addr,
                }))
            }
            (SocketAddr::V6(local_addr), SocketAddr::V6(peer_addr)) => {
                Ok(SocketAddrPair::V6(SocketAddrPairV6 {
                    local_addr,
                    peer_addr,
                }))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Other,
                "address families must match",
            )),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum NetworkLocationAddress {
    Unix(PathBuf),
    Tcp(SocketAddr),
    Hostname(String),
    Srv(String),
}

impl std::str::FromStr for NetworkLocationAddress {
    type Err = Box<dyn std::error::Error>;

    fn from_str(from: &str) -> Result<Self, Self::Err> {
        const UNIX_PREFIX: &str = "unix:";
        const DNS_PREFIX: &str = "dns:";

        if from.starts_with(UNIX_PREFIX) {
            let path = PathBuf::from(&from[UNIX_PREFIX.len()..]);
            return Ok(NetworkLocationAddress::Unix(path));
        }

        if from.starts_with(DNS_PREFIX) {
            let name = &from[DNS_PREFIX.len()..];
            return Ok(NetworkLocationAddress::Hostname(name.to_string()));
        }

        if let Ok(sa) = from.parse::<SocketAddr>() {
            return Ok(NetworkLocationAddress::Tcp(sa));
        }

        if from.chars().any(|x| x == '/') {
            return Ok(NetworkLocationAddress::Unix(PathBuf::from(from)));
        }

        Ok(NetworkLocationAddress::Hostname(from.to_string()))
    }
}

#[derive(Debug, Copy, Clone, Fail)]
#[fail(display = "must be a valid unix path starting with `/`, hostname, or SocketAddr")]
pub struct InvalidNetworkLocationAddress;
