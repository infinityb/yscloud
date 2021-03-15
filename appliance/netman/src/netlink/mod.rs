use std::fmt;
use std::time::Duration;
use std::net::Ipv6Addr;
use std::convert::TryInto;

use futures::StreamExt;
use netlink_packet_route::{
    LinkMessage, NetlinkHeader, NetlinkMessage, RtnlMessage, NLM_F_DUMP, NLM_F_REQUEST,
    // AddressMessage, RTM_NEWLINK, IFF_LOWER_UP, ARPHRD_ETHER, NLM_F_EXCL, RT_SCOPE_UNIVERSE,
    RouteMessage, NLM_F_CREATE, IFF_UP, AF_UNSPEC, ARPHRD_NETROM, NLM_F_ACK, 
};
use netlink_proto::{
    new_connection,
    sys::SocketAddr,
};

mod address;

pub use self::address::{
    add_address,
    AddAddressRequest,
    AddAddressResponse,
    del_address,
    DelAddressRequest,
    DelAddressResponse,
    list_addresses,
    ListAddressesRequest,
    ListAddressesResponse,
    AddressSpec,
};

#[derive(Default, Clone)]
pub struct HardwareAddress {
    // may grow to 8 if we need it
    pub data: [u8; HardwareAddress::MAX_LENGTH],
    pub length: usize,
}

impl fmt::Debug for HardwareAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "HardwareAddress({:?})", self.as_slice())
    }
}

impl HardwareAddress {
    pub const MAX_LENGTH: usize = 8;

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.length]
    }

    pub fn from_slice(data: &[u8]) -> HardwareAddress {
        let mut address = HardwareAddress::default();
        if HardwareAddress::MAX_LENGTH < data.len() {
            panic!("address too large");
        }
        address.data[..data.len()].copy_from_slice(data);
        address.length = data.len();
        address
    }
}

pub struct ListInterfaceRequest {}

impl Default for ListInterfaceRequest {
    fn default() -> ListInterfaceRequest {
        ListInterfaceRequest {}
    }
}

#[derive(Debug)]
pub struct ListInterfaceResponse {
    pub interfaces: Vec<InterfaceInfo>,
}

#[derive(Clone, Debug)]
pub struct InterfaceInfo {
    pub index: u32,
    pub name: String,
    pub address: HardwareAddress,
}

pub async fn list_interfaces(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: ListInterfaceRequest,
) -> Result<ListInterfaceResponse, netlink_proto::Error<RtnlMessage>>
{
    use netlink_packet_core::NetlinkPayload;
    use netlink_packet_route::rtnl::RtnlMessage;
    use netlink_packet_route::rtnl::link::nlas::Nla;

    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_DUMP | NLM_F_REQUEST,
            ..Default::default()
        },
        payload: RtnlMessage::GetLink(LinkMessage::default()).into(),
    };

    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    let mut interfaces = Vec::new();
    while let Some(packet) = response.next().await {
        // println!("list_interfaces[..] = {:#?}", packet);

        let inner;
        let link;
        let mut ifname = String::new();
        let mut address = HardwareAddress::default();
        if let NetlinkPayload::InnerMessage(inner_tmp) = packet.payload {
            inner = inner_tmp;
        } else {
            continue;
        }
        if let RtnlMessage::NewLink(link_tmp) = inner {
            link = link_tmp;
        } else {
            continue;
        }
        for i in &link.nlas {
            if let Nla::IfName(ifname_tmp) = i {
                ifname = ifname_tmp.clone();
            }
            if let Nla::Address(address_tmp) = i {
                address = HardwareAddress::from_slice(&address_tmp[..]);
            }

        }
        interfaces.push(InterfaceInfo {
            index: link.header.index,
            name: ifname,
            address,
        });
    }

    Ok(ListInterfaceResponse {
        interfaces,
    })
}


//

#[derive(Debug)]
pub struct AddRouteRequest {
    source_address: Option<Ipv6Addr>,
}

#[derive(Debug)]
pub struct AddRouteResponse {
    //
}

// ip route add default via 1.1.1.1 dev enp2s0f1
// sendmsg(3, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base={{len=44, type=RTM_NEWROUTE, flags=NLM_F_REQUEST|NLM_F_ACK|NLM_F_EXCL|NLM_F_CREATE, seq=1599535622, pid=0}, {rtm_family=AF_INET, rtm_dst_len=0, rtm_src_len=0, rtm_tos=0, rtm_table=RT_TABLE_MAIN, rtm_protocol=RTPROT_BOOT, rtm_scope=RT_SCOPE_UNIVERSE, rtm_type=RTN_UNICAST, rtm_flags=0}, [{{nla_len=8, nla_type=RTA_GATEWAY}, inet_addr("1.1.1.1")}, {{nla_len=8, nla_type=RTA_OIF}, if_nametoindex("enp2s0f1")}]}, iov_len=44}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 44
//
// ip -6 route add default via fe80::1 dev enp2s0f1
// sendmsg(3, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base={{len=76, type=RTM_NEWROUTE, flags=NLM_F_REQUEST|NLM_F_ACK|NLM_F_EXCL|NLM_F_CREATE, seq=1599535701, pid=0}, {rtm_family=AF_INET6, rtm_dst_len=0, rtm_src_len=0, rtm_tos=0, rtm_table=RT_TABLE_MAIN, rtm_protocol=RTPROT_BOOT, rtm_scope=RT_SCOPE_UNIVERSE, rtm_type=RTN_UNICAST, rtm_flags=0}, [{{nla_len=20, nla_type=RTA_DST}, inet_pton(AF_INET6, "::")}, {{nla_len=20, nla_type=RTA_GATEWAY}, inet_pton(AF_INET6, "fe80::1")}, {{nla_len=8, nla_type=RTA_OIF}, if_nametoindex("enp2s0f1")}]}, iov_len=76}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 76
//
pub async fn add_route(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: AddRouteRequest,
) -> Result<AddRouteResponse, netlink_proto::Error<RtnlMessage>>
{
    use netlink_packet_route::rtnl::route::nlas::Nla;
    use netlink_packet_route::rtnl::constants::{AF_INET6, RTPROT_RA, RT_TABLE_UNSPEC};

    let mut message = RouteMessage::default();
    message.header.address_family = AF_INET6.try_into().unwrap();
    message.header.destination_prefix_length = 64;
    message.header.source_prefix_length = 128;
    message.header.protocol = RTPROT_RA;
    message.header.table = RT_TABLE_UNSPEC;

    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_DUMP | NLM_F_CREATE,
            ..Default::default()
        },
        payload: RtnlMessage::GetRoute(message).into(),
    };

    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    // let mut addresses = Vec::new();
    while let Some(packet) = response.next().await {
        // println!("packet = {:?}", packet);
    }

    Ok(AddRouteResponse {})
}

pub async fn list_routes(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: ListAddressesRequest,
) -> Result<ListAddressesResponse, netlink_proto::Error<RtnlMessage>>
{
    use netlink_packet_core::NetlinkPayload;
    use netlink_packet_route::rtnl::RtnlMessage;
    use netlink_packet_route::rtnl::route::nlas::Nla;

    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_DUMP | NLM_F_REQUEST,
            ..Default::default()
        },
        payload: RtnlMessage::GetRoute(RouteMessage::default()).into(),
    };

    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    let mut addresses = Vec::new();
    while let Some(packet) = response.next().await {
        // println!("packet = {:?}", packet);

        // let inner;
        // let address_w;
        // let mut address: Vec<u8> = Vec::new();
        // let mut ifname = String::new();
        // let mut cacheinfo = None;

        // if let NetlinkPayload::InnerMessage(inner_tmp) = packet.payload {
        //     inner = inner_tmp;
        // } else {
        //     continue;
        // }
        // if let RtnlMessage::NewAddress(address_tmp) = inner {
        //     address_w = address_tmp;
        // } else {
        //     continue;
        // }
        // for i in &address_w.nlas {
        //     if let Nla::Address(address_tmp) = i {
        //         address = address_tmp.clone();
        //     }
        //     if let Nla::CacheInfo(cinfo_tmp) = i {
        //         if IfaCacheInfo::BUFFER_LENGTH <= cinfo_tmp.len() {
        //             cacheinfo = Some(IfaCacheInfo::copy_from_slice(&cinfo_tmp));
        //         }
        //     }
        // }

        // let addr = match address.len() {
        //     4 => {
        //         let mut addr_tmp: [u8; 4] = [0; 4];
        //         addr_tmp.clone_from_slice(&address[..]);
        //         std::net::IpAddr::V4(addr_tmp.into())
        //     },
        //     16 => {
        //         let mut addr_tmp: [u8; 16] = [0; 16];
        //         addr_tmp.clone_from_slice(&address[..]);
        //         std::net::IpAddr::V6(addr_tmp.into())
        //     }
        //     _ => continue,
        // };

        // addresses.push(AddressInfo {
        //     interface_index: address_w.header.index,
        //     address: addr,
        //     cacheinfo,
        // });
    }

    Ok(ListAddressesResponse {
        addresses,
    })
}

#[derive(Debug)]
pub struct InterfaceBringUpRequest {
    pub interface_index: u32,
}

#[derive(Debug)]
pub struct InterfaceBringUpResponse {
    //
}

pub async fn interface_bring_up(
    handle: &mut netlink_proto::ConnectionHandle<RtnlMessage>,
    lreq: InterfaceBringUpRequest,
) -> Result<InterfaceBringUpResponse, netlink_proto::Error<RtnlMessage>> {
    use std::convert::TryInto;
    use netlink_packet_core::NetlinkPayload;
    use netlink_packet_route::rtnl::RtnlMessage;
    use netlink_packet_route::rtnl::link::nlas::Nla;

    let mut msg = LinkMessage::default();
    msg.header.interface_family = AF_UNSPEC.try_into().unwrap();
    msg.header.index = lreq.interface_index;
    msg.header.link_layer_type = ARPHRD_NETROM;
    msg.header.flags = IFF_UP;
    msg.header.change_mask = IFF_UP;

    let request: NetlinkMessage<RtnlMessage> = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_REQUEST | NLM_F_ACK,
            ..Default::default()
        },
        payload: RtnlMessage::NewLink(msg).into(),
    };
    let mut response = handle.request(request, SocketAddr::new(0, 0))?;

    while let Some(packet) = response.next().await {
    }

    Ok(InterfaceBringUpResponse {})
}


#[derive(Debug, Copy, Clone)]
pub struct ValidityPeriod(pub u32);

impl ValidityPeriod {
    const INFINITE: ValidityPeriod = ValidityPeriod(0xFFFF_FFFF);

    pub fn is_infinite(&self) -> bool {
        self.0 == ValidityPeriod::INFINITE.0
    }

    pub fn as_duration(&self) -> std::time::Duration {
        if self.is_infinite() {
            Duration::new(u64::MAX, 0)
        } else {
            Duration::new(self.0 as u64, 0)
        }
    }
}

impl fmt::Display for ValidityPeriod {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_infinite() {
            write!(f, "forever")
        } else {
            write!(f, "{}s", self.0)
        }
    }
}

/// https://elixir.bootlin.com/linux/v3.4/C/ident/rta_cacheinfo
pub struct RtaCacheInfo {
    rta_clntref: u32,
    rta_lastuse: u32,
    rta_expires: u32,
    rta_error: u32,
    rta_used: u32,
    rta_id: u32,
    rta_ts: u32,
    rta_tsage: u32,
}
