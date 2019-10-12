use std::io;
use std::path::{Path, PathBuf};

use semver::Version;
use tracing::{Level, span, event};

use super::platform::{self, Executable};

pub fn find_artifact(base: &str, package_id: &str, version: &Version) -> io::Result<Executable> {
    DiskArtifactLoader { base }.find_artifact(package_id, version)
}

pub fn direct_load_artifact(path: &str) -> io::Result<Executable> {
    let pb: PathBuf = Path::new(path).into();
    if let Ok(aref) = Executable::open(pb) {
        return Ok(aref);
    }
    Err(io::Error::new(io::ErrorKind::Other, "no binary found"))
}

struct DiskArtifactLoader<'a> {
    base: &'a str,
}

impl<'a> DiskArtifactLoader<'a> {
    pub fn find_artifact(&self, package_id: &str, version: &Version) -> io::Result<Executable> {
        let logging_span_def = span!(Level::DEBUG, "find_artifact",
            package_id = package_id,
            version = %version
        );

        let logging_span = logging_span_def.enter();

        for p in platform::PLATFORM_TRIPLES {
            let name = format!("{}-v{}-{}{}", package_id, version, p, platform::EXTENSION);

            let mut pb: PathBuf = Path::new(self.base).into();
            pb.push(&name);

            event!(Level::DEBUG, search_path = ?pb.display());

            if let Ok(aref) = Executable::open(pb) {
                return Ok(aref);
            }
        }

        event!(Level::ERROR, "failed to find artifact");

        drop(logging_span);

        Err(io::Error::new(io::ErrorKind::Other, "no binary found"))
    }
}
