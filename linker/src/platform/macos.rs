use std::error::Error as StdError;
use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use digest::FixedOutput;
use nix::unistd::{execve, fork, lseek, unlink, write, ForkResult, Pid, Whence};
use rand::{thread_rng, Rng};
use sha2::Sha256;
use tempfile::{tempdir, TempDir};
use tracing::{event, Level};

use super::posix_imp::relabel_file_descriptors;
pub use super::posix_imp::run_reified;
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
    sha_state: Sha256,
}

impl ExecutableFactory {
    pub fn new(name: &str, _capacity: i64) -> io::Result<ExecutableFactory> {
        let temporary_dir = tempdir()?;
        let fully_qualified_path = temporary_dir.path().join(name);
        let backing_storage = File::create(&fully_qualified_path)?;

        let metadata = backing_storage.metadata()?;
        let permissions = metadata.permissions();
        backing_storage.set_permissions(permissions)?;

        Ok(ExecutableFactory {
            backing_storage,
            fully_qualified_path,
            temporary_dir,
            sha_state: Default::default(),
        })
    }

    pub fn validate_sha(&self, expect_sha: &str) -> io::Result<()> {
        let sha_result = self.sha_state.clone().fixed_result();
        let mut scratch = [0; 256 / 8 * 2];

        let got_sha = crate::util::hexify(&mut scratch[..], &sha_result[..]).unwrap();

        if expect_sha != got_sha {
            let msg = format!("sha mismatch {} != {}", expect_sha, got_sha);
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }

        Ok(())
    }

    pub fn finalize(self) -> Executable {
        Executable {
            path: self.fully_qualified_path.canonicalize().unwrap(),
            temporary_dir: Some(self.temporary_dir),
        }
    }
}

impl io::Write for ExecutableFactory {
    #[inline]
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let written = self.backing_storage.write(data)?;

        let sha_written = self.sha_state.write(&data[..written])?;
        assert_eq!(sha_written, written);

        Ok(written)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.backing_storage.flush()
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
                path: path.canonicalize().unwrap(),
                temporary_dir: None,
            })
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    pub fn execute(&self, arguments: &[&CStr], env: &[&CStr]) -> io::Result<Void> {
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
        CStr::from_bytes_with_nul(b"RUST_BACKTRACE=1\0").unwrap(),
        CStr::from_bytes_with_nul(b"YSCLOUD=1\0").unwrap(),
    ];

    // need to relabel file descriptors here.
    let mut cstrs: Vec<&CStr> = Vec::new();
    for arg in &e.arguments {
        cstrs.push(arg);
    }

    e.executable.execute(&cstrs[..], env)?;

    unreachable!();
}

pub struct ExecExtras {
    workdir: PathBuf,
}

impl ExecExtras {
    pub fn builder() -> ExecExtrasBuilder {
        Default::default()
    }
}

#[derive(Default)]
pub struct ExecExtrasBuilder {
    workdir: Option<PathBuf>,
}

impl ExecExtrasBuilder {
    pub fn set_user(&mut self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn set_group(&mut self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    pub fn set_workdir(&mut self, workdir: &Path) -> io::Result<()> {
        self.workdir = Some(workdir.to_owned());
        Ok(())
    }

    pub fn build(&self) -> ExecExtras {
        ExecExtras {
            workdir: self.workdir.as_ref().unwrap().clone(),
        }
    }
}

// let artifact_path = ((c.artifact.0).0).clone();
// let path_bytes = OsStrExt::as_bytes(artifact_path.as_os_str());
// let artifact_path = CString::new(path_bytes).expect("valid c-string");

fn exec_artifact_child(e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Void> {
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
    .map_err(|err| {
        event!(
            Level::WARN,
            "error opening temporary ({}:{}): {:?}",
            file!(),
            line!(),
            err
        );
        io::Error::new(io::ErrorKind::Other, err)
    })?;

    if let Err(err) = unlink(&path as &str) {
        event!(
            Level::WARN,
            "failed to unlink temporary file {}: {}",
            path,
            err
        );
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

    event!(
        Level::TRACE,
        "running {} {:?} in {} -- {}",
        package_id,
        arguments,
        e.workdir.display(),
        data
    );
    nix::unistd::chdir(&e.workdir).map_err(|err| {
        event!(
            Level::WARN,
            "error changing directory to {:?} ({}:{}): {:?}",
            e.workdir,
            file!(),
            line!(),
            err
        );
        io_other(err)
    })?;

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

fn io_other<E>(e: E) -> io::Error
where
    E: Into<Box<dyn StdError + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::Other, e)
}
