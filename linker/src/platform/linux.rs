use std::error::Error as StdError;
use std::ffi::CString;
use std::fmt;
use std::io;
use std::os::unix::io::FromRawFd;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use log::{info, warn};
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::{fexecve, fork, lseek64, write, ForkResult, Gid, Pid, Uid, Whence};
use users::{get_group_by_name, get_user_by_name};

use super::super::OwnedFd;
use super::posix_imp::relabel_file_descriptors;
use crate::AppPreforkConfiguration;

pub struct Executable(OwnedFd);

impl fmt::Display for Executable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "fd:{}", self.0.as_raw_fd())
    }
}

impl AsRawFd for Executable {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl Executable {
    pub fn open<P>(path: P) -> io::Result<Executable>
    where
        P: AsRef<Path>,
    {
        let path: &Path = path.as_ref();
        let artifact_file = open(path, OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty())
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

        Ok(Executable(unsafe { OwnedFd::from_raw_fd(artifact_file) }))
    }

    pub fn execute(&self, arguments: &[CString], env: &[CString]) -> io::Result<!> {
        fexecve(self.0.as_raw_fd(), arguments, env)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // successful executions of fexecve don't return.
        unreachable!();
    }
}

pub const EXTENSION: &str = "";

#[cfg(target_arch = "x86_64")]
pub const PLATFORM_TRIPLES: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "x86_64-unknown-linux",
];

pub fn keep_hook(c: &AppPreforkConfiguration, keep_map: &mut [bool]) {
    keep_map[c.artifact.0.as_raw_fd() as usize] = true;
}

pub trait SandboxingStrategy {
    fn preexec(&self) -> io::Result<()>;
}

impl SandboxingStrategy for () {
    fn preexec(&self) -> io::Result<()> {
        Ok(())
    }
}

pub struct UserChangeStrategy {
    workdir: Option<PathBuf>,
    set_user: Option<Uid>,
    set_group: Option<Gid>,
}

fn io_other<E>(e: E) -> io::Error
where
    E: Into<Box<dyn StdError + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::Other, e)
}

impl SandboxingStrategy for UserChangeStrategy {
    fn preexec(&self) -> io::Result<()> {
        if let Some(gid) = self.set_group {
            info!("setting gid = {:?}", gid);
            nix::unistd::setgid(gid).map_err(io_other)?;
        }
        if let Some(uid) = self.set_user {
            info!("setting uid = {:?}", uid);
            nix::unistd::setuid(uid).map_err(io_other)?;
        }
        if let Some(ref wd) = self.workdir {
            info!("setting cwd = {}", wd.display());
            nix::unistd::chdir(wd).map_err(io_other)?;
        }

        Ok(())
    }
}

pub struct ExecExtras {
    sandboxing_strategy: Option<Arc<dyn SandboxingStrategy>>,
}

impl ExecExtras {
    pub fn builder() -> ExecExtrasBuilder {
        Default::default()
    }
}

#[derive(Default)]
pub struct ExecExtrasBuilder {
    workdir: Option<PathBuf>,
    set_user: Option<Uid>,
    set_group: Option<Gid>,
}

impl ExecExtrasBuilder {
    pub fn set_user(&mut self, name: &str) -> io::Result<()> {
        let uid = get_user_by_name(name).ok_or_else(|| {
            let msg = format!("unknown user {}", name);
            io::Error::new(io::ErrorKind::Other, msg)
        })?;

        self.set_user = Some(Uid::from_raw(uid.uid()));
        Ok(())
    }

    pub fn set_group(&mut self, name: &str) -> io::Result<()> {
        let gid = get_group_by_name(name).ok_or_else(|| {
            let msg = format!("unknown group {}", name);
            io::Error::new(io::ErrorKind::Other, msg)
        })?;

        self.set_group = Some(Gid::from_raw(gid.gid()));
        Ok(())
    }

    pub fn set_workdir(&mut self, workdir: &Path) -> io::Result<()> {
        self.workdir = Some(workdir.to_owned());
        Ok(())
    }

    pub fn build(&self) -> ExecExtras {
        let mut sandboxing_strategy = None;

        if self.set_user.is_some() || self.set_group.is_some() {
            let obj: Box<dyn SandboxingStrategy> = Box::new(UserChangeStrategy {
                workdir: self.workdir.clone(),
                set_user: self.set_user.clone(),
                set_group: self.set_group.clone(),
            });

            sandboxing_strategy = Some(obj.into());
        }

        ExecExtras {
            sandboxing_strategy,
        }
    }
}

fn exec_artifact_child(ext: &ExecExtras, c: &AppPreforkConfiguration) -> io::Result<!> {
    let package_id = c.package_id.clone();
    let app_config = relabel_file_descriptors(&c)?;
    let tmpfile = open(
        "/tmp",
        OFlag::O_RDWR | OFlag::O_TMPFILE,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(io_other)?;

    let data = serde_json::to_string(&app_config)?;
    let data_len =
        write(tmpfile, data.as_bytes()).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    lseek64(tmpfile, 0, Whence::SeekSet)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    assert_eq!(data_len, data.len());
    let arguments = &[
        CString::new("yscloud-executable").unwrap(),
        CString::new("--config-fd").unwrap(),
        CString::new(format!("{}", tmpfile)).unwrap(),
    ];
    info!("running {} {:?} -- {}", package_id, arguments, data);

    if let Some(ref sandbox) = ext.sandboxing_strategy {
        sandbox.preexec()?;
    }
    let env = &[
        CString::new("RUST_BACKTRACE=1").unwrap(),
        CString::new("YSCLOUD=1").unwrap(),
    ];
    c.artifact.execute(arguments, env)?;

    unreachable!();
}

pub fn exec_artifact(e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Pid> {
    match fork() {
        Ok(ForkResult::Child) => {
            if let Err(err) = exec_artifact_child(e, &c) {
                warn!("failed to execute: {:?}", err);
                std::process::exit(1);
            } else {
                unreachable!();
            }
        }
        Ok(ForkResult::Parent { child, .. }) => Ok(child),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err)),
    }
}
