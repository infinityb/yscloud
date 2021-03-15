#![allow(deprecated)]

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::mem;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::time::{SystemTime, Duration, Instant};
use std::process::Command;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use futures::StreamExt;
use libc::{c_int, IPPROTO_ICMPV6};
use libc::{in6_addr, in6_pktinfo, sockaddr, sockaddr_in6};
use nix::sys::reboot::{reboot, RebootMode};
use nix::mount::{MsFlags, mount};
use nix::sys::socket::sockopt::Ipv6RecvPacketInfo;
use nix::sys::socket::{
    recvmsg, setsockopt, AddressFamily, CmsgSpace, ControlMessage, MsgFlags, SetSockOpt, SockFlag,
    SockType,
};
use chrono::offset::{TimeZone, Local};
use copy_arena::{Arena, Allocator};
use nix::sys::uio::IoVec;
use netlink_packet_route::{
    LinkMessage, NetlinkHeader, NetlinkMessage, RtnlMessage, NLM_F_DUMP, NLM_F_REQUEST,
    AddressMessage,
};
use netlink_proto::{
    new_connection,
    sys::SocketAddr,
    sys::protocols::NETLINK_ROUTE,
};

mod ip;
mod raw_radv;
mod radvd;
mod netlink;

pub use self::netlink::{add_address, list_interfaces, list_routes, list_addresses, ListInterfaceRequest, ListAddressesRequest, interface_bring_up, InterfaceBringUpRequest, AddAddressRequest, AddressSpec};
pub use self::radvd::{ByteIterRead, RouterAdvertisementOption, RouterAdvertisement};

/// Create an endpoint for communication
///
/// The `protocol` specifies a particular protocol to be used with the
/// socket.  Normally only a single protocol exists to support a
/// particular socket type within a given protocol family, in which case
/// protocol can be specified as `None`.  However, it is possible that many
/// protocols may exist, in which case a particular protocol must be
/// specified in this manner.
///
/// [Further reading](http://pubs.opengroup.org/onlinepubs/9699919799/functions/socket.html)
///
/// forked to make protocol a c_int since the original API was too restrictive to get ICMPv6
pub fn socket(
    domain: AddressFamily,
    ty: SockType,
    flags: SockFlag,
    protocol: c_int,
) -> nix::Result<RawFd> {
    // SockFlags are usually embedded into `ty`, but we don't do that in `nix` because it's a
    // little easier to understand by separating it out. So we have to merge these bitfields
    // here.
    let mut ty = ty as c_int;
    ty |= flags.bits();

    let res = unsafe { libc::socket(domain as c_int, ty, protocol) };

    nix::errno::Errno::result(res)
}

fn setup_icmp6_socket() -> nix::Result<RawFd> {
    use self::sockopt::{
        Icmp6Filter, Icmp6FilterValue, Ipv6Checksum, Ipv6MulticastHops, Ipv6RecvHopLimit,
        Ipv6UnicastHops, ND_ROUTER_ADVERT, ND_ROUTER_SOLICIT,
    };

    let sflags = SockFlag::empty();
    let fd = socket(AddressFamily::Inet6, SockType::Raw, sflags, IPPROTO_ICMPV6)?;
    setsockopt(fd, Ipv6RecvPacketInfo, &true)?;
    setsockopt(fd, Ipv6Checksum, &2)?;
    setsockopt(fd, Ipv6UnicastHops, &255)?;
    setsockopt(fd, Ipv6MulticastHops, &255)?;
    setsockopt(fd, Ipv6RecvHopLimit, &1)?;
    let mut filter = Icmp6FilterValue::block_all();
    filter.set_pass(ND_ROUTER_SOLICIT);
    filter.set_pass(ND_ROUTER_ADVERT);
    setsockopt(fd, Icmp6Filter, &filter)?;

    Ok(fd)
}

#[derive(Debug)]
struct AdvertisementWrapped<'a> {
    capture_instant: Instant,
    source_address: Ipv6Addr,
    target_address: Ipv6Addr,
    interface_index: u32,
    advertisement: RouterAdvertisement<'a>,
}

struct PrefixInformationCooked {
    prefix_length: u8,
    valid_lifetime: Duration,
    preferred_lifetime: Duration,
    prefix: std::net::Ipv6Addr,
}

impl<'a> AdvertisementWrapped<'a> {
    /// `is_managed` returns true when the host should use DHCPv6 to obtain
    /// addressing information.
    pub fn is_managed(&self) -> bool {
        self.advertisement.flags1 & 0x80 != 0
    }

    /// `has_supplemental_configuration` returns true if DHCPv6 can be used to find
    /// other servers on the network, e.g. DNS and time servers.  If `is_managed`
    /// returns true, this is guaranteed to be true since DHCPv6 must be used.
    pub fn has_supplemental_configuration(&self) -> bool {
        self.advertisement.flags1 & 0xc0 != 0
    }

    /// `slaac_only` returns true if the router advertisements do not suggest the
    /// presence of DHCPv6.
    pub fn slaac_only(&self) -> bool {
        self.advertisement.flags1 & 0xc0 == 0
    }

    pub fn prefix_information(&self) -> Option<PrefixInformationCooked> {
        let mut cooked = None;
        for opt in self.advertisement.options {
            if let RouterAdvertisementOption::PrefixInformation(pref) = opt {
                cooked = Some(PrefixInformationCooked {
                    prefix_length: pref.prefix_length,
                    valid_lifetime: Duration::new(pref.valid_lifetime as u64, 0),
                    preferred_lifetime: Duration::new(pref.preferred_lifetime as u64, 0),
                    prefix: pref.prefix,
                });
                break;
            }
        }
        cooked
    }

    pub fn valid_until(&self) -> Option<Instant> {
        let cooked = self.prefix_information()?;
        Some(self.capture_instant + cooked.valid_lifetime)
    }

    pub fn preferred_until(&self) -> Option<Instant> {
        let cooked = self.prefix_information()?;
        Some(self.capture_instant + cooked.preferred_lifetime)
    }
}

enum AdvertisementMetadataOption {
    ValidUntil(Instant),
    PreferredUntil(Instant),
}

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_BACKTRACE", "1");

    if let Err(err) = main2().await {
        println!("error: {}", err);
        println!();

        ::std::thread::sleep_ms(10000);

        reboot(RebootMode::RB_AUTOBOOT).expect("reboot failed");
    }

    reboot(RebootMode::RB_POWER_OFF).expect("PowerOff failed");
}

async fn main2() -> Result<(), Box<dyn ::std::error::Error>> {    
    // packet=NetlinkMessage { header: NetlinkHeader { length: 1316, message_type: 16, flags: 0, sequence_number: 3, port_number: 1 }, payload: InnerMessage(NewLink(LinkMessage { header: LinkHeader { interface_family: 0, index: 1, link_layer_}
    // UP resp: InterfaceBringUpResponse
    // request=NetlinkMessage {
    //     header: NetlinkHeader {
    //         length: 0,
    //         message_type: 16,
    //         flags: 5,
    //         sequence_number: 0,
    //         port_number: 0,
    //     },
    //     payload: InnerMessage(
    //         GetLink(
    //             LinkMessage {
    //                 header: LinkHeader {
    //                     interface_family: 0,
    //                     index: 2,
    //                     link_layer_type: 0,
    //                     flags: 1,
    //                     change_mask: 1,
    //                 },
    //                 nlas: [],
    //             },
    //         ),
    //     ),
    // }

    // sendto(6, {{len=32, type=RTM_GETLINK, flags=NLM_F_REQUEST|NLM_F_ACK, seq=3, pid=0}, {ifi_family=AF_UNSPEC, ifi_type=ARPHRD_NETROM, ifi_index=if_nametoindex("vshaw0"), ifi_flags=IFF_UP, ifi_change=0x1}}, 32, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 32
    // sendmsg(3, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base={{len=32, type=RTM_NEWLINK, flags=NLM_F_REQUEST|NLM_F_ACK, seq=1599521558, pid=0}, {ifi_family=AF_UNSPEC, ifi_type=ARPHRD_NETROM, ifi_index=if_nametoindex("enp4s0"), ifi_flags=IFF_UP, ifi_change=0x1}}, iov_len=32}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0)2

    let (conn, mut handle, mut unsol) = new_connection(NETLINK_ROUTE)?;

    tokio::spawn(conn);

    tokio::spawn(async move {
        while let Some(x) = unsol.next().await {
            println!("RX UNSOL {:?}", x);
        }
    });

    let response = list_routes(&mut handle, ListAddressesRequest {}).await?;

    let response = list_interfaces(&mut handle, ListInterfaceRequest {}).await?;
    for iface in &response.interfaces {
        interface_bring_up(&mut handle, InterfaceBringUpRequest {
            interface_index: iface.index,
        }).await?;
    }

    // let now = Instant::now();
    // for a in &response.addresses {
    //     let interface = interfaces.get(&a.interface_index).unwrap();
    //     println!("{}: {}", interface.name, a.address);
    //     if let Some(valid_ts) = a.preferred_until {
    //         if now < valid_ts {
    //             println!("    valid_lft expired {:?} ago", valid_ts - now);
    //         } else {
    //             println!("    valid_lft {:?}", now - valid_ts);
    //         }
    //     }
    //     if let Some(preferred_ts) = a.preferred_until {
    //         if now < preferred_ts {
    //             println!("    preferred_lft expired {:?} ago", preferred_ts - now);
    //         } else {
    //             println!("    preferred_lft {:?}", now - preferred_ts);
    //         }
    //     }
    // }

    // add_address(&mut handle, AddAddressRequest {
    //     address: AddressSpec {
    //         iface_index: 2,
    //         address: IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
    //         prefix_len: 24,
    //     },
    // }).await?;

    ::std::thread::sleep_ms(1000);

    Command::new("/usr/sbin/ip")
        .args(&["addr", "show"])
        .spawn()
        .expect("failed to execute process")
        .wait()
        .expect("failed to execute process");

    let fd = setup_icmp6_socket().unwrap();

    let mut rbuf = vec![0; 1500];

    let mut cmsg: CmsgSpace<[u8; 64]> = CmsgSpace::new(); // made up types and count
    let flags = MsgFlags::empty();
    for i in 0..3000_u64 {
        let rxm = recvmsg(
            fd,
            &[IoVec::from_mut_slice(&mut rbuf)],
            Some(&mut cmsg),
            flags,
        )
        .unwrap();

        let mut capture_wall = Local::now();
        let mut capture_instant = Instant::now();
        let mut source_init = false;
        let mut source_address = Ipv6Addr::UNSPECIFIED;

        let mut target_init = false;
        let mut target_address = Ipv6Addr::UNSPECIFIED;
        let mut interface_index: u32 = 0;

        if let Some(xx) = rxm.address {
            let (sa, sa_len) = unsafe { xx.as_ffi_pair() };
            if sa.sa_family == 10 && mem::size_of::<sockaddr_in6>() <= sa_len as usize {
                let sa6 = unsafe { &*(sa as *const sockaddr as *const sockaddr_in6) };
                source_address = sa6.sin6_addr.s6_addr.into();
                source_init = true;
            }
        }
        for m in rxm.cmsgs() {
            match m {
                ControlMessage::Ipv6PacketInfo(v) => {
                    target_address = v.ipi6_addr.s6_addr.into();
                    interface_index = v.ipi6_ifindex;
                    target_init = true;
                }
                _ => (),
            }
        }

        if !source_init {
            println!("source not initialized, dropped packet");
        }
        if !target_init {
            println!("target not initialized, dropped packet");
        }

        let advert_bytes = &rbuf[..rxm.bytes];
        let mut arena = Arena::new();
        let mut allocator = arena.allocator();
        let advertisement = RouterAdvertisement::read_byte_iter(
            &mut allocator, &mut advert_bytes.iter()).unwrap();
        let meta = AdvertisementWrapped {
            capture_instant,
            source_address,
            target_address,
            interface_index,
            advertisement,
        };

        println!("got advertisement: {:#?}", meta);

        let mut interfaces = HashMap::new();
        let response = list_interfaces(&mut handle, ListInterfaceRequest {}).await?;
        for iface in &response.interfaces {
            println!("interface[]={:?}", iface);
            interfaces.insert(iface.index, iface.clone());
        }

        const RFC3339_SECFMT: chrono::SecondsFormat = chrono::SecondsFormat::Secs;
        const RFC3339_USE_Z: bool = true;

        let interface;
        if let Some(interface_tmp) = interfaces.get(&meta.interface_index) {
            interface = interface_tmp;
        } else {
            continue;
        }
        
        println!("    interface: {}", interface.name);
        let mac_addr = interface.address.as_slice();
        if mac_addr.len() != 6 {
            continue;
        }

        let computed_ip_addr;
        let prefix_length;
        if let Some(pi) = meta.prefix_information() {
            println!("    prefix: {}/{}", pi.prefix, pi.prefix_length);

            let valid_lifetime = chrono::Duration::from_std(pi.valid_lifetime).unwrap();
            let valid_until = capture_wall + valid_lifetime;
            println!("    valid-until: {}", valid_until.to_rfc3339_opts(RFC3339_SECFMT, RFC3339_USE_Z));

            let preferred_lifetime = chrono::Duration::from_std(pi.preferred_lifetime).unwrap();
            let preferred_until = capture_wall + preferred_lifetime;
            println!("    preferred-until: {}", preferred_until.to_rfc3339_opts(RFC3339_SECFMT, RFC3339_USE_Z));

            computed_ip_addr = ip::slaac_autoconfig_mac48(mac_addr, &pi.prefix);
            prefix_length = pi.prefix_length;
        } else {
            continue;
        }

        let mut ifaddr = BTreeMap::new();
        let response = list_addresses(&mut handle, ListAddressesRequest {}).await?;
        for a in &response.addresses {
            ifaddr.insert((a.interface_index, a.address), a);
        }

        let mut replace_address = false;
        if ifaddr.contains_key(&(meta.interface_index, IpAddr::V6(computed_ip_addr))) {
            // meta.advertisement.
        } else {
            
        }
        add_address(&mut handle, AddAddressRequest {
            address: AddressSpec {
                iface_index: meta.interface_index,
                address: IpAddr::V6(computed_ip_addr),
                prefix_len: prefix_length,
            },
            preferred_until: None,
            valid_until: None,
        }).await?;

        //       5254:0012:34:56
        // fec0::5054:ff:fe12:3456
        println!("");

        Command::new("/usr/sbin/ip")
            .args(&["addr", "show"])
            .spawn()
            .expect("failed to execute process")
            .wait()
            .expect("failed to execute process");

        println!("");
    }

    const POWER_OFF_SECONDS: u16 = 60;

    for i in 0..POWER_OFF_SECONDS {
        println!("Hello, world!  Powering off in {}...", POWER_OFF_SECONDS - i);
        ::std::thread::sleep_ms(1000);
    }

    Ok(())
}

#[test]
fn max_radvd_duration_acceptable() {
    let std_dur = std::time::Duration::new(u32::max_value() as u64, 0);
    chrono::Duration::from_std(std_dur).unwrap();
}

#[allow(non_camel_case_types)]
struct in6_addr_debug<'a>(&'a in6_addr);
impl<'a> fmt::Debug for in6_addr_debug<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let addr: Ipv6Addr = self.0.s6_addr.into();
        f.debug_struct("in6_addr")
            .field("s6_addr", &addr)
            .finish()
    }
}

#[allow(non_camel_case_types)]
struct in6_pktinfo_debug<'a>(&'a in6_pktinfo);
impl<'a> fmt::Debug for in6_pktinfo_debug<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("in6_pktinfo")
            .field("ipi6_addr", &in6_addr_debug(&self.0.ipi6_addr))
            .field("ipi6_ifindex", &self.0.ipi6_ifindex)
            .finish()
    }
}

#[allow(non_camel_case_types)]
struct sockaddr_debug<'a>(&'a sockaddr);
impl<'a> fmt::Debug for sockaddr_debug<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("sockaddr")
            .field("sa_family", &self.0.sa_family)
            .field("sa_data", &self.0.sa_data)
            .finish()
    }
}

#[allow(non_camel_case_types)]
struct sockaddr_in6_debug<'a>(&'a sockaddr_in6);
impl<'a> fmt::Debug for sockaddr_in6_debug<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("sockaddr_in6")
            .field("sin6_family", &self.0.sin6_family)
            .field("sin6_port", &self.0.sin6_port)
            .field("sin6_flowinfo", &self.0.sin6_flowinfo)
            .field("sin6_addr", &in6_addr_debug(&self.0.sin6_addr))
            .field("sin6_scope_id", &self.0.sin6_scope_id)
            .finish()
    }
}

mod sockopt {
    use std::mem;
    use std::os::unix::io::RawFd;

    pub struct Icmp6FilterType(i32);

    pub const ND_ROUTER_SOLICIT: Icmp6FilterType = Icmp6FilterType(133);
    pub const ND_ROUTER_ADVERT: Icmp6FilterType = Icmp6FilterType(134);
    pub const ND_NEIGHBOR_SOLICIT: Icmp6FilterType = Icmp6FilterType(135);
    pub const ND_NEIGHBOR_ADVERT: Icmp6FilterType = Icmp6FilterType(136);
    pub const ND_REDIRECT: Icmp6FilterType = Icmp6FilterType(137);

    const IPV6_CHECKSUM: i32 = 7;
    const ICMP6_FILTER: i32 = 1;
    const IPV6_RECVHOPLIMIT: i32 = 51;

    use libc::{
        c_void, socklen_t, IPPROTO_ICMPV6, IPV6_MULTICAST_HOPS, IPV6_UNICAST_HOPS, SOL_IPV6,
        SOL_RAW,
    };
    use nix::sys::socket::SetSockOpt;

    trait Set<'a, T> {
        fn new(val: &'a T) -> Self;
        unsafe fn ffi_ptr(&self) -> *const c_void;

        unsafe fn ffi_len(&self) -> socklen_t;
    }

    struct SetStruct<'a, T: 'static> {
        ptr: &'a T,
    }

    impl<'a, T> Set<'a, T> for SetStruct<'a, T> {
        fn new(ptr: &'a T) -> SetStruct<'a, T> {
            SetStruct { ptr }
        }

        unsafe fn ffi_ptr(&self) -> *const c_void {
            self.ptr as *const T as *const core::ffi::c_void
        }

        unsafe fn ffi_len(&self) -> socklen_t {
            mem::size_of::<T>() as socklen_t
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Ipv6Checksum;

    impl SetSockOpt for Ipv6Checksum {
        type Val = i32;

        fn set(&self, fd: RawFd, val: &i32) -> nix::Result<()> {
            unsafe {
                let setter: SetStruct<_> = Set::new(val);
                let res = libc::setsockopt(
                    fd,
                    SOL_RAW,
                    IPV6_CHECKSUM,
                    setter.ffi_ptr(),
                    setter.ffi_len(),
                );
                nix::errno::Errno::result(res).map(drop)
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Ipv6UnicastHops;

    impl SetSockOpt for Ipv6UnicastHops {
        type Val = i32;

        fn set(&self, fd: RawFd, val: &i32) -> nix::Result<()> {
            unsafe {
                let setter: SetStruct<_> = Set::new(val);
                let res = libc::setsockopt(
                    fd,
                    SOL_IPV6,
                    IPV6_UNICAST_HOPS,
                    setter.ffi_ptr(),
                    setter.ffi_len(),
                );
                nix::errno::Errno::result(res).map(drop)
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Ipv6MulticastHops;

    impl SetSockOpt for Ipv6MulticastHops {
        type Val = i32;

        fn set(&self, fd: RawFd, val: &i32) -> nix::Result<()> {
            unsafe {
                let setter: SetStruct<_> = Set::new(val);
                let res = libc::setsockopt(
                    fd,
                    SOL_IPV6,
                    IPV6_MULTICAST_HOPS,
                    setter.ffi_ptr(),
                    setter.ffi_len(),
                );
                nix::errno::Errno::result(res).map(drop)
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Ipv6RecvHopLimit;

    impl SetSockOpt for Ipv6RecvHopLimit {
        type Val = i32;

        fn set(&self, fd: RawFd, val: &i32) -> nix::Result<()> {
            unsafe {
                let setter: SetStruct<_> = Set::new(val);
                let res = libc::setsockopt(
                    fd,
                    SOL_IPV6,
                    IPV6_RECVHOPLIMIT,
                    setter.ffi_ptr(),
                    setter.ffi_len(),
                );
                nix::errno::Errno::result(res).map(drop)
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Icmp6Filter;

    #[derive(Debug)]
    pub struct Icmp6FilterValue(pub [u32; 8]);

    impl Icmp6FilterValue {
        pub fn block_all() -> Icmp6FilterValue {
            Icmp6FilterValue([!0; 8])
        }

        pub fn set_pass(&mut self, type_: Icmp6FilterType) {
            let Icmp6FilterType(type_) = type_;
            let type_major = type_ >> 5;
            let type_minor = type_ & 31;
            self.0[type_major as usize] &= 0xFFFF_FFFF ^ (1 << type_minor);
        }
    }

    impl SetSockOpt for Icmp6Filter {
        type Val = Icmp6FilterValue;

        fn set(&self, fd: RawFd, val: &Icmp6FilterValue) -> nix::Result<()> {
            unsafe {
                let setter: SetStruct<_> = Set::new(val);
                let res = libc::setsockopt(
                    fd,
                    IPPROTO_ICMPV6,
                    ICMP6_FILTER,
                    setter.ffi_ptr(),
                    setter.ffi_len(),
                );
                nix::errno::Errno::result(res).map(drop)
            }
        }
    }

    #[test]
    fn icmp_filter_ok() {
        let mut i6f = Icmp6FilterValue::block_all();
        i6f.set_pass(super::sockopt::ND_ROUTER_SOLICIT);
        i6f.set_pass(super::sockopt::ND_ROUTER_ADVERT);
        assert_eq!(i6f.0[0], 0xffff_ffff);
        assert_eq!(i6f.0[1], 0xffff_ffff);
        assert_eq!(i6f.0[2], 0xffff_ffff);
        assert_eq!(i6f.0[3], 0xffff_ffff);
        assert_eq!(i6f.0[4], 0x9fff_ffff);
        assert_eq!(i6f.0[5], 0xffff_ffff);
        assert_eq!(i6f.0[6], 0xffff_ffff);
        assert_eq!(i6f.0[7], 0xffff_ffff);
    }
}
