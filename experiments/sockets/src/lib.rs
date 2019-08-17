use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};

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

pub struct Connected {
    inner: OwnedFd,
}

impl Connected {
    pub unsafe fn from_raw_fd(raw: RawFd) -> Self {
        Connected {
            inner: OwnedFd(raw),
        }
    }
}

enum ListenerInner {
    AcceptingFd(OwnedFd),
    Fixed(Vec<Connected>),
}

pub struct Incoming<RemoteAddr> {
    _temporary_marker: std::marker::PhantomData<RemoteAddr>,
}

pub struct TcpListener {
    inner: ListenerInner,
}

impl TcpListener {
    pub fn fixed(connected_list: Vec<Connected>) -> Self {
        TcpListener {
            inner: ListenerInner::Fixed(connected_list),
        }
    }

    pub unsafe fn from_raw_fd(raw: RawFd) -> Self {
        TcpListener {
            inner: ListenerInner::AcceptingFd(OwnedFd(raw)),
        }
    }
}

// #[derive(Serialize, Deserialize, Clone, Debug)]
// #[serde(rename_all = "snake_case")]
// pub struct UnixDomainBinder {
//     pub path: PathBuf,
//     #[serde(default="start_listen_default")]
//     pub start_listen: bool,
//     #[serde(default="Default::default")]
//     pub flags: Vec<SocketFlag>,
// }
// pub fn bind_unix_socket(ub: &UnixDomainBinder) -> io::Result<OwnedFd> {
//     let fd: OwnedFd = nix_socket(
//         AddressFamily::Unix,
//         SockType::Stream,
//         SockFlag::empty(),
//         None,
//     )
//     .map(|f| unsafe { FromRawFd::from_raw_fd(f) })
//     .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

//     let addr = UnixAddr::new(&ub.path).unwrap();
//     if let Err(err) = bind(fd.as_raw_fd(), &SockAddr::Unix(addr)) {
//         if err == nix::Error::Sys(nix::errno::Errno::EADDRINUSE) {
//             unlink(&ub.path).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
//         }
//         bind(fd.as_raw_fd(), &SockAddr::Unix(addr))
//             .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
//     }

//     if ub.start_listen {
//         // 128 from rust stdlib
//         ::nix::sys::socket::listen(fd.as_raw_fd(), 128)
//             .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
//     }
//     fchmodat(
//         None,
//         &ub.path,
//         Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
//         FchmodatFlags::FollowSymlink,
//     )
//     .map_err(|e| {
//         let msg = format!("fchmodat of {}: {}", ub.path.display(), e);
//         io::Error::new(io::ErrorKind::Other, msg)
//     })?;

//     Ok(fd)
// }
// use tokio_reactor::{Handle, PollEvented};

// pub struct RawConnected {
//     io: PollEvented<OwnedFd>,
// }

// impl AsyncRead for RawConnected {
//     fn poll_read(self: Pin<&mut Self>, ctx: &mut Context, data: &mut [u8]) -> Poll<Result<usize, io::Error>>
//     {
//         unimplemented!();
//     }

//     fn poll_read_vectored(self: Pin<&mut Self>,
//     cx: &mut Context,
//     vec: &mut [IoSliceMut]) -> Poll<Result<usize, io::Error>> {
//         unimplemented!();
//     }
// }

// impl AsyncWrite for RawConnected {
//     fn poll_write(self: Pin<&mut Self>, ctx: &mut Context, data: &[u8]) -> Poll<Result<usize, io::Error>>
//     {
//         unimplemented!();
//     }

//     fn poll_flush(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Result<(), io::Error>>
//     {
//         unimplemented!();
//     }

//     fn poll_close(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Result<(), io::Error>>
//     {
//         unimplemented!();
//     }
// }

// pub trait RawListener: Sized {
//     type RemoteAddr;

//     fn incoming(self) -> Incoming<Self::RemoteAddr> {
//         unimplemented!();
//     }
// }

// use std::pin::Pin;
// use std::task::Context;

// impl<S> futures::stream::Stream for Incoming<S> {
//     type Item = Result<(RawConnected, S), ()>;

//     fn poll_next(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>>
//     {
//         unimplemented!();
//     }
// }

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
