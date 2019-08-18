use std::io;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::pin::Pin;

use bytes::BytesMut;
use failure::{Error, Fail};
use ksuid::Ksuid;
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};
use futures::prelude::{Sink, Future, Stream};
use log::warn;

use crate::sni::SocketAddrPair;
use crate::state_track::{SessionExport, SessionManager};

#[derive(Debug, Fail)]
#[fail(display = "protocol error")]
struct MgmtProtocolError {
    recoverable: bool,
    message: String,
}

pub struct AsciiManagerServer {}

impl AsciiManagerServer {
    pub fn new() -> AsciiManagerServer {
        AsciiManagerServer {}
    }
}

pub enum AsciiManagerRequest {
    PrintActiveSessions,
    DestroySession(Ksuid),
}

fn destroy_session_from_str_iter<'a, I>(mut parts: I) -> Result<Ksuid, Error>
where
    I: Iterator<Item = &'a str>,
{
    let ksuid = parts.next().ok_or_else(|| MgmtProtocolError {
        recoverable: true,
        message: "destroy-session takes one argument (a ksuid)".into(),
    })?;

    let ksuid = Ksuid::from_base62(ksuid).map_err(|_err| MgmtProtocolError {
        recoverable: true,
        message: "destroy-session first argument: invalid ksuid".into(),
    })?;

    Ok(ksuid)
}

impl FromStr for AsciiManagerRequest {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split_whitespace();
        let command = parts.next().ok_or_else(|| MgmtProtocolError {
            recoverable: true,
            message: format!("unknown command: {}", value),
        })?;

        match command {
            "print-active-sessions" => Ok(AsciiManagerRequest::PrintActiveSessions),
            "destroy-session" => {
                destroy_session_from_str_iter(parts).map(AsciiManagerRequest::DestroySession)
            }
            _ => Err(MgmtProtocolError {
                recoverable: true,
                message: format!("unknown command: {}", value),
            }
            .into()),
        }
    }
}

pub enum AsciiManagerResponse {
    PrintActive(Vec<SessionExport>),
    GenericOk,
    GenericError,
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

fn write_session_info<W>(wri: &mut W, si: &SessionExport) -> io::Result<()>
where
    W: std::io::Write,
{
    write!(wri, "{} {} ", si.session_id.fmt_base62(), si.state.as_str())?;
    write_sock_addr_pair(wri, &si.client_conn)?;
    if let Some(ref bn) = si.backend_name {
        write!(wri, "backend_name={} ", bn)?;
    }
    if let Some(ref bc) = si.backend_conn {
        write!(wri, "backend_connect_addr=")?;
        write_sock_addr_pair(wri, bc)?;
    }
    if let Some(ref bcl) = si.backend_connect_latency {
        write!(wri, "backend_connect_latency_ms={} ", bcl.as_millis())?;
    }
    write!(
        wri,
        "session_age_ms={} ",
        si.start_time.elapsed().as_millis()
    )?;
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
    sess_infos: &[SessionExport],
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

    type Error = Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut scratch = vec![0; 1024];

        match item {
            AsciiManagerResponse::PrintActive(ref sessinfo) => {
                encode_print_active_sessions(&mut scratch, dst, sessinfo)?;
            }
            AsciiManagerResponse::GenericOk => {
                dst.extend_from_slice(b"OK\n");
            }
            AsciiManagerResponse::GenericError => {
                dst.extend_from_slice(b"ERROR\n");
            }
        }

        Ok(())
    }
}

impl Decoder for AsciiManagerServer {
    type Item = AsciiManagerRequest;

    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        use std::str::from_utf8;

        let line = match src.iter().position(|x| *x == b'\n') {
            Some(eol) => src.split_to(eol + 1).freeze(),
            None => {
                if 4096 < src.len() {
                    return Err(MgmtProtocolError {
                        recoverable: false,
                        message: "line too long - connection terminated".to_string(),
                    }
                    .into());
                }

                return Ok(None);
            }
        };

        let mut line = from_utf8(&*line).map_err(|e| MgmtProtocolError {
            recoverable: false,
            message: format!("could not process line: {}", e),
        })?;

        line = line.trim();
        AsciiManagerRequest::from_str(line).map(Some)
    }
}

pub fn start_management_client<A>(
    sessman: Arc<Mutex<SessionManager>>,
    client: Framed<A, AsciiManagerServer>,
) -> Pin<Box<dyn Future<Output=Result<(), Error>> + Send>>
where
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (sink, stream) = client.split();

    let fut = stream
        .into_future()
        .map_err(|(e, _stream)| e)
        .and_then(move |(req, stream)| recursive_handle(sessman, stream, sink, req));
    Box::new(fut)
}

fn handle_query(
    sessman: &Mutex<SessionManager>,
    req: AsciiManagerRequest,
) -> Result<AsciiManagerResponse, Error> {
    let mut sessions = sessman.lock().unwrap();
    match req {
        AsciiManagerRequest::PrintActiveSessions => {
            let sessions = sessions.get_sessions();
            Ok(AsciiManagerResponse::PrintActive(sessions))
        }
        AsciiManagerRequest::DestroySession(ref sess_id) => match sessions.destroy(sess_id) {
            Ok(()) => Ok(AsciiManagerResponse::GenericOk),
            Err(()) => Ok(AsciiManagerResponse::GenericError),
        },
    }
}

async fn recursive_handle<St, Si>(
    sessman: Arc<Mutex<SessionManager>>,
    _stream: St,
    sink: Si,
    req: Option<AsciiManagerRequest>,
) -> Pin<Box<dyn Future<Output=Result<(), Error>> + Send>>
where
    St: Stream<Item=Result<AsciiManagerRequest, Error>> + Send + Sync + 'static,
    Si: Sink<AsciiManagerResponse, Error = Error> + Send + Sync + 'static,
{
    async {
        if let Some(req) = req {
            match handle_query(&sessman, req) {
                Ok(resp) => {
                    sink.send(resp).await?;
                },
                Err(err) => {
                    warn!("client error: {}", err);
                    sink.send(AsciiManagerResponse::GenericError).await?;
                }
            }
            Ok(())
        } else {
            Ok(())
        }
    }.boxed()
}
