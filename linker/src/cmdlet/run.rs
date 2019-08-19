use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use clap::{App, Arg, SubCommand};
use log::{debug, info, trace, warn};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::Pid;

use yscloud_config_model::DeploymentManifest;

use crate::platform::exec_artifact;
use crate::reify_service_connections;
use crate::{CARGO_PKG_VERSION, SUBCOMMAND_RUN};

pub fn get_subcommand() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND_RUN)
        .version(CARGO_PKG_VERSION)
        .about("link and run a deployment")
        .arg(
            Arg::with_name("approot")
                .long("approot")
                .value_name("DIR")
                .help("an application state directory root")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("manifest")
                .long("manifest")
                .value_name("FILE")
                .help("The deployment manifest to link up and run")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("artifacts")
                .long("artifacts")
                .value_name("DIR")
                .help("an artifact directory containing dependencies of the manifest")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("artifact-override")
                .long("artifact-override")
                .value_name("PACKAGE_ID:PATH")
                .help("Override a Package ID with some other path")
                .multiple(true)
                .takes_value(true),
        )
}

pub fn main(matches: &clap::ArgMatches) {
    let approot = matches.value_of("approot").unwrap();
    let approot = Path::new(approot).to_owned();
    trace!("got approot: {}", approot.display());

    let artifacts = matches.value_of("artifacts").unwrap();
    trace!("got artifacts: {:?}", artifacts);

    let manifest_path = matches.value_of("manifest").unwrap();
    trace!("got manifest: {:?}", manifest_path);

    let mut overrides: HashMap<String, String> = HashMap::new();
    if let Some(override_args) = matches.values_of_lossy("artifact-override") {
        for arg in override_args {
            let mut split_iter = arg.split(':');
            let package_name = split_iter.next().unwrap().to_string();
            let artifact_path = split_iter.next().unwrap().to_string();
            overrides.insert(package_name, artifact_path);
        }

        warn!("development mode - using path overrides: {:?}", overrides);
    }

    let rdr = File::open(&manifest_path).unwrap();
    let mut target_deployment_manifest =
        serde_json::from_reader::<_, DeploymentManifest>(rdr).unwrap();
    target_deployment_manifest.path_overrides = overrides;

    let reified =
        reify_service_connections(&target_deployment_manifest, artifacts, &approot).unwrap();

    #[derive(Clone, Debug)]
    struct ChildInfo {
        package_name: String,
        sent_kill: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    }

    let mut pids = HashMap::<Pid, ChildInfo>::new();
    for a in reified {
        let package_id = a.cfg.package_id.clone();
        let child = exec_artifact(&a.extras, a.cfg).unwrap();

        debug!("made child: {} {:?}", package_id, child);
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
                info!("sending {} ({}) SIGTERM", pid, info.package_name);
                let sent_kill = kill(*pid, Signal::SIGTERM).is_ok();
                info!(
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
            info!("got {}, signaling to children to terminate", sig);
            kill_all(&kill_targets, false);
        }
        if let Some(sig) = signals.forever().next() {
            signals.close();
            info!(
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
                info!("child {} exited {}", child_info.package_name, exit_code);
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
                info!(
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
                warn!("waitpid got an unexpected {:?}", ws);
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
