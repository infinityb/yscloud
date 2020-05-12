use std::collections::HashMap;
use std::convert::TryInto;
use std::error::Error as StdError;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

use failure::Fallible;
use futures::future::{Future, FutureExt};
use futures::stream::StreamExt;
use semver::Version;
use sockets::socketpair_raw;
use tracing::{event, Level};
use uuid::Uuid;
use yscloud_config_model::{
    DeployedApplicationManifest, DeploymentManifest, FileDescriptorRemote, Protocol,
    PublicServiceBinder, Sandbox, ServiceFileDirection, SideCarServiceInfo, SocketInfo, SocketMode,
};

use crate::platform::{Executable, ExecutableFactory};
use crate::{
    artifact::direct_load_artifact, bind_service, AppPreforkConfiguration, ExecExtras,
    ExecSomething, ServiceFileDescriptor,
};

const DEFAULT_MAX_ARTIFACT_SIZE: u64 = 50 * 1 << 20; // 50 MB

pub fn start(cfg: Config) {
    let rdr = File::open("example-deployment-manifest.json").unwrap();
    let mut target_deployment_manifest =
        serde_json::from_reader::<_, DeploymentManifest>(rdr).unwrap();

    target_deployment_manifest.path_overrides = cfg.overrides.clone();

    let fut = download_components(&cfg, &target_deployment_manifest);
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let xx = rt.block_on(fut).unwrap();
    let reified = reify_service_connections(&target_deployment_manifest, xx, &cfg.approot).unwrap();
    crate::platform::run_reified(reified);
}

#[derive(Clone)]
pub struct Config {
    pub approot: PathBuf,
    // this might be a remote resource or a local one - our type is incorrect here.
    pub artifacts: String,
    pub control_socket: PathBuf,
    pub overrides: HashMap<String, String>,
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

// fn fetch_component_metadata<'a>(
//     dm: &'a DeploymentManifest,
// ) -> Fallible<HashMap<PackageKey<'a>, &'a DeployedApplicationManifest>> {
//     let dm: DeploymentManifest = dm.clone();

//     let mut reg_futures: Vec<
//         Pin<Box<dyn Future<Output = Fallible<(PackageKey, RegistryEntry)>> + Send>>,
//     > = Vec::new();

//     for component in &dm.components {
//         if dm.path_overrides.get(&component.package_id).is_none() {

//             let component_version = component.version.clone();
//             let component_package_id = component.package_id.clone();

//             let triple_to_sha256 = HashMap::new();
//             triple_to_sha256.insert((), ());

//             RegistryEntry {
//                 version: component.version.clone(),
//                 sha256s: triple_to_sha256,
//                 manifest:
//             }

//             reg_futures.push(
//                 async move {
//                     let component_req = VersionReq::exact(&component_version);

//                     RegistryEntry {
//                         version: component_version.clone(),
//                         sha256s:
//                     }
//                     let entry = reg_clone
//                         .find_best_entry_for_version(&component_package_id, &component_req)
//                         .await?;

//                     Ok((
//                         PackageKey {
//                             package_id: component_package_id,
//                             version: component_version,
//                         },
//                         entry,
//                     ))
//                 }
//                     .boxed(),
//             );
//         }
//     }

//     let out: HashMap<_, _> = futures::future::try_join_all(reg_futures)
//         .await?
//         .into_iter()
//         .collect();

//     Ok(out)
// }

async fn download_components(
    cfg: &Config,
    dm: &DeploymentManifest,
) -> Fallible<HashMap<PackageKey, Component>> {
    let dm: DeploymentManifest = dm.clone();

    let mut futures: Vec<Pin<Box<dyn Future<Output = Fallible<(PackageKey, Component)>> + Send>>> =
        vec![];
    for component in &dm.components {
        let pkg_key = PackageKey {
            package_id: component.package_id.clone(),
            version: component.version.clone(),
        };
        if let Some(path) = dm.path_overrides.get(&component.package_id) {
            let path: String = path.to_string();
            futures.push(
                async move {
                    event!(
                        Level::WARN,
                        "because of override, trying to find package {:?} @ {}",
                        component.package_id,
                        path
                    );
                    event!(
                        Level::WARN,
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
            futures.push(
                async move {
                    let executable = find_artifact(&cfg, component).await?;
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
        dam: &DeployedApplicationManifest,
    ) -> Fallible<Executable> {
        let mut artifact_specific: Option<(&str, &str)> = None;
        for p in crate::platform::PLATFORM_TRIPLES {
            if let Some(artifact) = dam.artifacts.get(*p) {
                artifact_specific = Some((*p, &artifact.sha256));
                break;
            }
        }

        let (platform_triple, sha) = artifact_specific.ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "platform not supported",
        ))?;

        let uri = format!(
            "{}/{}-v{}-{}",
            cfg.artifacts, dam.package_id, dam.version, platform_triple
        );
        let filename = &uri[cfg.artifacts.len() + 1..];

        event!(Level::INFO, "fetching {} from {}", dam.package_id, uri);
        // FIXME: async
        let response = reqwest::get(&uri).await?;
        // FIXME: add this information to the registry.
        let content_length = response.content_length().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no content-length on server")
        })?;

        if DEFAULT_MAX_ARTIFACT_SIZE < content_length {
            event!(
                Level::ERROR,
                "file-size of {} is {} bytes - this exceeds the maximum artifact size",
                dam.package_id,
                content_length
            );
            return Err(io::Error::new(io::ErrorKind::Other, "artifact too large").into());
        }

        event!(
            Level::INFO,
            "file-size of {} is {} bytes",
            dam.package_id,
            content_length
        );

        let mut fac = ExecutableFactory::new(filename, content_length.try_into()?)?;

        let mut resp_data = response.bytes_stream();
        while let Some(v) = resp_data.next().await {
            let v = v?;
            fac.write(&v[..])?;
        }
        fac.validate_sha(&sha)?;
        Ok(fac.finalize())
    }
}

fn reify_service_connections(
    dm: &DeploymentManifest,
    mut component_artifacts: HashMap<PackageKey, Component>,
    approot: &Path,
) -> Result<Vec<crate::ExecSomething>, Box<dyn StdError>> {
    let mut instances = HashMap::<Uuid, ExecSomething>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let pkg_key = PackageKey {
            package_id: component.package_id.clone(),
            version: component.version.clone(),
        };

        let component_artifact = component_artifacts
            .remove(&pkg_key)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "internal error?"))?;

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
                    tenant_id: dm.tenant_id.clone(),
                    deployment_name: dm.deployment_name.clone(),
                    package_id: component.package_id.clone(),
                    artifact: component_artifact.executable,
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

        event!(
            Level::INFO,
            "binding public service {} to {:?}",
            ps.service_id.service_name,
            ps.binder
        );
        let service_sock = bind_service(&ps.binder)?;
        event!(
            Level::INFO,
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
