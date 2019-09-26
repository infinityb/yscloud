use std::path::Path;

use clap::{App, Arg, SubCommand};
use log::trace;

use crate::daemon::{start, Config};
use crate::{CARGO_PKG_VERSION, SUBCOMMAND_START_DAEMON};

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_START_DAEMON)
        .version(CARGO_PKG_VERSION)
        .about("unimplemented")
        .arg(
            Arg::with_name("approot")
                .long("approot")
                .value_name("DIR")
                .help("an application state directory root")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("registry")
                .long("registry")
                .value_name("DIR")
                .help("an artifact registry directory containing metadata about the available artifacts")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("artifacts")
                .long("artifacts")
                .value_name("DIR-or-URL")
                .help("an artifact directory containing dependencies of the manifest")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("control-socket")
                .long("control-socket")
                .value_name("PATH")
                .help("path to bind the control socket")
                .required(true)
                .takes_value(true),
        )
}

pub fn main(matches: &clap::ArgMatches) {
    let approot = matches.value_of("approot").unwrap();
    let approot = Path::new(approot).to_owned();
    trace!("got approot: {}", approot.display());

    let artifacts = matches.value_of("artifacts").unwrap().to_string();
    trace!("got artifacts: {:?}", artifacts);

    let registry = matches.value_of("registry").unwrap().to_string();
    trace!("got registry: {:?}", registry);

    let control_socket = matches.value_of("control-socket").unwrap();
    let control_socket = Path::new(control_socket).to_owned();
    trace!("got control-socket: {:?}", control_socket.display());

    start(Config {
        approot,
        artifacts,
        registry,
        control_socket,
    });
}
