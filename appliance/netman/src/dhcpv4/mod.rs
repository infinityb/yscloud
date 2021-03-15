
type DhcpResult<T> = Result<T, anyhow::Error>;

struct DhcpRequestConfig {
	op: u8,
	htype: u8,
	hlen: u8,
	hops: u8,
	xid: u32,
	secs: u16,
	flags: u16,
	ciaddr: u32,
	yiaddr: u32,
	siaddr: u32,
	giaddr: u32,
	chaddr: [u32,

}

pub fn deserialize_request(data: &[u8]) -> anyhow::Result<DhcpRequestConfig> {
	/
}

pub fn serialize_request<'a>(scratch: &'a mut [u8], config: &DhcpRequestConfig) -> Option<&'a [u8]> {
	Some(&[])
}

// pub fn deserialize_reponse()

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

}