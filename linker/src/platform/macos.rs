use std::ffi::CString;
use std::fmt;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use log::{info, log, warn};
use nix::unistd::{execve, fork, lseek, unlink, write, ForkResult, Pid, Whence};
use rand::{thread_rng, Rng};

use super::posix_imp::relabel_file_descriptors;
use crate::AppPreforkConfiguration;

pub struct Executable(PathBuf);

impl fmt::Display for Executable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "file:{}", self.0.display())
    }
}

impl Executable {
    pub fn open<P>(path: P) -> io::Result<Executable>
    where
        P: AsRef<Path>,
    {
        let path: &Path = path.as_ref();
        if path.exists() {
            Ok(Executable(path.into()))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    pub fn execute(&self, arguments: &[CString]) -> io::Result<!> {
        let path_bytes = OsStrExt::as_bytes(self.0.as_os_str());
        let artifact_path = CString::new(path_bytes).expect("valid c-string");

        execve(&artifact_path, arguments, &[])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // successful executions of execve don't return.
        unreachable!();
    }
}

pub const EXTENSION: &str = "";

#[cfg(target_arch = "x86_64")]
pub const PLATFORM_TRIPLES: &[&str] = &["x86_64-apple-darwin"];

pub fn keep_hook(_: &AppPreforkConfiguration, _keep_map: &mut [bool]) {}

pub struct ExecConfig {
    executable: super::Executable,
    arguments: Vec<CString>,
    /* extra_files: Vec<OwnedFd>, */
}

fn execute_child(e: &ExecConfig) -> io::Result<!> {
    // need to relabel file descriptors here.
    e.executable.execute(&e.arguments)?;
}

pub struct ExecExtras {
    _fields: (),
}

impl ExecExtras {
    pub fn builder() -> ExecExtrasBuilder {
        Default::default()
    }
}

#[derive(Default)]
pub struct ExecExtrasBuilder {
    _fields: (),
}

impl ExecExtrasBuilder {
    pub fn set_user(&mut self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn set_group(&mut self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn set_workdir(&mut self, _: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn build(&self) -> ExecExtras {
        ExecExtras { _fields: () }
    }
}

// let artifact_path = ((c.artifact.0).0).clone();
// let path_bytes = OsStrExt::as_bytes(artifact_path.as_os_str());
// let artifact_path = CString::new(path_bytes).expect("valid c-string");

fn exec_artifact_child(_e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<!> {
    use nix::fcntl::open;
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    let package_id = c.package_id.clone();

    let app_config = relabel_file_descriptors(&c)?;

    let path = format!("/tmp/yscloud-{}-{}", c.instance_id, thread_rng().gen::<u64>());
    let tmpfile = open(
        &path as &str,
        OFlag::O_CREAT | OFlag::O_RDWR,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    if let Err(err) = unlink(&path as &str) {
        warn!("failed to unlink temporary file {}: {}", path, err);
    }

    let data = serde_json::to_string(&app_config)?;
    let data_len =
        write(tmpfile, data.as_bytes()).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    lseek(tmpfile, 0, Whence::SeekSet).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    assert_eq!(data_len, data.len());
    let arguments = vec![
        CString::new("yscloud-executable").unwrap(),
        CString::new("--config-fd").unwrap(),
        CString::new(format!("{}", tmpfile)).unwrap(),
    ];

    info!("running {} {:?} -- {}", package_id, arguments, data,);
    execute_child(&ExecConfig {
        executable: c.artifact,
        arguments,
    })
}

pub fn exec_artifact(e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Pid> {
    match fork() {
        Ok(ForkResult::Child) => {
            if let Err(err) = exec_artifact_child(e, c) {
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
