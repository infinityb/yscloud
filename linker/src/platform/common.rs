use std::io;

use tracing::{event, Level};
use digest::{Digest, FixedOutput};
use sha2::{Sha256, Sha512};
use sha3::{Keccak512, Sha3_512};

#[derive(Copy, Clone, Debug)]
pub enum ExecutableFactoryHasher {
    Sha256,
    Sha512,
    Sha3_512,
    Keccak512,
}

pub struct ExecutableFactoryCommon {
    has_written: bool,
    sha256_state: Option<Sha256>,
    sha512_state: Option<Sha512>,
    sha3_512_state: Option<Sha3_512>,
    keccak512_state: Option<Keccak512>,
}

impl Default for ExecutableFactoryCommon {
    fn default() -> ExecutableFactoryCommon {
        ExecutableFactoryCommon {
            has_written: false,
            sha256_state: None,
            sha512_state: None,
            sha3_512_state: None,
            keccak512_state: None,
        }
    }
}

impl ExecutableFactoryCommon {
    /// panics if data has been written
    pub fn enable_hasher(&mut self, h: ExecutableFactoryHasher) {
        if self.has_written {
            panic!("can't add hasher after data has been written");
        }

        match h {
            ExecutableFactoryHasher::Sha256 => {
                self.sha256_state = Some(Default::default());
            }
            ExecutableFactoryHasher::Sha512 => {
                self.sha512_state = Some(Default::default());
            }
            ExecutableFactoryHasher::Sha3_512 => {
                self.sha3_512_state = Some(Default::default());
            }
            ExecutableFactoryHasher::Keccak512 => {
                self.keccak512_state = Some(Default::default());
            }
        }
    }

    pub fn validate_hash(&self, h: ExecutableFactoryHasher, expect_hash: &str) -> io::Result<()> {
        fn missing_hash_err(hasher: ExecutableFactoryHasher) -> std::io::Error {
            io::Error::new(io::ErrorKind::Other, format!("hash wasn't initialized: {:?}", hasher))
        }

        let hash_result_sha256;
        let hash_result_sha512;
        let hash_result_sha3_512;
        let hash_result_keccak512;
        let hash_slice;

        match h {
            ExecutableFactoryHasher::Sha256 => {
                let hash_state = self.sha256_state.clone().ok_or_else(|| missing_hash_err(h))?;
                hash_result_sha256 = hash_state.finalize_fixed();
                hash_slice = &hash_result_sha256[..];
            }
            ExecutableFactoryHasher::Sha512 => {
                let hash_state = self.sha512_state.clone().ok_or_else(|| missing_hash_err(h))?;
                hash_result_sha512 = hash_state.finalize_fixed();
                hash_slice = &hash_result_sha512[..];
            }
            ExecutableFactoryHasher::Sha3_512 => {
                let hash_state = self.sha3_512_state.clone().ok_or_else(|| missing_hash_err(h))?;
                hash_result_sha3_512 = hash_state.finalize_fixed();
                hash_slice = &hash_result_sha3_512[..];
            }
            ExecutableFactoryHasher::Keccak512 => {
                let hash_state = self.keccak512_state.clone().ok_or_else(|| missing_hash_err(h))?;
                hash_result_keccak512 = hash_state.finalize_fixed();
                hash_slice = &hash_result_keccak512[..];
            }
        }

        let mut scratch = [0; 512 / 8 * 2];
        let got_hash = crate::util::hexify(&mut scratch[..], hash_slice).unwrap();
        event!(
            Level::DEBUG,
            "checking hash {:?}: expecting: {}, got: {}",
            h,
            expect_hash,
            got_hash,
        );
        if expect_hash != got_hash {
            let msg = format!("hash ({:?}) mismatch {} != {}", h, expect_hash, got_hash);
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }

        Ok(())
    }

    pub fn hash_update(&mut self, data: &[u8]) {
        self.has_written = true;

        if let Some(ref mut hasher) = self.sha256_state {
            hasher.update(&data);
        }
        if let Some(ref mut hasher) = self.sha512_state {
            hasher.update(&data);
        }
        if let Some(ref mut hasher) = self.sha3_512_state {
            hasher.update(&data);
        }
        if let Some(ref mut hasher) = self.keccak512_state {
            hasher.update(&data);
        }
    }
}