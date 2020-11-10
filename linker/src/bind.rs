use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::unix::io::{AsRawFd, FromRawFd};

use nix::sys::socket::{
    bind, setsockopt, socket as nix_socket, sockopt::ReusePort, AddressFamily, InetAddr, SockAddr,
    SockFlag, SockProtocol, SockType, UnixAddr,
};
use nix::sys::stat::{fchmodat, FchmodatFlags, Mode};
use nix::unistd::unlink;

use owned_fd::OwnedFd;
use yscloud_config_model::{NativePortBinder, UnixDomainBinder};

pub fn bind_tcp_socket(np: &NativePortBinder) -> io::Result<OwnedFd> {
    let fd: OwnedFd = nix_socket(
        AddressFamily::Inet6,
        SockType::Stream,
        SockFlag::empty(),
        SockProtocol::Tcp,
    )
    .map(|f| unsafe { FromRawFd::from_raw_fd(f) })
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let ip_addr = np
        .bind_address
        .parse()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let saddr = match ip_addr {
        IpAddr::V4(a) => SocketAddr::V4(SocketAddrV4::new(a, np.port)),
        IpAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(a, np.port, 0, 0)),
    };

    setsockopt(fd.as_raw_fd(), ReusePort, &true)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    bind(fd.as_raw_fd(), &SockAddr::Inet(InetAddr::from_std(&saddr)))
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    if np.start_listen {
        // 128 from rust stdlib
        ::nix::sys::socket::listen(fd.as_raw_fd(), 128)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    Ok(fd)
}

pub fn bind_unix_socket(ub: &UnixDomainBinder) -> io::Result<OwnedFd> {
    let fd: OwnedFd = nix_socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )
    .map(|f| unsafe { FromRawFd::from_raw_fd(f) })
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let addr = UnixAddr::new(&ub.path).unwrap();
    if let Err(err) = bind(fd.as_raw_fd(), &SockAddr::Unix(addr)) {
        if err == nix::Error::Sys(nix::errno::Errno::EADDRINUSE) {
            unlink(&ub.path).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        }
        bind(fd.as_raw_fd(), &SockAddr::Unix(addr))
            .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{}: {}", ub.path.display(), err)))?;
    }

    if ub.start_listen {
        // 128 from rust stdlib
        ::nix::sys::socket::listen(fd.as_raw_fd(), 128)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    fchmodat(
        None,
        &ub.path,
        Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
        FchmodatFlags::FollowSymlink,
    )
    .map_err(|e| {
        let msg = format!("fchmodat of {}: {}", ub.path.display(), e);
        io::Error::new(io::ErrorKind::Other, msg)
    })?;

    Ok(fd)
}
