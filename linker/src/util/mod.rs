pub fn hexify<'sc>(scratch: &'sc mut [u8], data: &[u8]) -> Option<&'sc str> {
    static HEX_CHARS: &[u8] = b"0123456789abcdef";
    let hex_length = data.len() * 2;

    if scratch.len() < hex_length {
        return None;
    }

    let mut scratch_iter = scratch.iter_mut();
    for by in data {
        // TODO(sell): does the compiler know we can do unchecked accesses
        // at this location?  Or are we doing something too complicated for
        // the optimizer to inspect.
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by >> 4)];
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by & 0x0F)];
    }

    drop(scratch_iter);

    Some(::std::str::from_utf8(&scratch[..hex_length]).unwrap())
}
