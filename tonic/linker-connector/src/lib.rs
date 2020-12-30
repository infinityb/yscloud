use std::io;
use std::net::{TcpListener as StdTcpListener, TcpStream as StdTcpStream};
use std::os::unix::io::FromRawFd;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Poll, Context};
use std::pin::Pin;

use futures::future::{Future, FutureExt};
use futures::stream::{self, Stream, StreamExt, select_all};
use hyper::Uri;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{event, Level};

pub struct ListenerBuilder {
    connecteds: Vec<TcpStream>,
    listening: Vec<TcpListener>,
}

impl ListenerBuilder {
    pub unsafe fn push_connected_descriptor(&mut self, file_no: RawFd) -> io::Result<()> {
        let std = StdTcpStream::from_raw_fd(file_no);
        let stream = TcpStream::from_std(std)?;
        self.connecteds.push(stream);
        Ok(())
    }

    pub unsafe fn push_listening_descriptor(&mut self, file_no: RawFd) -> io::Result<()> {
        let std = StdTcpListener::from_raw_fd(file_no);
        let listener = TcpListener::from_std(std)?;
        self.listening.push(listener);
        Ok(())
    }

    pub fn build(self) -> Listener {
        Listener {
            connecteds: self.connecteds,
            listening: self.listening,
        }
    }
}

pub struct Listener {
    connecteds: Vec<TcpStream>,
    listening: Vec<TcpListener>,
}

impl Listener {
    pub fn builder() -> ListenerBuilder {
        ListenerBuilder {
            connecteds: Vec::new(),
            listening: Vec::new(),
        }
    }

    pub fn into_incoming(self) -> impl Stream<Item=io::Result<TcpStream>> {
        let connecteds = stream::iter(self.connecteds.into_iter().map(Ok));
        connecteds.chain(select_all(self.listening.into_iter()))
    }
}

pub struct ConnectorBuilder {
    service_name: String,
    connecteds: Vec<TcpStream>,
}

impl ConnectorBuilder {
    pub unsafe fn push_connected_descriptor(&mut self, file_no: RawFd) -> io::Result<()> {
        let std = StdTcpStream::from_raw_fd(file_no);
        let stream = TcpStream::from_std(std)?;
        self.connecteds.push(stream);
        Ok(())
    }

    pub fn build(mut self) -> Connector {
        self.connecteds.reverse();
        let pending_connections = self.connecteds.len();
        
        Connector {
            finished: pending_connections == 0,
            inner: Arc::new(ConnectorInner {
                service_name: self.service_name,
                remaining_connectors: AtomicUsize::new(pending_connections),
                connecteds: Mutex::new(self.connecteds),
            })
        }
    }
}

#[derive(Clone)]
pub struct Connector {
    // local fast path.
    finished: bool,
    inner: Arc<ConnectorInner>,
}

struct ConnectorInner {
    service_name: String,
    remaining_connectors: AtomicUsize,
    connecteds: Mutex<Vec<TcpStream>>,
}

impl Connector {
    pub fn builder(name: &str) -> ConnectorBuilder {
        ConnectorBuilder {
            service_name: name.to_string(),
            connecteds: Vec::new(),
        }
    }
}

impl tower_service::Service<Uri> for Connector {
    type Response = TcpStream;

    type Error = io::Error;

    type Future = Pin<Box<dyn Future<Output=io::Result<TcpStream>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        if self.finished {
            return Poll::Pending;
        }

        // see if we can relax this later.
        if self.inner.remaining_connectors.load(Ordering::SeqCst) == 0 {
            self.finished = true;
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        fn conn_refused() -> io::Result<TcpStream> {
            Err(io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused"))
        }

        event!(Level::DEBUG, "detouring call to connect to {} to yscloud service {:?}",
            dst, self.inner.service_name);
        
        if !self.finished && self.inner.remaining_connectors.load(Ordering::SeqCst) == 0 {
            self.finished = true;
        }
        if self.finished {
            return async move { conn_refused() }.boxed();
        }

        let inner = Arc::clone(&self.inner);
        async move {
            let mut connecteds = inner.connecteds.lock().await;
            if let Some(conn) = connecteds.pop() {
                inner.remaining_connectors.store(connecteds.len(), Ordering::SeqCst);

                Ok(conn)
            } else {
                conn_refused()
            }
        }.boxed()
    }
}

