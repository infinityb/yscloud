use std::io;
use std::fs::File;
use std::net::{TcpStream};
use std::os::unix::io::{AsRawFd, IntoRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixStream};

use nix::sys::socket::{AddressFamily, SockType, SockFlag, socketpair};


pub struct OwnedFd(RawFd);

impl FromRawFd for OwnedFd {
    unsafe fn from_raw_fd(raw: RawFd) -> OwnedFd {
        OwnedFd(raw)
    }
}

impl AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.0);
    }
}

impl From<File> for OwnedFd {
    fn from(fd: File) -> OwnedFd {
        OwnedFd(fd.into_raw_fd())
    }
}

enum ConnectedInner {
    Tcp(TcpStream),
    Unix(UnixStream),
}

pub struct Connected {
    inner: OwnedFd,
}

enum ListenerInner {
    AcceptingFd(OwnedFd),
    Fixed(Vec<Connected>),
}

pub struct Listener {
    inner: ListenerInner,
}

impl Connected {
    pub unsafe fn from_raw_fd(raw: RawFd) -> Self {
        Connected { inner: OwnedFd(raw) }
    }
}

impl Listener {
    pub fn fixed(connected_list: Vec<Connected>) -> Self {
        Listener {
            inner: ListenerInner::Fixed(connected_list),
        }
    }

    pub unsafe fn from_raw_fd(raw: RawFd) -> Self {
        Listener {
            inner: ListenerInner::AcceptingFd(OwnedFd(raw)),
        }
    }
}


#[cfg(feature = "tokio")]
impl AsyncRead for Connected {}

#[cfg(feature = "tokio")]
impl AsyncWrite for Connected {}

#[cfg(feature = "tokio")]
impl Stream for Listener {}


pub fn socketpair_raw() -> io::Result<(OwnedFd, OwnedFd)> {
    let (left, right) = socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::empty(),
    )
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    Ok((OwnedFd(left), OwnedFd(right)))
}