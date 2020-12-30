use std::path::PathBuf;
use std::borrow::Cow;
use std::fmt;

use serde::{de, Deserialize, Deserializer,  Serialize, Serializer};
use serde::ser::SerializeMap;

use crate::torrent_id::TorrentId;

// --

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TorrentMeta {
    #[serde(default)]
    pub announce: String,
    #[serde(rename = "announce-list", default)]
    pub announce_list: Vec<Vec<String>>,
    #[serde(default)]
    pub comment: String,
    #[serde(rename = "created by", default)]
    pub created_by: String,
    #[serde(rename = "creation date", default)]
    pub creation_date: u64,
    pub info: TorrentMetaInfo,
}

#[derive(Debug, Clone)]
pub struct TorrentMetaInfo {
    pub piece_length_shift: u8,
    pub pieces: Vec<TorrentId>,
    pub name: String,
    pub files: Vec<TorrentMetaInfoFile>,
    pub length: u64,
    pub private: Option<bool>,
}

impl TorrentMetaInfo {
    pub fn is_multi_file(&self) -> bool {
        !self.files.is_empty()
    }
}

impl Serialize for TorrentMetaInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer 
    {
        use crate::torrent::torrent_id_buf::TorrentRawBytes;

        let mut entry_count = 4;
        if self.private.is_some() {
            entry_count += 1;
        }

        let mut map = serializer.serialize_map(Some(entry_count))?;

        if self.is_multi_file() {
            map.serialize_entry(&TorrentMetaInfoKey::Files, &self.files)?;
        } else {
            map.serialize_entry(&TorrentMetaInfoKey::Length, &self.length)?;
        }

        map.serialize_entry(&TorrentMetaInfoKey::Name, &self.name)?;
        map.serialize_entry(&TorrentMetaInfoKey::PieceLength, &(1u32 << self.piece_length_shift))?;
        map.serialize_entry(&TorrentMetaInfoKey::Pieces, &TorrentRawBytes(&self.pieces))?;

        if let Some(p) = self.private {
            map.serialize_entry(&TorrentMetaInfoKey::Private, &p)?;
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for TorrentMetaInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(TorrentMetaInfoVisitor)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TorrentMetaInfoKey {
    Files,
    Length,
    Name,
    #[serde(rename = "name.utf-8")]
    NameUtf8,
    #[serde(rename = "piece length")]
    PieceLength,
    Pieces,
    Private,
}

struct TorrentMetaInfoVisitor;

impl<'de> serde::de::Visitor<'de> for TorrentMetaInfoVisitor {
    type Value = TorrentMetaInfo;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "a non-empty byte-array with a multiple of 20 bytes"
        )
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut piece_length_shift = None;
        let mut pieces = None;
        let mut name = None;
        let mut name_utf8 = None;
        let mut files = None;
        let mut length = None;
        let mut private = None;

        while let Some(k) = map.next_key()? {
            match k {
                TorrentMetaInfoKey::PieceLength => {
                    let value = map.next_value()?;
                    if piece_length_shift.is_some() {
                        return Err(serde::de::Error::duplicate_field("piece length"));
                    }
                    piece_length_shift = Some(piece_length_shift::visit_u32(value)?);
                }
                TorrentMetaInfoKey::Pieces => {
                    let piece_data: Cow<serde_bytes::Bytes> = map.next_value()?;
                    if pieces.is_some() {
                        return Err(serde::de::Error::duplicate_field("pieces"));
                    }
                    pieces = Some(torrent_id_buf::visit_buf(&piece_data)?);
                }
                TorrentMetaInfoKey::Name => {
                    let value: String = map.next_value()?;
                    if name.is_some() {
                        return Err(serde::de::Error::duplicate_field("name"));
                    }
                    name = Some(value);
                }
                TorrentMetaInfoKey::NameUtf8 => {
                    let value: String = map.next_value()?;
                    if name_utf8.is_some() {
                        return Err(serde::de::Error::duplicate_field("name.utf-8"));
                    }
                    name_utf8 = Some(value);
                }
                TorrentMetaInfoKey::Files => {
                    let value: Vec<TorrentMetaInfoFile> = map.next_value()?;
                    if files.is_some() {
                        return Err(serde::de::Error::duplicate_field("files"));
                    }
                    files = Some(value);
                }
                TorrentMetaInfoKey::Length => {
                    let value: u64 = map.next_value()?;
                    if length.is_some() {
                        return Err(serde::de::Error::duplicate_field("length"));
                    }
                    length = Some(value);
                }
                TorrentMetaInfoKey::Private => {
                    let value: bool = map.next_value()?;
                    if private.is_some() {
                        return Err(serde::de::Error::duplicate_field("private"));
                    }
                    private = Some(value);
                }
            }
        }

        let piece_length_shift = piece_length_shift.ok_or_else(|| {
            serde::de::Error::missing_field("piece length")
        })?;
        let pieces = pieces.ok_or_else(|| {
            serde::de::Error::missing_field("pieces")
        })?;
        let mut name = name.ok_or_else(|| {
            serde::de::Error::missing_field("name")
        })?;
        if let Some(name_v) = name_utf8 {
            name = name_v;
        }

        let final_files;
        let final_length;

        match (files, length) {
            (None, None) => {
                let msg = "missing field: either files or length must be specified";
                return Err(serde::de::Error::custom(msg));
            }
            (None, Some(length_val)) => {
                final_files = vec![];
                final_length = length_val;
            }
            (Some(mut files_val), None) => {
                let mut length_val = 0;
                for f in &mut files_val {
                    f.global_offset = length_val;
                    length_val += f.length;
                }

                final_files = files_val;
                final_length = length_val;
            }
            (Some(..), Some(..)) => {
                let msg = "too many fields: files and length are mutually exclusive";
                return Err(serde::de::Error::custom(msg));
            }
        };

        Ok(TorrentMetaInfo {
            piece_length_shift,
            pieces,
            name,
            files: final_files,
            length: final_length,
            private,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TorrentMetaInfoFile {
    pub length: u64,
    #[serde(skip)]
    pub global_offset: u64,
    #[serde(with = "bt_pathbuf")]
    pub path: PathBuf,
}

mod torrent_id_buf {
    use serde::{Serialize, Serializer};

    use crate::TorrentId;

    const EXPECTATION: &str = "a non-empty byte-array with a multiple of 20 bytes";

    pub fn visit_buf<E>(piece_data: &[u8]) -> Result<Vec<TorrentId>, E>
    where
        E: serde::de::Error,
    {
        if piece_data.len() % 20 != 0 || piece_data.len() == 0 {
            return Err(serde::de::Error::invalid_length(
                piece_data.len(),
                &EXPECTATION,
            ));
        }

        let mut piece_shas = Vec::new();
        for c in piece_data.chunks(20) {
            let mut tid = TorrentId::zero();
            tid.as_mut_bytes().copy_from_slice(c);
            piece_shas.push(tid);
        }

        Ok(piece_shas)
    }

    pub struct TorrentRawBytes<'a>(pub &'a [TorrentId]);

    impl Serialize for TorrentRawBytes<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer 
        {
            let mut flattened = Vec::new();
            for tid in self.0 {
                flattened.extend(tid.as_bytes());
            }
            serializer.serialize_bytes(&flattened)
        }
    }
}

mod piece_length_shift {
    const EXPECTATION: &str = "an integer power of two within [0, 4294967296)";

    pub fn visit_u32<E>(v: u32) -> Result<u8, E>
    where
        E: serde::de::Error,
    {
        let candidate = 1 << v.trailing_zeros();
        if v != candidate {
            return Err(<E as serde::de::Error>::invalid_value(
                serde::de::Unexpected::Unsigned(v as u64),
                &EXPECTATION,
            ));
        }
        Ok(v.trailing_zeros() as u8)
    }
}

mod bt_pathbuf {
    use std::borrow::Cow;
    use std::fmt;
    use std::path::{Component, Path, PathBuf};

    use serde::de::{self, Deserializer, SeqAccess, Visitor};
    use serde::ser::{self, SerializeSeq, Serializer};

    pub fn serialize<S>(buf: &PathBuf, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(buf.components().count()))?;

        for co in buf.components() {
            match co {
                Component::Prefix(..) | Component::RootDir => {
                    return Err(ser::Error::custom("path must not be absolute"));
                }
                Component::CurDir | Component::ParentDir => {
                    return Err(ser::Error::custom("path must be canonical"));
                }
                Component::Normal(v) => {
                    seq.serialize_element(v)?;
                }
            }
        }

        seq.end()
    }

    struct _Visitor;

    impl<'de> Visitor<'de> for _Visitor {
        type Value = PathBuf;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a non-empty vector of path elements")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut buf = PathBuf::new();

            let mut observed_element = false;
            while let Some(part) = seq.next_element::<Cow<Path>>()? {
                buf.push(part);
                observed_element = true;
            }
            if !observed_element {
                return Err(de::Error::custom("path vec must be non-empty"));
            }

            Ok(buf)
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(_Visitor)
    }
}
