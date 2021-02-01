use std::collections::HashMap;
use std::path::{PathBuf, Path};
use std::fs::File;
use std::io;
use std::process::Command;

use clap::{App, Arg, SubCommand};
use tracing::{event, Level};

use super::common;
use crate::start_daemon::{start, Config};
use crate::CARGO_PKG_VERSION;

pub const SUBCOMMAND_NAME: &str = "setup-container";

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("unimplemented")
        .arg(
            Arg::with_name("image")
                .long("image")
                .value_name("PATH")
                .help("the code archive")
                .required(true)
                .takes_value(true)
                .validator_os(|_| Ok(())),
        )
        .arg(
            Arg::with_name("workdir")
                .long("workdir")
                .value_name("PATH")
                .help("the working directory")
                .required(true)
                .takes_value(true)
                .validator_os(|_| Ok(())),
        )
        .arg(
            Arg::with_name("persist")
                .long("persist")
                .value_name("PATH")
                .help("the working directory")
                .required(true)
                .takes_value(true)
                .validator_os(|_| Ok(())),
        )
        .arg(common::artifact_override())
}

#[cfg(not(target_os = "linux"))]
pub fn main(matches: &clap::ArgMatches) {
    eprintln!("Linux only");
    panic!();
}

#[cfg(target_os = "linux")]
pub fn main(matches: &clap::ArgMatches) {
    use crate::platform::container::{mount_nix_squashfs, Config as ContainerConfig};

    let image = matches.value_of_os("image").unwrap();
    let image = Path::new(image).to_owned();
    let workdir = matches.value_of_os("workdir").unwrap();
    let workdir = Path::new(workdir).to_owned();
    let persist = matches.value_of_os("persist").unwrap();
    let persist = Path::new(persist).to_owned();

    mount_nix_squashfs(&workdir, &ContainerConfig {
        persistence_path: persist,
        code_archive_path: image,
        ephemeral_storage_kilobytes: 0,
        enable_proc: false,
        enable_dev: false,
        extra_mounts: Vec::new(),
    }).unwrap();

    Command::new("/nix/entrypoint")
        .args(&["--nofork", "--nopid", "--runasroot", "--config", "/persist/inspircd.config"])
        .spawn().unwrap()
        .wait().unwrap();
}