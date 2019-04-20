use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use time::Tm;

use zip::result::ZipError;
use zip::ZipArchive;

pub enum NodeData {
    Directory(Directory),
    Regular(Regular),
}

pub enum NodeKind {
    Directory,
    Regular,
}

pub struct StaticEntry {
    name: String,
    data: NodeData,
}

pub struct Directory {
    entries: HashMap<String, StaticEntry>,
}

pub struct Regular {
    last_modified_time: Tm,
    etag: String,
    raw_data: Arc<[u8]>,
}

pub struct Metadata {
    node_kind: NodeKind,
    last_modified_time: Option<Tm>,
    // length is zero if self.node_kind == NodeKind::Directory
    length: usize,
}

pub enum ErrorKind {
    NotFound,
    Other,
}

pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    fn new<M: Into<String>>(kind: ErrorKind, msg: M) -> Error {
        Error {
            kind,
            message: msg.into(),
        }
    }

    fn not_found() -> Error {
        Error::new(ErrorKind::NotFound, "not found")
    }
}

impl StaticEntry {
    pub fn metadata(&self) -> Metadata {
        let node_kind;
        let last_modified_time;
        let length;
        match self.data {
            NodeData::Directory(ref dir) => {
                node_kind = NodeKind::Directory;
                last_modified_time = None;
                length = 0;
            }
            NodeData::Regular(ref reg) => {
                node_kind = NodeKind::Regular;
                last_modified_time = Some(reg.last_modified_time);
                length = reg.raw_data.len();
            }
        }
        Metadata {
            node_kind,
            last_modified_time,
            length,
        }
    }
}

impl StaticEntry {}

impl Directory {
    fn recurse_helper<'dir, 'path, 'iter, I>(
        &'dir self,
        path: &'iter mut I,
    ) -> Result<&'dir Directory, Error>
    where
        I: Iterator<Item = &'path str>,
    {
        let mut entry_path: Vec<&Directory> = Vec::new();
        entry_path.push(self);

        for element in path {
            if element == ".." {
                if entry_path.pop().is_none() {
                    return Err(Error::not_found());
                }
                continue;
            }

            if element == "." {
                continue;
            }

            let dir = entry_path.last().unwrap();
            let entry = dir.entries.get(element).ok_or_else(Error::not_found)?;

            match entry.data {
                NodeData::Directory(ref dir) => {
                    entry_path.push(dir);
                }
                NodeData::Regular(..) => {
                    return Ok(dir);
                }
            }
        }

        Ok(self)
    }

    pub fn stat(&self, path: &str) -> Result<Metadata, Error> {
        let mut path_iter = path.split('/');
        let last_dir = self.recurse_helper(&mut path_iter)?;

        let metadata = match path_iter.next() {
            Some(name) => match last_dir.entries.get(name) {
                Some(entry) => entry.metadata(),
                None => return Err(Error::not_found()),
            },
            None => {
                return Ok(Metadata {
                    node_kind: NodeKind::Directory,
                    last_modified_time: None,
                    length: 0,
                });
            }
        };

        if path_iter.next().is_some() {
            // trailing path elements detected.
            return Err(Error::not_found());
        }

        Ok(metadata)
    }

    pub fn list_dir<'dir>(&'dir self, path: &str) -> Result<Vec<(&'dir str, Metadata)>, Error> {
        let mut path_iter = path.split('/');
        let last_dir = self.recurse_helper(&mut path_iter)?;
        if path_iter.next().is_some() {
            // trailing path elements detected.
            return Err(Error::not_found());
        }

        let mut out: Vec<(&str, Metadata)> = Vec::new();
        for (k, v) in &last_dir.entries {
            out.push((k, v.metadata()));
        }

        Ok(out)
    }

    pub fn open_file(&self, path: &str) -> Result<io::Cursor<Arc<[u8]>>, Error> {
        let mut path_iter = path.split('/');
        let last_dir = self.recurse_helper(&mut path_iter)?;
        let filename = match path_iter.next() {
            Some(filename) => filename,
            None => return Err(Error::new(ErrorKind::Other, "not a file")),
        };
        if path_iter.next().is_some() {
            // trailing path elements detected.
            return Err(Error::not_found());
        }

        let last = last_dir
            .entries
            .get(filename)
            .ok_or_else(Error::not_found)?;

        match last.data {
            NodeData::Directory(..) => unreachable!(),
            NodeData::Regular(ref reg) => Ok(io::Cursor::new(Arc::clone(&reg.raw_data))),
        }
    }
}

#[allow(clippy::unreadable_literal)]
mod unix {
    pub const S_IFDIR: u32 = 0o0040000;
    pub const S_IFREG: u32 = 0o0100000;
}

struct ZipSmallVfs<R>
where
    R: Read + Seek,
{
    archive: ZipArchive<R>,
}

impl<R> ZipSmallVfs<R>
where
    R: Read + Seek,
{
    pub fn new(archive: ZipArchive<R>) -> ZipSmallVfs<R> {
        // validate all `last_modified_time` values
        // validate all files are compression_method == Stored
        // validate all files are unencrypted
        // validate all CRC32s
        // validate all files are uniquely named
        // validate all files have (or can simulate) UNIX metadata
        // validate all files are S_IFDIR *xor* S_IFREG
        ZipSmallVfs { archive }
    }
}

// pub struct ZipReader {
//     //
// }

// impl Read for ZipReader {
//     fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
//         unimplemented!();
//     }
// }

// impl Seek for ZipReader {
//     fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
//         unimplemented!();
//     }
// }

// fn deprefix<'a>(v: &'a str, pref: &'static str) -> Option<&'a str> {
//     if v.starts_with(pref) {
//         Some(&v[pref.len()..])
//     } else {
//         None
//     }
// }

// fn map_zip_err(z: ZipError) -> Error {
//     match z {
//         ZipError::Io(ioerr) => {
//             Error::new(ErrorKind::Other, format!("I/O error: {}", ioerr))
//         }
//         ZipError::InvalidArchive(info) => {
//             Error::new(ErrorKind::Other, format!("invalid zip: {}", info))
//         }
//         ZipError::UnsupportedArchive(info) => {
//             Error::new(ErrorKind::Other, format!("unsupported zip: {}", info))
//         }
//         ZipError::FileNotFound => Error::not_found(),
//     }
// }

// impl<R> SmallVfs for ZipSmallVfs<R> where R: Read + Seek {
//     type Reader = ZipReader;

//     fn stat(&self, path: &str) -> Result<Metadata, Error> {
//         let root_rel_path = match deprefix(path, "/") {
//             Some(v) => v,
//             None => return Err(Error::new(ErrorKind::Other, "absolute path required")),
//         };
//         let file = self.archive.by_name(root_rel_path).map_err(map_zip_err)?;
//         let meta = file.unix_mode().ok_or_else(|| {
//             Error::new(ErrorKind::Other, "Need unix metadata")
//         })?;

//         let is_directory = meta & unix::S_IFDIR > 0;
//         let is_regular = meta & unix::S_IFREG > 0;
//         let node_kind = match (is_directory, is_regular) {
//             (false, false) => return Err(Error::not_found()),
//             (false, true) => NodeType::Regular,
//             (true, false) => NodeType::Directory,
//             (true, true) => return Err(Error::new(ErrorKind::Other, "invalid metadata")),
//         };

//         Ok(Metadata {
//             node_kind,
//             last_modified_time: (),
//         })
//     }

//     fn list_dir(&self, path: &str) -> Result<Vec<Metadata>, Error> {
//         //
//     }

//     fn open_file(&self, path: &str) -> Result<Self::Reader, Error> {
//         //
//     }
// }

//         // ZipFileData {
//         //     system: Unix,
//         //     version_made_by: 30,
//         //     encrypted: false,
//         //     compression_method: Stored,
//         //     last_modified_time: DateTime {
//         //         year: 2019,
//         //         month: 2,
//         //         day: 26,
//         //         hour: 11,
//         //         minute: 59,
//         //         second: 48
//         //     },
//         //     crc32: 865444579,
//         //     compressed_size: 19295,
//         //     uncompressed_size: 19295,
//         //     file_name: "5-7278f4c2dcc1341d126e.js",
//         //     file_name_raw: [ ... ],
//         //     file_comment: "",
//         //     header_start: 0,
//         //     data_start: 0,
//         //     external_attributes: 2175008768
//         // }

//         // ZipFileData {
//         //     system: Unix,
//         //     version_made_by: 30,
//         //     encrypted: false,
//         //     compression_method: Stored,
//         //     last_modified_time: DateTime {
//         //         year: 2019,
//         //         month: 2,
//         //         day: 25,
//         //         hour: 17,
//         //         minute: 5,
//         //         second: 16
//         //     },
//         //     crc32: 0,
//         //     compressed_size: 0,
//         //     uncompressed_size: 0,
//         //     file_name: "static/",
//         //     file_name_raw: [
//         //         115,
//         //         116,
//         //         97,
//         //         116,
//         //         105,
//         //         99,
//         //         47
//         //     ],
//         //     file_comment: "",
//         //     header_start: 1684604,
//         //     data_start: 0,
//         //     external_attributes: 1106051088
//         // },
