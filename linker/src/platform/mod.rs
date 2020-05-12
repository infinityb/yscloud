use std::ffi::CStr;
use std::fmt;
use std::io;
use std::path::Path;

use nix::unistd::Pid;

use crate::{AppPreforkConfiguration, Void};

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

#[cfg(any(target_os = "macos", target_os = "linux"))]
use self::posix as posix_imp;

pub use self::imp::{run_reified, ExecExtras, ExecExtrasBuilder};
pub const EXTENSION: &str = imp::EXTENSION;
pub const PLATFORM_TRIPLES: &[&str] = imp::PLATFORM_TRIPLES;

pub struct ExecutableFactory(imp::ExecutableFactory);

impl ExecutableFactory {
    pub fn new(name: &str, capacity: i64) -> io::Result<ExecutableFactory> {
        imp::ExecutableFactory::new(name, capacity).map(ExecutableFactory)
    }

    pub fn validate_sha(&self, sha: &str) -> io::Result<()> {
        self.0.validate_sha(sha)
    }

    pub fn finalize(self) -> Executable {
        Executable(self.0.finalize())
    }
}

impl io::Write for ExecutableFactory {
    #[inline]
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.write(data)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    #[inline]
    fn write_fmt(&mut self, fmt: fmt::Arguments) -> io::Result<()> {
        self.0.write_fmt(fmt)
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
