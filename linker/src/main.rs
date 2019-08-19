#![feature(never_type)]

use std::collections::HashMap;
use std::error::Error as StdError;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use clap::{App, Arg};
use env_logger::Builder;
use log::{debug, error, info, trace, warn, LevelFilter};
use uuid::Uuid;

use sockets::{socketpair_raw, OwnedFd};
use yscloud_config_model::{
    AppConfiguration, DeployedApplicationManifest, DeploymentManifest,
    FileDescriptorInfo, FileDescriptorRemote, Protocol, PublicServiceBinder,
    Sandbox, ServiceFileDirection, SideCarServiceInfo, SocketInfo, SocketMode,
};

use crate::artifact::{find_artifact, load_artifact};
use crate::platform::{ExecExtras, Executable};

mod artifact;
mod bind;
mod cmdlet;
pub mod platform;

const SUBCOMMAND_CREATE_RELEASE: &str = "create-release";
const SUBCOMMAND_RUN: &str = "run";
const SUBCOMMAND_EXPORT_MANIFEST: &str = "export-manifest";

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    let matches = App::new("yscloud-linker")
        .version(CARGO_PKG_VERSION)
        .author("Stacey Ell <stacey.ell@gmail.com>")
        .about("Microservice/sidecar linker and privilege separation")
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity for this package"),
        )
        .arg(
            Arg::with_name("d")
                .short("d")
                .multiple(true)
                .help("Sets the level of verbosity for all packages (debugging)"),
        )
        .subcommand(cmdlet::create_release::get_subcommand())
        .subcommand(cmdlet::run::get_subcommand())
        .get_matches();

    let mut builder = Builder::from_default_env();
    builder.default_format_module_path(true);
    match matches.occurrences_of("v") {
        0 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Error),
        1 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Warn),
        2 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Info),
        3 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Debug),
        4 | _ => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Trace),
    };
    match matches.occurrences_of("d") {
        0 => {
            builder.filter_module(CARGO_PKG_NAME, LevelFilter::Error);
            builder.filter(None, LevelFilter::Error);
        }
        1 => {
            builder.filter_module(CARGO_PKG_NAME, LevelFilter::Warn);
            builder.filter(None, LevelFilter::Warn);
        }
        2 => {
            builder.filter_module(CARGO_PKG_NAME, LevelFilter::Info);
            builder.filter(None, LevelFilter::Info);
        }
        3 => {
            builder.filter_module(CARGO_PKG_NAME, LevelFilter::Debug);
            builder.filter(None, LevelFilter::Debug);
        }
        4 | _ => {
            builder.filter_module(CARGO_PKG_NAME, LevelFilter::Trace);
            builder.filter(None, LevelFilter::Trace);
        }
    };
    builder.init();

    trace!("logger initialized - trace check");
    debug!("logger initialized - debug check");
    info!("logger initialized - info check");
    warn!("logger initialized - warn check");
    error!("logger initialized - error check");

    match matches.subcommand() {
        (SUBCOMMAND_CREATE_RELEASE, Some(args)) => {
            cmdlet::create_release::main(args);
        }
        (SUBCOMMAND_RUN, Some(args)) => {
            cmdlet::run::main(args);
        }
        _ => panic!("bad argument parse"),
    }
}

pub struct AppPreforkConfiguration {
    package_id: String,
    artifact: Executable,
    instance_id: Uuid,
    version: String,
    files: Vec<ServiceFileDescriptor>,
    extras: serde_json::Map<String, serde_json::Value>,
}

pub struct ServiceFileDescriptor {
    file: OwnedFd,
    direction: ServiceFileDirection,
    service_name: String,
    remote: FileDescriptorRemote,
}

fn bind_service(binder: &PublicServiceBinder) -> io::Result<OwnedFd> {
    match *binder {
        PublicServiceBinder::NativePortBinder(ref np) => bind::bind_tcp_socket(np),
        PublicServiceBinder::UnixDomainBinder(ref ub) => bind::bind_unix_socket(ub),
        PublicServiceBinder::WebServiceBinder(ref _ws) => {
            Err(io::Error::new(io::ErrorKind::Other, "unimplemented"))
        }
    }
}

struct ExecSomething {
    extras: ExecExtras,
    cfg: AppPreforkConfiguration,
}

fn reify_service_connections(
    dm: &DeploymentManifest,
    artifact_path: &str,
    approot: &Path,
) -> Result<Vec<ExecSomething>, Box<dyn StdError>> {
    let mut instances = HashMap::<Uuid, ExecSomething>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let artifact = if let Some(path) = dm.path_overrides.get(&component.package_id) {
            warn!(
                "because of override, trying to find package {:?} @ {}",
                component.package_id, path
            );
            load_artifact(&path)?
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
