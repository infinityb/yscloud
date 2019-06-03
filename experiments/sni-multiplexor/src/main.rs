#![feature(await_macro, async_await)]

use std::fs::File;
use std::net::TcpListener as StdTcpListener;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use log::{info};
use clap::{App, Arg};
use ksuid::Ksuid;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use tokio::codec::Decoder;
use tokio::net::TcpListener;
use tokio::net::UnixListener;
use tokio::prelude::{Future, Sink, Stream};
use tokio::reactor::Handle;
use tokio::sync::mpsc::channel;

use yscloud_config_model::AppConfiguration;
use crate::state_track::{SessionCommand, SessionCommandData, SessionCreateCommand};

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
    let mgmt_listener: UnixListener = UnixListener::from_std(management_sock, &def_handler).unwrap();

    let resolver: MemoryResolver = serde_json::from_str(&extras_str).unwrap();
    let resolver: Box<Resolver + Send + Sync> = Box::new(resolver);
    let resolver: Arc<Mutex<Arc<Resolver + Send + Sync>>> = Arc::new(Mutex::new(resolver.into()));

    let server_resolver = resolver.clone();
    let sessman = Arc::new(Mutex::new(SessionManager::new()));
    let mgmt_sessman = Arc::clone(&sessman);

    let (stats_tx, stats_rx) = channel(128);

    let sessman_stats = Arc::clone(&sessman);
    let stats_gather = stats_rx.for_each(move |cmd| {
        let mut sessman = sessman_stats.lock().unwrap();
        sessman.apply_command(&cmd);
        Ok(())
    }).map_err(|e| info!("stats receiver died: {}", e));

    let mgmt_server = mgmt_listener
        .incoming()
        .for_each(move |socket| {
            let framed = AsciiManagerServer::new().framed(socket);
            tokio::spawn(start_management_client(mgmt_sessman.clone(), framed));
            Ok(())
        })
        .map_err(|e| eprintln!("accept error: {}", e));

    let data_server = data_listener
        .incoming()
        .map(move |socket| (stats_tx.clone(), server_resolver.clone(), socket))
        .for_each(move |(stats_tx, server_resolver, socket)| {
            let session_id = Ksuid::generate();

            let start_time = Instant::now();
            let client_conn = SocketAddrPair::from_pair(socket.local_addr()?, socket.peer_addr()?)?;

            tokio::spawn(stats_tx.send(SessionCommand {
                session_id,
                data: SessionCommandData::Create(SessionCreateCommand {
                    start_time,
                    client_conn: client_conn.clone(),
                }),
            }).map_err(|e| info!("failed to send to stats gatherer: {}", e)).and_then(move |stats_tx| {
                let client_meta = ClientMetadata {
                    session_id,
                    start_time,
                    client_conn,
                    stats_tx,
                };
                let resolver = server_resolver.lock().unwrap().clone();
                let framed = SniDetectorCodec::new(session_id).framed(socket);
                start_client(resolver, client_meta, framed)
            }));
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    tokio::run(stats_gather.join(mgmt_server).join(data_server).map(|_| ()));
}
