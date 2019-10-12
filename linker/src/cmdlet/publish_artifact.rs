use std::path::Path;

use clap::{App, Arg, SubCommand};
use log::trace;
use semver::Version;

use super::common;
use crate::publish_artifact::{start, Config};
use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "publish-artifact";

fn version_validator(v: String) -> Result<(), String> {
    match Version::parse(&v) {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("{}", err)),
    }
}

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("publish an artifact to a registry")
        .arg(common::registry())
        .arg(
            Arg::with_name("package-id")
                .long("package-id")
                .value_name("PACKAGE_ID")
                .help("the package ID of the artifact")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("version")
                .long("version")
                .value_name("VERSION")
                .help("the version of the artifact")
                .required(true)
                .takes_value(true)
                .validator(version_validator),
        )
        .arg(
            Arg::with_name("artifact")
                .long("artifact")
                .value_name("FILE")
                .help("the binary of the artifact to publish")
                .required(true)
                .takes_value(true)
                .validator_os(|_| Ok(())),
        )
        .arg(
            Arg::with_name("host-triple")
                .long("host-triple")
                .value_name("TRIPLE")
                .help("the host triple of the artifact")
                .required(true)
                .takes_value(true),
        )
}

pub fn main(matches: &clap::ArgMatches) {
    let registry = matches.value_of_os("registry").unwrap();
    let registry = Path::new(registry).to_owned();
    trace!("got registry: {}", registry.display());

    let package_id = matches.value_of("package-id").unwrap().to_string();
    trace!("got package-id: {:?}", package_id);

    let version = matches.value_of("version").unwrap().to_string();
    let version = Version::parse(&version).unwrap();
    trace!("got version: {:?}", version);

    let artifact = matches.value_of_os("artifact").unwrap();
    let artifact = Path::new(artifact).to_owned();
    trace!("got artifact: {}", artifact.display());

    let host_triple = matches.value_of("host-triple").unwrap().to_string();
    trace!("got host-triple: {:?}", host_triple);

    start(Config {
        registry,
        package_id,
        version,
        artifact,
        host_triple,
    });
}
