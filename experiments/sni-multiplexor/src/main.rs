use std::collections::BTreeMap;
use std::fs::File;
use std::net::TcpListener as StdTcpListener;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::sync::Arc;
use std::pin::Pin;

use clap::{App, Arg};
use futures::future;
use futures::stream::{self, SelectAll, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::net::UnixListener;
use tokio::sync::{mpsc, Mutex};
use yscloud_config_model::{AppConfiguration, FileDescriptorRemote, SocketFlag};
use tracing::{event, Level};
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;
use tracing_subscriber::FmtSubscriber;
use trust_dns_resolver::TokioAsyncResolver;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};

mod context;
mod dialer;
mod erased;
mod mgmt_proto;
// mod resolver2;
mod error;
mod helpers;
mod ioutil;
mod model;
mod resolver;
mod sni2;
mod sni_base;
mod state_track;

use self::mgmt_proto::start_management_client;
use self::model::{
    config::ConfigResolverInit, ClientCtx, HaproxyProxyHeaderVersion, SocketAddrPair,
};
use self::resolver::BackendManager;
use self::state_track::SessionManager;
use self::sni2::Dialer;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let mut my_subscriber_builder = FmtSubscriber::builder();

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
    }

    let extras_str = serde_json::to_string(&config.extras).unwrap();
    let debug_level: DebugLevel = serde_json::from_str(&extras_str).unwrap();

    let verbosity = debug_level.verbosity.unwrap_or_else(|| {
        matches.occurrences_of("v")
    });
    let should_print_test_logging = 4 < verbosity;

    my_subscriber_builder = my_subscriber_builder.with_max_level(match verbosity {
        0 => TracingLevelFilter::ERROR,
        1 => TracingLevelFilter::WARN,
        2 => TracingLevelFilter::INFO,
        3 => TracingLevelFilter::DEBUG,
        _ => TracingLevelFilter::TRACE,
    });

    tracing::subscriber::set_global_default(my_subscriber_builder.finish())
        .expect("setting tracing default failed");

    if should_print_test_logging {
        print_test_logging();
    }

    let mut sni_director_sock = None;
    let mut sni_director_sock_haproxy_proxy_header_version = None;
    let mut management_sock = None;
    for file in &config.files {
        if file.service_name == "org.yshi.sni_multiplexor.https" {
            // 128 taken from rust stdlib
            ::nix::sys::socket::listen(file.file_num, 128).unwrap();

            sni_director_sock = Some(unsafe { StdTcpListener::from_raw_fd(file.file_num) });
            if let FileDescriptorRemote::Socket(ref si) = file.remote {
                for flag in &si.flags {
                    if *flag == SocketFlag::BehindHaproxy {
                        sni_director_sock_haproxy_proxy_header_version =
                            Some(HaproxyProxyHeaderVersion::Version1);
                    }
                }
            }

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

    let resolver_init: ConfigResolverInit = serde_json::from_str(&extras_str).unwrap();

    let mut backends = BTreeMap::new();
    for (k, v) in resolver_init.hostnames.into_iter() {
        backends.insert(k, v.into());
    }

    let resolver = Arc::new(Mutex::new(BackendManager {
        backends: Arc::new(backends),
    }));
    let mgmt_resolver = Arc::clone(&resolver);
    let data_resolver = resolver;

    let (mut client_futures_tx, mut client_futures) = mpsc::channel(16);

    let sessman = Arc::new(Mutex::new(SessionManager::new()));
    let mgmt_sessman = Arc::clone(&sessman);
    let data_sessman = sessman;

    let client_futures_tx_clone = client_futures_tx.clone();
    client_futures_tx.send(tokio::spawn(async move {
        let mut client_futures_tx = client_futures_tx_clone;

        let mut mgmt_listener: UnixListener = UnixListener::from_std(management_sock).unwrap();

        let mut mgmt_incoming = mgmt_listener.incoming();
        while let Some(socket) = mgmt_incoming.next().await {
            let socket = match socket {
                Ok(s) => s,
                Err(err) => {
                    event!(Level::WARN, "failure accepting socket: {}", err);
                    return;
                }
            };

            let sessman = mgmt_sessman.clone();
            let resolver = mgmt_resolver.clone();

            let join = tokio::spawn(async move {
                if let Err(err) = start_management_client(sessman, resolver, socket).await {
                    event!(Level::WARN, "management client terminated: {}", err);
                }
            });

            if let Err(err) = client_futures_tx.send(join).await {
                event!(Level::WARN, "failed to send future to task reaper: {:?}", err);
                return;
            }
        }
    })).await.unwrap();

    let client_futures_tx_clone = client_futures_tx.clone();

    let mut resolver_config = ResolverConfig::new();
    resolver_config.add_name_server(trust_dns_resolver::config::NameServerConfig {
        socket_addr: resolver_init.upstream_dns,
        protocol: trust_dns_resolver::config::Protocol::Tcp,
        tls_dns_name: None,
    });
    let resolver: TokioAsyncResolver = TokioAsyncResolver::tokio(
        resolver_config,
        ResolverOpts::default(),
    ).await?;

    let dialer = Arc::new(Dialer { resolver });

    client_futures_tx.send(tokio::spawn(async move {
        let mut client_futures_tx = client_futures_tx_clone;

        let mut data_listener: TcpListener = TcpListener::from_std(sni_director_sock).unwrap();

        let mut data_incoming = data_listener.incoming();

        while let Some(socket) = data_incoming.next().await {
            let socket = match socket {
                Ok(s) => s,
                Err(err) => {
                    event!(Level::WARN, "failure accepting socket: {}", err);
                    return;
                }
            };

            let laddr = match socket.local_addr() {
                Ok(laddr) => laddr,
                Err(err) => {
                    event!(Level::WARN, "bad local address for socket: {}", err);
                    continue;
                }
            };
            
            let dialer = Arc::clone(&dialer);
            let data_sessman = Arc::clone(&data_sessman);
            let data_resolver = Arc::clone(&data_resolver);
            let join = tokio::spawn(async move {
                let paddr = match socket.peer_addr() {
                    Ok(paddr) => paddr,
                    Err(err) => {
                        event!(Level::WARN, "bad peer address for socket: {}", err);
                        return;
                    }
                };
                let client_conn = match SocketAddrPair::from_pair(laddr, paddr) {
                    Ok(paddr) => paddr,
                    Err(err) => {
                        event!(Level::WARN, "bad address pair for socket: {}", err);
                        return;
                    }
                };

                let res = sni2::sni_connect_and_copy(
                    dialer,
                    data_sessman,
                    data_resolver,
                    client_conn,
                    socket,
                    ClientCtx {
                        proxy_header_version: sni_director_sock_haproxy_proxy_header_version,
                    },
                )
                .await;

                if let Err(err) = res {
                    event!(Level::WARN, "error handling client: {:?}", err);
                }
            });

            if let Err(err) = client_futures_tx.send(join).await {
                event!(Level::WARN, "failed to send future to task reaper: {:?}", err);
                return;
            }
        }
    })).await.unwrap();

    let mut resolved = client_futures.buffer_unordered(1024);

    while let Some(res) = resolved.next().await {
        if let Err(err) = res {
            if err.is_panic() {
                std::panic::resume_unwind(err.into_panic());
            }

            event!(Level::WARN, "task exited uncleanly: {:?}", err);
        }
    }

    Ok(())
}


#[allow(clippy::cognitive_complexity)] // macro bug around event!()
fn print_test_logging() {
    event!(Level::TRACE, "logger initialized - trace check");
    event!(Level::DEBUG, "logger initialized - debug check");
    event!(Level::INFO, "logger initialized - info check");
    event!(Level::WARN, "logger initialized - warn check");
    event!(Level::ERROR, "logger initialized - error check");
}
