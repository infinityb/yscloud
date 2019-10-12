#![cfg(target_os = "linux")]

use std::ffi::{CString, OsStr};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{RawFd, FromRawFd, AsRawFd};

pub use nix::fcntl::SealFlag;
pub use nix::sys::memfd::MemFdCreateFlag;
use nix::fcntl::FcntlArg;
pub use nix::sys::stat::Mode;
use nix::sys::memfd::memfd_create;
use nix::sys::stat::fchmod;

use owned_fd::OwnedFd;


#[derive(Debug)]
pub struct MemFd {
    owned_fd: OwnedFd,
}

impl MemFd {
    pub fn new<P: AsRef<OsStr> + ?Sized>(name: &P) -> Result<MemFd, nix::Error> {
        MemFdOptions::new().open(name)
    }

    pub fn set_capacity(&mut self, size: i64) -> Result<(), nix::Error> {
        self.owned_fd.set_capacity(size)
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.owned_fd.as_raw_fd()
    }

    pub fn into_owned_fd(self) -> OwnedFd {
        self.owned_fd
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
        self.owned_fd.read(buf)
    }
}

impl Write for MemFd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.owned_fd.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.owned_fd.flush()
    }
}

impl Seek for MemFd {
    fn seek(&mut self, s: SeekFrom) -> io::Result<u64> {
        self.owned_fd.seek(s)
    }
}

pub struct MemFdOptions {
    flags: MemFdCreateFlag,
    mode: Option<Mode>,
    capacity: Option<i64>,
}

impl MemFdOptions {
    pub fn new() -> MemFdOptions {
        MemFdOptions {
            flags: MemFdCreateFlag::empty(),
            mode: None,
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

    pub fn unset_mode(&mut self) -> &mut MemFdOptions {
        self.mode = None;
        self
    }

    pub fn set_mode(&mut self, val: Mode) -> &mut MemFdOptions {
        self.mode = Some(val);
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

        let raw_fd = memfd_create(&name, self.flags)?;
        let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

        if let Some(mode) = self.mode {
            fchmod(owned_fd.as_raw_fd(), mode)?;
        }

        let mut memfd = MemFd { owned_fd };
        if let Some(capacity) = self.capacity {
            memfd.set_capacity(capacity)?;
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
