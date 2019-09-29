pub fn hexify<'sc>(scratch: &'sc mut [u8], data: &[u8]) -> Option<&'sc str> {
    static HEX_CHARS: &[u8] = b"0123456789abcdef";

    let mut scratch_iter = scratch.iter_mut();

    for by in data {
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by >> 4)];
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by & 0x0F)];
    }

    drop(scratch_iter);

    Some(std::str::from_utf8(&scratch[..data.len() * 2]).unwrap())
}
