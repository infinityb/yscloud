use std::{io, fmt};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

use tls::{ClientHello, Extension, ExtensionServerName};

pub const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};

pub const ALERT_UNRECOGNIZED_NAME: AlertError = AlertError {
    alert_description: 112,
};


#[derive(Debug, Copy, Clone)]
pub struct AlertError {
    alert_description: u8,
}

impl std::error::Error for AlertError {}

impl fmt::Display for AlertError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "alert #{}", self.alert_description)
    }
}

pub fn get_server_names<'arena>(
    hello: &ClientHello<'arena>,
) -> Result<&'arena ExtensionServerName<'arena>, io::Error> {
    for ext in hello.extensions.0 {
        match *ext {
            Extension::ServerName(name_ext) => return Ok(name_ext),
            Extension::Unknown(..) => (),
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, ALERT_INTERNAL_ERROR))
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

impl From<ppp::model::Header> for SocketAddrPair {
	fn from(h: ppp::model::Header) -> SocketAddrPair {
		match h.addresses {
			ppp::model::Addresses::IPv4 {
				source_address,
				destination_address,
				source_port,
				destination_port,
			} => SocketAddrPair::V4(SocketAddrPairV4 {
				local_addr: SocketAddrV4::new(destination_address.into(), destination_port.unwrap_or(0)),
				peer_addr: SocketAddrV4::new(source_address.into(), source_port.unwrap_or(0)),
			}),
			ppp::model::Addresses::IPv6 {
				source_address,
				destination_address,
				source_port,
				destination_port,
			} => SocketAddrPair::V6(SocketAddrPairV6 {
				local_addr: SocketAddrV6::new(
					destination_address.into(),
					destination_port.unwrap_or(0),
					0, 0),
				peer_addr: SocketAddrV6::new(
					source_address.into(),
					source_port.unwrap_or(0),
					0, 0),
			}),
			ppp::model::Addresses::Unix { .. } => SocketAddrPair::Unix,
			ppp::model::Addresses::None => SocketAddrPair::Unknown,
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
