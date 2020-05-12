use std::error::Error as StdError;
use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use digest::FixedOutput;
use nix::fcntl::{fcntl, open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::{execveat, fork, lseek64, write, ForkResult, Gid, Pid, Uid, Whence};
use sha2::Sha256;
use tracing::{event, Level};
use users::{get_group_by_name, get_user_by_name};

use super::posix_imp::relabel_file_descriptors;
pub use super::posix_imp::run_reified;
use crate::AppPreforkConfiguration;
use crate::Void;

use memfd::{MemFd, MemFdOptions, SealFlag};
use owned_fd::OwnedFd;

#[derive(Debug)]
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

fn nix_error_to_io_error(err: nix::Error) -> io::Error {
    match err {
        nix::Error::Sys(syserr) => io::Error::from_raw_os_error(syserr as i32),
        nix::Error::InvalidPath => io::Error::new(io::ErrorKind::Other, "Invalid path (nix)"),
        nix::Error::InvalidUtf8 => io::Error::new(io::ErrorKind::Other, "Invalid UTF-8 (nix)"),
        nix::Error::UnsupportedOperation => {
            io::Error::new(io::ErrorKind::Other, "Unsupported operation (nix)")
        }
    }
}

pub struct ExecutableFactory {
    mem_fd: MemFd,
    sha_state: Sha256,
}

impl ExecutableFactory {
    pub fn new(name: &str, capacity: i64) -> io::Result<ExecutableFactory> {
        let mut mem_fd = MemFdOptions::new()
            .cloexec(true)
            .allow_sealing(true)
            .with_capacity(capacity)
            .set_mode(Mode::S_IRWXU | Mode::S_IRGRP | Mode::S_IXGRP | Mode::S_IROTH | Mode::S_IXOTH)
            .open(name)
            .map_err(|e| nix_error_to_io_error(e))?;

        mem_fd
            .seal(SealFlag::F_SEAL_SEAL | SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_GROW)
            .map_err(|e| nix_error_to_io_error(e))?;

        Ok(ExecutableFactory {
            mem_fd,
            sha_state: Default::default(),
        })
    }

    pub fn validate_sha(&self, expect_sha: &str) -> io::Result<()> {
        let sha_result = self.sha_state.clone().fixed_result();
        let mut scratch = [0; 256 / 8 * 2];

        let got_sha = crate::util::hexify(&mut scratch[..], &sha_result[..]).unwrap();

        event!(
            Level::DEBUG,
            "checking sha: expecting: {}, got: {}",
            expect_sha,
            got_sha
        );
        if expect_sha != got_sha {
            let msg = format!("sha mismatch {} != {}", expect_sha, got_sha);
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }

        Ok(())
    }

    pub fn finalize(self) -> Executable {
        Executable(self.mem_fd.into_owned_fd())
    }
}

impl io::Write for ExecutableFactory {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let written = write(self.mem_fd.as_raw_fd(), data).map_err(nix_error_to_io_error)?;

        let sha_written = self.sha_state.write(&data[..written])?;
        assert_eq!(sha_written, written);

        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        // we always immediately write to the backing storage, so we can leave this empty.
        Ok(())
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

    pub fn execute(&self, arguments: &[&CStr], env: &[&CStr]) -> io::Result<Void> {
        use nix::fcntl::{AtFlags, FcntlArg, FdFlag};

        fcntl(self.0.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let program_name = CString::new("").unwrap();
        execveat(
            self.0.as_raw_fd(),
            &program_name,
            arguments,
            env,
            AtFlags::AT_EMPTY_PATH,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // successful invokations of execveat don't return.
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
        if let Some(ref wd) = self.workdir {
            std::fs::create_dir_all(wd)?;

            nix::unistd::chown(wd, self.set_user, self.set_group)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        }

        if let Some(gid) = self.set_group {
            event!(Level::INFO, "setting gid = {:?}", gid);
            nix::unistd::setgid(gid).map_err(io_other)?;
        }
        if let Some(uid) = self.set_user {
            event!(Level::INFO, "setting uid = {:?}", uid);
            nix::unistd::setuid(uid).map_err(io_other)?;
        }
        if let Some(ref wd) = self.workdir {
            event!(Level::INFO, "setting cwd = {}", wd.display());
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

fn exec_artifact_child(ext: &ExecExtras, c: &AppPreforkConfiguration) -> io::Result<Void> {
    let package_id = c.package_id.clone();
    let app_config = relabel_file_descriptors(&c)?;
    let tmpfile = open(
        "/tmp",
        OFlag::O_RDWR | OFlag::O_TMPFILE,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(|e| {
        event!(Level::WARN, "error opening temporary: {:?}", e);
        io_other(e)
    })?;

    let data = serde_json::to_string(&app_config)?;
    let data_len =
        write(tmpfile, data.as_bytes()).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    lseek64(tmpfile, 0, Whence::SeekSet)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    assert_eq!(data_len, data.len());
    let tmpfile = format!("{}\0", tmpfile);
    let arguments: &[&CStr] = &[
        CStr::from_bytes_with_nul(b"yscloud-executable\0").unwrap(),
        CStr::from_bytes_with_nul(b"--config-fd\0").unwrap(),
        CStr::from_bytes_with_nul(tmpfile.as_bytes()).unwrap(),
    ];
    event!(
        Level::INFO,
        "running {} {:?} -- {}",
        package_id,
        arguments,
        data
    );

    if let Some(ref sandbox) = ext.sandboxing_strategy {
        sandbox.preexec()?;
    }
    let env: &[&CStr] = &[
        CStr::from_bytes_with_nul(b"RUST_BACKTRACE=1\0").unwrap(),
        CStr::from_bytes_with_nul(b"YSCLOUD=1\0").unwrap(),
    ];
    c.artifact.execute(arguments, env)?;

    unreachable!();
}

pub fn exec_artifact(e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Pid> {
    match fork() {
        Ok(ForkResult::Child) => {
            if let Err(err) = exec_artifact_child(e, &c) {
                event!(Level::WARN, "failed to execute: {:?}", err);
                std::process::exit(1);
            } else {
                unreachable!();
            }
        }
        Ok(ForkResult::Parent { child, .. }) => Ok(child),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err)),
    }
}
