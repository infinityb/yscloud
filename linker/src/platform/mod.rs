use std::ffi::CStr;
use std::fmt;
use std::io;
use std::path::Path;

use nix::unistd::Pid;

use yscloud_config_model::ImageType;

use crate::{AppPreforkConfiguration, Void};

mod common;
pub use self::common::{ExecutableFactoryHasher, ExecutableFactoryCommon};

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use self::linux as imp;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
use self::macos as imp;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod posix;

#[cfg(target_os = "linux")]
pub use self::linux::container as container;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use self::posix as posix_imp;

pub use self::imp::{run_reified, ExecExtras, ExecExtrasBuilder};
pub const EXTENSION: &str = imp::EXTENSION;
pub const PLATFORM_TRIPLES: &[&str] = imp::PLATFORM_TRIPLES;

pub struct ExecutableFactory {
    os_impl: imp::ExecutableFactory,
    common: ExecutableFactoryCommon,
}

impl ExecutableFactory {
    pub fn new_unspecified(name: &str, capacity: i64) -> io::Result<ExecutableFactory> {
        let os_impl = imp::ExecutableFactory::new_unspecified(name, capacity)?;
        Ok(ExecutableFactory { os_impl, common: Default::default() })
    }

    #[cfg(target_os = "linux")]
    pub fn new_in_memory(name: &str, capacity: i64) -> io::Result<ExecutableFactory> {
        let os_impl = imp::ExecutableFactory::new_in_memory(name, capacity)?;
        Ok(ExecutableFactory { os_impl, common: Default::default() })
    }

    pub fn new_on_disk(name: &str, capacity: i64, root: &Path) -> io::Result<ExecutableFactory> {
        let os_impl = imp::ExecutableFactory::new_on_disk(name, capacity, root)?;
        Ok(ExecutableFactory { os_impl, common: Default::default() })
    }

    pub fn enable_hasher(&mut self, h: ExecutableFactoryHasher) {
        self.common.enable_hasher(h)
    }

    pub fn validate_hash(&self, h: ExecutableFactoryHasher, expect_sha: &str) -> io::Result<()> {
        self.common.validate_hash(h, expect_sha)
    }

    pub fn finalize_executable(self) -> Executable {
        Executable(self.os_impl.finalize_executable())
    }
}

impl io::Write for ExecutableFactory {
    #[inline]
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let written = self.os_impl.write(data)?;
        self.common.hash_update(&data[..written]);
        Ok(written)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.os_impl.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice]) -> io::Result<usize> {
        self.os_impl.write_vectored(bufs)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.os_impl.write_all(buf)
    }

    #[inline]
    fn write_fmt(&mut self, fmt: fmt::Arguments) -> io::Result<()> {
        self.os_impl.write_fmt(fmt)
    }
}

#[derive(Debug)]
pub struct Executable(imp::Executable);

impl Executable {
    pub fn open<P>(path: P) -> io::Result<Executable>
    where
        P: AsRef<Path>,
    {
        imp::Executable::open(path).map(Executable)
    }

    pub fn execute(&self, arguments: &[&CStr], env: &[&CStr]) -> io::Result<Void> {
        imp::Executable::execute(&self.0, arguments, env)
    }
}

impl fmt::Display for Executable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub struct ArtifactRef(Executable);

impl fmt::Display for ArtifactRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub fn exec_artifact(e: &ExecExtras, c: AppPreforkConfiguration) -> io::Result<Pid> {
    imp::exec_artifact(e, c)
}
