use std::fmt;
use std::time::{Instant, Duration};
use std::net::Ipv6Addr;
use std::convert::TryInto;

use futures::StreamExt;
use netlink_packet_core::NetlinkPayload;
use netlink_packet_route::rtnl::constants::{AF_INET6, AF_INET};
use netlink_packet_route::{
    NetlinkHeader, NetlinkMessage, RtnlMessage, NLM_F_DUMP, NLM_F_REQUEST,
    AddressMessage, NLM_F_CREATE, NLM_F_ACK, NLM_F_EXCL, RT_SCOPE_UNIVERSE,
};
use netlink_packet_route::rtnl::address::nlas::Nla;
use netlink_packet_route::rtnl::constants::{RTPROT_RA, RT_TABLE_UNSPEC};
use netlink_proto::{
    new_connection,
    sys::SocketAddr,
};

use super::ValidityPeriod;

#[derive(Debug)]
pub struct AddAddressRequest {
    pub address: AddressSpec,
    pub preferred_until: Option<Instant>,
    pub valid_until: Option<Instant>,
}

#[derive(Debug)]
pub struct AddAddressResponse {
    //
}

// --

#[derive(Debug)]
pub struct ListAddressesRequest {
}

impl Default for ListAddressesRequest {
    fn default() -> ListAddressesRequest {
        ListAddressesRequest {}
    }
}

#[derive(Debug)]
pub struct ListAddressesResponse {
    pub addresses: Vec<AddressInfo>,
}

// --

#[derive(Debug)]
pub struct DelAddressRequest {
    pub address: AddressSpec,
}

#[derive(Debug)]
pub struct DelAddressResponse {
    //
}

// --

#[derive(Debug)]
pub struct AddressSpec {
    pub iface_index: u32,
    pub address: std::net::IpAddr,
    pub prefix_len: u8,
}

// --

#[derive(Debug)]
pub struct AddressInfo {
    pub interface_index: u32,
    pub address: std::net::IpAddr,
    pub preferred_until: Option<Instant>,
    pub valid_until: Option<Instant>,
}

// --

pub async fn list_addresses(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: ListAddressesRequest,
) -> Result<ListAddressesResponse, netlink_proto::Error<RtnlMessage>>
{
    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_DUMP | NLM_F_REQUEST,
            ..Default::default()
        },
        payload: RtnlMessage::GetAddress(AddressMessage::default()).into(),
    };

    let now = Instant::now();
    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    let mut addresses = Vec::new();
    while let Some(packet) = response.next().await {
        let inner;
        let address_w;
        let mut address: Vec<u8> = Vec::new();
        let mut ifname = String::new();
        let mut cacheinfo = None;

        if let NetlinkPayload::InnerMessage(inner_tmp) = packet.payload {
            inner = inner_tmp;
        } else {
            continue;
        }
        if let RtnlMessage::NewAddress(address_tmp) = inner {
            address_w = address_tmp;
        } else {
            continue;
        }
        for i in &address_w.nlas {
            if let Nla::Address(address_tmp) = i {
                address = address_tmp.clone();
            }
            if let Nla::CacheInfo(cinfo_tmp) = i {
                if IfaCacheInfo::BUFFER_LENGTH <= cinfo_tmp.len() {
                    cacheinfo = Some(IfaCacheInfo::copy_from_slice(&cinfo_tmp));
                }
            }
        }
        let addr = match address.len() {
            4 => {
                let mut addr_tmp: [u8; 4] = [0; 4];
                addr_tmp.clone_from_slice(&address[..]);
                std::net::IpAddr::V4(addr_tmp.into())
            },
            16 => {
                let mut addr_tmp: [u8; 16] = [0; 16];
                addr_tmp.clone_from_slice(&address[..]);
                std::net::IpAddr::V6(addr_tmp.into())
            }
            _ => continue,
        };

        let mut addr_info = AddressInfo {
            interface_index: address_w.header.index,
            address: addr,
            preferred_until: None,
            valid_until: None,
        };
        if let Some(ci) = cacheinfo {
            ci.add_cache_information(&mut addr_info, now);
        }

        addresses.push(addr_info);
    }

    Ok(ListAddressesResponse {
        addresses,
    })
}

// ip addr add 1.1.1.1/32 dev enp2s0f1
// sendmsg(3, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base={{len=40, type=RTM_NEWADDR, flags=NLM_F_REQUEST|NLM_F_ACK|NLM_F_EXCL|NLM_F_CREATE, seq=1599578738, pid=0}, {ifa_family=AF_INET, ifa_prefixlen=29, ifa_flags=0, ifa_scope=RT_SCOPE_UNIVERSE, ifa_index=if_nametoindex("enp2s0f1")}, [{{nla_len=8, nla_type=IFA_LOCAL}, inet_addr("1.1.1.1")}, {{nla_len=8, nla_type=IFA_ADDRESS}, inet_addr("1.1.1.1")}]}, iov_len=40}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 40
//
pub async fn add_address(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: AddAddressRequest,
) -> Result<AddAddressResponse, netlink_proto::Error<RtnlMessage>>
{
    let now = Instant::now();
    let mut message = address_spec_to_address_message(&lreq.address);

    if lreq.preferred_until.is_some() || lreq.valid_until.is_some() {
        // 0xFFFF_FFFF is reserved for infinite/forever.
        const MAX_VALID_PERIOD: u64 = (u32::max_value() - 1) as u64;

        let mut cache_info = IfaCacheInfo::zero();
        if let Some(pref) = lreq.preferred_until {
            let mut secs = now.duration_since(pref).as_secs();
            if MAX_VALID_PERIOD < secs {
                secs = MAX_VALID_PERIOD;
            }
            cache_info.ifa_prefered = ValidityPeriod(secs as u32);
        }
        if let Some(val) = lreq.valid_until {
            let mut secs = now.duration_since(val).as_secs();
            if MAX_VALID_PERIOD < secs {
                secs = MAX_VALID_PERIOD;
            }
            cache_info.ifa_valid = ValidityPeriod(secs as u32);
        }

        message.nlas.push(unimplemented!("add cache info"));
    }

    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_REQUEST | NLM_F_ACK | NLM_F_EXCL | NLM_F_CREATE,
            ..Default::default()
        },
        payload: RtnlMessage::NewAddress(message).into(),
    };

    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    // let mut addresses = Vec::new();
    while let Some(packet) = response.next().await {
        println!("packet = {:?}", packet);
    }

    Ok(AddAddressResponse {})
}

// ip addr del 1.1.1.1/32 dev enp2s0f1
// sendmsg(3, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base={{len=40, type=RTM_DELADDR, flags=NLM_F_REQUEST|NLM_F_ACK, seq=1599578750, pid=0}, {ifa_family=AF_INET, ifa_prefixlen=29, ifa_flags=0, ifa_scope=RT_SCOPE_UNIVERSE, ifa_index=if_nametoindex("enp2s0f1")}, [{{nla_len=8, nla_type=IFA_LOCAL}, inet_addr("1.1.1.1")}, {{nla_len=8, nla_type=IFA_ADDRESS}, inet_addr("1.1.1.1")}]}, iov_len=40}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 40
//
pub async fn del_address(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: AddAddressRequest,
) -> Result<AddAddressResponse, netlink_proto::Error<RtnlMessage>>
{
    let message = address_spec_to_address_message(&lreq.address);
    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_REQUEST | NLM_F_ACK,
            ..Default::default()
        },
        payload: RtnlMessage::DelAddress(message).into(),
    };

    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    // let mut addresses = Vec::new();
    while let Some(packet) = response.next().await {
        println!("packet = {:?}", packet);
    }

    Ok(AddAddressResponse {})
}

// --

fn address_spec_to_address_message(aspec: &AddressSpec) -> AddressMessage {
    let mut message = AddressMessage::default();
    let ip_buf;
    match aspec.address {
        std::net::IpAddr::V4(v4_addr) => {
            message.header.family = AF_INET as u8;
            ip_buf = v4_addr.octets().to_vec();
        }
        std::net::IpAddr::V6(v6_addr) => {
            message.header.family = AF_INET6 as u8;
            ip_buf = v6_addr.octets().to_vec();
        }
    };
    message.header.prefix_len = aspec.prefix_len;
    message.header.flags = 0;
    message.header.scope = RT_SCOPE_UNIVERSE;
    message.header.index = aspec.iface_index;
    
    message.nlas.push(Nla::Local(ip_buf.clone()));
    message.nlas.push(Nla::Address(ip_buf));

    message
}

// --

#[derive(Debug)]
struct IfaCacheInfo {
    /// ifa_prefered is the preferred TTL of the address, seconds.
    ifa_prefered: ValidityPeriod,
    /// ifa_valid is the validity TTL of the address, seconds.
    ifa_valid: ValidityPeriod,
    /// created timestamp, hundredths of seconds
    cstamp: u32,
    /// updated timestamp, hundredths of seconds
    tstamp: u32,
}

impl IfaCacheInfo {
    const BUFFER_LENGTH: usize = 12;

    fn zero() -> IfaCacheInfo {
        IfaCacheInfo {
            ifa_prefered: ValidityPeriod(0),
            ifa_valid: ValidityPeriod(0),
            cstamp: 0,
            tstamp: 0,
        }
    }

    fn to_byte_vec(&self) -> Vec<u8> {
        let mut data = [0; IfaCacheInfo::BUFFER_LENGTH];

        unimplemented!("populate vec");

    }

    fn copy_to_slice_fixed(&self, data: &mut [u8; IfaCacheInfo::BUFFER_LENGTH]) {
        let buf = self.ifa_prefered.0.to_ne_bytes();
        data[..4].copy_from_slice(&buf[..]);

        let buf = self.ifa_valid.0.to_ne_bytes();
        data[4..][..4].copy_from_slice(&buf[..]);

        let buf = self.cstamp.to_ne_bytes();
        data[8..][..4].copy_from_slice(&buf[..]);

        let buf = self.tstamp.to_ne_bytes();
        data[12..][..4].copy_from_slice(&buf[..]);
    }

    fn copy_from_slice(data: &[u8]) -> IfaCacheInfo {
        if data.len() < IfaCacheInfo::BUFFER_LENGTH {
            panic!("buffer too small.");
        }
        let mut buf: [u8; 4] = [0; 4];
        buf.copy_from_slice(&data[..4]);
        let ifa_prefered = ValidityPeriod(u32::from_ne_bytes(buf));
        buf.copy_from_slice(&data[4..][..4]);
        let ifa_valid = ValidityPeriod(u32::from_ne_bytes(buf));
        buf.copy_from_slice(&data[8..][..4]);
        let cstamp = u32::from_ne_bytes(buf);
        buf.copy_from_slice(&data[12..][..4]);
        let tstamp = u32::from_ne_bytes(buf);

        IfaCacheInfo {
            ifa_prefered,
            ifa_valid,
            cstamp,
            tstamp,
        }
    }

    fn add_cache_information(&self, info: &mut AddressInfo, now: Instant) {
        info.preferred_until = None;
        info.valid_until = None;
        if !self.ifa_prefered.is_infinite() {
            info.preferred_until = Some(now  + self.ifa_prefered.as_duration());
        }
        if !self.ifa_valid.is_infinite() {
            info.valid_until = Some(now  + self.ifa_valid.as_duration());
        }
    }
}
