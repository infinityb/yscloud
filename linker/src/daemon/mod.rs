use failure::Fallible;
use semver::Version;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::convert::TryInto;
use std::fs::File;

use futures::future::{Future, FutureExt};
use log::{info, warn};
use sockets::socketpair_raw;
use uuid::Uuid;
use yscloud_config_model::{
    DeployedApplicationManifest, DeploymentManifest, FileDescriptorRemote, Protocol,
    PublicServiceBinder, Sandbox, ServiceFileDirection, SideCarServiceInfo, SocketInfo, SocketMode,
};

use crate::platform::{ExecutableFactory, Executable};
use crate::registry::{FileRegistry, RegistryEntry};
use crate::{
    artifact::direct_load_artifact, artifact::find_artifact, bind_service, AppPreforkConfiguration,
    ExecExtras, ExecSomething, ServiceFileDescriptor,
};

mod artifact_loader;

pub fn start(cfg: Config) {
    let registry = FileRegistry::new(&cfg.registry);

    let rdr = File::open("example-deployment-manifest.json").unwrap();
    let target_deployment_manifest =
        serde_json::from_reader::<_, DeploymentManifest>(rdr).unwrap();

    let fut = download_components(&cfg, &registry, &target_deployment_manifest);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let xx = rt.block_on(fut);
    println!("xx = {:?}", xx);
}

#[derive(Clone)]
pub struct Config {
    pub approot: PathBuf,
    pub artifacts: String,
    pub registry: String,
    pub control_socket: PathBuf,
}

#[derive(Debug)]
struct Component {
    executable: Executable,
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Debug, Hash, Clone)]
struct PackageKey {
    package_id: String,
    version: Version,
}

async fn fetch_component_metadata(
    reg: &FileRegistry,
    dm: &DeploymentManifest,
) -> Fallible<HashMap<PackageKey, RegistryEntry>> {
    let dm: DeploymentManifest = dm.clone();

    let mut reg_futures: Vec<
        Pin<Box<dyn Future<Output = Fallible<(PackageKey, RegistryEntry)>> + Send>>,
    > = Vec::new();
    for component in &dm.components {
        if dm.path_overrides.get(&component.package_id).is_none() {
            reg_futures.push(
                async move {
                    let entry = reg
                        .load_version(&component.package_id, &component.version)
                        .await?;

                    Ok((
                        PackageKey {
                            package_id: component.package_id.clone(),
                            version: component.version.clone(),
                        },
                        entry,
                    ))
                }
                    .boxed(),
            );
        }
    }

    let out: HashMap<_, _> = futures::future::try_join_all(reg_futures)
        .await?
        .into_iter()
        .collect();

    Ok(out)
}

async fn download_components(
    cfg: &Config,
    reg: &FileRegistry,
    dm: &DeploymentManifest,
) -> Fallible<HashMap<PackageKey, Component>> {
    let dm: DeploymentManifest = dm.clone();

    // registry lookups
    let reg_entries = fetch_component_metadata(reg, &dm).await?;

    let reg_entries = Arc::new(reg_entries);
    let mut futures: Vec<Pin<Box<dyn Future<Output = Fallible<(PackageKey, Component)>> + Send>>> =
        vec![];

    // artifact fetch
    for component in &dm.components {
        let pkg_key = PackageKey {
            package_id: component.package_id.clone(),
            version: component.version.clone(),
        };
        if let Some(path) = dm.path_overrides.get(&component.package_id) {
            let path: String = path.to_string();
            futures.push(
                async move {
                    warn!(
                        "because of override, trying to find package {:?} @ {}",
                        component.package_id, path
                    );
                    warn!(
                        "package {:?} code signing is not required (local path)",
                        component.package_id
                    );

                    let executable = direct_load_artifact(&path)?;
                    Ok((pkg_key, Component { executable }))
                }
                    .boxed(),
            );
        } else {
            let cfg = cfg.clone();
            let reg_entries = Arc::clone(&reg_entries);

            futures.push(
                async move {
                    let executable =
                        find_artifact(&cfg, &*reg_entries, &pkg_key.package_id, &pkg_key.version).await?;

                    //

                    Ok((pkg_key, Component { executable }))
                }
                    .boxed(),
            );
        };
    }

    let resolved = futures::future::try_join_all(futures).await?;
    return Ok(resolved.into_iter().collect());

    async fn find_artifact(
        cfg: &Config,
        reg: &HashMap<PackageKey, RegistryEntry>,
        package_id: &str,
        version: &Version,
    ) -> Fallible<Executable> {
        let reg_entry = reg
            .get(&PackageKey {
                package_id: package_id.to_string(),
                version: version.clone(),
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "unregistered")
            })?;

        let mut artifact_specific: Option<(&str, &str)> = None;
        for p in crate::platform::PLATFORM_TRIPLES {
            if let Some(sha) = reg_entry.sha256s.get(*p) {
                artifact_specific = Some((*p, sha));
                break;
            }
        }

        let (platform_triple, sha) = artifact_specific.ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "platform not supported",
        ))?;

        let uri = format!(
            "{}/{}-{}-{}",
            cfg.artifacts, package_id, version, platform_triple
        );
        let filename = &uri[cfg.artifacts.len() + 1..];


        // FIXME: async
        let mut response = reqwest::get(&uri)?;
        // FIXME: add this information to the registry.
        let content_length = response.content_length().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no content-length on server")
        })?;

        let mut fac = ExecutableFactory::new(filename, content_length.try_into()?)?;
        response.copy_to(&mut fac)?;

        fac.validate_sha(&sha)?;

        Ok(fac.finalize())
    }
}

fn reify_service_connections(
    dm: &DeploymentManifest,
    artifact_path: &str,
    approot: &Path,
) -> Result<Vec<crate::ExecSomething>, Box<dyn StdError>> {
    let mut instances = HashMap::<Uuid, ExecSomething>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let artifact = if let Some(path) = dm.path_overrides.get(&component.package_id) {
            warn!(
                "because of override, trying to find package {:?} @ {}",
                component.package_id, path
            );
            direct_load_artifact(&path)?
        } else {
            find_artifact(artifact_path, &component.package_id, &component.version)?
        };

        let instance_id = Uuid::new_v4();

        let mut builder = ExecExtras::builder();

        let mut workdir = approot.to_owned();
        workdir.push(&dm.deployment_name);
        workdir.push(&component.package_id);
        builder.set_workdir(&workdir).unwrap();

        if let Sandbox::UnixUserConfinement(ref user, ref group) = component.sandbox {
            builder.set_user(user).unwrap();
            builder.set_group(group).unwrap();
        }

        instances.insert(
            instance_id,
            ExecSomething {
                extras: builder.build(),
                cfg: AppPreforkConfiguration {
                    package_id: component.package_id.clone(),
                    artifact,
                    version: format!("{}", component.version),
                    instance_id,
                    files: Default::default(),
                    extras: component.extras.clone(),
                },
            },
        );
        instance_components.insert(instance_id, component);
        instance_by_package.insert(&component.package_id, instance_id);
    }

    for ps in &dm.public_services {
        let instance_id = instance_by_package
            .get(&*ps.service_id.package_id)
            .ok_or_else(|| {
                format!(
                    "internal error: unknown package {:?}",
                    ps.service_id.package_id
                )
            })?;

        let instance = instances
            .get_mut(instance_id)
            .ok_or_else(|| format!("internal error: unknown instance {:?}", instance_id))?;

        info!(
            "binding public service {} to {:?}",
            ps.service_id.service_name, ps.binder
        );
        let service_sock = bind_service(&ps.binder)?;
        info!(
            "binded public service {} to {:?} - fd = {}",
            ps.service_id.service_name,
            ps.binder,
            service_sock.as_raw_fd(),
        );

        instance.cfg.files.push(ServiceFileDescriptor {
            file: service_sock,
            direction: ServiceFileDirection::ServingListening,
            service_name: ps.service_id.service_name.clone(),
            remote: FileDescriptorRemote::Socket(SocketInfo {
                mode: SocketMode::Listening,
                protocol: Protocol::Stream,
                flags: match ps.binder {
                    PublicServiceBinder::NativePortBinder(ref np) => np.flags.clone(),
                    PublicServiceBinder::UnixDomainBinder(ref ub) => ub.flags.clone(),
                    PublicServiceBinder::WebServiceBinder(ref ws) => ws.flags.clone(),
                },
            }),
        });
    }

    for (local_instance_id, local_cfg) in &instance_components {
        for ls in &local_cfg.required_local_services {
            let remote_instance_id = instance_by_package
                .get(&*ls.package_id)
                .ok_or_else(|| format!("internal error: unknown package {:?}", ls.package_id))?;

            let remote_cfg = instance_components.get(remote_instance_id).ok_or_else(|| {
                format!("internal error: unknown instance {:?}", remote_instance_id)
            })?;

            let (local_sock, remote_sock) = socketpair_raw()?;

            {
                let local_instance = instances.get_mut(local_instance_id).ok_or_else(|| {
                    format!("internal error: unknown instance {:?}", local_instance_id)
                })?;

                local_instance.cfg.files.push(ServiceFileDescriptor {
                    file: local_sock,
                    direction: ServiceFileDirection::Consuming,
                    service_name: ls.service_name.clone(),
                    remote: FileDescriptorRemote::SideCarService(SideCarServiceInfo {
                        instance_id: *remote_instance_id,
                        package_id: remote_cfg.package_id.clone(),
                        version: remote_cfg.version.clone(),
                    }),
                });
            }

            {
                let remote_instance = instances.get_mut(remote_instance_id).ok_or_else(|| {
                    format!("internal error: unknown instance {:?}", remote_instance_id)
                })?;

                remote_instance.cfg.files.push(ServiceFileDescriptor {
                    file: remote_sock,
                    direction: ServiceFileDirection::ServingConnected,
                    service_name: ls.service_name.clone(),
                    remote: FileDescriptorRemote::SideCarService(SideCarServiceInfo {
                        instance_id: *local_instance_id,
                        package_id: local_cfg.package_id.clone(),
                        version: local_cfg.version.clone(),
                    }),
                });
            }
        }
    }

    Ok(instances.into_iter().map(|(_, v)| v).collect())
}
