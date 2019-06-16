use std::io;
use std::path::{Path, PathBuf};

use log::{debug};
use semver::Version;

use super::platform::{self, Executable};

pub fn find_artifact(base: &str, package_id: &str, version: &Version) -> io::Result<Executable> {
    for p in platform::PLATFORM_TRIPLES {
        let name = format!("{}-v{}-{}{}", package_id, version, p, platform::EXTENSION);

        let mut pb: PathBuf = Path::new(base).into();
        pb.push(&name);

        debug!("trying to find {:?}", pb.display());
        if let Ok(aref) = Executable::open(pb) {
            return Ok(aref);
        }
    }

    Err(io::Error::new(io::ErrorKind::Other, "no binary found"))
}

pub fn load_artifact(base: &str) -> io::Result<Executable> {
    let pb: PathBuf = Path::new(base).into();
    if let Ok(aref) = Executable::open(pb) {
        return Ok(aref);
    }
    Err(io::Error::new(io::ErrorKind::Other, "no binary found"))
}
