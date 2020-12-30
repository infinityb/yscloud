use std::fs::File;
use std::borrow::Cow;
use std::io::{self, BufReader, BufRead, Read};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::collections::HashSet;

use nix::mount::{umount2, MntFlags};
use nix::sched::{unshare, CloneFlags};

pub fn unmount_filesystems(selector: &dyn Fn(&Path) -> bool) -> io::Result<()> {
    fn mount_point<'a>(source: &'a [u8]) -> Option<Cow<'a, Path>> {
        let mut parts = source.split(|x| *x == b' ');
        parts.next()?;
        
        let mount_point = parts.next()?;
        let mount_point = OsStr::from_bytes(mount_point);
        Some(Cow::Borrowed(Path::new(mount_point)))
    }

    let mut mounts = BufReader::new(File::open("/proc/mounts")?);
    let mut mount_targets: VecDeque<PathBuf> = VecDeque::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        mounts.read_until(0x0A, &mut buf)?;
        if buf.len() == 0 {
            break;
        }

        if let Some(vv) = mount_point(&buf) {
            if !selector(&*vv) {
                continue;
            }
            mount_targets.push_back(PathBuf::from(vv));
        }
    }

    let mut unmount_limit = mount_targets.len() * 10;

    while 0 < unmount_limit && !mount_targets.is_empty() {
        eprintln!("unmount_limit={:?}, remaining={:?}", unmount_limit, mount_targets.len());
        unmount_limit -= 1;
        let target = mount_targets.pop_front().unwrap();
        if let Err(err) = umount2(&target, MntFlags::MNT_DETACH) {
            // eprintln!("umount {:?} -> error was {}", target.display(), err);
            mount_targets.push_back(target);
        } else {
            eprintln!("unmounted: {:?}", target.display());
        }
    }

    if !mount_targets.is_empty() {
        eprintln!("remaining mounts: {:?}", mount_targets);
    }

    Ok(())
}
