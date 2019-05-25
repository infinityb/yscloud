#![feature(never_type)]

use std::collections::{HashMap, VecDeque};
use std::error::Error as StdError;
use std::fs::File;
use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::unix::io::{FromRawFd, AsRawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use clap::{App, Arg, SubCommand};
use env_logger::Builder;
use log::{debug, error, info, log, trace, warn, LevelFilter};
use nix::sys::signal::{kill, Signal};
use nix::sys::socket::{
    setsockopt, sockopt::ReusePort,
    bind, socket as nix_socket, AddressFamily, InetAddr, SockAddr,
    SockFlag, SockProtocol, SockType, UnixAddr,
};
use nix::sys::stat::{fchmodat, FchmodatFlags, Mode};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{unlink, Pid};

use sockets::{OwnedFd, socketpair_raw};
use yscloud_config_model::{
    permissions, AppConfiguration, ApplicationManifest, DeployedApplicationManifest,
    DeployedPublicService, DeploymentManifest, FileDescriptorInfo, FileDescriptorRemote,
    NativePortBinder, Protocol, PublicService, PublicServiceBinder, Sandbox, ServiceFileDirection,
    ServiceId, SideCarServiceInfo, SocketInfo, SocketMode, UnixDomainBinder, WebServiceBinder,
};
use semver::Version;
use serde_json::{self, json, json_internal};
use uuid::Uuid;


mod artifact;
pub mod platform;

pub use self::artifact::find_artifact;
pub use self::platform::{exec_artifact, ExecExtras, ExecExtrasBuilder, Executable};

fn json_assert_object(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(v) => v,
        _ => panic!("bad json value type"),
    }
}

#[allow(dead_code)]
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
            sandbox: Sandbox::Unconfined,
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
            sandbox: Sandbox::Unconfined,
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
            sandbox: Sandbox::Unconfined,
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

        let components = Vec::<DeployedApplicationManifest>::new();

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
                sandbox: Sandbox::PermissionSet(vec![
                    // transloading feature.
                    permissions::NETWORK_OUTGOING_TCP,
                    // to persist user media
                    permissions::DISK_APP_LOCAL_STORAGE,
                ]),
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
                sandbox: Sandbox::PermissionSet(vec![
                    // to persist crypto information
                    permissions::DISK_APP_LOCAL_STORAGE,
                ]),
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
                sandbox: Sandbox::PermissionSet(vec![]),
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
    const EXPORT_MANIFEST_SUBCOMMAND: &str = "export-manifest";

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

    let export_manifest = SubCommand::with_name(EXPORT_MANIFEST_SUBCOMMAND)
        .version(CARGO_PKG_VERSION)
        .about("export a deployment manifest")
        .arg(
            Arg::with_name("name")
                .long("name")
                .value_name("name")
                .help("the name of the deployment manifest")
                .required(true)
                .takes_value(true),
        );

    let run = SubCommand::with_name(RUN_SUBCOMMAND)
        .version(CARGO_PKG_VERSION)
        .about("link and run a deployment")
        .arg(
            Arg::with_name("approot")
                .long("approot")
                .value_name("DIR")
                .help("an application state directory root")
                .required(true)
                .takes_value(true),
        )
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
        .subcommand(export_manifest)
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
    if let Some(matches) = matches.subcommand_matches(EXPORT_MANIFEST_SUBCOMMAND) {
        main_export_manifest(matches);
        return;
    }
}

fn sfshost_deployment_manifest() -> DeploymentManifest {
    DeploymentManifest {
        deployment_name: "sfshost.localhost.yshi.com".into(),
        public_services: vec![DeployedPublicService {
            service_id: ServiceId {
                package_id: "org.yshi.sfshost".into(),
                service_name: "org.yshi.sfshost.https".into(),
            },
            binder: PublicServiceBinder::NativePortBinder(NativePortBinder {
                bind_address: "::".into(),
                port: 1443,
                start_listen: true,
            }),
        }],
        components: vec![
            DeployedApplicationManifest {
                package_id: "org.yshi.sfshost".into(),
                version: Version::parse("1.0.55").unwrap(),
                sandbox: Sandbox::Unconfined,
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
                sandbox: Sandbox::Unconfined,
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
                sandbox: Sandbox::Unconfined,
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.log_target.v1.LogTarget".into()],
                required_remote_services: vec![],
                required_local_services: vec![],
                extras: Default::default(),
            },
        ],
    }
}

fn staticserver_deployment_manifest() -> DeploymentManifest {
    DeploymentManifest {
        deployment_name: "staceyell.com".into(),
        public_services: vec![
            DeployedPublicService {
                service_id: ServiceId {
                    package_id: "org.yshi.staticserver".into(),
                    service_name: "org.yshi.staticserver.https".into(),
                },
                binder: PublicServiceBinder::UnixDomainBinder(UnixDomainBinder {
                    path: Path::new("/var/run/https.staceyell.com").into(),
                    start_listen: true,
                }),
            },
            DeployedPublicService {
                service_id: ServiceId {
                    package_id: "org.yshi.staticserver".into(),
                    service_name: "org.yshi.staticserver.http".into(),
                },
                binder: PublicServiceBinder::UnixDomainBinder(UnixDomainBinder {
                    path: Path::new("/var/run/http.staceyell.com").into(),
                    start_listen: true,
                }),
            },
        ],
        components: vec![
            DeployedApplicationManifest {
                package_id: "org.yshi.staticserver".into(),
                version: Version::parse("1.0.2").unwrap(),
                sandbox: Sandbox::UnixUserConfinement(
                    "staceyell-com-serv".into(),
                    "staceyell-com-serv".into(),
                ),
                provided_remote_services: vec![
                    "org.yshi.staticserver.http".into(),
                    "org.yshi.staticserver.https".into(),
                ],
                provided_local_services: vec![],
                required_remote_services: vec![],
                required_local_services: vec![ServiceId {
                    package_id: "org.yshi.file-logger".into(),
                    service_name: "org.yshi.log_target.v1.LogTarget".into(),
                }],
                extras: json_assert_object(json!({
                    "acme_directory": ".",
                    "allowed_hostnames": [
                        "www.staceyell.com",
                        "staceyell.com"
                    ]
                })),
            },
            DeployedApplicationManifest {
                package_id: "org.yshi.file-logger".into(),
                version: Version::parse("1.0.1").unwrap(),
                sandbox: Sandbox::UnixUserConfinement(
                    "staceyell-com-log".into(),
                    "staceyell-com-log".into(),
                ),
                provided_remote_services: vec![],
                provided_local_services: vec!["org.yshi.log_target.v1.LogTarget".into()],
                required_remote_services: vec![],
                required_local_services: vec![],
                extras: Default::default(),
            },
        ],
    }
}

fn main_export_manifest(matches: &clap::ArgMatches) {
    const SFSHOST_MANIFEST_NAME: &str = "sfshost";
    const STATICSERVER_MANIFEST_NAME: &str = "staticserver";

    let target_deployment_manifest = match matches.value_of("name").unwrap() {
        SFSHOST_MANIFEST_NAME => sfshost_deployment_manifest(),
        STATICSERVER_MANIFEST_NAME => staticserver_deployment_manifest(),
        _ => unimplemented!(),
    };

    let x = serde_json::to_string_pretty(&target_deployment_manifest).unwrap();
    println!("{}", x);
}

fn main_create_release(matches: &clap::ArgMatches) {
    let binary_path = matches.value_of("binary").unwrap();
    trace!("got binary: {:?}", binary_path);

    let manifest_path = matches.value_of("manifest").unwrap();
    trace!("got manifest: {:?}", manifest_path);
}

fn main_run(matches: &clap::ArgMatches) {
    let approot = matches.value_of("approot").unwrap();
    let approot = Path::new(approot).to_owned();
    trace!("got approot: {}", approot.display());

    let artifacts = matches.value_of("artifacts").unwrap();
    trace!("got artifacts: {:?}", artifacts);

    let manifest_path = matches.value_of("manifest").unwrap();
    trace!("got manifest: {:?}", manifest_path);

    let rdr = File::open(&manifest_path).unwrap();
    let target_deployment_manifest = serde_json::from_reader::<_, DeploymentManifest>(rdr).unwrap();

    let reified =
        reify_service_connections(&target_deployment_manifest, artifacts, &approot).unwrap();

    #[derive(Clone)]
    struct ChildInfo {
        package_name: String,
        sent_kill: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    }

    let mut pids = HashMap::<Pid, ChildInfo>::new();
    for a in reified {
        let package_id = a.cfg.package_id.clone();
        let child = exec_artifact(&a.extras, a.cfg).unwrap();

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

pub struct ServiceFileDescriptor {
    file: OwnedFd,
    direction: ServiceFileDirection,
    service_name: String,
    remote: FileDescriptorRemote,
}

fn bind_tcp_socket(np: &NativePortBinder) -> io::Result<OwnedFd> {
    let fd: OwnedFd = nix_socket(
        AddressFamily::Inet6,
        SockType::Stream,
        SockFlag::empty(),
        SockProtocol::Tcp,
    )
    .map(|f| unsafe { FromRawFd::from_raw_fd(f) })
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let ip_addr = np
        .bind_address
        .parse()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let saddr = match ip_addr {
        IpAddr::V4(a) => SocketAddr::V4(SocketAddrV4::new(a, np.port)),
        IpAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(a, np.port, 0, 0)),
    };

    setsockopt(fd.as_raw_fd(), ReusePort, &true)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    bind(fd.as_raw_fd(), &SockAddr::Inet(InetAddr::from_std(&saddr)))
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    if np.start_listen {
        // 128 from rust stdlib
        ::nix::sys::socket::listen(fd.as_raw_fd(), 128)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    Ok(fd)
}

fn bind_unix_socket(ub: &UnixDomainBinder) -> io::Result<OwnedFd> {
    let fd: OwnedFd = nix_socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )
    .map(|f| unsafe { FromRawFd::from_raw_fd(f) })
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let addr = UnixAddr::new(&ub.path).unwrap();
    if let Err(err) = bind(fd.as_raw_fd(), &SockAddr::Unix(addr)) {
        if err == nix::Error::Sys(nix::errno::Errno::EADDRINUSE) {
            unlink(&ub.path).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        }
        bind(fd.as_raw_fd(), &SockAddr::Unix(addr))
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    if ub.start_listen {
        ::nix::sys::socket::listen(fd.as_raw_fd(), 10)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    fchmodat(
        None,
        &ub.path,
        Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
        FchmodatFlags::FollowSymlink,
    )
    .map_err(|e| {
        let msg = format!("fchmodat of {}: {}", ub.path.display(), e);
        io::Error::new(io::ErrorKind::Other, msg)
    })?;

    Ok(fd)
}

fn bind_service(binder: &PublicServiceBinder) -> io::Result<OwnedFd> {
    match *binder {
        PublicServiceBinder::NativePortBinder(ref np) => bind_tcp_socket(np),
        PublicServiceBinder::UnixDomainBinder(ref ub) => bind_unix_socket(ub),
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
) -> Result<Vec<ExecSomething>, Box<StdError>> {
    let mut instances = HashMap::<Uuid, ExecSomething>::new();
    let mut instance_components = HashMap::<Uuid, &DeployedApplicationManifest>::new();
    let mut instance_by_package = HashMap::<&str, Uuid>::new();

    for component in &dm.components {
        let artifact = find_artifact(artifact_path, &component.package_id, &component.version)?;

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
            ps.service_id.service_name, ps.binder, service_sock.as_raw_fd(),
        );

        instance.cfg.files.push(ServiceFileDescriptor {
            file: service_sock,
            direction: ServiceFileDirection::ServingListening,
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

#[allow(dead_code)]
fn mvp_deployment() -> Result<(), Box<StdError>> {
    let mvp_deployment_manifest = DeploymentManifest {
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
                    start_listen: true,
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
                    start_listen: true,
                }),
            },
        ],
        components: vec![DeployedApplicationManifest {
            package_id: "org.yshi.sfshost".into(),
            version: Version::parse("1.0.55").unwrap(),
            // we don't have permission support yet, so we must allow
            // unconfined access to the system.
            sandbox: Sandbox::Unconfined,
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
