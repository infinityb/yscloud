use ip_network::{Ipv4Network, Ipv6Network};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

// FIXME: when we can make a Ipv4Network at compile time, do so.
const UNSAFE_NETWORKS_V4: &[&str] = &[
    "127.0.0.0/8",    // IPv4 loopback
    "10.0.0.0/8",     // RFC1918
    "172.16.0.0/12",  // RFC1918
    "192.168.0.0/16", // RFC1918
    "100.64.0.0/10",  // RFC6598
];

// FIXME: when we can make a Ipv6Network at compile time, do so.
const UNSAFE_NETWORKS_V6: &[&str] = &[
    "::1/128",        // IPv6 loopback
    "fe80::/10",      // IPv6 link-local
];


pub fn is_ip_addr_safe(ip: &IpAddr) -> bool {
    match *ip {
        IpAddr::V4(v4) => is_ipv4_addr_safe(v4),
        IpAddr::V6(v6) => is_ipv6_addr_safe(v6),
    }
}

pub fn is_ipv4_addr_safe(ip: Ipv4Addr) -> bool {
    for addr in UNSAFE_NETWORKS_V4 {
        let network = addr.parse::<Ipv4Network>().unwrap();
        if network.contains(ip) {
            return true;
        }
    }
    false
}

pub fn is_ipv6_addr_safe(ip: Ipv6Addr) -> bool {
    for addr in UNSAFE_NETWORKS_V6 {
        let network = addr.parse::<Ipv6Network>().unwrap();
        if network.contains(ip) {
            return true;
        }
    }
    false
}