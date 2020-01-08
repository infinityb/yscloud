use std::collections::BTreeMap;
use std::fs::File;
use std::net::TcpListener as StdTcpListener;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::sync::Arc;

use clap::{App, Arg};
use futures::future::{self, FutureExt};
use futures::stream::StreamExt;
use log::warn;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use yscloud_config_model::AppConfiguration;

// mod abortable_stream;
mod config;
mod context;
mod dialer;
mod erased;
mod ioutil;
mod mgmt_proto;
mod resolver2;
mod resolver;
mod sni2;
// mod sni;
mod sni_base;
mod state_track;

use self::config::ResolverInit;
use self::mgmt_proto::{start_management_client};
use self::resolver::{NetworkLocation, BackendManager, BackendSet};
use self::sni_base::SocketAddrPair;
use self::state_track::SessionManager;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    let matches = App::new("sni-director")
        .version(CARGO_PKG_VERSION)
        .author("Stacey Ell <stacey.ell@gmail.com>")
        .about("TLS multiplexor using SNI")
        .arg(
            Arg::with_name("config-fd")
                .long("config-fd")
                .value_name("number")
                .takes_value(true)
                .help("cloud config file descriptor, for yscloud internal usage"),
        )
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
        .get_matches();

    let config_fd = matches
        .value_of("config-fd")
        .expect("only runnable as yscloud program for now");

    let config_file = unsafe { File::from_raw_fd(config_fd.parse::<RawFd>().unwrap()) };
    let config: AppConfiguration = serde_json::from_reader(config_file).unwrap();

    #[derive(Deserialize, Serialize)]
    struct DebugLevel {
        verbosity: Option<u64>,
        debug: Option<u64>,
    }

    let extras_str = serde_json::to_string(&config.extras).unwrap();
    let debug_level: DebugLevel = serde_json::from_str(&extras_str).unwrap();

    let mut builder = env_logger::Builder::from_default_env();
    builder.default_format_module_path(true);

    match debug_level
        .verbosity
        .unwrap_or_else(|| matches.occurrences_of("v"))
    {
        0 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Error),
        1 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Warn),
        2 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Info),
        3 => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Debug),
        4 | _ => builder.filter_module(CARGO_PKG_NAME, LevelFilter::Trace),
    };

    match debug_level
        .debug
        .unwrap_or_else(|| matches.occurrences_of("d"))
    {
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

    let mut sni_director_sock = None;
    let mut management_sock = None;
    for file in &config.files {
        if file.service_name == "org.yshi.sni_multiplexor.https" {
            // 128 taken from rust stdlib
            ::nix::sys::socket::listen(file.file_num, 128).unwrap();

            sni_director_sock = Some(unsafe { StdTcpListener::from_raw_fd(file.file_num) });
            break;
        }
    }
    for file in &config.files {
        if file.service_name == "org.yshi.sni_multiplexor.v1.SniMultiplexor" {
            // 128 taken from rust stdlib
            ::nix::sys::socket::listen(file.file_num, 128).unwrap();

            management_sock = Some(unsafe { StdUnixListener::from_raw_fd(file.file_num) });
            break;
        }
    }
    let sni_director_sock = sni_director_sock.expect("sni_director_sock");
    let management_sock = management_sock.expect("management_sock");

    let mut data_listener: TcpListener =
        TcpListener::from_std(sni_director_sock).unwrap();
    let mut mgmt_listener: UnixListener =
        UnixListener::from_std(management_sock).unwrap();

    let resolver_init: ResolverInit = serde_json::from_str(&extras_str).unwrap();

    let mut backends = BTreeMap::new();
    for (k, v) in resolver_init.hostnames.into_iter() {
        backends.insert(
            k,
            BackendSet::from_list(vec![NetworkLocation {
                use_haproxy_header_v: v.use_haproxy_header,
                address: v.location,
                stats: (),
            }]),
        );
    }

    let resolver = Arc::new(Mutex::new(BackendManager { backends: Arc::new(backends) }));
    let mgmt_resolver = Arc::clone(&resolver);
    let data_resolver = resolver;

    let sessman = Arc::new(Mutex::new(SessionManager::new()));
    let mgmt_sessman = Arc::clone(&sessman);
    let data_sessman = sessman;

    let mgmt_server = async {
        let mut mgmt_incoming = mgmt_listener.incoming();
        loop {
            let sessman = mgmt_sessman.clone();
            let resolver = mgmt_resolver.clone();

            let (next_item, tail) = mgmt_incoming.into_future().await;
            mgmt_incoming = tail;

            match next_item {
                Some(Ok(socket)) => {
                    tokio::spawn(async move {
                        if let Err(err) = start_management_client(sessman, resolver, socket).await {
                            warn!("management client terminated: {}", err);
                        }
                    });
                },
                Some(Err(err)) => {
                    warn!("failed to accept a socket: {}", err);
                }
                None => break,
            };
        }
    };

    let data_server = async {
        let mut data_incoming = data_listener.incoming();

        loop {
            let (next_item, tail) = data_incoming.into_future().await;
            data_incoming = tail;

            let socket = match next_item {
                Some(socket) => socket,
                None => break,
            };

            let mut socket = socket.expect("FIXME");

            let laddr = match socket.local_addr() {
                Ok(laddr) => laddr,
                Err(err) => {
                    warn!("bad local address for socket: {}", err);
                    continue;
                }
            };
            let paddr = match socket.peer_addr() {
                Ok(paddr) => paddr,
                Err(err) => {
                    warn!("bad peer address for socket: {}", err);
                    continue;
                }
            };
            let client_conn = match SocketAddrPair::from_pair(laddr, paddr) {
                Ok(paddr) => paddr,
                Err(err) => {
                    warn!("bad address pair for socket: {}", err);
                    continue;
                }
            };

            let data_sessman = Arc::clone(&data_sessman);
            let data_resolver = Arc::clone(&data_resolver);
            tokio::spawn(async move {
                sni2::sni_connect_and_copy(
                    data_sessman,
                    data_resolver,
                    client_conn,
                    socket,
                    sni2::ClientCtx {
                        proxy_header_version: None,
                    },
                ).await;
            });
        }
    };

    let mut runtime_builder = tokio::runtime::Builder::new();
    let mut runtime = runtime_builder.build().unwrap();

    runtime.block_on(future::join(mgmt_server, data_server).map(|_| ()));
}
