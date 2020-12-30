use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::unistd::pivot_root;
use nix::mount::{umount, MsFlags, umount2, MntFlags};
use nix::sched::{unshare, CloneFlags};

use super::mount::unmount_filesystems;
use super::unshare::restrict_filesystem;

fn nix_error_io(e: nix::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("{}", e))
}

pub struct Config {
    pub persistence_path: PathBuf,
    pub code_archive_path: PathBuf,
    // if zero, ephemeral storage is disabled.
    pub ephemeral_storage_kilobytes: u64,
    pub enable_proc: bool,
    pub enable_dev: bool,
}

pub fn mount_nix_squashfs(workdir: &Path, config: &Config) -> Result<(), failure::Error> {
    let mount_null_str: Option<&'static str> = None;

    let mut root_target: PathBuf = workdir.into();
    root_target.push("rootfs");
    
    let mut old_root_target: PathBuf = root_target.clone();
    old_root_target.push(".old");

    let mut nix_target: PathBuf = root_target.clone();
    nix_target.push("nix");

    let mut persist_target: PathBuf = root_target.clone();
    persist_target.push("persist");

    let mut ephemeral_target: PathBuf = root_target.clone();
    ephemeral_target.push("run");

    let mut proc_target: PathBuf = root_target.clone();
    proc_target.push("proc");

    let mut dev_target: PathBuf = root_target.clone();
    dev_target.push("dev");

    std::fs::create_dir_all(&root_target)?;

    // unshare mount ns.  If we fail at any point, all the mounts will be
    // unmounted when this fork of the program terminates, so error handling
    // does not need to be complicated here.
    unshare(CloneFlags::CLONE_NEWNS
        | CloneFlags::CLONE_NEWPID
        | CloneFlags::CLONE_NEWIPC)?;

    nix::mount::mount(
        Some("none"),
        "/",
        mount_null_str,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        mount_null_str
    )?;

    nix::mount::mount(
        Some("tmpfs"),
        &root_target,
        Some("tmpfs"),
        MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
        Some("size=320k"), // half of 640k, not enough for everybody.
    )?;

    std::fs::create_dir_all(&old_root_target)?;
    std::fs::create_dir_all(&nix_target)?;
    std::fs::create_dir_all(&persist_target)?;

    if config.ephemeral_storage_kilobytes > 0 {
        std::fs::create_dir_all(&ephemeral_target)?;
    }

    if config.enable_proc {
        std::fs::create_dir_all(&proc_target)?;
    }

    if config.enable_dev {
        std::fs::create_dir_all(&dev_target)?;
    }

    nix::mount::mount(
        Some(&config.code_archive_path),
        &nix_target,
        Some("squashfs"),
        MsFlags::empty(),
        mount_null_str,
    )?;

    nix::mount::mount(
        Some(&config.persistence_path),
        &persist_target,
        mount_null_str,
        MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_NOSUID | MsFlags::MS_BIND,
        mount_null_str,
    )?;

    if config.ephemeral_storage_kilobytes > 0 {
        let mount_options = format!("size={}k", config.ephemeral_storage_kilobytes);
        nix::mount::mount(
            Some("tmpfs"),
            &ephemeral_target,
            Some("tmpfs"),
            MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
            Some(&mount_options[..]),
        )?;

    }

    if config.enable_proc {
        nix::mount::mount(
            Some("proc"),
            &proc_target,
            Some("proc"),
            MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
            mount_null_str,
        )?;
    }

    if config.enable_dev {
        nix::mount::mount(
            Some("udev"),
            &dev_target,
            Some("devtmpfs"),
            MsFlags::MS_NOSUID,
            mount_null_str,
        )?;
    }

    pivot_root(&root_target, &old_root_target)?;
    umount2("/.old", MntFlags::MNT_DETACH)?;
    if let Err(err) = std::fs::remove_dir("/.old") {
        eprintln!("failed to unmount old-root, continuing anyway");
    }

    nix::mount::mount(
        mount_null_str,
        "/",
        mount_null_str,
        MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_NOEXEC
            | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
        mount_null_str,
    )?;

    unshare(CloneFlags::CLONE_NEWUTS)?;

    Ok(())
}