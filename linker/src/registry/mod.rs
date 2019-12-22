use std::pin::Pin;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::read_dir;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::collections::BTreeMap;

use failure::Fallible;
use semver::{Version, VersionReq};
use futures::prelude::{Future};
use futures::future::FutureExt;

use yscloud_config_model::{RegistryEntry, ApplicationManifest};

pub trait Registry {
    fn find_best_entry_for_version(
        &self,
        package_id: &str,
        ver_req: &VersionReq,
    ) -> Pin<Box<dyn Future<Output=Fallible<RegistryEntry>> + Send + 'static>>;
}


#[derive(Clone)]
pub struct RegistryShared(Arc<dyn Registry + Send + Sync + 'static>);

pub struct FileRegistry {
    base_path: PathBuf,
}

pub struct MemRegistry {
    known_entries: BTreeMap<(String, Version), RegistryEntry>,
}

impl RegistryShared {
    pub fn shared<T>(reg: T) -> RegistryShared where T: Registry + Send + Sync + 'static {
        let boxed: Box<dyn Registry + Send + Sync + 'static> = Box::new(reg);
        let arc: Arc<dyn Registry + Send + Sync + 'static> = boxed.into();
        RegistryShared(arc)
    }
}

impl Registry for RegistryShared {
    fn find_best_entry_for_version(
        &self,
        package_id: &str,
        ver_req: &VersionReq,
    ) -> Pin<Box<dyn Future<Output=Fallible<RegistryEntry>> + Send + 'static>> {
        Registry::find_best_entry_for_version(&*self.0, package_id, ver_req)
    }
}

impl FileRegistry {
    pub fn new(base_path: &Path) -> FileRegistry {
        FileRegistry {
            base_path: base_path.to_owned(),
        }
    }
}

impl Registry for FileRegistry {
    fn find_best_entry_for_version(
        &self,
        package_id: &str,
        ver_req: &VersionReq,
    ) -> Pin<Box<dyn Future<Output=Fallible<RegistryEntry>> + Send + 'static>> {
        let base_path = self.base_path.clone();
        let package_id = package_id.to_owned();
        let ver_req = ver_req.to_owned();

        async move {
            let mut current_winner: Option<Version> = None;
            for version in registry_file_get_versions(&base_path, &package_id)? {
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
                .ok_or_else(|| {
                    let msg = format!("no valid versions for {}", package_id);
                    io::Error::new(io::ErrorKind::Other, msg)
                })?;

            registry_file_load_version(&base_path, &package_id, &winner)
        }.boxed()
    }
}

fn registry_file_get_versions(base_path: &Path, package_id: &str) -> Fallible<Vec<Version>> {
    fn version_from_filename(s: OsString) -> Result<Version, ()> {
        if let Ok(vs) = s.into_string() {
            if !vs.starts_with("v") {
                return Err(());
            }
            if let Ok(vv) = Version::parse(&vs[1..]) {
                return Ok(vv);
            }
        }
        Err(())
    }

    let mut path: PathBuf = base_path.to_owned();
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

fn registry_file_load_version(base_path: &Path, package_id: &str, version: &Version) -> Fallible<RegistryEntry> {
    let mut hash_path = base_path.to_owned();
    hash_path.push(package_id);
    hash_path.push(format!("v{}", version));
    hash_path.push("sha256");

    let mut sha256s: HashMap<String, String> = Default::default();
    let file = File::open(&hash_path)?;
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

    let mut manifest_path = base_path.to_owned();
    manifest_path.push(package_id);
    manifest_path.push(format!("v{}", version));
    manifest_path.push("manifest.json");

    let file = File::open(&manifest_path)?;
    let manifest: ApplicationManifest = serde_json::from_reader(BufReader::new(file))?;

    Ok(RegistryEntry {
        version: version.clone(),
        sha256s,
        manifest,
    })
}

impl Default for MemRegistry {
    fn default() -> MemRegistry {
        MemRegistry {
            known_entries: Default::default(),
        }
    }
}

impl MemRegistry {
    pub fn add_package(&mut self, package_id: &str, entry: RegistryEntry) {
        let pkg_key = (package_id.to_owned(), entry.version.clone());
        self.known_entries.insert(pkg_key, entry);
    }
}

impl Registry for MemRegistry {
    fn find_best_entry_for_version(
        &self,
        package_id: &str,
        ver_req: &VersionReq,
    ) -> Pin<Box<dyn Future<Output=Fallible<RegistryEntry>> + Send + 'static>> {
        let min_version = Version::new(0, 0, 0);
        let min_key = (package_id.to_owned(), min_version);
        
        let mut current_winner: Option<&Version> = None;
        for ((p_id, version), _entry) in self.known_entries.range(min_key..) {
            if p_id != package_id {
                break;
            }
            if !ver_req.matches(version) {
                continue;
            }
            if let Some(old_candidate) = current_winner.as_mut() {
                if **old_candidate < *version {
                    *old_candidate = version;
                }
            } else {
                current_winner = Some(version);
            }
        }

        futures::future::ready(match current_winner {
            Some(winning_version) => {
                let key = (package_id.to_owned(), winning_version.to_owned());
                Ok(self.known_entries.get(&key).unwrap().clone())
            },
            None => {
                Err(io::Error::new(io::ErrorKind::Other, "no valid versions").into())
            }
        }).boxed()
    }
}