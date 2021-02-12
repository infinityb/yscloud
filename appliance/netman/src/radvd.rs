use std::mem;
use std::fmt;
use std::slice;

use copy_arena::Allocator;
use byteorder::{BigEndian, ByteOrder};

pub trait ByteIterRead<'arena>: Sized + 'arena {
    type Error;

    fn read_byte_iter(_: &mut Allocator<'arena>, _: &mut slice::Iter<u8>) -> Result<Self, Self::Error>;
}

#[derive(Debug)]
pub struct Error {
    
}

impl Error {
    fn truncated() -> Self {
        Error {}
    }
}

const ROUTER_ADVERTISED_STATIC_LEN: usize = 16;
const MANAGED_ADDRESS_CONFIGURATION: u8 = 0x80;
const OTHER_CONFIGURATION: u8 = 0x40;

#[derive(Debug, Clone, Copy)]
pub enum RouterAdvertisementOption<'a> {
    SourceLinkLayerAddress(LinkLayerAddress),
    TargetLinkLayerAddress(LinkLayerAddress),
    Mtu(Mtu),
    PrefixInformation(&'a PrefixInformation),
    DnsSearchList(&'a [u8]),
    Unknown(u8, &'a [u8]),
}

#[derive(Debug)]
pub struct RouterAdvertisement<'a> {
    pub type_: u8,
    pub code: u8,
    pub checksum: u16,
    pub cur_hop_limit: u8,
    pub flags1: u8,
    pub router_lifetime: u16,
    pub reachable_time: u32,
    pub retrans_timer: u32,
    pub options: &'a [RouterAdvertisementOption<'a>],
}

fn iter_skip<'a>(length: usize, iter: &mut slice::Iter<'a, u8>) -> Result<&'a [u8], Error> {
    let slice = iter.as_slice();
    if slice.len() < length {
        return Err(Error::truncated());
    }
    let (head, rest) = slice.split_at(length);
    *iter = rest.iter();
    Ok(head)
}

fn get_type_len(data: &mut slice::Iter<u8>) -> Result<Option<(u8, usize)>, Error> {
    let type_ = match data.next() {
        Some(t) => t,
        None => return Ok(None),
    };
    let length_byte = data.next().ok_or_else(|| Error{})?;
    let length = *length_byte as usize * 8 - 2;
    Ok(Some((*type_, length)))
}

impl<'arena> ByteIterRead<'arena> for &'arena [RouterAdvertisementOption<'arena>] {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        struct OptionSlice<'arena, 'data> {
            unparsed: &'data [u8],
            next: Option<&'arena OptionSlice<'arena, 'data>>,
        }

        let mut option_count = 0;
        let mut data_counting = data.clone();
        while let Some((_, length)) = get_type_len(&mut data_counting)? {
            iter_skip(length, &mut data_counting)?;
            option_count += 1;
        }

        let options = arena.alloc_slice_fn(option_count, |_| RouterAdvertisementOption::Unknown(0, &[]));
        for opt in options.iter_mut() {
            let (type_, length) = get_type_len(data)?.unwrap();
            let option_data = iter_skip(length, data)?;
            match type_ {
                OPTION_TYPE_SOURCE_LINK_LAYER_ADDRESS => {
                    let addr = LinkLayerAddress::read_byte_iter(arena, &mut option_data.iter())?;
                    *opt = RouterAdvertisementOption::SourceLinkLayerAddress(addr);
                    // unimplemented!("assert alignment");
                }
                OPTION_TYPE_TARGET_LINK_LAYER_ADDRESS => {
                    let addr = LinkLayerAddress::read_byte_iter(arena, &mut option_data.iter())?;
                    *opt = RouterAdvertisementOption::SourceLinkLayerAddress(addr);
                    // unimplemented!("assert alignment");
                }
                OPTION_TYPE_PREFIX_INFORMATION => {
                    let addr = PrefixInformation::read_byte_iter(arena, &mut option_data.iter())?;
                    *opt = RouterAdvertisementOption::PrefixInformation(arena.alloc(addr));
                    // unimplemented!("assert alignment");
                }
                OPTION_TYPE_REDIRECT_HEADER => {
                    unimplemented!();
                }
                OPTION_TYPE_MTU => {
                    let mtu = Mtu::read_byte_iter(arena, &mut option_data.iter())?;
                    *opt = RouterAdvertisementOption::Mtu(mtu);
                    // unimplemented!("assert alignment");
                }
                OPTION_DNS_SEARCH_LIST => {
                    // this seems useless to us?
                    // let dns_str = ::std::str::from_utf8(option_data)
                    //     .map_err(|e| Error{})?;
                    // gotta parse something like b'\x00\x00\x00\x00\x00\n\x07yyc-int\x11yasashiisyndicate\x03org\x00\x00'
                    let copied = arena.alloc_slice(option_data);
                    *opt = RouterAdvertisementOption::DnsSearchList(copied);
                    // unimplemented!("assert alignment");
                }
                _ => {
                    let copied = arena.alloc_slice(option_data);
                    *opt = RouterAdvertisementOption::Unknown(type_, copied);
                    // unimplemented!("assert alignment");
                }
            }
        }

        Ok(options)
    }
}

impl<'arena> ByteIterRead<'arena> for RouterAdvertisement<'arena> {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let type_ = ByteIterRead::read_byte_iter(arena, data)?;
        let code = ByteIterRead::read_byte_iter(arena, data)?;
        let checksum = ByteIterRead::read_byte_iter(arena, data)?;
        let cur_hop_limit = ByteIterRead::read_byte_iter(arena, data)?;
        let flags1 = ByteIterRead::read_byte_iter(arena, data)?;
        let router_lifetime = ByteIterRead::read_byte_iter(arena, data)?;
        let reachable_time = ByteIterRead::read_byte_iter(arena, data)?;
        let retrans_timer = ByteIterRead::read_byte_iter(arena, data)?;

        let options = ByteIterRead::read_byte_iter(arena, data)?;

        Ok(RouterAdvertisement {
            type_,
            code,
            checksum,
            cur_hop_limit,
            flags1,
            router_lifetime,
            reachable_time,
            retrans_timer,
            options,
        })
    }
}

impl<'arena> ByteIterRead<'arena> for [u8; 6] {
    type Error = Error;

    fn read_byte_iter(_arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let mut buf = [0; 6];

        if data.as_slice().len() < buf.len() {
            return Err(Error{});
        }

        for (o, i) in buf.iter_mut().zip(data) {
            *o = *i;
        }

        Ok(buf)
    }
}

impl<'arena> ByteIterRead<'arena> for [u8; 16] {
    type Error = Error;

    fn read_byte_iter(_arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let mut buf = [0; 16];

        if data.as_slice().len() < buf.len() {
            return Err(Error{});
        }

        for (o, i) in buf.iter_mut().zip(data) {
            *o = *i;
        }

        Ok(buf)
    }
}

#[derive(Clone, Copy)]
pub struct LinkLayerAddress([u8; 6]);

impl<'arena> ByteIterRead<'arena> for LinkLayerAddress {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {        
        let buf = ByteIterRead::read_byte_iter(arena, data)?;
        Ok(LinkLayerAddress(buf))
    }
}

impl LinkLayerAddress {
    pub fn zero() -> LinkLayerAddress {
        LinkLayerAddress([0; 6])
    }
}

impl fmt::Debug for LinkLayerAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct MacAddr([u8; 6]);

        impl fmt::Debug for MacAddr {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "MacAddr([0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}])",
                    self.0[0].to_be(), self.0[1].to_be(),
                    self.0[2].to_be(), self.0[3].to_be(),
                    self.0[4].to_be(), self.0[5].to_be(),)
            }
        }

        f.debug_tuple("LinkLayerAddress")
            .field(&MacAddr(self.0))
            .finish()
    }
}

#[derive(Clone, Copy)]
pub struct Mtu {
    mtu: u32,
}

impl<'arena> ByteIterRead<'arena> for Mtu {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let buf: [u8; 6] = ByteIterRead::read_byte_iter(arena, data)?;
        
        let mut reserved1_buf: [u8; 2] = [0; 2];
        reserved1_buf.copy_from_slice(&buf[..2]);
        let reserved1 = u16::from_ne_bytes(reserved1_buf);

        let mut mtu_buf: [u8; 4] = [0; 4];
        mtu_buf.copy_from_slice(&buf[2..]);
        let mtu = u32::from_ne_bytes(mtu_buf);

        if reserved1 != 0 {
            return Err(Error{});
        }

        Ok(Mtu { mtu })
    }
}

impl fmt::Debug for Mtu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mtu = self.mtu.to_be();
        f.debug_struct("Mtu")
            .field("mtu", &mtu)
            .finish()
    }
}

impl<'arena> ByteIterRead<'arena> for std::net::Ipv6Addr {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let buf: [u8; 16] = ByteIterRead::read_byte_iter(arena, data)?;
        Ok(buf.into())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrefixInformation {
    pub prefix_length: u8,
    pub flags1: u8, 
    pub valid_lifetime: u32,
    pub preferred_lifetime: u32,
    pub reserved2: u32,
    pub prefix: std::net::Ipv6Addr,
}

impl<'arena> ByteIterRead<'arena> for u8 {
    type Error = Error;

    fn read_byte_iter(_arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        data.next().cloned().ok_or_else(Error::truncated)
    }
}

impl<'arena> ByteIterRead<'arena> for u16 {
    type Error = Error;

    fn read_byte_iter(_arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        const VALUE_LEN: usize = mem::size_of::<u16>();

        let slice = data.as_slice();
        if slice.len() < VALUE_LEN {
            return Err(Error::truncated());
        }
        let value = BigEndian::read_u16(slice);
        for _ in 0..VALUE_LEN {
            data.next().unwrap();
        }
        Ok(value)
    }
}

impl<'arena> ByteIterRead<'arena> for u32 {
    type Error = Error;

    fn read_byte_iter(_arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        const VALUE_LEN: usize = mem::size_of::<u32>();

        let slice = data.as_slice();
        if slice.len() < VALUE_LEN {
            return Err(Error::truncated());
        }
        let value = BigEndian::read_u32(slice);
        for _ in 0..VALUE_LEN {
            data.next().unwrap();
        }
        Ok(value)
    }
}

impl<'arena> ByteIterRead<'arena> for PrefixInformation {
    type Error = Error;

    fn read_byte_iter(arena: &mut Allocator<'arena>, data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let prefix_length = ByteIterRead::read_byte_iter(arena, data)?;
        let flags1 = ByteIterRead::read_byte_iter(arena, data)?;
        let valid_lifetime = ByteIterRead::read_byte_iter(arena, data)?;
        let preferred_lifetime = ByteIterRead::read_byte_iter(arena, data)?;
        let reserved2 = ByteIterRead::read_byte_iter(arena, data)?;
        let prefix = ByteIterRead::read_byte_iter(arena, data)?;

        Ok(PrefixInformation {
            prefix_length,
            flags1,
            valid_lifetime,
            preferred_lifetime,
            reserved2,
            prefix,
        })
    }
}

const OPTION_TYPE_SOURCE_LINK_LAYER_ADDRESS: u8 = 1;
const OPTION_TYPE_TARGET_LINK_LAYER_ADDRESS: u8 = 2;
const OPTION_TYPE_PREFIX_INFORMATION: u8 = 3;
const OPTION_TYPE_REDIRECT_HEADER: u8 = 4;
const OPTION_TYPE_MTU: u8 = 5;
const OPTION_DNS_SEARCH_LIST: u8 = 31;


#[cfg(test)]
mod tests {
    use std::mem;
    use super::{RouterAdvertisement, ByteIterRead};
    use copy_arena::{Arena, Allocator};

    // HE
    const ADVERT1: &[u8] = &[
        134, 0, 105, 118, 64, 0, 0, 30, 0, 0, 0, 0, 0, 0, 0, 0, 3, 4, 64, 224, 0, 0, 1, 44, 0, 0,
        0, 180, 0, 0, 0, 0, 32, 1, 4, 112, 0, 11, 1, 233, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 80, 4,
        177, 173, 35,
    ];

    #[test]
    fn advert1_print() {
        let mut arena = Arena::new();
        let mut allocator = arena.allocator();
        let x = RouterAdvertisement::read_byte_iter(&mut allocator, &mut ADVERT1.iter()).unwrap();
        panic!("{:#?}", x);
    }

    // Shaw
    const ADVERT2: &[u8] = &[
        134, 0, 215, 59, 64, 64, 0, 30, 0, 0, 0, 0, 0, 0, 0, 0, 3, 4, 64, 224, 0, 1, 81, 128, 0, 0,
        56, 64, 0, 0, 0, 0, 38, 4, 61, 9, 42, 127, 158, 1, 0, 0, 0, 0, 0, 0, 0, 0, 31, 5, 0, 0, 0,
        0, 0, 10, 7, 121, 121, 99, 45, 105, 110, 116, 17, 121, 97, 115, 97, 115, 104, 105, 105,
        115, 121, 110, 100, 105, 99, 97, 116, 101, 3, 111, 114, 103, 0, 0, 5, 1, 0, 0, 0, 0, 5,
    220, 1, 1, 0, 13, 185, 76, 238, 254,
    ];

    #[test]
    fn advert2_print() {
        let mut arena = Arena::new();
        let mut allocator = arena.allocator();
        let x = RouterAdvertisement::read_byte_iter(&mut allocator, &mut ADVERT2.iter()).unwrap();
        panic!("{:#?}", x);
    }

    // Telus
    const ADVERT3: &[u8] = &[
        134, 0, 66, 208, 64, 64, 0, 30, 0, 0, 0, 0, 0, 0, 0, 0, 3, 4, 64, 224, 0, 1, 81, 128, 0, 0,
        56, 64, 0, 0, 0, 0, 32, 1, 5, 106, 113, 181, 59, 1, 0, 0, 0, 0, 0, 0, 0, 0, 31, 5, 0, 0, 0,
        0, 0, 10, 7, 121, 121, 99, 45, 105, 110, 116, 17, 121, 97, 115, 97, 115, 104, 105, 105,
        115, 121, 110, 100, 105, 99, 97, 116, 101, 3, 111, 114, 103, 0, 0, 5, 1, 0, 0, 0, 0, 5,
        220, 1, 1, 0, 13, 185, 76, 229, 234,
    ];

    #[test]
    fn advert3_print() {
        let mut arena = Arena::new();
        let mut allocator = arena.allocator();
        let x = RouterAdvertisement::read_byte_iter(&mut allocator, &mut ADVERT3.iter()).unwrap();
        panic!("{:#?}", x);
    }
}
