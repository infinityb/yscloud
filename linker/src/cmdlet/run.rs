use std::collections::HashMap;
use std::error::Error as StdError;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use clap::{App, Arg, SubCommand};
use tracing::{event, span, Level};
use uuid::Uuid;

use sockets::socketpair_raw;
use yscloud_config_model::{
    DeployedApplicationManifest, DeploymentManifest, FileDescriptorRemote, Protocol,
    PublicServiceBinder, Sandbox, ServiceFileDirection, SideCarServiceInfo, SocketInfo, SocketMode,
};

use super::common;
use crate::artifact::{direct_load_artifact, find_artifact};
use crate::{
    bind_service, AppPreforkConfiguration, ExecExtras, ExecSomething, ServiceFileDescriptor,
};

use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "run";

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("link and run a deployment")
        .arg(common::approot())
        .arg(
            Arg::with_name("manifest")
                .long("manifest")
                .value_name("FILE")
                .help("The deployment manifest to link up and run")
                .required(true)
                .takes_value(true),
        )
        .arg(common::artifacts())
        .arg(common::artifact_override())
}

fn is_url(maybe_url: &str) -> bool {
    maybe_url.starts_with("http://") || maybe_url.starts_with("https://")
}

pub fn main(matches: &clap::ArgMatches) {
    let approot = matches.value_of("approot").unwrap();
    let approot = Path::new(approot).to_owned();
    let artifacts = matches.value_of("artifacts").unwrap();
    let manifest_path = matches.value_of("manifest").unwrap();

    let mut overrides: HashMap<String, String> = HashMap::new();
    if let Some(override_args) = matches.values_of_lossy("artifact-override") {
        for arg in override_args {
            let mut split_iter = arg.split(':');
            let package_name = split_iter.next().unwrap().to_string();
            let artifact_path = split_iter.next().unwrap().to_string();
            overrides.insert(package_name, artifact_path);
        }

        event!(
            Level::WARN,
            "development mode - using path overrides: {:?}",
            overrides
        );
    }

    event!(Level::INFO,
        approot = ?approot,
        artifacts = artifacts,
        manifest_path = manifest_path,
        overrides = ?overrides,
        "starting",
    );

    let rdr = File::open(&manifest_path).unwrap();
    let mut target_deployment_manifest =
        serde_json::from_reader::<_, DeploymentManifest>(rdr).unwrap();
    target_deployment_manifest.path_overrides = overrides;

    if is_url(&artifacts) {
        // download artifacts somewhere
    }

    let reified =
        reify_service_connections(&target_deployment_manifest, artifacts, &approot).unwrap();

    crate::platform::run_reified(reified);
}

fn reify_service_connections(
    dm: &DeploymentManifest,
    artifact_path: &str,
    approot: &Path,
) -> Result<Vec<crate::ExecSomething>, Box<dyn StdError>> {
    let span = span!(
        Level::INFO,
        "reify_service_connections",
        artifact_path = artifact_path,
        approot = &approot.display().to_string()[..]
    );

    let mut instances = HashMap::<Uuid, ExecSomething>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let artifact = if let Some(path) = dm.path_overrides.get(&component.package_id) {
            event!(
                Level::WARN,
                "because of override, trying to find package {:?} @ {}",
                component.package_id,
                path
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
            event!(
                parent: &span,
                Level::INFO,
                confinement.kind = "UNIX",
                confinement.unix_user = &user[..],
                confinement.unix_group = &group[..],
                "setting up confinement: UNIX({}:{})",
                user,
                group
            );
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
    event!(
        parent: &span,
        Level::TRACE,
        instance_component_count = instance_components.len()
    );

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

        event!(Level::TRACE,
            service_name = &ps.service_id.service_name[..],
            bind_target = ?ps.binder,
            "binding public service",
        );
        let service_sock = bind_service(&ps.binder)?;
        event!(Level::INFO,
            service_name = &ps.service_id.service_name[..],
            bind_target = ?ps.binder,
            file_descriptor = service_sock.as_raw_fd(),
            "binding public service complete",
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
