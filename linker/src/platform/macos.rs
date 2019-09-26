use std::ffi::CString;
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use log::{info, warn};
use nix::unistd::{execve, fork, lseek, unlink, write, ForkResult, Pid, Whence};
use rand::{thread_rng, Rng};
use tempfile::{tempdir, TempDir};

use super::posix_imp::relabel_file_descriptors;
use crate::AppPreforkConfiguration;
use crate::Void;

#[derive(Debug)]
pub struct Executable {
    path: PathBuf,
    temporary_dir: Option<TempDir>,
}

pub struct ExecutableFactory {
    backing_storage: File,
    fully_qualified_path: PathBuf,
    temporary_dir: TempDir,
}

impl ExecutableFactory {
    pub fn new(name: &str, capacity: i64) -> io::Result<ExecutableFactory> {
        let temporary_dir = tempdir()?;
        let fully_qualified_path = temporary_dir.path().join(name);
        let backing_storage = File::create(&fully_qualified_path)?;
        Ok(ExecutableFactory {
            backing_storage,
            fully_qualified_path,
            temporary_dir,
        })
    }

    pub fn validate_sha(&self, sha: &str) -> io::Result<()> {
        // FIXME
        warn!("STUB - sha validation not implemented");
        Ok(())
    }

    pub fn finalize(self) -> Executable {
        Executable {
            path: self.fully_qualified_path,
            temporary_dir: Some(self.temporary_dir),
        }
    }
}

impl io::Write for ExecutableFactory {
    #[inline]
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.backing_storage.write(data)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.backing_storage.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice]) -> io::Result<usize> {
        self.backing_storage.write_vectored(bufs)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.backing_storage.write_all(buf)
    }

    #[inline]
    fn write_fmt(&mut self, fmt: fmt::Arguments) -> io::Result<()> {
        self.backing_storage.write_fmt(fmt)
    }
}

impl fmt::Display for Executable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "file:{}", self.path.display())
    }
}

impl Executable {
    pub fn open<P>(path: P) -> io::Result<Executable>
    where
        P: AsRef<Path>,
    {
        let path: &Path = path.as_ref();
        if path.exists() {
            Ok(Executable {
                path: path.into(),
                temporary_dir: None,
            })
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    pub fn execute(&self, arguments: &[CString], env: &[CString]) -> io::Result<Void> {
        let path_bytes = OsStrExt::as_bytes(self.path.as_os_str());
        let artifact_path = CString::new(path_bytes).expect("valid c-string");

        execve(&artifact_path, arguments, env)
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

fn execute_child(e: &ExecConfig) -> io::Result<Void> {
    let env = &[
        CString::new("RUST_BACKTRACE=1").unwrap(),
        CString::new("YSCLOUD=1").unwrap(),
    ];
    // need to relabel file descriptors here.
    e.executable.execute(&e.arguments, env)?;

    unreachable!();
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

fn exec_artifact_child(_e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Void> {
    use nix::fcntl::open;
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    let package_id = c.package_id.clone();

    let app_config = relabel_file_descriptors(&c)?;

    let path = format!(
        "/tmp/yscloud-{}-{}",
        c.instance_id,
        thread_rng().gen::<u64>()
    );
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
    })?;

    unreachable!();
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
