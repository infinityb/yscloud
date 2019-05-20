use std::io;
use std::sync::{Arc, Mutex};

use tokio::codec::Decoder;
use tokio::net::tcp::TcpListener;
use tokio::prelude::{Future, Stream};

mod config;
mod sni;

use self::config::{MemoryResolver, Resolver};
use self::sni::{start_client, ClientMetadata, SniDetectorCodec, SocketAddrPair};

fn main() {
    env_logger::init();

    let addr = "127.0.0.1:6142".parse().unwrap();
    let listener = TcpListener::bind(&addr).unwrap();

    let resolver: MemoryResolver = serde_json::from_reader(io::stdin()).unwrap();
    let resolver: Box<Resolver + Send + Sync> = Box::new(resolver);
    let resolver: Arc<Mutex<Arc<Resolver + Send + Sync>>> = Arc::new(Mutex::new(resolver.into()));

    let server_resolver = resolver.clone();
    let server = listener
        .incoming()
        .for_each(move |socket| {
            let addresses = SocketAddrPair::from_pair(socket.local_addr()?, socket.peer_addr()?)?;
            let client_meta = ClientMetadata { addresses };

            let resolver = server_resolver.lock().unwrap().clone();
            let framed = SniDetectorCodec::new().framed(socket);
            tokio::spawn(start_client(resolver, client_meta, framed));
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    println!("server running on localhost:6142");

    tokio::run(server);
}
