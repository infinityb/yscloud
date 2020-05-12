use std::collections::HashMap;
use std::path::Path;

use clap::{App, Arg, SubCommand};
use tracing::{event, Level};

use super::common;
use crate::start_daemon::{start, Config};
use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "start-daemon";

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("unimplemented")
        .arg(common::approot())
        .arg(common::artifacts())
        .arg(
            Arg::with_name("control-socket")
                .long("control-socket")
                .value_name("PATH")
                .help("path to bind the control socket")
                .required(true)
                .takes_value(true)
                .validator_os(|_| Ok(())),
        )
        .arg(common::artifact_override())
}

pub fn main(matches: &clap::ArgMatches) {
    let approot = matches.value_of_os("approot").unwrap();
    let approot = Path::new(approot).to_owned();
    event!(Level::TRACE, "got approot: {}", approot.display());

    let artifacts = matches.value_of("artifacts").unwrap().to_string();
    event!(Level::TRACE, "got artifacts: {:?}", artifacts);

    let control_socket = matches.value_of_os("control-socket").unwrap();
    let control_socket = Path::new(control_socket).to_owned();
    event!(
        Level::TRACE,
        "got control-socket: {:?}",
        control_socket.display()
    );

    let mut overrides: HashMap<String, String> = HashMap::new();
    if let Some(override_args) = matches.values_of_lossy("artifact-override") {
        for arg in override_args {
            let mut split_iter = arg.splitn(2, ':');
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

    start(Config {
        approot,
        artifacts,
        control_socket,
        overrides,
    });
}
