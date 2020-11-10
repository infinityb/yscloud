use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io;
use std::path::PathBuf;

use clap::{App, Arg, SubCommand};
use failure::{Fail, Fallible};
use serde_json::{json, Value};
use tokio::runtime;
use tracing::{event, Level};
use yscloud_config_model::{
    ApplicationDeploymentTemplate, ArtifactHashSet, DeployedApplicationManifest,
    DeployedPublicService, DeploymentManifest, RegistryEntry, Sandbox, ServiceId,
};

use super::common;
use crate::registry::{FileRegistry, Registry, RegistryShared};
use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "create-release";

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("press a release")
        .arg(common::registry())
        .arg(
            Arg::with_name("deployment-template")
                .long("deployment-template")
                .value_name("FILE")
                .help("The deployment template to use")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("deployment-name")
                .long("deployment-name")
                .value_name("deployment-name")
                .help("The deployment name to press a release for")
                .required(true)
                .takes_value(true),
        )
}

pub fn main(matches: &clap::ArgMatches) {
    let registry_path = PathBuf::from(matches.value_of_os("registry").unwrap());

    let deployment_name = matches.value_of("deployment-name").unwrap();
    event!(Level::TRACE, "got deployment name: {:?}", deployment_name);

    let deployment_tpl_path = PathBuf::from(matches.value_of_os("deployment-template").unwrap());

    let mut rdr = File::open(&deployment_tpl_path).unwrap();
    let ad: ApplicationDeploymentTemplate = serde_json::from_reader(&mut rdr).unwrap();

    let registry = RegistryShared::shared(FileRegistry::new(&registry_path));

    let mut rt = runtime::Builder::new().basic_scheduler().build().unwrap();

    let resolved = rt.block_on(resolve(&registry, &ad)).unwrap();

    let stdout = io::stdout();
    serde_json::to_writer(stdout.lock(), &resolved).unwrap();
}

#[derive(Debug, Fail)]
struct MissingServiceName {
    service_name: String,
}

impl fmt::Display for MissingServiceName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Missing service name: {}", self.service_name)
    }
}

async fn resolve(
    reg: &RegistryShared,
    template: &ApplicationDeploymentTemplate,
) -> Fallible<DeploymentManifest> {
    let mut out = DeploymentManifest {
        deployment_name: template.deployment_name.clone(),
        public_services: Vec::new(),
        components: Vec::new(),
        path_overrides: Default::default(),
    };

    let mut unresolved_local_services: VecDeque<String> = Default::default();
    let mut resolved_local_services = BTreeMap::new();

    for ps in &template.public_services {
        let impl_req = template
            .service_implementations
            .get(&ps.service_name)
            .ok_or_else(|| MissingServiceName {
                service_name: ps.service_name.clone(),
            })?;

        unresolved_local_services.push_back(ps.service_name.clone());
        out.public_services.push(DeployedPublicService {
            service_id: ServiceId {
                package_id: impl_req.package_id.clone(),
                service_name: ps.service_name.clone(),
            },
            binder: ps.binder.clone(),
        });
    }

    while let Some(ps) = unresolved_local_services.pop_front() {
        if resolved_local_services.get(&ps).is_some() {
            continue;
        }

        let impl_req =
            template
                .service_implementations
                .get(&ps)
                .ok_or_else(|| MissingServiceName {
                    service_name: ps.clone(),
                })?;

        let found: RegistryEntry = reg
            .find_best_entry_for_version(&impl_req.package_id, &impl_req.version_req)
            .await?;

        for rls in &found.manifest.required_local_services {
            unresolved_local_services.push_back(rls.clone());
        }

        let mut required_local_services = Vec::new();

        for rls in &found.manifest.required_local_services {
            let service_impl =
                template
                    .service_implementations
                    .get(rls)
                    .ok_or_else(|| MissingServiceName {
                        service_name: ps.clone(),
                    })?;

            required_local_services.push(ServiceId {
                package_id: service_impl.package_id.clone(),
                service_name: rls.clone(),
            });
        }

        let extras: Value = template
            .configuration
            .get(&impl_req.package_id)
            .map(|x| x.clone())
            .unwrap_or_else(|| json!({}));

        let sandbox = template
            .sandbox
            .get(&impl_req.package_id)
            .cloned()
            .unwrap_or(Sandbox::Unconfined);

        let mut artifacts = BTreeMap::new();
        for (trip, sha256) in &found.sha256s {
            artifacts.insert(
                trip.clone(),
                ArtifactHashSet {
                    content_length: None,
                    sha256: sha256.clone(),
                },
            );
        }

        resolved_local_services.insert(
            ps,
            DeployedApplicationManifest {
                package_id: impl_req.package_id.clone(),
                version: found.version,
                provided_local_services: found.manifest.provided_local_services,
                provided_remote_services: found.manifest.provided_remote_services,
                required_local_services,
                required_remote_services: found.manifest.required_remote_services,
                sandbox,
                extras,
                artifacts,
            },
        );
    }

    out.components
        .extend(resolved_local_services.into_iter().map(|(_, v)| v));

    Ok(out)
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_staticserver_simple() {
        use std::collections::BTreeMap;

        use semver::{Version, VersionReq};
        use tokio::runtime::current_thread::Runtime;

        use yscloud_config_model::{
            permissions, ApplicationDeploymentRequirement, ApplicationDeploymentTemplate,
            ApplicationManifest, DeploymentManifest, NativePortBinder, PublicService,
            PublicServiceBinder, RegistryEntry, Sandbox, SocketFlag,
        };

        use super::resolve;
        use crate::registry::{MemRegistry, RegistryShared};

        let file_logger_manifest = ApplicationManifest {
            provided_remote_services: Vec::new(),
            provided_local_services: vec!["org.yshi.log_target.v1.LogTarget".to_string()],
            required_remote_services: Vec::new(),
            required_local_services: Vec::new(),
            permissions: vec![permissions::UNCONSTRAINED],
        };

        let staticserver_manifest = ApplicationManifest {
            provided_remote_services: vec!["org.yshi.staticserver.http".to_string()],
            provided_local_services: Vec::new(),
            required_remote_services: Vec::new(),
            required_local_services: vec!["org.yshi.log_target.v1.LogTarget".to_string()],
            permissions: vec![permissions::UNCONSTRAINED],
        };

        let mut registry = MemRegistry::default();
        registry.add_package(
            "org.yshi.staticserver",
            RegistryEntry {
                version: Version::parse("1.0.5").unwrap(),
                sha256s: Default::default(),
                manifest: staticserver_manifest,
            },
        );
        registry.add_package(
            "org.yshi.file-logger",
            RegistryEntry {
                version: Version::parse("1.0.0").unwrap(),
                sha256s: Default::default(),
                manifest: file_logger_manifest.clone(),
            },
        );
        registry.add_package(
            "org.yshi.file-logger",
            RegistryEntry {
                version: Version::parse("1.0.1").unwrap(),
                sha256s: Default::default(),
                manifest: file_logger_manifest.clone(),
            },
        );
        registry.add_package(
            "org.yshi.file-logger",
            RegistryEntry {
                version: Version::parse("1.0.2").unwrap(),
                sha256s: Default::default(),
                manifest: file_logger_manifest.clone(),
            },
        );
        registry.add_package(
            "org.yshi.file-logger",
            RegistryEntry {
                version: Version::parse("2.0.0-alpha1").unwrap(),
                sha256s: Default::default(),
                manifest: file_logger_manifest.clone(),
            },
        );
        registry.add_package(
            "org.yshi.file-logger",
            RegistryEntry {
                version: Version::parse("2.0.0").unwrap(),
                sha256s: Default::default(),
                manifest: file_logger_manifest.clone(),
            },
        );
        let registry = RegistryShared::shared(registry);

        let xx = serde_json::from_str(r#"{
            "deployment_name": "aibi.yshi.org",
            "public_services": [
                {
                    "service_name": "org.yshi.sfshost.https",
                    "binder": {
                        "native_port_binder": {
                          "bind_address": "::",
                          "port": 1443
                        }
                    }
                }
            ],
            "service_implementations": {
                "org.yshi.log_target.v1.LogTarget": {
                    "package_id": "org.yshi.file-logger",
                    "version_req": "^1.0"
                },
                "org.yshi.sfshost.https": {
                    "package_id": "org.yshi.sfshost.https",
                    "version_req": "^1.0"
                }
            },
            "configuration": {
                "org.yshi.sfshost": {
                    "vhosts": {
                        "localhost": {
                            "directory": "./test",
                            "password": "foobar"
                        },
                        "localhost:1443": {
                            "directory": "./test",
                            "password": "foobar"
                        }
                    }
                }
            },
            "sandbox": {
                "org.yshi.sfshost": {
                    "unix_user_confinement": [
                        "sfs-aibi-log",
                        "sfs-aibi-log"
                    ]
                }
            }
        }"#).unwrap();

        let template = ApplicationDeploymentTemplate {
            deployment_name: "example-deployment".into(),
            public_services: vec![PublicService {
                service_name: "org.yshi.staticserver.http".into(),
                binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
                    bind_address: "::".into(),
                    port: 8080,
                    start_listen: true,
                    flags: vec![SocketFlag::BehindHaproxy, SocketFlag::StartListen],
                }),
            }],
            service_implementations: {
                let mut map = BTreeMap::<String, ApplicationDeploymentRequirement>::new();

                map.insert(
                    "org.yshi.log_target.v1.LogTarget".into(),
                    ApplicationDeploymentRequirement {
                        package_id: "org.yshi.file-logger".into(),
                        version_req: VersionReq::parse("^1.0").unwrap(),
                    },
                );

                map.insert(
                    "org.yshi.staticserver.http".into(),
                    ApplicationDeploymentRequirement {
                        package_id: "org.yshi.staticserver".into(),
                        version_req: VersionReq::parse("^1.0").unwrap(),
                    },
                );

                map
            },
            configuration: {
                let mut map = BTreeMap::<String, serde_json::Value>::new();

                map.insert(
                    "org.yshi.staticserver".into(),
                    serde_json::json!({
                        "allowed_hostnames": ["foobar-0436e87111796739188f.nydus.yshi.org"],
                    }),
                );

                map
            },
            sandbox: {
                let mut map = BTreeMap::<String, Sandbox>::new();

                map.insert(
                    "org.yshi.file-logger".into(),
                    Sandbox::UnixUserConfinement("sfs-aibi-log".into(), "sfs-aibi-log".into()),
                );

                map.insert(
                    "org.yshi.staticserver".into(),
                    Sandbox::UnixUserConfinement("sfs-aibi-fe".into(), "sfs-aibi-fe".into()),
                );

                map
            },
        };

        let dm_expect: DeploymentManifest = serde_json::from_str(
            r#"{
          "deployment_name": "example-deployment",
          "public_services": [
            {
              "service_id": {
                "package_id": "org.yshi.staticserver",
                "service_name": "org.yshi.staticserver.http"
              },
              "binder": {
                "native_port_binder": {
                  "bind_address": "::",
                  "port": 8080,
                  "start_listen": true,
                  "flags": ["behind_haproxy", "start_listen"]
                }
              }
            }
          ],
          "components": [
            {
              "package_id": "org.yshi.file-logger",
              "version": "1.0.2",
              "provided_local_services": [
                "org.yshi.log_target.v1.LogTarget"
              ],
              "provided_remote_services": [],
              "required_remote_services": [],
              "required_local_services": [],
              "permissions": [
                "org.yshi.permissions.unconstrained"
              ],
              "sandbox": {
                "unix_user_confinement": [
                  "sfs-aibi-log",
                  "sfs-aibi-log"
                ]
              },
              "extras": {}
            },
            {
              "package_id": "org.yshi.staticserver",
              "version": "1.0.5",
              "provided_local_services": [],
              "provided_remote_services": [
                "org.yshi.staticserver.http"
              ],
              "required_remote_services": [],
              "required_local_services": [
                {
                  "package_id": "org.yshi.file-logger",
                  "service_name": "org.yshi.log_target.v1.LogTarget"
                }
              ],
              "permissions": ["org.yshi.permissions.unconstrained"],
              "sandbox": {
                "unix_user_confinement": [
                  "sfs-aibi-fe",
                  "sfs-aibi-fe"
                ]
              },
              "extras": {
                "allowed_hostnames": ["foobar-0436e87111796739188f.nydus.yshi.org"]
              }
            }
          ]
        }"#,
        )
        .unwrap();

        let mut rt = Runtime::new().unwrap();
        let dm = rt.block_on(resolve(&registry, &template)).unwrap();
        println!("dm = {:#?}", dm);
        assert_eq!(dm_expect, dm);
    }
}
