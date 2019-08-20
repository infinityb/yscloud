use std::io;
use std::str::FromStr;

use bytes::BytesMut;
use failure::{Error, Fail};
use ksuid::Ksuid;
use tokio::codec::{Decoder, Encoder, Framed};
use tokio::io::{AsyncRead, AsyncWrite};

use tokio::sync::Lock;

use crate::resolver::BackendManager;
use crate::resolver::{NetworkLocation, NetworkLocationAddress};
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
    ReplaceBackend(ReplaceBackend),
    DumpBackends,
}

pub struct ReplaceBackend {
    hostname: String,
    target_address: NetworkLocation,
    flags: Vec<ReplaceBackendFlag>,
}

pub enum ReplaceBackendFlag {
    UseHaproxy(i8),
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

fn replace_backend_from_str_iter<'a, I>(mut parts: I) -> Result<ReplaceBackend, Error>
where
    I: Iterator<Item = &'a str>,
{
    let hostname = parts.next().ok_or_else(|| MgmtProtocolError {
        recoverable: true,
        message: "replace-backend takes arguments: <hostname> <target-address> [...flags]".into(),
    })?;
    let address: NetworkLocationAddress = parts
        .next()
        .ok_or_else(|| MgmtProtocolError {
            recoverable: true,
            message: "replace-backend takes arguments: <hostname> <target-address> [...flags]"
                .into(),
        })?
        .parse()
        .map_err(|err| MgmtProtocolError {
            recoverable: true,
            message: format!("failed to parse target-address: {}", err),
        })?;

    let mut use_haproxy = None;
    while let Some(flag) = parts.next() {
        match flag {
            "use-haproxy-v1" => {
                if use_haproxy.is_some() {
                    return Err(MgmtProtocolError {
                        recoverable: true,
                        message: "already set haproxy header".into(),
                    }
                    .into());
                }
                use_haproxy = Some(1);
            }
            "use-haproxy-v2" => {
                if use_haproxy.is_some() {
                    return Err(MgmtProtocolError {
                        recoverable: true,
                        message: "already set haproxy header".into(),
                    }
                    .into());
                }
                use_haproxy = Some(2);
            }
            _ => {
                return Err(MgmtProtocolError {
                    recoverable: true,
                    message: format!("unknown flag: {:?}", flag),
                }
                .into());
            }
        }
    }

    let target_address = NetworkLocation {
        use_haproxy_header_v: match use_haproxy {
            None => false,
            Some(1) => true,
            Some(2) => {
                return Err(MgmtProtocolError {
                    recoverable: true,
                    message: "use-haproxy-v2 not supported yet".into(),
                }
                .into());
            }
            _ => unreachable!(),
        },
        address,
        stats: (),
    };

    Ok(ReplaceBackend {
        hostname: hostname.into(),
        target_address,
        flags: Vec::new(),
    })
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
            "dump-backends" => Ok(AsciiManagerRequest::DumpBackends),
            "destroy-session" => {
                destroy_session_from_str_iter(parts).map(AsciiManagerRequest::DestroySession)
            }
            "replace-backend" => {
                replace_backend_from_str_iter(parts).map(AsciiManagerRequest::ReplaceBackend)
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
    DumpBackends(BackendManager),
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
    // if let Some(ref bc) = si.backend_conn {
    //     write!(wri, "backend_connect_addr=")?;
    //     write_sock_addr_pair(wri, bc)?;
    // }
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
            AsciiManagerResponse::DumpBackends(ref bm) => {
                let data = serde_json::to_string(&*bm.backends).unwrap();
                dst.extend_from_slice(data.as_bytes());
                dst.extend_from_slice(b"\nOK\n");
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

pub async fn start_management_client<A>(
    mut sessman: Lock<SessionManager>,
    mut backends: Lock<BackendManager>,
    client: Framed<A, AsciiManagerServer>,
) -> Result<(), Error>
where
    A: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;

    let (mut sink, mut stream) = client.split();
    loop {
        let (next_item, stream_tail) = stream.into_future().await;
        stream = stream_tail;

        let item = match next_item {
            Some(item) => item,
            None => break,
        };
        let item = match item {
            Ok(item) => item,
            Err(err) => {
                if let Some(mgmt_err) = err.downcast_ref::<MgmtProtocolError>() {
                    if mgmt_err.recoverable {
                        sink.send(AsciiManagerResponse::GenericError).await.unwrap();
                        continue;
                    }
                }
                return Err(err);
            }
        };

        let response = handle_query(&mut sessman, &mut backends, item).await?;
        sink.send(response).await.unwrap();
    }

    Ok(())
}

async fn handle_query(
    sessman: &mut Lock<SessionManager>,
    backend_man: &mut Lock<BackendManager>,
    req: AsciiManagerRequest,
) -> Result<AsciiManagerResponse, Error> {
    match req {
        AsciiManagerRequest::PrintActiveSessions => {
            let sessions = sessman.lock().await;
            let sessions = sessions.get_sessions();
            Ok(AsciiManagerResponse::PrintActive(sessions))
        }
        AsciiManagerRequest::DestroySession(ref sess_id) => {
            let mut sessions = sessman.lock().await;
            match sessions.destroy(sess_id) {
                Ok(()) => Ok(AsciiManagerResponse::GenericOk),
                Err(()) => Ok(AsciiManagerResponse::GenericError),
            }
        }
        AsciiManagerRequest::DumpBackends => {
            let backends = backend_man.lock().await;
            Ok(AsciiManagerResponse::DumpBackends(BackendManager::clone(&backends)))
        }
        AsciiManagerRequest::ReplaceBackend(repl) => {
            let mut backends = backend_man.lock().await;
            backends.replace_backend(&repl.hostname, repl.target_address);
            Ok(AsciiManagerResponse::GenericOk)
        }
    }
}
