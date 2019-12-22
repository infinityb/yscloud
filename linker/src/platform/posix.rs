use std::collections::HashMap;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::Pid;
use tracing::{Level, event, span};

use super::imp;
use crate::platform::exec_artifact;
use crate::{AppConfiguration, AppPreforkConfiguration, FileDescriptorInfo};

pub fn relabel_file_descriptors(c: &AppPreforkConfiguration) -> io::Result<AppConfiguration> {
    // this seems ghetto?

    let mut keep_map = [false; 2048];
    for v in keep_map.iter_mut().take(3) {
        *v = true;
    }

    imp::keep_hook(c, &mut keep_map[..]);

    for f in &c.files {
        let file_num = f.file.as_raw_fd() as usize;
        if keep_map.len() < file_num {
            event!(Level::WARN,
                "a keep-file is over {} - are we leaking file descriptors?",
                keep_map.len()
            );
            continue;
        }
        event!(
            Level::DEBUG,
            "keeping {} for {}: {:?}",
            file_num, c.artifact, f.service_name
        );
        keep_map[file_num] = true;
    }
    for (i, keep) in keep_map.iter().enumerate() {
        if !*keep && nix::unistd::close(i as i32).is_ok() {
            event!(Level::DEBUG, "closed {} for {}:{}", i, c.package_id, c.instance_id);
        }
    }

    Ok(AppConfiguration {
        tenant_id: c.tenant_id.clone(),
        deployment_name: c.deployment_name.clone(),
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

pub fn run_reified(reified: Vec<crate::ExecSomething>) {
    #[derive(Clone, Debug)]
    struct ChildInfo {
        package_name: String,
        sent_kill: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    }

    let span = span!(Level::INFO, "run_reified");
    let _span_entered = span.enter();

    let mut pids = HashMap::<Pid, ChildInfo>::new();
    for a in reified {
        let package_id = a.cfg.package_id.clone();

        event!(Level::DEBUG, package_id = &package_id[..], "creating process");
        let child = exec_artifact(&a.extras, a.cfg).unwrap();
        event!(Level::DEBUG, package_id = &package_id[..], child.pid = ?child, "created process");

        pids.insert(
            child,
            ChildInfo {
                package_name: package_id,
                sent_kill: Arc::new(AtomicBool::new(false)),
                running: Arc::new(AtomicBool::new(true)),
            },
        );
    }

    fn kill_all(pids: &HashMap<Pid, ChildInfo>, second_kill: bool) {
        for (pid, info) in pids {
            if info.running.load(Ordering::SeqCst)
                && (!info.sent_kill.load(Ordering::SeqCst) || second_kill)
            {
                if !second_kill {
                    info.sent_kill.store(true, Ordering::SeqCst);
                }
                event!(Level::INFO, "sending {} ({}) SIGTERM", pid, info.package_name);
                let sent_kill = kill(*pid, Signal::SIGTERM).is_ok();
                event!(Level::INFO, 
                    "sent {} ({}) SIGTERM, successful: {}",
                    pid, info.package_name, sent_kill
                );
            }
        }
    }

    use signal_hook::iterator::Signals;
    let signals = Signals::new(&[signal_hook::SIGINT]).unwrap();

    let kill_targets = pids.clone();
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            signals.close();
            event!(Level::INFO, "got {}, signaling to children to terminate", sig);
            kill_all(&kill_targets, false);
        }
        if let Some(sig) = signals.forever().next() {
            signals.close();
            event!(Level::INFO, 
                "got {}, signaling to children to terminate (2nd attempt)",
                sig
            );
            kill_all(&kill_targets, true);
        }
    });

    let mut remaining_children = pids.len();
    let mut child_exited_nonzero = false;
    while 0 < remaining_children {
        match waitpid(None, None) {
            Ok(WaitStatus::Exited(pid, exit_code)) => {
                let child_info = &pids[&pid];
                event!(Level::INFO, "child {} exited {}", child_info.package_name, exit_code);
                if exit_code != 0 {
                    child_exited_nonzero = true;
                }
                remaining_children -= 1;
                child_info.running.store(false, Ordering::SeqCst);
                kill_all(&pids, false);
            }
            // literally why.
            Ok(WaitStatus::Signaled(pid, sig, cored)) => {
                let child_info = &pids[&pid];
                event!(Level::INFO, 
                    "child {} exited via signal {}",
                    child_info.package_name, sig
                );
                remaining_children -= 1;
                if cored {
                    // we might also want to detect bad exit signals?
                    child_exited_nonzero = true;
                }
                child_info.running.store(false, Ordering::SeqCst);
                kill_all(&pids, false);
            }
            Ok(ws) => {
                event!(Level::WARN,"waitpid got an unexpected {:?}", ws);
            }
            Err(err) => {
                panic!("waitpid err {}", err);
            }
        };
    }
    if child_exited_nonzero {
        std::process::exit(1);
    }
}
