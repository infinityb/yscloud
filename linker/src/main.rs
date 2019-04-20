#![feature(never_type)]

use std::collections::{HashMap, VecDeque};
use std::error::Error as StdError;
use std::fs::File;
use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::unix::io::{AsRawFd, IntoRawFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use clap::{App, Arg, SubCommand};
use env_logger::Builder;
use log::{debug, error, info, log, trace, warn, LevelFilter};
use nix::sys::signal::{kill, Signal};
use nix::sys::socket::{
    bind, socket as nix_socket, socketpair as nix_socketpair, AddressFamily, InetAddr, SockAddr,
    SockFlag, SockProtocol, SockType,
};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

use registry_model::{
    permissions, AppConfiguration, ApplicationManifest, DeployedApplicationManifest,
    DeployedPublicService, DeploymentManifest, FileDescriptorInfo, FileDescriptorRemote,
    NativePortBinder, Protocol, PublicService, PublicServiceBinder, ServiceFileDirection,
    ServiceId, SideCarServiceInfo, SocketInfo, SocketMode, WebServiceBinder,
};
use semver::Version;
use serde_json::{self, json, json_internal};
use uuid::Uuid;

mod artifact;
pub mod platform;

pub use self::artifact::find_artifact;
pub use self::platform::{exec_artifact, ExecExtras, Executable};

fn json_assert_object(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(v) => v,
        _ => panic!("bad json value type"),
    }
}

fn demo() {
    mvp_deployment().unwrap();

    #[derive(Hash, PartialEq, Eq, Debug, Clone)]
    struct ApplicationRelease {
        package_id: String,
        version: Version,
    };

    let mut registry = HashMap::<String, Vec<ApplicationManifest>>::new();
    registry.insert(
        "org.yshi.sfshost".into(),
        vec![ApplicationManifest {
            package_id: "org.yshi.sfshost".into(),
            version: Version::parse("1.0.0").unwrap(),
            provided_remote_services: vec!["org.yshi.sfshost.https".into()],
            provided_local_services: vec![],
            required_remote_services: vec![],
            required_local_services: vec![
                "org.yshi.acmesidecar.v1.AcmeSidecar".into(),
                "org.yshi.log_target.v1.LogTarget".into(),
            ],
            permissions: vec![permissions::UNCONSTRAINED],
        }],
    );
    registry.insert(
        "org.yshi.log-aggregator".into(),
        vec![ApplicationManifest {
            package_id: "org.yshi.log-aggregator".into(),
            version: Version::parse("1.0.1").unwrap(),
            provided_remote_services: vec![],
            provided_local_services: vec!["org.yshi.log_target.v1.LogTarget".into()],
            required_remote_services: vec!["org.yshi.log-storage.v1.LogReceiver".into()],
            required_local_services: vec![],
            permissions: vec![permissions::UNCONSTRAINED],
        }],
    );
    registry.insert(
        "org.yshi.acmesidecar".into(),
        vec![ApplicationManifest {
            package_id: "org.yshi.acmesidecar".into(),
            version: Version::parse("1.0.0").unwrap(),
            provided_remote_services: vec![],
            provided_local_services: vec!["org.yshi.acmesidecar.v1.AcmeSidecar".into()],
            required_remote_services: vec![],
            required_local_services: vec!["org.yshi.log_target.v1.LogTarget".into()],
            permissions: vec![permissions::UNCONSTRAINED],
        }],
    );

    let x = serde_json::to_string_pretty(&registry).unwrap();
    println!("{}", x);

    struct DeploymentOptions {
        deployment_name: String,
        public_services: Vec<PublicService>,
        service_mapping: HashMap<String, String>,
    }

    fn generate_deployment(
        registry: &HashMap<String, Vec<ApplicationManifest>>,
        options: &DeploymentOptions,
    ) -> Result<DeploymentManifest, Box<StdError>> {
        // let services: Vec<ApplicationManifest> = Vec::new();

        let mut apps = HashMap::<&str, &ApplicationManifest>::new();

        let mut unresolved_services = VecDeque::<&str>::new();
        let mut deployed_public_services = Vec::new();

        for ps in &options.public_services {
            let unk_pkg = || format!("unknown public service {:?}", ps);

            let app_name = options
                .service_mapping
                .get(&ps.service_name)
                .ok_or_else(unk_pkg)?;
            let manifests = registry.get(app_name).ok_or_else(unk_pkg)?;

            let highest = manifests
                .iter()
                .max_by_key(|a| &a.version)
                .ok_or_else(unk_pkg)?;

            for rls in &highest.required_local_services {
                unresolved_services.push_back(rls);
            }

            deployed_public_services.push(DeployedPublicService {
                service_id: ServiceId {
                    package_id: app_name.clone(),
                    service_name: ps.service_name.clone(),
                },
                binder: ps.binder.clone(),
            })
        }

        while let Some(s) = unresolved_services.pop_front() {
            let unk_pkg = || format!("unknown public service {}", s);

            let app_name = options.service_mapping.get(s).ok_or_else(unk_pkg)?;
            let manifests = registry.get(app_name).ok_or_else(unk_pkg)?;

            let highest = manifests
                .iter()
                .max_by_key(|a| &a.version)
                .ok_or_else(unk_pkg)?;

            apps.insert(&highest.package_id, highest);

            for rls in &highest.required_local_services {
                unresolved_services.push_back(rls);
            }
        }

        println!("apps={:#?}", apps);

        let mut components = Vec::<DeployedApplicationManifest>::new();

        Ok(DeploymentManifest {
            deployment_name: options.deployment_name.clone(),
            public_services: deployed_public_services,
            components,
        })
    }

    let deployment = generate_deployment(
        &registry,
        &DeploymentOptions {
            deployment_name: "shortfiles.staceyell.com".into(),
            public_services: vec![PublicService {
                service_name: "org.yshi.sfshost.https".into(),
                binder: PublicServiceBinder::WebServiceBinder(WebServiceBinder {
                    hostname: "shortfiles.staceyell.com".into(),
                }),
            }],
            service_mapping: {
                let mut map = HashMap::new();

                map.insert("org.yshi.sfshost.https".into(), "org.yshi.sfshost".into());
                map.insert(
                    "org.yshi.acmesidecar".into(),
                    "org.yshi.acmesidecar.v1.AcmeSidecar".into(),
                );
                map.insert(
                    "org.yshi.log-storage".into(),
                    "org.yshi.log-storage.v1.LogReceiver".into(),
                );
                map.insert(
                    "org.yshi.log-aggregator".into(),
                    "org.yshi.log_target.v1.LogTarget".into(),
                );

                map
            },
        },
    )
    .unwrap();

    let x = serde_json::to_string_pretty(&deployment).unwrap();
    println!("{}", x);

    let target_deployment_manifest = DeploymentManifest {
        deployment_name: "shortfiles.staceyell.com".into(),
        public_services: vec![DeployedPublicService {
            service_id: ServiceId {
                package_id: "org.yshi.sfshost".into(),
                service_name: "org.yshi.sfshost.https".into(),
            },
            binder: PublicServiceBinder::WebServiceBinder(WebServiceBinder {
                hostname: "shortfiles.staceyell.com".into(),
            }),
        }],
        components: vec![
            DeployedApplicationManifest {
                package_id: "org.yshi.sfshost".into(),
                version: Version::parse("1.0.55").unwrap(),
                permissions: vec![
                    // transloading feature.
                    permissions::NETWORK_OUTGOING_TCP,
                    // to persist user media
                    permissions::DISK_APP_LOCAL_STORAGE,
                ],
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.sfshost.https".into()],
                required_remote_services: vec![],
                required_local_services: vec![
                    ServiceId {
                        package_id: "org.yshi.log-aggregator".into(),
                        service_name: "org.yshi.log-aggregator.log-receiver".into(),
                    },
                    ServiceId {
                        package_id: "org.yshi.acmehelper".into(),
                        service_name: "org.yshi.tls.certificate-issuer".into(),
                    },
                ],
                extras: json_assert_object(json!({
                    "vhosts": {
                        "localhost": {
                            "directory": "",
                            "password": "foobar",
                        }
                    }
                })),
            },
            DeployedApplicationManifest {
                package_id: "org.yshi.acmehelper".into(),
                version: Version::parse("1.0.1").unwrap(),
                permissions: vec![
                    // to persist crypto information
                    permissions::DISK_APP_LOCAL_STORAGE,
                ],
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.tls.certificate-issuer".into()],
                required_remote_services: vec![],
                required_local_services: vec![ServiceId {
                    package_id: "org.yshi.log-aggregator".into(),
                    service_name: "org.yshi.log-aggregator.log-receiver".into(),
                }],
                extras: Default::default(),
            },
            DeployedApplicationManifest {
                package_id: "org.yshi.log-aggregator".into(),
                version: Version::parse("1.0.1").unwrap(),
                permissions: vec![],
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.log-aggregator.log-receiver".into()],
                required_remote_services: vec!["org.yshi.log-storage.log-receiver".into()],
                required_local_services: vec![],
                extras: Default::default(),
            },
        ],
    };

    let x = serde_json::to_string_pretty(&target_deployment_manifest).unwrap();
    println!("{}", x);

    // let dag = service_dag(&target_deployment_manifest).unwrap();
    // let x = serde_json::to_string_pretty(&dag).unwrap();
    // println!("{}", x);

    return;
}

fn main() {
    const CREATE_RELEASE_SUBCOMMAND: &str = "create-release";
    const RUN_SUBCOMMAND: &str = "run";
    const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
    const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

    let create_release = SubCommand::with_name(CREATE_RELEASE_SUBCOMMAND)
        .version(CARGO_PKG_VERSION)
        .about("press a release")
        .arg(
            Arg::with_name("binary")
                .long("binary")
                .value_name("FILE")
                .help("The binary to publish")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("manifest")
                .long("manifest")
                .value_name("FILE")
                .help("The manifest to publish")
                .required(true)
                .takes_value(true),
        );

    let run = SubCommand::with_name(RUN_SUBCOMMAND)
        .version(CARGO_PKG_VERSION)
        .about("link and run a deployment")
        .arg(
            Arg::with_name("manifest")
                .long("manifest")
                .value_name("FILE")
                .help("The deployment manifest to link up and run")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("artifacts")
                .long("artifacts")
                .value_name("DIR")
                .help("an artifact directory containing dependencies of the manifest")
                .required(true)
                .takes_value(true),
        );

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
        .subcommand(create_release)
        .subcommand(run)
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

    if let Some(matches) = matches.subcommand_matches(CREATE_RELEASE_SUBCOMMAND) {
        main_create_release(matches);
        return;
    }
    if let Some(matches) = matches.subcommand_matches(RUN_SUBCOMMAND) {
        main_run(matches);
        return;
    }
}

fn main_create_release(matches: &clap::ArgMatches) {
    let binary_path = matches.value_of("binary").unwrap();
    trace!("got binary: {:?}", binary_path);

    let manifest_path = matches.value_of("manifest").unwrap();
    trace!("got manifest: {:?}", manifest_path);
}

fn main_run(matches: &clap::ArgMatches) {
    let artifacts = matches.value_of("artifacts").unwrap();
    trace!("got artifacts: {:?}", artifacts);

    let manifest_path = matches.value_of("manifest").unwrap();
    trace!("got manifest: {:?}", manifest_path);

    let target_deployment_manifest = DeploymentManifest {
        deployment_name: "sfshost.localhost.yshi.com".into(),
        public_services: vec![
            DeployedPublicService {
                service_id: ServiceId {
                    package_id: "org.yshi.sfshost".into(),
                    service_name: "org.yshi.sfshost.https".into(),
                },
                binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
                    bind_address: "::".into(),
                    port: 1443,
                }),
            },
            // DeployedPublicService {
            //     service_id: ServiceId {
            //         package_id: "org.yshi.sfshost".into(),
            //         service_name: "org.yshi.sfshost.http".into(),
            //     },
            //     binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
            //         bind_address: "::".into(),
            //         port: 1080,
            //     }),
            // },
        ],
        components: vec![
            DeployedApplicationManifest {
                package_id: "org.yshi.sfshost".into(),
                version: Version::parse("1.0.55").unwrap(),
                permissions: vec![permissions::UNCONSTRAINED],
                provided_remote_services: vec!["org.yshi.sfshost.https".into()],
                provided_local_services: vec![],
                required_remote_services: vec![],
                required_local_services: vec![
                    ServiceId {
                        package_id: "org.yshi.file-logger".into(),
                        service_name: "org.yshi.log_target.v1.LogTarget".into(),
                    },
                    ServiceId {
                        package_id: "org.yshi.selfsigned-issuer".into(),
                        service_name: "org.yshi.certificate_issuer.v1.CertificateIssuer".into(),
                    },
                ],
                extras: json_assert_object(json!({
                    "vhosts": {
                        "localhost:1443": {
                            "directory": "./test",
                            "password": "foobar",
                        },
                        "localhost": {
                            "directory": "./test",
                            "password": "foobar",
                        }
                    }
                })),
            },
            DeployedApplicationManifest {
                package_id: "org.yshi.selfsigned-issuer".into(),
                version: Version::parse("0.1.0").unwrap(),
                permissions: vec![permissions::UNCONSTRAINED],
                provided_remote_services: vec![],
                provided_local_services: vec![
                    "org.yshi.certificate_issuer.v1.CertificateIssuer".into()
                ],
                required_remote_services: vec![],
                required_local_services: vec![ServiceId {
                    package_id: "org.yshi.file-logger".into(),
                    service_name: "org.yshi.log_target.v1.LogTarget".into(),
                }],
                extras: Default::default(),
            },
            DeployedApplicationManifest {
                package_id: "org.yshi.file-logger".into(),
                version: Version::parse("1.0.1").unwrap(),
                permissions: vec![permissions::UNCONSTRAINED],
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.log_target.v1.LogTarget".into()],
                required_remote_services: vec![],
                required_local_services: vec![],
                extras: Default::default(),
            },
        ],
    };

    let x = serde_json::to_string_pretty(&target_deployment_manifest).unwrap();
    println!("{}", x);

    let reified = reify_service_connections(&target_deployment_manifest, artifacts).unwrap();

    #[derive(Clone)]
    struct ChildInfo {
        package_name: String,
        sent_kill: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    }

    let mut extras = ExecExtras::builder();

    #[cfg(target_os = "linux")]
    {
        extras.set_user("nobody").unwrap();
        extras.set_group("nogroup").unwrap();
    }

    let extras = extras.build();

    let mut pids = HashMap::<Pid, ChildInfo>::new();
    for a in reified {
        let package_id = a.package_id.clone();
        let child = exec_artifact(&extras, a).unwrap();

        debug!("made child: {} {:?}", package_id, child);

        pids.insert(
            child,
            ChildInfo {
                package_name: package_id,
                sent_kill: Arc::new(AtomicBool::new(false)),
                running: Arc::new(AtomicBool::new(true)),
            },
        );
    }

    fn kill_all(pids: &HashMap<Pid, ChildInfo>, second_kill: bool) {
        for (pid, info) in pids {
            if info.running.load(Ordering::SeqCst)
                && (!info.sent_kill.load(Ordering::SeqCst) || second_kill)
            {
                if !second_kill {
                    info.sent_kill.store(true, Ordering::SeqCst);
                }
                info!("sending {} ({}) SIGINT", pid, info.package_name);
                let _ = kill(*pid, Signal::SIGINT);
            }
        }
    }

    use signal_hook::iterator::Signals;
    let signals = Signals::new(&[signal_hook::SIGINT]).unwrap();

    let kill_targets = pids.clone();
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            signals.close();
            info!("got {}, signaling to children to terminate", sig);
            kill_all(&kill_targets, false);
        }
        if let Some(sig) = signals.forever().next() {
            signals.close();
            info!(
                "got {}, signaling to children to terminate (2nd attempt)",
                sig
            );
            kill_all(&kill_targets, true);
        }
    });

    let mut remaining_children = pids.len();
    while 0 < remaining_children {
        match waitpid(None, None) {
            Ok(WaitStatus::Exited(pid, exit_code)) => {
                let child_info = &pids[&pid];
                info!("child {} exited {}", child_info.package_name, exit_code);
                remaining_children -= 1;
                child_info.running.store(false, Ordering::SeqCst);
                kill_all(&pids, false);
            }
            // literally why.
            Ok(WaitStatus::Signaled(pid, sig, _cored)) => {
                let child_info = &pids[&pid];
                info!(
                    "child {} exited via signal {}",
                    child_info.package_name, sig
                );
                remaining_children -= 1;
                child_info.running.store(false, Ordering::SeqCst);
                kill_all(&pids, false);
            }
            Ok(ws) => {
                warn!("waitpid got an unexpected {:?}", ws);
            }
            Err(err) => {
                panic!("waitpid err {}", err);
            }
        };
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

pub struct OwnedFd(RawFd);

impl AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.0);
    }
}

impl From<File> for OwnedFd {
    fn from(fd: File) -> OwnedFd {
        OwnedFd(fd.into_raw_fd())
    }
}

pub fn socketpair() -> io::Result<(OwnedFd, OwnedFd)> {
    let (left, right) = nix_socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::empty(),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok((OwnedFd(left), OwnedFd(right)))
}

pub struct ServiceFileDescriptor {
    file: OwnedFd,
    direction: ServiceFileDirection,
    service_name: String,
    remote: FileDescriptorRemote,
}

fn bind_tcp_socket(np: &NativePortBinder) -> io::Result<OwnedFd> {
    let fd = nix_socket(
        AddressFamily::Inet6,
        SockType::Stream,
        SockFlag::empty(),
        SockProtocol::Tcp,
    )
    .map(OwnedFd)
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let ip_addr = np
        .bind_address
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let saddr = match ip_addr {
        IpAddr::V4(a) => SocketAddr::V4(SocketAddrV4::new(a, np.port)),
        IpAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(a, np.port, 0, 0)),
    };

    bind(fd.0, &SockAddr::Inet(InetAddr::from_std(&saddr)))
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    ::nix::sys::socket::listen(fd.0, 10).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(fd)
}

fn bind_service(binder: &PublicServiceBinder) -> io::Result<OwnedFd> {
    match *binder {
        PublicServiceBinder::NativePortBinder(ref np) => bind_tcp_socket(np),
        PublicServiceBinder::WebServiceBinder(ref _ws) => {
            Err(io::Error::new(io::ErrorKind::Other, "unimplemented"))
        }
    }
}

fn reify_service_connections(
    dm: &DeploymentManifest,
    artifact_path: &str,
) -> Result<Vec<AppPreforkConfiguration>, Box<StdError>> {
    let mut instances = HashMap::<Uuid, AppPreforkConfiguration>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let artifact = find_artifact(artifact_path, &component.package_id, &component.version)?;

        let instance_id = Uuid::new_v4();

        instances.insert(
            instance_id,
            AppPreforkConfiguration {
                package_id: component.package_id.clone(),
                artifact,
                version: format!("{}", component.version),
                instance_id,
                files: Default::default(),
                extras: component.extras.clone(),
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
            ps.service_id.service_name, ps.binder, service_sock.0,
        );

        instance.files.push(ServiceFileDescriptor {
            file: service_sock,
            direction: ServiceFileDirection::Serving,
            service_name: ps.service_id.service_name.clone(),
            remote: FileDescriptorRemote::Socket(SocketInfo {
                mode: SocketMode::Listening,
                protocol: Protocol::Stream,
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

            let (local_sock, remote_sock) = socketpair()?;

            {
                let local_instance = instances.get_mut(local_instance_id).ok_or_else(|| {
                    format!("internal error: unknown instance {:?}", local_instance_id)
                })?;

                local_instance.files.push(ServiceFileDescriptor {
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

                remote_instance.files.push(ServiceFileDescriptor {
                    file: remote_sock,
                    direction: ServiceFileDirection::Serving,
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

fn mvp_deployment() -> Result<(), Box<StdError>> {
    let mut mvp_deployment_manifest = DeploymentManifest {
        deployment_name: "aibi.yshi.org".into(),
        public_services: vec![
            DeployedPublicService {
                service_id: ServiceId {
                    package_id: "org.yshi.sfshost".into(),
                    service_name: "org.yshi.sfshost.https".into(),
                },
                binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
                    bind_address: "::".into(),
                    port: 443,
                }),
            },
            DeployedPublicService {
                service_id: ServiceId {
                    package_id: "org.yshi.sfshost".into(),
                    service_name: "org.yshi.sfshost.http".into(),
                },
                binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
                    bind_address: "::".into(),
                    port: 80,
                }),
            },
        ],
        components: vec![DeployedApplicationManifest {
            package_id: "org.yshi.sfshost".into(),
            version: Version::parse("1.0.55").unwrap(),
            permissions: vec![
                // we don't have permission support yet, so we must allow
                // unconstrained access to the system.
                permissions::UNCONSTRAINED,
            ],
            provided_local_services: vec![
                "org.yshi.sfshost.http".into(),
                "org.yshi.sfshost.https".into(),
            ],
            provided_remote_services: vec![],
            required_remote_services: vec![],
            required_local_services: vec![],
            extras: Default::default(),
        }],
    };

    // checks(&mvp_deployment_manifest).unwrap();
    let x = serde_json::to_string_pretty(&mvp_deployment_manifest).unwrap();
    println!("{}", x);

    Ok(())
}
