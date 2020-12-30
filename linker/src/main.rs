use std::io;

use clap::{App, AppSettings, Arg};
use uuid::Uuid;
use tracing::{event, Level};
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;
use tracing_subscriber::FmtSubscriber;

use owned_fd::OwnedFd;
use yscloud_config_model::{
    AppConfiguration, FileDescriptorInfo, FileDescriptorRemote, PublicServiceBinder,
    ServiceFileDirection,
};

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
    let mut my_subscriber_builder = FmtSubscriber::builder();

    use self::cmdlet::{create_release, publish_artifact, run, start_daemon, unstable_setup_container};
    let app = App::new(CARGO_PKG_NAME)
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
        .subcommand(start_daemon::get_subcommand());

    #[cfg(target_os = "linux")]
    let app = app.subcommand(unstable_setup_container::get_subcommand());

    let matches = app.get_matches();

    let verbosity = matches.occurrences_of("v");
    let should_print_test_logging = 4 < verbosity;

    my_subscriber_builder = my_subscriber_builder.with_max_level(match verbosity {
        0 => TracingLevelFilter::ERROR,
        1 => TracingLevelFilter::WARN,
        2 => TracingLevelFilter::INFO,
        3 => TracingLevelFilter::DEBUG,
        _ => TracingLevelFilter::TRACE,
    });

    tracing::subscriber::set_global_default(my_subscriber_builder.finish())
        .expect("setting tracing default failed");

    if should_print_test_logging {
        print_test_logging();
    }

    let (sub_name, args) = matches.subcommand();
    let main_function = match sub_name {
        create_release::SUBCOMMAND_NAME => create_release::main,
        publish_artifact::SUBCOMMAND_NAME => publish_artifact::main,
        run::SUBCOMMAND_NAME => run::main,
        start_daemon::SUBCOMMAND_NAME => start_daemon::main,
        unstable_setup_container::SUBCOMMAND_NAME => unstable_setup_container::main,
        _ => panic!("bad argument parse"),
    };
    main_function(args.expect("subcommand args"));
}

pub struct AppPreforkConfiguration {
    deployment_name: String,
    package_id: String,
    artifact: Executable,
    instance_id: Uuid,
    version: String,
    files: Vec<ServiceFileDescriptor>,
    extras: serde_json::Value,
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


#[allow(clippy::cognitive_complexity)] // macro bug around event!()
fn print_test_logging() {
    event!(Level::TRACE, "logger initialized - trace check");
    event!(Level::DEBUG, "logger initialized - debug check");
    event!(Level::INFO, "logger initialized - info check");
    event!(Level::WARN, "logger initialized - warn check");
    event!(Level::ERROR, "logger initialized - error check");
}

