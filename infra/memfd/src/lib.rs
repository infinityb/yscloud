#![cfg(target_os = "linux")]

extern crate nix;

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;

use nix::fcntl::FcntlArg;
pub use nix::fcntl::SealFlag;
use nix::sys::memfd::memfd_create;
pub use nix::sys::memfd::MemFdCreateFlag;
use nix::unistd::{lseek64, read, write};

#[derive(Debug)]
pub struct OwnedFd(RawFd);

impl OwnedFd {
    pub fn into_raw_fd(self) -> RawFd {
        let fd_no = self.0;
        ::std::mem::forget(self);
        fd_no
    }

    pub fn into_file(self) -> File {
        use std::os::unix::io::FromRawFd;
        unsafe { FromRawFd::from_raw_fd(self.into_raw_fd()) }
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.0);
    }
}

impl Read for OwnedFd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read(self.0, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

impl Write for OwnedFd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write(self.0, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for OwnedFd {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        use nix::unistd::Whence;

        let offset;
        let whence;
        match pos {
            SeekFrom::Start(off) => {
                if off > i64::max_value() as u64 {
                    panic!();
                }
                offset = off as i64;
                whence = Whence::SeekSet;
            }
            SeekFrom::End(off) => {
                offset = off;
                whence = Whence::SeekEnd;
            }
            SeekFrom::Current(off) => {
                offset = off;
                whence = Whence::SeekCur;
            }
        }
        lseek64(self.0, offset, whence)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
            .map(|v| {
                if v < 0 {
                    panic!();
                }
                v as u64
            })
    }
}

#[derive(Debug)]
pub struct MemFd(OwnedFd);

impl MemFd {
    pub fn new<P: AsRef<OsStr> + ?Sized>(name: &P) -> Result<MemFd, nix::Error> {
        MemFdOptions::new().open(name)
    }

    pub fn set_capacity(&mut self, size: i64) -> Result<(), nix::Error> {
        nix::unistd::ftruncate(self.as_raw_fd(), size)
    }

    pub fn as_raw_fd(&self) -> RawFd {
        (self.0).0
    }

    pub fn into_owned_fd(self) -> OwnedFd {
        self.0
    }

    pub fn seal(&mut self, flags: SealFlag) -> Result<i32, nix::Error> {
        // pub fn fcntl(fd: RawFd, arg: FcntlArg) -> Result<c_int>
        ::nix::fcntl::fcntl(self.as_raw_fd(), FcntlArg::F_ADD_SEALS(flags))
    }

    pub fn get_seals(&self) -> Result<SealFlag, nix::Error> {
        ::nix::fcntl::fcntl(self.as_raw_fd(), FcntlArg::F_GET_SEALS)
            .map(|b| SealFlag::from_bits(b).unwrap())
    }
}

impl Read for MemFd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for MemFd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for MemFd {
    fn seek(&mut self, s: SeekFrom) -> io::Result<u64> {
        self.0.seek(s)
    }
}

pub struct MemFdOptions {
    flags: MemFdCreateFlag,
    capacity: Option<i64>,
}

impl MemFdOptions {
    pub fn new() -> MemFdOptions {
        MemFdOptions {
            flags: MemFdCreateFlag::empty(),
            capacity: None,
        }
    }

    pub fn cloexec(&mut self, val: bool) -> &mut MemFdOptions {
        if val {
            self.flags.insert(MemFdCreateFlag::MFD_CLOEXEC);
        } else {
            self.flags.remove(MemFdCreateFlag::MFD_CLOEXEC);
        }
        self
    }

    pub fn allow_sealing(&mut self, val: bool) -> &mut MemFdOptions {
        if val {
            self.flags.insert(MemFdCreateFlag::MFD_ALLOW_SEALING);
        } else {
            self.flags.remove(MemFdCreateFlag::MFD_ALLOW_SEALING);
        }
        self
    }

    pub fn with_capacity(&mut self, capacity: i64) -> &mut MemFdOptions {
        self.capacity = Some(capacity);
        self
    }

    pub fn open<P: AsRef<OsStr> + ?Sized>(&self, name: &P) -> Result<MemFd, nix::Error> {
        // OsStr's probably don't have nulls in them... panic if they do.
        let name = CString::new(name.as_ref().as_bytes()).expect("null byte in path");
        let mut memfd = MemFd(OwnedFd(try!(memfd_create(&name, self.flags))));

        if let Some(capacity) = self.capacity {
            try!(memfd.set_capacity(capacity));
        }

        Ok(memfd)
    }
}

#[cfg(test)]
mod tests {
    use nix::fcntl::SealFlag::{F_SEAL_GROW, F_SEAL_SEAL, F_SEAL_SHRINK};

    use super::{MemFd, MemFdOptions};

    #[test]
    fn test_open() {
        for _ in 0..4096 {
            drop(MemFd::new("foobar").unwrap());
        }
    }

    #[test]
    fn test_size_limited_file_api() {
        use std::io::Write;

        let mut memfile = MemFdOptions::new()
            .allow_sealing(true)
            .with_capacity(32 * 1024)
            .open("ayylmaxo")
            .unwrap();

        memfile
            .seal(F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW)
            .unwrap();

        let mut file = big.into_owned_fd().into_file();

        let mut buf = [0; 256];
        for (o, i) in buf.iter_mut().zip(0..) {
            *o = i;
        }

        let mut wrote = 0;
        while wrote < (32 << 10) {
            wrote += file.write(&buf[..]).unwrap();
            println!("wrote {}", wrote);
        }

        match file.write(&buf[..]) {
            Ok(_) => panic!("wrote more than allowed!"),
            Err(ref err) => {
                // XXX: should be EPERM - panic on other errors
            }
        }
    }
}
