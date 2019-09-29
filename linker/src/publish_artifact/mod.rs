use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;

use digest::FixedOutput;
use semver::Version;
use serde::Serialize;
use sha2::Sha256;
use sha3::Keccak512;

pub fn start(cfg: Config) {
    eprintln!("config: {:#?}", cfg);

    let mut artifact_file = File::open(cfg.artifact).expect("artifact not found");

    let mut sha256_state = Sha256::default();
    let mut keccak512_state = Keccak512::default();

    io::copy(&mut artifact_file, &mut sha256_state).unwrap();
    let mut sha256_scratch = [0; 256 / 8 * 2];
    let sha256_str = tohex(&mut sha256_scratch, &sha256_state.fixed_result()).unwrap();

    artifact_file.seek(SeekFrom::Start(0)).unwrap();
    io::copy(&mut artifact_file, &mut keccak512_state).unwrap();
    let mut keccak512_scratch = [0; 512 / 8 * 2];
    let keccak512_str = tohex(&mut keccak512_scratch, &keccak512_state.fixed_result()).unwrap();

    let metadata = artifact_file.metadata().expect("metadata fetch failure");

    let serialized = serde_json::to_string(&ArtifactEntry {
        file_size: metadata.len(),
        sha256: sha256_str,
        keccak512: keccak512_str,
    })
    .unwrap();

    eprintln!("{} {}", cfg.host_triple, serialized);

    unimplemented!();
}

#[derive(Clone, Debug)]
pub struct Config {
    pub registry: PathBuf,
    pub package_id: String,
    pub version: Version,
    pub host_triple: String,
    pub artifact: PathBuf,
}

fn tohex<'sc>(scratch: &'sc mut [u8], data: &[u8]) -> Option<&'sc str> {
    static HEX_CHARS: &[u8] = b"0123456789abcdef";
    let hex_length = data.len() * 2;

    if scratch.len() < hex_length {
        return None;
    }

    let mut scratch_iter = scratch.iter_mut();
    for by in data {
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by >> 4)];
        let next = scratch_iter.next()?;
        *next = HEX_CHARS[usize::from(*by & 0x0F)];
    }

    drop(scratch_iter);
    Some(::std::str::from_utf8(&scratch[..hex_length]).unwrap())
}

#[derive(Serialize)]
struct ArtifactEntry<'a> {
    file_size: u64,
    sha256: &'a str,
    keccak512: &'a str,
}
