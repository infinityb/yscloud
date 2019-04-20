use std::ffi::CString;
use std::fmt;
use std::io;
use std::path::Path;

use nix::unistd::Pid;

use crate::AppPreforkConfiguration;

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

pub use self::imp::{ExecExtras, ExecExtrasBuilder};
pub const EXTENSION: &str = imp::EXTENSION;
pub const PLATFORM_TRIPLES: &[&str] = imp::PLATFORM_TRIPLES;

pub struct Executable(imp::Executable);

impl Executable {
    pub fn open<P>(path: P) -> io::Result<Executable>
    where
        P: AsRef<Path>,
    {
        imp::Executable::open(path).map(Executable)
    }

    pub fn execute(&self, arguments: &[CString]) -> io::Result<!> {
        imp::Executable::execute(&self.0, arguments)
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
