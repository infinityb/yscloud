
fn json_assert_object(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(v) => v,
        _ => panic!("bad json value type"),
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
                flags: Vec::new(),
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
        path_overrides: HashMap::new(),
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
                    flags: Vec::new(),
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
                    flags: Vec::new(),
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
        path_overrides: HashMap::new(),
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
    ) -> Result<DeploymentManifest, Box<dyn StdError>> {
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
            path_overrides: HashMap::new(),
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
                    flags: Vec::new(),
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
                flags: Vec::new(),
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
        path_overrides: HashMap::new(),
    };

    let x = serde_json::to_string_pretty(&target_deployment_manifest).unwrap();
    println!("{}", x);

    // let dag = service_dag(&target_deployment_manifest).unwrap();
    // let x = serde_json::to_string_pretty(&dag).unwrap();
    // println!("{}", x);

    return;
}


#[allow(dead_code)]
fn mvp_deployment() -> Result<(), Box<dyn StdError>> {
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
                    flags: Vec::new(),
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
                    flags: Vec::new(),
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
        path_overrides: HashMap::new(),
    };

    // checks(&mvp_deployment_manifest).unwrap();
    let x = serde_json::to_string_pretty(&mvp_deployment_manifest).unwrap();
    println!("{}", x);

    Ok(())
}

