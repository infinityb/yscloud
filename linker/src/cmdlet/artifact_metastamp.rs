use std::path::Path;
use std::fs::File;
use std::io::{Read, BufReader};
use std::collections::BTreeMap;

use clap::{App, Arg, SubCommand};
use semver::Version;
use tracing::{event, Level};
use digest::{Digest, Update, FixedOutput};
use serde_json::Value;
use sha2::{Sha256, Sha512};
use sha3::{Sha3_512, Keccak512};

use super::common;
use crate::publish_artifact::{start, Config};
use crate::CARGO_PKG_VERSION;
use crate::util::hexify;

pub const SUBCOMMAND_NAME: &str = "artifact-metastamp";

fn version_validator(v: String) -> Result<(), String> {
    match Version::parse(&v) {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("{}", err)),
    }
}

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_NAME)
        .version(CARGO_PKG_VERSION)
        .about("create a metadata line for a file")
        .arg(
            Arg::with_name("artifact-path")
                .long("artifact-path")
                .value_name("ARTIFACT_PATH")
                .help("the path of the artifact")
                .required(true)
                .takes_value(true),
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
    let artifact_path = matches.value_of_os("artifact-path").unwrap();
    let artifact_path = Path::new(artifact_path).to_owned();
    event!(Level::TRACE, "got artifact_path: {}", artifact_path.display());

    let host_triple = matches.value_of("host-triple").unwrap().to_string();
    event!(Level::TRACE, "got host-triple: {:?}", host_triple);

    let mut sha2_256 = Sha256::new();
    let mut sha2_512 = Sha512::new();
    let mut sha3_512 = Sha3_512::new();
    let mut keccak_512 = Keccak512::new();

    let mut file = File::open(&artifact_path).unwrap();
    let mut buf = vec![0; 128 * 1024];
    let mut seen_bytes = 0;
    loop {
        let read_length = file.read(&mut buf[..]).unwrap();
        if read_length == 0 {
            break;
        }

        seen_bytes += read_length as u64;
        Update::update(&mut sha2_256, &buf[..read_length]);
        Update::update(&mut sha2_512, &buf[..read_length]);
        Update::update(&mut sha3_512, &buf[..read_length]);
        Update::update(&mut keccak_512, &buf[..read_length]);
        // sha2_512.update(&buf[..read_length]);
        // sha3_512.update(&buf[..read_length]);
        // keccak_512.update(&buf[..read_length]);
    }

    let mut scratch = [0; 128]; // 512 / 4
    let mut metadata: BTreeMap<&'static str, serde_json::Value> = Default::default();

    let hash = hexify(&mut scratch[..], &sha2_256.finalize_fixed()).unwrap();
    metadata.insert("sha256", Value::String(hash.to_string()));

    let hash = hexify(&mut scratch[..], &sha2_512.finalize_fixed()).unwrap();
    metadata.insert("sha512", Value::String(hash.to_string()));

    let hash = hexify(&mut scratch[..], &sha3_512.finalize_fixed()).unwrap();
    metadata.insert("sha3-512", Value::String(hash.to_string()));

    let hash = hexify(&mut scratch[..], &keccak_512.finalize_fixed()).unwrap();
    metadata.insert("keccak-512", Value::String(hash.to_string()));

    metadata.insert("size_bytes", Value::Number(seen_bytes.into()));

    let data = serde_json::to_string(&metadata).unwrap();
    println!("{}  {}", host_triple, data);
    // start(Config {
    //     registry,
    //     package_id,
    //     version,
    //     artifact,
    //     host_triple,
    // });
}
