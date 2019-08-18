#![feature(async_await)]
use std::fs::File;
use std::net::TcpListener as StdTcpListener;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::{App, Arg};
use ksuid::Ksuid;
use log::LevelFilter;
use log::warn;
use serde::{Deserialize, Serialize};
use tokio::codec::Decoder;
use tokio::net::TcpListener;
use tokio::net::UnixListener;
use tokio::reactor::Handle;
use tokio::sync::mpsc::channel;
use futures::future::FutureExt;
use futures::stream::StreamExt;

use crate::state_track::{SessionCommand, SessionCommandData, SessionCreateCommand};
use yscloud_config_model::AppConfiguration;

mod config;
mod mgmt_proto;
mod sni;
mod state_track;

use self::config::{MemoryResolver, Resolver};
use self::mgmt_proto::{start_management_client, AsciiManagerServer};
use self::sni::{start_client, ClientMetadata, SniDetectorCodec, SocketAddrPair};
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

    let def_handler = Handle::default();
    let data_listener: TcpListener =
        TcpListener::from_std(sni_director_sock, &def_handler).unwrap();
    let mgmt_listener: UnixListener =
        UnixListener::from_std(management_sock, &def_handler).unwrap();

    let resolver: MemoryResolver = serde_json::from_str(&extras_str).unwrap();
    let resolver: Box<dyn Resolver + Send + Sync + Unpin + 'static> = Box::new(resolver);
    let resolver: Arc<Mutex<Arc<dyn Resolver + Send + Sync + Unpin + 'static>>> =
        Arc::new(Mutex::new(resolver.into()));

    let server_resolver = resolver.clone();
    let sessman = Arc::new(Mutex::new(SessionManager::new()));
    let mgmt_sessman = Arc::clone(&sessman);

    let (stats_tx, stats_rx) = channel(128);

    let sessman_stats = Arc::clone(&sessman);
    let stats_gather = async {
        let mut stats_rx = stats_rx;
        loop {
            let (next_item, tail) = stats_rx.into_future().await;
            stats_rx = tail;

            let v = match next_item {
                Some(v) => v,
                None => break,
            };
            let mut sessman = sessman_stats.lock().unwrap();
            sessman.apply_command(v);
        }
    };

    let mgmt_server = async {
        let mut mgmt_incoming = mgmt_listener.incoming();
        loop {
            let (next_item, tail) = mgmt_incoming.into_future().await;
            mgmt_incoming = tail;

            let socket = match next_item {
                Some(socket) => socket,
                None => break,
            };

            let socket = socket.expect("FIXME");
            let framed = AsciiManagerServer::new().framed(socket);
            tokio::spawn(async {
                if let Err(err) = start_management_client(mgmt_sessman.clone(), framed).await {
                    warn!("management client terminated: {}", err);
                }
            });
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

            let socket = socket.expect("FIXME");
            let start_time = Instant::now();
                let session_id = Ksuid::generate();

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

            let (creat, aborter) = SessionCreateCommand::new(start_time, client_conn.clone());

            stats_tx
                .send(SessionCommand {
                    session_id,
                    data: SessionCommandData::Create(creat),
                })
                .await.expect("FIXME");

            tokio::spawn(async {
                let mut stats_tx = stats_tx.clone();
                let client_meta = ClientMetadata {
                    session_id,
                    start_time,
                    client_conn,
                    stats_tx,
                    aborter,
                };
                let resolver = server_resolver.lock().unwrap().clone();
                let framed = SniDetectorCodec::new(session_id).framed(socket);

                if let Err(err) = start_client(resolver, client_meta, framed).await {
                    warn!("start_client returned error: {}", err);
                }
            });
        }
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(stats_gather.join(mgmt_server).join(data_server).map(|_| ()));
}
