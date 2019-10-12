use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use nix::unistd::{read, write};

#[cfg(target_os = "macos")]
use nix::unistd::lseek as lseek64;

#[cfg(target_os = "linux")]
use nix::unistd::lseek64 as lseek64;

#[derive(Debug)]
pub struct OwnedFd {
    raw_fd: RawFd,
}

impl OwnedFd {
    pub fn set_capacity(&mut self, size: i64) -> Result<(), nix::Error> {
        nix::unistd::ftruncate(self.raw_fd, size)
    }

    pub fn into_raw_fd(self) -> RawFd {
        let fd_no = self.raw_fd;
        ::std::mem::forget(self);
        fd_no
    }

    pub fn into_file(self) -> File {
        unsafe { FromRawFd::from_raw_fd(self.into_raw_fd()) }
    }
}

impl FromRawFd for OwnedFd {
    unsafe fn from_raw_fd(fd: RawFd) -> OwnedFd {
        OwnedFd { raw_fd: fd }
    }
}

impl From<File> for OwnedFd {
    fn from(fd: File) -> OwnedFd {
        OwnedFd { raw_fd: fd.into_raw_fd() }
    }
}

impl AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.raw_fd);
    }
}

impl Read for OwnedFd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read(self.raw_fd, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

impl Write for OwnedFd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write(self.raw_fd, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
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
        lseek64(self.raw_fd, offset, whence)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
            .map(|v| {
                if v < 0 {
                    panic!();
                }
                v as u64
            })
    }
}