use bytes::{Bytes, BytesMut};

#[derive(Debug)]
pub enum BytesCow<'a> {
    Owned(Bytes),
    Borrowed(&'a [u8]),
}

impl<'a> BytesCow<'a> {
    pub fn as_slice<'z>(&'z self) -> &'z [u8]
    where
        'a: 'z,
    {
        match *self {
            BytesCow::Owned(ref by) => by,
            BytesCow::Borrowed(by) => by,
        }
    }

    pub fn into_owned(self) -> BytesCow<'static> {
        BytesCow::Owned(match self {
            BytesCow::Owned(ref by) => by.clone(),
            BytesCow::Borrowed(by) => {
                let mut x = BytesMut::new();
                x.extend_from_slice(by);
                x.freeze()
            }
        })
    }
}
