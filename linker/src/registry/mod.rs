use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::read_dir;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use failure::Fallible;
use semver::{Version, VersionReq};

#[derive(Clone)]
pub struct FileRegistry(Arc<Registry>);

impl FileRegistry {
    pub fn new(base_path: &Path) -> FileRegistry {
        let base_path: PathBuf = base_path.to_owned();
        let opener: Box<dyn RegistryOpen + Send + Sync + 'static> =
            Box::new(FileRegistryOpen { base_path });

        FileRegistry(Arc::new(Registry { opener }))
    }

    pub async fn load_version(
        &self,
        package_id: &str,
        version: &Version,
    ) -> Fallible<RegistryEntry> {
        self.0.load_version(package_id, version)
    }

    pub async fn find_best_version(
        &self,
        package_id: &str,
        ver_req: VersionReq,
    ) -> Fallible<RegistryEntry> {
        self.0.find_best_version(package_id, ver_req)
    }
}

struct Registry {
    opener: Box<dyn RegistryOpen + Send + Sync + 'static>,
}

struct FileRegistryOpen {
    base_path: PathBuf,
}

impl RegistryOpen for FileRegistryOpen {
    fn get_versions(&self, package_id: &str) -> Fallible<Vec<Version>> {
        let mut path = self.base_path.clone();
        path.push(package_id);

        let mut out = Vec::new();

        for dir in read_dir(&path)? {
            let dir_entry = dir?;

            if let Ok(version) = version_from_filename(dir_entry.file_name()) {
                out.push(version);
            }
        }

        Ok(out)
    }

    fn load_version(&self, package_id: &str, version: &Version) -> Fallible<RegistryEntry> {
        let mut path = self.base_path.clone();
        path.push(package_id);
        path.push(format!("v{}", version));
        path.push("sha256");

        let mut sha256s: HashMap<String, String> = Default::default();

        println!("open file: {}", path.display());
        let file = File::open(&path)?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let line = line.trim();
            if line == "" {
                continue;
            }

            let mut line_parts = line.splitn(2, "  ");
            let sha256 = line_parts.next().unwrap();
            let host_triple = line_parts
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "invalid sha file"))?;

            sha256s.insert(host_triple.to_string(), sha256.to_string());
        }

        Ok(RegistryEntry {
            version: version.clone(),
            sha256s,
        })
    }
}

trait RegistryOpen {
    fn get_versions(&self, package_id: &str) -> Fallible<Vec<Version>>;

    fn load_version(&self, package_id: &str, version: &Version) -> Fallible<RegistryEntry>;
}

pub struct RegistryEntry {
    pub version: Version,
    // platform triple -> hex of hash
    pub sha256s: HashMap<String, String>,
}

impl Registry {
    fn load_version(&self, package_id: &str, version: &Version) -> Fallible<RegistryEntry> {
        self.opener.load_version(package_id, version)
    }

    fn find_best_version(&self, package_id: &str, ver_req: VersionReq) -> Fallible<RegistryEntry> {
        let mut current_winner: Option<Version> = None;
        for version in self.opener.get_versions(package_id)? {
            if ver_req.matches(&version) {
                if let Some(old_candidate) = current_winner.as_mut() {
                    if *old_candidate < version {
                        *old_candidate = version;
                    }
                } else {
                    current_winner = Some(version);
                }
            }
        }

        let winner = current_winner
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no valid versions"))?;
        self.load_version(package_id, &winner)
    }
}

fn version_from_filename(s: OsString) -> Result<Version, ()> {
    if let Ok(vs) = s.into_string() {
        if let Ok(vv) = Version::parse(&vs) {
            return Ok(vv);
        }
    }
    Err(())
}
