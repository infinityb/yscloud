use std::net::TcpStream;
use std::os::unix::io::IntoRawFd;

use clap::{App, Arg};
use hyper::{Client, Uri, Body};
use tracing::{event, Level};
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;
use tracing_subscriber::FmtSubscriber;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_NAME: &str = env!("CARGO_PKG_NAME");

mod pb {
    tonic::include_proto!("org.yshi.certificate_issuer.v1");
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut my_subscriber_builder = FmtSubscriber::builder();

    let app = App::new(CARGO_PKG_NAME)
        .version(CARGO_PKG_VERSION)
        .author("Stacey Ell <software@e.staceyell.com>")
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        );

    let matches = app.get_matches();

    let verbosity = matches.occurrences_of("v");
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

    // let stream = TcpStream::connect("google.com:80")?;
    // let raw_fd = stream.into_raw_fd();

    // let mut conn = linker_connector::Connector::builder("foobar");
    // unsafe { conn.push_connected_descriptor(raw_fd) }?;
    // let connector = conn.build();

    // let client: Client<_, Body> = Client::builder().build(connector);

    // for i in 0..4 {
    //     let res = client.get(Uri::from_static("http://google.com")).await?;
    //     println!("status({}): {}", i, res.status());
    //     let buf = hyper::body::to_bytes(res).await?;
    //     println!("body({}): {:?}", i, buf);
    // }

    // event!(Level::INFO, "Hello, world!");
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