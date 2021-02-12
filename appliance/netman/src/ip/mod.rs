use std::net::Ipv6Addr;

pub fn slaac_autoconfig_mac48(mac: &[u8], prefix: &Ipv6Addr) -> Ipv6Addr {
    if mac.len() != 6 {
        panic!("mac length must be 6 bytes");
    }

    const FIXED: [u8; 2] = [0xFF, 0xFE];

    let mut buf = prefix.octets();
    for (idx, octet) in buf[8..].iter_mut().enumerate() {
        *octet = match idx {
            0 => mac[idx] ^ 0x02,
            1 | 2 => mac[idx],
            3 | 4 => FIXED[idx - 3],
            5 | 6 | 7 => mac[idx - 2],
            _ => unreachable!(),
        };
    }
    buf.into()
}