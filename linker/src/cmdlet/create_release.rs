use std::fs::File;
use std::path::PathBuf;

use clap::{App, Arg, SubCommand};
use log::{debug, trace};
use yscloud_config_model::{ApplicationDeployment, ApplicationManifest};

use super::common;
use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "create-release";

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("press a release")
        .arg(common::registry())
        .arg(
            Arg::with_name("deployment-templates")
                .long("deployment-templates")
                .value_name("DIR")
                .help("The deployment template directory to use")
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
    let deployment_name = matches.value_of("deployment-name").unwrap();
    trace!("got deployment name: {:?}", deployment_name);

    let mut deployment_tpl_path = PathBuf::from(matches.value_of("deployment-templates").unwrap());
    deployment_tpl_path.push(deployment_name);
    deployment_tpl_path.push("deployment.json");

    let mut rdr = File::open(&deployment_tpl_path).unwrap();
    let ad: ApplicationDeployment = serde_json::from_reader(&mut rdr).unwrap();

    println!("{:?}", ad);

    for package_id in found_package_ids(&ad) {
        let mut registry_path = PathBuf::from(matches.value_of("registry").unwrap());
        trace!("got registry: {:?}", registry_path);
        registry_path.push(package_id);
        registry_path.push("manifest.json");

        debug!("opening: {:?}", registry_path);
        let mut rdr = File::open(&registry_path).unwrap();
        let am: ApplicationManifest = serde_json::from_reader(&mut rdr).unwrap();

        println!("{:?}", am);
    }
}

fn found_package_ids(ad: &ApplicationDeployment) -> Vec<String> {
    ad.service_implementations.values().cloned().collect()
}
