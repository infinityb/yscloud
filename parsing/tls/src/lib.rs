use std::fmt;
use std::slice;
use std::error;

use byteorder::{BigEndian, ByteOrder};
use copy_arena::Allocator;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Truncated,
    ProtocolViolation,
    Other,
}

#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn truncated() -> Error {
        Error {
            kind: ErrorKind::Truncated,
            message: "message truncated".into(),
        }
    }

    pub fn protocol_violation() -> Error {
        Error {
            kind: ErrorKind::ProtocolViolation,
            message: "protocol violation".into(),
        }
    }
}

pub trait ByteIterRead<'arena>: Sized + 'arena {
    type Error;

    fn read_byte_iter(
        _: &mut Allocator<'arena>,
        _: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error>;
}

pub trait ByteIterReadNoAlloc<'arena>: ByteIterRead<'arena> {
    fn read_byte_iter_no_alloc(_: &mut slice::Iter<u8>) -> Result<Self, Self::Error>;
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

fn iter_fill<'d, I>(dst: &mut [u8], iter: &mut I) -> Result<(), Error> where I: Iterator<Item=&'d u8> {
    let mut copied = 0;
    for (o, i) in dst.iter_mut().zip(iter) {
        *o = *i;
        copied += 1;
    }
    if copied != dst.len() {
        return Err(Error::truncated());
    }
    Ok(())
}

#[derive(Debug, Copy, Clone)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
}

impl<'arena> ByteIterReadNoAlloc<'arena> for ProtocolVersion {
    fn read_byte_iter_no_alloc(data: &mut slice::Iter<u8>) -> Result<Self, Self::Error> {
        let major = *data.next().ok_or_else(Error::truncated)?;
        let minor = *data.next().ok_or_else(Error::truncated)?;
        Ok(ProtocolVersion { major, minor })
    }
}

impl<'arena> ByteIterRead<'arena> for ProtocolVersion {
    type Error = Error;

    fn read_byte_iter(
        _: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        ByteIterReadNoAlloc::read_byte_iter_no_alloc(data)
    }
}

const RANDOM_BYTES_RANDOM_BYTES_LEN: usize = 28;

#[derive(Debug, Copy, Clone)]
pub struct Random {
    gmt_unix_time: u32,
    random_bytes: [u8; RANDOM_BYTES_RANDOM_BYTES_LEN],
}

impl Random {
    pub fn zero() -> Random {
        Random {
            gmt_unix_time: 0,
            random_bytes: [0; RANDOM_BYTES_RANDOM_BYTES_LEN],
        }
    }
}

impl<'arena> ByteIterRead<'arena> for Random {
    type Error = Error;

    fn read_byte_iter(
        _: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let gmt_unix_time_bytes = iter_skip(4, data)?;
        let mut out = Random::zero();
        out.gmt_unix_time = BigEndian::read_u32(gmt_unix_time_bytes);
        iter_fill(&mut out.random_bytes, data)?;
        Ok(out)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ClientHello<'arena> {
    pub client_version: ProtocolVersion,
    pub random: Random,
    pub session_id: SessionID<'arena>,
    pub cipher_suites: CipherSuites<'arena>,
    pub compression_methods: CompressionMethods<'arena>,
    pub extensions: Extensions<'arena>,
}

impl<'arena> ByteIterRead<'arena> for ClientHello<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let client_version = ProtocolVersion::read_byte_iter(alloc, data)?;
        let random = Random::read_byte_iter(alloc, data)?;
        let session_id = SessionID::read_byte_iter(alloc, data)?;
        let cipher_suites = CipherSuites::read_byte_iter(alloc, data)?;
        let compression_methods = CompressionMethods::read_byte_iter(alloc, data)?;
        let extensions = Extensions::read_byte_iter(alloc, data)?;

        Ok(ClientHello {
            client_version,
            random,
            session_id,
            cipher_suites,
            compression_methods,
            extensions,
        })
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Handshake<'arena> {
    ClientHello(&'arena ClientHello<'arena>),
    Unknown(u8, &'arena [u8]),
}

#[derive(Debug, Copy, Clone)]
pub struct Record<'de> {
    pub content_type: u8,
    pub proto_version: ProtocolVersion,
    pub data: &'de [u8],
}

#[derive(Debug, Copy, Clone)]
pub struct RecordPrefix {
    pub content_type: u8,
    pub proto_version: ProtocolVersion,
    pub length: u16,
}

#[derive(Debug, Copy, Clone)]
pub struct SessionID<'arena>(&'arena [u8]);

impl<'arena> ByteIterRead<'arena> for SessionID<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let length = *data.next().ok_or_else(Error::truncated)?;
        if 32 < length {
            return Err(Error::protocol_violation());
        }
        let data = iter_skip(length as usize, data)?;
        Ok(SessionID(alloc.alloc_slice(data)))
    }
}

const TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: u16 = 0xc02b;
const TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: u16 = 0xc02f;
const TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256: u16 = 0xcca9;
const TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256: u16 = 0xcca8;
const TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: u16 = 0xc02c;
const TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: u16 = 0xc030;
const TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA: u16 = 0xc00a;
const TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA: u16 = 0xc009;
const TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA: u16 = 0xc013;
const TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA: u16 = 0xc014;
const TLS_DHE_RSA_WITH_AES_128_CBC_SHA: u16 = 0x0033;
const TLS_DHE_RSA_WITH_AES_256_CBC_SHA: u16 = 0x0039;
const TLS_RSA_WITH_AES_128_CBC_SHA: u16 = 0x002f;
const TLS_RSA_WITH_AES_256_CBC_SHA: u16 = 0x0035;
const TLS_RSA_WITH_3DES_EDE_CBC_SHA: u16 = 0x000a;

#[derive(Copy, Debug, Clone)]
#[allow(non_camel_case_types)]
pub enum CipherSuite {
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_RSA_WITH_AES_128_CBC_SHA,
    TLS_RSA_WITH_AES_256_CBC_SHA,
    TLS_RSA_WITH_3DES_EDE_CBC_SHA,
    Unknown(u16),
}

#[derive(Debug, Copy, Clone)]
pub enum CompressionMethod {
    None,
}

impl<'arena> ByteIterRead<'arena> for CompressionMethod {
    type Error = Error;

    fn read_byte_iter(
        _alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let val = *data.next().ok_or_else(Error::truncated)?;
        if val != 0 {
            return Err(Error::protocol_violation());
        }
        Ok(CompressionMethod::None)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct CompressionMethods<'arena>(&'arena [CompressionMethod]);

impl<'arena> ByteIterRead<'arena> for CompressionMethods<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let length = *data.next().ok_or_else(Error::truncated)? as usize;
        let methods = alloc.alloc_slice_fn(length, |_| CompressionMethod::None);
        for m in methods.iter_mut() {
            *m = CompressionMethod::read_byte_iter(alloc, data)?;
        }
        Ok(CompressionMethods(methods))
    }
}

#[derive(Debug, Copy, Clone)]
pub struct CipherSuites<'arena>(pub &'arena [CipherSuite]);

impl<'arena> ByteIterRead<'arena> for CipherSuites<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let mut length: u16 = 0;
        length += u16::from(*data.next().ok_or_else(Error::truncated)?);
        length <<= 8;
        length += u16::from(*data.next().ok_or_else(Error::truncated)?);
        if length & 0x01 != 0 {
            // length must be even.
            return Err(Error::protocol_violation());
        }
        if 65534 < length {
            return Err(Error::protocol_violation());
        }
        length /= 2;

        let suites = alloc.alloc_slice_fn(length as usize, |_| CipherSuite::Unknown(0));
        for s in suites.iter_mut() {
            let mut current_cipher: u16 = 0;
            current_cipher += u16::from(*data.next().ok_or_else(Error::truncated)?);
            current_cipher <<= 8;
            current_cipher += u16::from(*data.next().ok_or_else(Error::truncated)?);

            *s = match current_cipher {
                TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 => {
                    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
                }
                TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 => {
                    CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
                }
                TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 => {
                    CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
                }
                TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 => {
                    CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
                }
                TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 => {
                    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
                }
                TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 => {
                    CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
                }
                TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA => {
                    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA
                }
                TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA => {
                    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA
                }
                TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA => {
                    CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA
                }
                TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA => {
                    CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA
                }
                TLS_DHE_RSA_WITH_AES_128_CBC_SHA => CipherSuite::TLS_DHE_RSA_WITH_AES_128_CBC_SHA,
                TLS_DHE_RSA_WITH_AES_256_CBC_SHA => CipherSuite::TLS_DHE_RSA_WITH_AES_256_CBC_SHA,
                TLS_RSA_WITH_AES_128_CBC_SHA => CipherSuite::TLS_RSA_WITH_AES_128_CBC_SHA,
                TLS_RSA_WITH_AES_256_CBC_SHA => CipherSuite::TLS_RSA_WITH_AES_256_CBC_SHA,
                TLS_RSA_WITH_3DES_EDE_CBC_SHA => CipherSuite::TLS_RSA_WITH_3DES_EDE_CBC_SHA,
                _ => CipherSuite::Unknown(current_cipher),
            };
        }
        Ok(CipherSuites(suites))
    }
}

const HANDSHAKE_CLIENT_HELLO: u8 = 1;

impl<'arena> ByteIterRead<'arena> for Handshake<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let msg_type = *data.next().ok_or_else(Error::truncated)?;
        let mut length: usize = 0;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        let data = iter_skip(length, data)?;

        match msg_type {
            HANDSHAKE_CLIENT_HELLO => {
                let mut handshake_data = data.iter();
                let client_hello = ClientHello::read_byte_iter(alloc, &mut handshake_data)?;
                let client_hello = alloc.alloc(client_hello);
                Ok(Handshake::ClientHello(client_hello))
            }
            _ => {
                Ok(Handshake::Unknown(msg_type, alloc.alloc_slice(data)))
            }
        }
    }
}

pub fn extract_record_prefix(data: &mut slice::Iter<u8>) -> Result<Option<RecordPrefix>, Error> {
    let content_type = match data.next() {
        Some(ct) => *ct,
        None => return Ok(None),
    };
    let proto_version: ProtocolVersion = ByteIterReadNoAlloc::read_byte_iter_no_alloc(data)?;
    let length = BigEndian::read_u16(iter_skip(2, data)?);
    Ok(Some(RecordPrefix {
        content_type,
        proto_version,
        length,
    }))
}

pub fn extract_record<'de>(data: &mut slice::Iter<'de, u8>) -> Result<Option<Record<'de>>, Error> {
    let record = match extract_record_prefix(data)? { Some(x) => x, None => return Ok(None) };

    let data = iter_skip(record.length as usize, data)?;
    Ok(Some(Record {
        proto_version: record.proto_version,
        content_type: record.content_type,
        data,
    }))
}

const TLS_EXTENSION_SERVER_NAME: u16 = 0x0000;

#[derive(Debug, Copy, Clone)]
pub struct ExtensionServerNameEntry<'arena>(pub &'arena str);

impl<'arena> ByteIterRead<'arena> for ExtensionServerNameEntry<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let name_type = *data.next().ok_or_else(Error::truncated)?;
        if name_type != 0 {
            return Err(Error::protocol_violation());
        }

        let mut length: usize = 0;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;

        let host_name_bytes = alloc.alloc_slice(iter_skip(length, data)?);
        let host_name =
            std::str::from_utf8(host_name_bytes).map_err(|_| Error::protocol_violation())?;
        Ok(ExtensionServerNameEntry(host_name))
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ExtensionServerName<'arena>(pub &'arena [ExtensionServerNameEntry<'arena>]);

impl<'arena> ByteIterRead<'arena> for ExtensionServerName<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        fn size_server_name(data: &mut slice::Iter<u8>) -> Result<usize, Error> {
            let mut counter = 0;
            while let Some(_name_type) = data.next() {
                counter += 1;

                let mut length: usize = 0;
                length += *data.next().ok_or_else(Error::truncated)? as usize;
                length <<= 8;
                length += *data.next().ok_or_else(Error::truncated)? as usize;

                iter_skip(length, data)?;
            }
            Ok(counter)
        }

        let mut length: usize = 0;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;

        let mut server_name_data = iter_skip(length, data)?.iter();
        let entry_number = size_server_name(&mut server_name_data.clone())?;

        let entries = alloc.alloc_slice_fn(entry_number, |_| ExtensionServerNameEntry(""));
        for e in entries.iter_mut() {
            *e = ExtensionServerNameEntry::read_byte_iter(alloc, &mut server_name_data)?;
        }
        Ok(ExtensionServerName(entries))
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Extension<'arena> {
    ServerName(&'arena ExtensionServerName<'arena>),
    Unknown(u16, &'arena [u8]),
}

impl<'arena> ByteIterRead<'arena> for Extension<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        let mut ex_type: u16 = 0;
        ex_type += u16::from(*data.next().ok_or_else(Error::truncated)?);
        ex_type <<= 8;
        ex_type += u16::from(*data.next().ok_or_else(Error::truncated)?);

        let mut length: usize = 0;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;

        let ex_data = iter_skip(length, data)?;
        match ex_type {
            TLS_EXTENSION_SERVER_NAME => {
                let mut ex_data_iter = ex_data.iter();
                let server_name = ExtensionServerName::read_byte_iter(alloc, &mut ex_data_iter)?;
                Ok(Extension::ServerName(alloc.alloc(server_name)))
            }
            _ => Ok(Extension::Unknown(ex_type, alloc.alloc_slice(ex_data))),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Extensions<'arena>(pub &'arena [Extension<'arena>]);

impl<'arena> ByteIterRead<'arena> for Extensions<'arena> {
    type Error = Error;

    fn read_byte_iter(
        alloc: &mut Allocator<'arena>,
        data: &mut slice::Iter<u8>,
    ) -> Result<Self, Self::Error> {
        fn size_extensions(data: &mut slice::Iter<u8>) -> Result<usize, Error> {
            let mut counter = 0;
            while let Some(_) = data.next() {
                counter += 1;

                // second byte of the type
                data.next().ok_or_else(Error::truncated)?;

                let mut length: usize = 0;
                length += *data.next().ok_or_else(Error::truncated)? as usize;
                length <<= 8;
                length += *data.next().ok_or_else(Error::truncated)? as usize;

                iter_skip(length, data)?;
            }
            Ok(counter)
        }

        let mut length: usize = 0;
        length += *data.next().ok_or_else(Error::truncated)? as usize;
        length <<= 8;
        length += *data.next().ok_or_else(Error::truncated)? as usize;

        let mut extensions = iter_skip(length, data)?.iter();
        let length = size_extensions(&mut extensions.clone())?;
        let ex_entries = alloc.alloc_slice_fn(length, |_| Extension::Unknown(0, &[]));
        for e in ex_entries.iter_mut() {
            *e = ByteIterRead::read_byte_iter(alloc, &mut extensions)?;
        }
        Ok(Extensions(ex_entries))
    }
}

pub const RECORD_CONTENT_TYPE_HANDSHAKE: u8 = 22;

#[cfg(test)]
mod tests {
    use super::{ByteIterRead, Handshake, extract_record, RECORD_CONTENT_TYPE_HANDSHAKE};
    use copy_arena::Arena;

    const EXAMPLE_1: &[u8] = b"\x16\x03\x01\x02q\x01\x00\x02m\x03\x03\x0c\x88\xc1\xc4F\xba\xfb\"y\xdf\x9f\x8f\xee/!b\x06i\n=q\x04/\xe6\";\xb4\x10\x9c\x96\x13m f\xee\xafs\xb9sn\x90\t\xe8\x87\x16\x043T\xb6\xbc%/l\xdd\xe1\x10\xf6\x18S\x07\x17\xb3I\\\xc9\x00$\x13\x01\x13\x03\x13\x02\xc0+\xc0/\xcc\xa9\xcc\xa8\xc0,\xc00\xc0\n\xc0\t\xc0\x13\xc0\x14\x003\x009\x00/\x005\x00\n\x01\x00\x02\x00\x00\x00\x00\x13\x00\x11\x00\x00\x0ewww.google.com\x00\x17\x00\x00\xff\x01\x00\x01\x00\x00\n\x00\x0e\x00\x0c\x00\x1d\x00\x17\x00\x18\x00\x19\x01\x00\x01\x01\x00\x0b\x00\x02\x01\x00\x00\x10\x00\x0e\x00\x0c\x02h2\x08http/1.1\x00\x05\x00\x05\x01\x00\x00\x00\x00\x003\x00k\x00i\x00\x1d\x00 XN\x1e},\xf0\x16\xe4\x8b\xc5\xf0rl\x07\xbd\xf7\x1c\xa04\xdc\x9a\x02m\xee\xe7\x03N\x7f\x91\x07\xf3k\x00\x17\x00A\x047\x9bGE]p\x14\x7f.\xff\x8fj\x1fN\xb6\xaa\xebk\x15 \x02\x7f\x1f\x8dW\'^\x18\xd7 +0\xd3\xc6)0\x04\xacT\x9f\xcf\xfcr\x12`\x19\xc6wXw\xe1\x90\x14\xfa\xab\xb8\xbf\xc8\xdd3\x80\xec\xb8{\x00+\x00\t\x08\x03\x04\x03\x03\x03\x02\x03\x01\x00\r\x00\x18\x00\x16\x04\x03\x05\x03\x06\x03\x08\x04\x08\x05\x08\x06\x04\x01\x05\x01\x06\x01\x02\x03\x02\x01\x00-\x00\x02\x01\x01\x00\x1c\x00\x02@\x01\x00)\x01\x05\x00\xe0\x00\xda\x00\xf1\xa5d\xfe\xf1R\xdd\xf8\xcf\xb8]\xd0N\xf4[6\xca \x9aG\x9ck\xd8\xb5P\xe0\x10?(\x1aI\x96\t\x87\xc8d\x91s\xd9\x96@\xf3`\xed#\xb9*j\xc1\x94[\x19\xb3\xca&\x10!~\xc5{\x06~\xe0 \xf6p\xb2\xa1\x12\xb5,\xaf\x98\xdf\x94\xda\x15\xe8\xa1\xe7,\x9e\xc2\x0e\x83\xb6\x10\xc0\xd5\x87\xc6P,\xfe<~\xf2\xd5\xbd\xc43\x9d\x9e\x1f\x13\xa6B\x1c\x8b\xdc\xa5{\xb9\x86Y\xe7\x10\xe7J\xfa!e\xb8\xb6#\x00\xb1*\x99\x7ff\x03\xd0\xcb1V\x91\xb24\xd4\xc4q\x053\x01\x04I\xae\xa9\xc5\x80\xef\xa0 c\x08\xb9m\x93\x9a\xd0k%[! 2\xd7\x08T\x8a\x03u\xce.\xf1\xbd\x9e\x04L\x06_,;\xd2r\x94\xe7\xec\xb5\xf8h\xa3\xb7\x8d\x8f\x05\xcd\x9a\xcd\xad68\xe0\xae\x0c\x97\x98\xcd\x89Kh\'K\x1a\x8eFB,r*\x00! \x92\xac\xb6\x99{CN\xcb6Q\xa5\xd1(\x8dE\xed-\xa9\xb1S\xcaO\x0e\\e\r\x89<\xad\xf5S*";

    #[test]
    fn example_1() {
        let mut arena = Arena::with_capacity(32 * 1024);
        let mut allocator = arena.allocator();

        let mut data_size = 0;
        let mut record_iter = EXAMPLE_1.iter();
        while let Some(record) = extract_record(&mut record_iter).unwrap() {
            if record.content_type != RECORD_CONTENT_TYPE_HANDSHAKE {
                continue;
            }
            data_size += record.data.len();
        }

        let unframed_data = allocator.alloc_slice_default(data_size);
        let mut unframed_data_write = &mut unframed_data[..];

        let mut record_iter = EXAMPLE_1.iter();
        while let Some(record) = extract_record(&mut record_iter).unwrap() {
            if record.content_type != RECORD_CONTENT_TYPE_HANDSHAKE {
                continue;
            }

            let (to_write, rest) = unframed_data_write.split_at_mut(record.data.len());
            to_write.copy_from_slice(record.data);
            unframed_data_write = rest;
        }
        assert_eq!(unframed_data_write.len(), 0);
        drop(unframed_data_write);

        let handshake = Handshake::read_byte_iter(&mut allocator, &mut unframed_data.iter()).unwrap();
        println!("{:?}", handshake);
        panic!();
    }
}