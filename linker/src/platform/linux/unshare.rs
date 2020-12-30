use std::fs::File;
use std::borrow::Cow;
use std::io::{self, BufReader, BufRead, Read};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::collections::HashSet;

use super::mount::unmount_filesystems;

use nix::mount::{umount, MsFlags};
use nix::sched::{unshare, CloneFlags};

fn nix_error_io(e: nix::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("{}", e))
}

pub fn restrict_filesystem() -> io::Result<()> {
    let mut ignore = HashSet::new();
    ignore.insert(PathBuf::from("/"));

    // unshare mount ns
    unshare(CloneFlags::CLONE_NEWNS)
        .map_err(nix_error_io)?;

    // this separates us from existing mountpoints
    let mount_null_str: Option<&'static str> = None;
    if let Err(err) = nix::mount::mount(
        Some("none"),
        "/",
        mount_null_str,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        mount_null_str)
    {
        eprintln!("error doing rprivate mount: {}", err);
        return Err(nix_error_io(err));
    }

    unmount_filesystems(&|p| !ignore.contains(p))
}

pub fn restrict_network() -> io::Result<()> {
    //unshare network ns

    unshare(CloneFlags::CLONE_NEWNET)
        .map_err(nix_error_io)?;

    Ok(())
}