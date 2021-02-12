use std::mem;
use std::fmt;
use std::slice;


const ROUTER_ADVERTISED_STATIC_LEN: usize = 16;
const MANAGED_ADDRESS_CONFIGURATION: u8 = 0x80;
const OTHER_CONFIGURATION: u8 = 0x40;

#[repr(C, packed)]
pub struct RouterAdvertisement {
    type_: u8,
    code: u8,
    checksum: u16,
    cur_hop_limit: u8,
    flags1: u8,
    router_lifetime: u16,
    reachable_time: u32,
    retrans_timer: u32,
    options: [u8],
}

impl fmt::Debug for RouterAdvertisement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("RouterAdvertisement");
        let type_ = self.type_;
        d.field("type_", &type_);
        let code = self.code;
        d.field("code", &code);
        let checksum = self.checksum;
        d.field("checksum", &checksum);
        let cur_hop_limit = self.cur_hop_limit;
        d.field("cur_hop_limit", &cur_hop_limit);
        let flags1 = self.flags1;
        d.field("flags1", &flags1);
        let router_lifetime = self.router_lifetime;
        d.field("router_lifetime", &router_lifetime);
        let reachable_time = self.reachable_time;
        d.field("reachable_time", &reachable_time);
        let retrans_timer = self.retrans_timer;
        d.field("retrans_timer", &retrans_timer);
        d.field("options", &DebugOptsIter(self.options()));
        d.finish()
    }
}


struct DebugOptsIter<'a>(OptIter<'a>);

impl<'a> fmt::Debug for DebugOptsIter<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_list().entries(self.0.clone()).finish()
    }
}

#[derive(Debug)]
pub struct Error {
    //
}

impl RouterAdvertisement {
    pub fn from_bytes<'a>(data: &'a [u8]) -> Result<&'a RouterAdvertisement, Error> {
        #[repr(C, packed)]
        pub struct Repr {
            ptr: *const u8,
            len: usize,
        }

        if data.len() < ROUTER_ADVERTISED_STATIC_LEN {
            return Err(Error{});
        }

        let r = Repr {
            ptr: data.as_ptr(),
            len: data.len() - ROUTER_ADVERTISED_STATIC_LEN,
        };

        Ok(unsafe { mem::transmute(r) })
    }

    pub fn is_managed(&self) -> bool {
        self.flags1 & MANAGED_ADDRESS_CONFIGURATION > 0
    }

    pub fn is_other_managed(&self) -> bool {
        self.flags1 & OTHER_CONFIGURATION > 0
    }

    pub fn options(&self) -> OptIter {
        OptIter::new(self.options.iter())
    }
}

pub struct LinkLayerAddress([u8; 6]);

impl LinkLayerAddress {
    #[allow(clippy::needless_lifetimes)]
    pub fn from_bytes<'a>(data: &'a [u8]) -> Result<&'a LinkLayerAddress, Error> {
        if data.len() != mem::size_of::<LinkLayerAddress>() {
            return Err(unimplemented!());
        }
        let addr = data.as_ptr() as *const u8 as *const LinkLayerAddress;
        Ok(unsafe { &*addr })
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


#[repr(C, packed)]
pub struct Mtu {
    reserved1: u16,
    mtu: u32,
}

impl fmt::Debug for Mtu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let reserved1 = self.reserved1;
        let mtu = self.mtu.to_be();
        f.debug_struct("Mtu")
            .field("reserved1", &reserved1)
            .field("mtu", &mtu)
            .finish()
    }
}

impl Mtu {
    pub fn from_bytes<'a>(data: &'a [u8]) -> Result<&'a Mtu, Error> {
        if data.len() != mem::size_of::<Mtu>() {
            return Err(unimplemented!());
        }

        let pref = data.as_ptr() as *const u8 as *const Mtu;
        Ok(unsafe { &*pref })
    }

    pub fn value(&self) -> u32 {
        self.mtu.to_be()
    }
}

#[repr(C, packed)]
// #[derive(Debug)]
pub struct IPv6Address([u16; 8]);

impl fmt::Debug for IPv6Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IPv6Address([0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}])",
            self.0[0].to_be(), self.0[1].to_be(), self.0[2].to_be(), self.0[3].to_be(),
            self.0[4].to_be(), self.0[5].to_be(), self.0[6].to_be(), self.0[7].to_be())
    }
}


#[repr(C, packed)]
#[derive(Debug)]
pub struct PrefixInformation {
    prefix_length: u8,
    flags1: u8, 
    valid_lifetime: u32,
    preferred_lifetime: u32,
    reserved2: u32,
    prefix: IPv6Address,
}

impl PrefixInformation {
    pub fn from_bytes<'a>(data: &'a [u8]) -> Result<&'a PrefixInformation, Error> {
        if data.len() != mem::size_of::<PrefixInformation>() {
            return Err(unimplemented!());
        }

        let pref = data.as_ptr() as *const u8 as *const PrefixInformation;
        Ok(unsafe { &*pref })
    }
}

#[derive(Debug)]
pub enum RouterAdvertisementOption<'a> {
    SourceLinkLayerAddress(&'a LinkLayerAddress),
    TargetLinkLayerAddress(&'a LinkLayerAddress),
    Mtu(&'a Mtu),
    PrefixInformation(&'a PrefixInformation),
    DnsSearchList(&'a str),
    Unknown(u8, &'a [u8]),
}

#[derive(Debug, Clone)]
pub struct OptIter<'a> {
    parent: slice::Iter<'a, u8>,
}

const OPTION_TYPE_SOURCE_LINK_LAYER_ADDRESS: u8 = 1;
const OPTION_TYPE_TARGET_LINK_LAYER_ADDRESS: u8 = 2;
const OPTION_TYPE_PREFIX_INFORMATION: u8 = 3;
const OPTION_TYPE_REDIRECT_HEADER: u8 = 4;
const OPTION_TYPE_MTU: u8 = 5;
const OPTION_DNS_SEARCH_LIST: u8 = 31;

impl<'a> Iterator for OptIter<'a> {
    type Item = RouterAdvertisementOption<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let type_ = self.parent.next()?;
        let length_byte = self.parent.next()?;
        let length = *length_byte as usize * 8 - 2;

        let mut slice = self.parent.as_slice();
        self.parent = slice[length..].iter();
        slice = &slice[..length];
        
        match *type_ {
            OPTION_TYPE_SOURCE_LINK_LAYER_ADDRESS => {
                let addr = LinkLayerAddress::from_bytes(slice).unwrap();
                Some(RouterAdvertisementOption::SourceLinkLayerAddress(addr))
            }
            OPTION_TYPE_TARGET_LINK_LAYER_ADDRESS => {
                let addr = LinkLayerAddress::from_bytes(slice).unwrap();
                Some(RouterAdvertisementOption::SourceLinkLayerAddress(addr))
            }
            OPTION_TYPE_PREFIX_INFORMATION => {
                let addr = PrefixInformation::from_bytes(slice).unwrap();
                Some(RouterAdvertisementOption::PrefixInformation(addr))
            }
            OPTION_TYPE_REDIRECT_HEADER => {
                unimplemented!();
            }
            OPTION_TYPE_MTU => {
                let mtu = Mtu::from_bytes(slice).unwrap();
                Some(RouterAdvertisementOption::Mtu(mtu))
            }
            OPTION_DNS_SEARCH_LIST => {
                // this seems useless to us?
                Some(RouterAdvertisementOption::DnsSearchList(::std::str::from_utf8(slice).unwrap()))
            }
            _ => {
                Some(RouterAdvertisementOption::Unknown(*type_, slice))
            }
        }
    }
}

impl<'a> OptIter<'a> {
    fn new(parent: slice::Iter<'a, u8>) -> OptIter<'a> {
        OptIter { parent }
    }
}
