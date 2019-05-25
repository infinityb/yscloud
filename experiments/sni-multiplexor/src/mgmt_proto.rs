use bytes::BytesMut;
use log::error;
use std::io;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::{future, Future, Sink, Stream};

use crate::sni::SocketAddrPair;
use crate::state_track::{Session, SessionManager};

pub struct AsciiManagerServer {}

impl AsciiManagerServer {
    pub fn new() -> AsciiManagerServer {
        AsciiManagerServer {}
    }
}

pub enum AsciiManagerRequest {
    PrintActiveSessions,
}

impl FromStr for AsciiManagerRequest {
    type Err = io::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "print-active-sessions" => AsciiManagerRequest::PrintActiveSessions,
            _ => return Err(io::Error::new(io::ErrorKind::Other, "unknown command")),
        })
    }
}

pub enum AsciiManagerResponse {
    PrintActive(Vec<Session>),
}

fn write_sock_addr_pair<W>(wri: &mut W, sap: &SocketAddrPair) -> io::Result<()>
where
    W: std::io::Write,
{
    match *sap {
        SocketAddrPair::V4(ref ap) => {
            write!(wri, "{}:{},", ap.local_addr.ip(), ap.local_addr.port())?;
            write!(wri, "{}:{} ", ap.peer_addr.ip(), ap.peer_addr.port())?;
        }
        SocketAddrPair::V6(ref ap) => {
            write!(wri, "[{}]:{},", ap.local_addr.ip(), ap.local_addr.port())?;
            write!(wri, "[{}]:{} ", ap.peer_addr.ip(), ap.peer_addr.port())?;
        }
    }
    Ok(())
}

fn write_session_info<W>(wri: &mut W, si: &Session) -> io::Result<()>
where
    W: std::io::Write,
{
    write!(wri, "{} ", si.state.as_str())?;
    write_sock_addr_pair(wri, &si.client_conn)?;

    if let Some(ref bc) = si.backend_conn {
        write!(wri, "backend_connect_addr=")?;
        write_sock_addr_pair(wri, bc)?;
    }
    if let Some(ref bcl) = si.backend_connect_latency {
        write!(wri, "backend_connect_latency_ms={} ", bcl.as_millis())?;
    }
    write!(
        wri,
        "last_xmit_ago_ms={} ",
        si.last_xmit.elapsed().as_millis()
    )?;
    write!(
        wri,
        "client_to_backend_bytes={} ",
        si.bytes_client_to_backend
    )?;
    write!(
        wri,
        "backend_to_client_bytes={} ",
        si.bytes_backend_to_client
    )?;

    writeln!(wri)?;

    Ok(())
}

fn encode_print_active_sessions(
    scratch: &mut [u8],
    out: &mut BytesMut,
    sess_infos: &[Session],
) -> io::Result<()> {
    for si in sess_infos {
        let cur_pos = {
            let mut cur = io::Cursor::new(&mut scratch[..]);
            write_session_info(&mut cur, si)?;
            cur.position() as usize
        };
        out.extend_from_slice(&scratch[..cur_pos as usize]);
    }

    out.extend_from_slice(b"END\n");

    Ok(())
}

impl Encoder for AsciiManagerServer {
    type Item = AsciiManagerResponse;

    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut scratch = vec![0; 1024];

        match item {
            AsciiManagerResponse::PrintActive(ref sessinfo) => {
                encode_print_active_sessions(&mut scratch, dst, sessinfo)?;
            }
        }

        Ok(())
    }
}

impl Decoder for AsciiManagerServer {
    type Item = AsciiManagerRequest;

    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        use std::str::from_utf8;

        let line = match src.iter().position(|x| *x == b'\n') {
            Some(eol) => src.split_to(eol + 1).freeze(),
            None => return Ok(None),
        };
        let mut line = from_utf8(&*line).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        line = line.trim();
        AsciiManagerRequest::from_str(line).map(Some)
    }
}

pub fn start_management_client<A>(
    sessman: Arc<Mutex<SessionManager>>,
    client: Framed<A, AsciiManagerServer>,
) -> Box<dyn Future<Item = (), Error = ()> + Send>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (sink, stream) = client.split();

    let fut = stream
        .into_future()
        .map_err(|(e, _stream)| {
            error!("error receiving ascii manager request: {}", e);
        })
        .and_then(move |(req, stream)| recursive_handle(sessman, stream, sink, req));
    Box::new(fut)
}

fn recursive_handle<St, Si>(
    sessman: Arc<Mutex<SessionManager>>,
    stream: St,
    sink: Si,
    req: Option<AsciiManagerRequest>,
) -> Box<dyn Future<Item = (), Error = ()> + Send>
where
    St: Stream<Item = AsciiManagerRequest, Error = io::Error> + Send + Sync + 'static,
    Si: Sink<SinkItem = AsciiManagerResponse, SinkError = io::Error> + Send + Sync + 'static,
{
    let sessions = sessman.lock().unwrap().get_sessions();
    if let Some(req) = req {
        let fut = sink
            .send(AsciiManagerResponse::PrintActive(sessions))
            .map_err(|e| {
                error!("error sending ascii manager response: {}", e);
            })
            .map(move |sink| {
                recursive_handle(sessman, stream, sink, Some(req));
            });

        Box::new(fut)
    } else {
        Box::new(future::ok(()))
    }
}
