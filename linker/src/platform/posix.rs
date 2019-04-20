use std::io;
use std::os::unix::io::AsRawFd;

use log::{debug, log, warn};

use super::imp;
use crate::{AppConfiguration, AppPreforkConfiguration, FileDescriptorInfo};

pub fn relabel_file_descriptors(c: &AppPreforkConfiguration) -> io::Result<AppConfiguration> {
    // this seems ghetto?

    let mut keep_map = [false; 256];
    for v in keep_map.iter_mut().take(3) {
        *v = true;
    }

    imp::keep_hook(c, &mut keep_map[..]);

    for f in &c.files {
        let file_num = f.file.as_raw_fd() as usize;
        if keep_map.len() < file_num {
            warn!(
                "a keep-file is over {} - are we leaking file descriptors?",
                keep_map.len()
            );
            continue;
        }
        debug!(
            "keeping {} for {}: {:?}",
            file_num, c.artifact, f.service_name
        );
        keep_map[file_num] = true;
    }
    for (i, keep) in keep_map.iter().enumerate() {
        if !*keep && nix::unistd::close(i as i32).is_ok() {
            debug!("closed {} for {}:{}", i, c.package_id, c.instance_id);
        }
    }

    Ok(AppConfiguration {
        package_id: c.package_id.clone(),
        instance_id: c.instance_id,
        version: c.version.clone(),
        files: c
            .files
            .iter()
            .map(|f| FileDescriptorInfo {
                file_num: f.file.as_raw_fd(),
                direction: f.direction.clone(),
                service_name: f.service_name.clone(),
                remote: f.remote.clone(),
            })
            .collect(),
        extras: c.extras.clone(),
    })
}
