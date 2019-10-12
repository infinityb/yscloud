use std::io;

use clap::{App, AppSettings, Arg};
use env_logger::Builder;
use log::{debug, error, info, trace, warn, LevelFilter};
use uuid::Uuid;

use owned_fd::OwnedFd;
use yscloud_config_model::{
    AppConfiguration, FileDescriptorInfo, FileDescriptorRemote, PublicServiceBinder,
    ServiceFileDirection,
};

use tracing::{event, Level};
use tracing_subscriber::FmtSubscriber;
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;

pub mod platform;

mod artifact;
mod bind;
mod cmdlet;
mod publish_artifact;
mod registry;
mod start_daemon;
mod util;

use crate::platform::{ExecExtras, Executable};

pub enum Void {}

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    let mut my_subscriber_builder = FmtSubscriber::builder()
        .with_ansi(true);

    use self::cmdlet::{create_release, publish_artifact, run, start_daemon};
    let matches = App::new(CARGO_PKG_NAME)
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
        .subcommand(create_release::get_subcommand())
        .subcommand(publish_artifact::get_subcommand())
        .subcommand(run::get_subcommand())
        .subcommand(start_daemon::get_subcommand())
        .get_matches();

    let mut print_test_logging = false;
    let mut builder = Builder::from_default_env();
    builder.default_format_module_path(true);

    let mut verbosity = matches.occurrences_of("v");
    let debugging = matches.occurrences_of("d");
    if verbosity < debugging {
        verbosity = debugging;
    }
    if 4 < verbosity {
        print_test_logging = true;
    }

    match debugging {
        0 => builder.filter_level(LevelFilter::Error),
        1 => builder.filter_level(LevelFilter::Warn),
        2 => builder.filter_level(LevelFilter::Info),
        3 => builder.filter_level(LevelFilter::Debug),
        _ => builder.filter_level(LevelFilter::Trace),
    };

    match verbosity {
        0 => builder.filter_module("yscloud_linker", LevelFilter::Error),
        1 => builder.filter_module("yscloud_linker", LevelFilter::Warn),
        2 => builder.filter_module("yscloud_linker", LevelFilter::Info),
        3 => builder.filter_module("yscloud_linker", LevelFilter::Debug),
        _ => builder.filter_module("yscloud_linker", LevelFilter::Trace),
    };

    match debugging {
        0 => my_subscriber_builder = my_subscriber_builder.with_max_level(TracingLevelFilter::ERROR),
        1 => my_subscriber_builder = my_subscriber_builder.with_max_level(TracingLevelFilter::WARN),
        2 => my_subscriber_builder = my_subscriber_builder.with_max_level(TracingLevelFilter::INFO),
        3 => my_subscriber_builder = my_subscriber_builder.with_max_level(TracingLevelFilter::DEBUG),
        _ => my_subscriber_builder = my_subscriber_builder.with_max_level(TracingLevelFilter::TRACE),
    };

    tracing::subscriber::set_global_default(my_subscriber_builder.finish())
        .expect("setting tracing default failed");

    builder.init();

    if print_test_logging {
        event!(Level::TRACE, "logger initialized - trace check");
        event!(Level::DEBUG, "logger initialized - debug check");
        event!(Level::INFO, "logger initialized - info check");
        event!(Level::WARN, "logger initialized - warn check");
        event!(Level::ERROR, "logger initialized - error check");

        trace!("logger initialized - trace check");
        debug!("logger initialized - debug check");
        info!("logger initialized - info check");
        warn!("logger initialized - warn check");
        error!("logger initialized - error check");
    }

    let (sub_name, args) = matches.subcommand();
    let main_function = match sub_name {
        create_release::SUBCOMMAND_NAME => create_release::main,
        publish_artifact::SUBCOMMAND_NAME => publish_artifact::main,
        run::SUBCOMMAND_NAME => run::main,
        start_daemon::SUBCOMMAND_NAME => start_daemon::main,
        _ => panic!("bad argument parse"),
    };
    main_function(args.expect("subcommand args"));
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

pub struct ExecSomething {
    extras: ExecExtras,
    cfg: AppPreforkConfiguration,
}
