use std::io;

use clap::{App, AppSettings, Arg};
use env_logger::Builder;
use log::{debug, error, info, trace, warn, LevelFilter};
use uuid::Uuid;

use sockets::OwnedFd;
use yscloud_config_model::{
    AppConfiguration, FileDescriptorInfo, FileDescriptorRemote, PublicServiceBinder,
    ServiceFileDirection,
};

pub enum Void {}

use crate::platform::{ExecExtras, Executable};

mod artifact;
mod bind;
mod cmdlet;
mod daemon;
pub mod platform;
mod registry;

const SUBCOMMAND_CREATE_RELEASE: &str = "create-release";
// const SUBCOMMAND_EXPORT_MANIFEST: &str = "export-manifest";
const SUBCOMMAND_RUN: &str = "run";
const SUBCOMMAND_START_DAEMON: &str = "start-daemon";

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    let matches = App::new("yscloud-linker")
        .version(CARGO_PKG_VERSION)
        .author("Stacey Ell <stacey.ell@gmail.com>")
        .about("Microservice/sidecar linker and privilege separation")
        .setting(AppSettings::SubcommandRequiredElseHelp)
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
        .subcommand(cmdlet::daemon::get_subcommand())
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
        (SUBCOMMAND_START_DAEMON, Some(args)) => {
            cmdlet::daemon::main(args);
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
