use std::collections::HashMap;
use std::env::args;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read};
use std::process;
use std::str::FromStr;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use toml;

use registry_model::{
    AppConfig, AppDependencyPinned, AppDependencyReq, AppRelease, Manifest, Registry, RegistryMut,
};

fn create_manifest(registry: &Registry, config: &AppConfig) -> io::Result<Manifest> {
    let mut pins: Vec<AppDependencyPinned> = Vec::new();
    for (p_id, dep) in config.dependencies.iter() {
        let release = registry
            .find_best_release(p_id, &dep.version)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to find a release for {}:{}", p_id, dep.version),
                )
            })?;

        pins.push(release.clone());
    }

    Ok(Manifest {
        package_id: config.package_id.clone(),
        version: config.version.clone(),
        binary_sha: "".into(),
        version_pins: pins,
    })
}

fn suggest_version(
    registry: &Registry,
    config: &AppConfig,
    args: &ProgramArgs,
) -> io::Result<Version> {
    let mut ver = config.version.clone();
    match args.rel_type {
        ReleaseType::Major => (),
        ReleaseType::Minor => {
            ver.minor = 0;
            ver.patch = 0;
        }
        ReleaseType::Patch => {
            ver.patch = 0;
        }
    }

    let ver_req = format!("^{}.{}.{}", ver.major, ver.minor, ver.patch);
    let req = VersionReq::parse(&ver_req).unwrap();

    let release = registry
        .find_best_release(&config.package_id, &req)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to find a release for {}", ver_req),
            )
        })?;

    let mut ver = release.version.clone();
    match args.rel_type {
        ReleaseType::Major => ver.increment_major(),
        ReleaseType::Minor => ver.increment_minor(),
        ReleaseType::Patch => ver.increment_patch(),
    }
    Ok(ver)
}

// [package.metadata.yscloud]
// package_id = "org.yasashiisyndicate.ircc-daemon"

// [package.metadata.yscloud.dependencies]
// "org.yasashiisyndicate.acme-sidecar" = { "version" = "0.1" }

#[derive(Serialize, Deserialize, Debug)]
struct CargoInfo {
    package: CargoInfoPackage,
}

impl From<CargoInfo> for AppConfig {
    fn from(ci: CargoInfo) -> AppConfig {
        AppConfig {
            package_id: ci.package.metadata.yscloud.package_id,
            version: ci.package.version,
            dependencies: ci.package.metadata.yscloud.dependencies,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct CargoInfoPackage {
    metadata: CargoInfoPackageMetadata,
    version: Version,
}

#[derive(Serialize, Deserialize, Debug)]
struct CargoInfoPackageMetadata {
    yscloud: YsCloudInfo,
}

#[derive(Serialize, Deserialize, Debug)]
struct YsCloudInfo {
    package_id: String,
    #[serde(default)]
    dependencies: HashMap<String, AppDependencyReq>,
}

#[derive(Debug)]
enum ArgsError {
    Truncated,
    Invalid(Box<Error>),
    Unknown(String),
}

#[derive(Debug)]
enum ReleaseType {
    Patch,
    Minor,
    Major,
}

impl FromStr for ReleaseType {
    type Err = Box<Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "patch" => Ok(ReleaseType::Patch),
            "minor" => Ok(ReleaseType::Minor),
            "major" => Ok(ReleaseType::Major),
            _ => Err(format!("unknown release-type {}", s).into()),
        }
    }
}

#[derive(Debug)]
struct ProgramArgs {
    rel_type: ReleaseType,
    bare_args: Vec<String>,
}

fn parse_args<I>(args_iter: &mut I) -> Result<ProgramArgs, ArgsError>
where
    I: Iterator<Item = String>,
{
    let mut bare_args = Vec::new();

    let mut rel_type = ReleaseType::Minor;
    while let Some(arg) = args_iter.next() {
        match &arg[..] {
            "--release-type" => {
                rel_type = args_iter
                    .next()
                    .ok_or_else(|| ArgsError::Truncated)?
                    .parse()
                    .map_err(ArgsError::Invalid)?;
            }
            "--" => break,
            _ if arg.starts_with("--") => {
                return Err(ArgsError::Unknown(arg));
            }
            _ => {
                bare_args.push(arg);
            }
        }
    }
    bare_args.extend(args_iter);

    Ok(ProgramArgs {
        rel_type,
        bare_args,
    })
}

fn main() {
    let mut args_iter = args();
    let args = parse_args(&mut args_iter).unwrap();
    let mut registry = Registry::load_from("/Users/sell/dev/registry/com.staceyell").unwrap();
    let mut cloud = File::open("Cargo_test.toml").unwrap();
    let mut output = String::new();
    cloud.read_to_string(&mut output).unwrap();
    let app_config: CargoInfo = toml::from_str(&output).unwrap();
    let app_config: AppConfig = app_config.into();

    let manifest = create_manifest(&registry, &app_config).unwrap();
    let sugg_ver = suggest_version(&registry, &app_config, &args).unwrap();
    if sugg_ver != app_config.version {
        println!(
            "please update the Cargo version to {} and commit+tag it",
            sugg_ver
        );
        process::exit(1);
    }
    let mut reg_mut = RegistryMut::open("/Users/sell/dev/registry/com.staceyell").unwrap();

    let release = AppRelease {
        package_id: "org.yasashiisyndicate.ircc-daemon".into(),
        version: sugg_ver,
        manifest_sha: "ayaya".into(),
        binary_sha: "byaya".into(),
    };
    // reg_mut.add(release.clone()).unwrap();
    println!(
        "Added release commit record to registry: {}:{}",
        release.package_id, release.version
    );

    registry = Registry::load_from("/Users/sell/dev/registry/com.staceyell").unwrap();
}
