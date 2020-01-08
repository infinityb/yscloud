use std::io;
use std::str::FromStr;
use std::sync::Arc;
use std::borrow::Cow;
use std::collections::BTreeSet;

use bytes::{Bytes, BytesMut};
use failure::{Error, Fail};
use ksuid::Ksuid;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use log::{debug, info};

use crate::resolver::BackendManager;
use crate::resolver::{NetworkLocation, NetworkLocationAddress};
use crate::sni_base::SocketAddrPair;
use crate::state_track::{SessionExport, SessionManager};
use crate::ioutil::{read_into, write_from};

#[derive(Debug, Fail)]
#[fail(display = "protocol error")]
struct MgmtProtocolError {
    recoverable: bool,
    message: String,
}

#[derive(Debug)]
pub enum AsciiManagerRequest {
    PrintActiveSessions,
    DestroySession(Ksuid),
    ReplaceBackend(BackendArgs),
    AddHeldBackend(BackendArgs),
    DumpBackends,
    RemoveBackends(RemoveBackends),
    Quit,
}

#[derive(Clone, Debug)]
pub struct BackendArgs {
    hostname: String,
    target_address: NetworkLocation,
    flags: Vec<BackendArgsFlags>,
}

#[derive(Debug, Clone)]
pub enum BackendArgsFlags {
    UseHaproxy(i8),
}

#[derive(Debug)]
pub struct RemoveBackends {
    hostname: String,
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

fn backend_args_from_str_iter<'a, I>(mut parts: I) -> Result<BackendArgs, Error>
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

    Ok(BackendArgs {
        hostname: hostname.into(),
        target_address,
        flags: Vec::new(),
    })
}

fn remove_backends_from_str_iter<'a, I>(mut parts: I) -> Result<RemoveBackends, Error>
where
    I: Iterator<Item = &'a str>,
{
    let hostname = parts.next().ok_or_else(|| MgmtProtocolError {
        recoverable: true,
        message: "replace-backend takes arguments: <hostname> <target-address> [...flags]".into(),
    })?;

    Ok(RemoveBackends {
        hostname: hostname.into(),
    })
}

fn decode_ascii_manager_request(src: &mut BytesMut) -> Result<Option<AsciiManagerRequest>, Error> {
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
                backend_args_from_str_iter(parts).map(AsciiManagerRequest::ReplaceBackend)
            }
            "add-held-backend" => {
                backend_args_from_str_iter(parts).map(AsciiManagerRequest::AddHeldBackend)
            }
            "remove-backends" => {
                remove_backends_from_str_iter(parts).map(AsciiManagerRequest::RemoveBackends)
            }
            "quit" => Ok(AsciiManagerRequest::Quit),
            _ => Err(MgmtProtocolError {
                recoverable: true,
                message: format!("unknown command: {}", value),
            }
            .into()),
        }
    }
}

pub enum AsciiManagerResponse<'a> {
    PrintActive(Cow<'a, [SessionExport]>),
    GenericOk,
    GenericError(Cow<'a, str>),
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
        SocketAddrPair::Unix => {
            write!(wri, "unix,unix ")?;
        }
        SocketAddrPair::Unknown => {
            write!(wri, "unknown,unknown ")?;
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

fn encode_ascii_manager_response<'a>(item: &AsciiManagerResponse<'a>, dst: &mut BytesMut) -> Result<(), Error> {
    let mut scratch = [0; 1024];
    match *item {
        AsciiManagerResponse::PrintActive(ref sessinfo) => {
            encode_print_active_sessions(&mut scratch[..], dst, sessinfo)?;
        }
        AsciiManagerResponse::GenericOk => {
            dst.extend_from_slice(b"OK\n");
        }
        AsciiManagerResponse::GenericError(ref message) => {
            dst.extend_from_slice(b"ERROR");
            if message.len() > 0 {
                dst.extend_from_slice(b": ");
                dst.extend_from_slice(message.as_bytes());   
            }
            dst.extend_from_slice(b"\n");
        }
        AsciiManagerResponse::DumpBackends(ref bm) => {
            let data = serde_json::to_string(&*bm.backends).unwrap();
            dst.extend_from_slice(data.as_bytes());
            dst.extend_from_slice(b"\nOK\n");
        }
    }

    Ok(())
}

pub async fn start_management_client<Socket>(
    sessman: Arc<Mutex<SessionManager>>,
    backends: Arc<Mutex<BackendManager>>,
    mut socket: Socket,
) -> Result<(), Error>
where
    Socket: AsyncRead + AsyncWrite + Unpin,
{
    let mut read_buf = BytesMut::with_capacity(1024);
    let mut write_buf = BytesMut::with_capacity(16 * 1024);
    let mut to_write: Bytes = write_buf.split_off(0).freeze();

    let mut held_backends: BTreeSet<(String, Ksuid)> = BTreeSet::new();

    loop {
        if to_write.len() > 0 {
            if write_from(&mut socket, &mut to_write).await? == 0 {
                break;
            }

            if to_write.len() > 0 {
                continue;
            }
        }

        if read_into(&mut socket, &mut read_buf).await? == 0 {
            break;
        }

        let request = match decode_ascii_manager_request(&mut read_buf) {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(err) => {
                if let Some(mgmt_err) = err.downcast_ref::<MgmtProtocolError>() {
                    if mgmt_err.recoverable {
                        let item = AsciiManagerResponse::GenericError(Cow::Borrowed(&mgmt_err.message));
                        encode_ascii_manager_response(&item, &mut write_buf)?;
                        to_write = write_buf.split().freeze();
                        continue;
                    }
                }
                return Err(err);
            }
        };

        if let AsciiManagerRequest::Quit = request {
            break;
        }

        let response = handle_query(&sessman, &backends, request, &mut held_backends).await?;
        encode_ascii_manager_response(&response, &mut write_buf)?;
        to_write = write_buf.split().freeze();
    }

    if held_backends.len() > 0 {
        debug!("destroying held bindings...");

        let mut backends = backends.lock().await;
        for h in held_backends.into_iter() {
            let h: (String, Ksuid) = h;
            backends.remove_backend(&h.0, h.1);
        }
    }

    info!("mgmt client close completed");

    Ok(())
}

async fn handle_query(
    sessman: &Arc<Mutex<SessionManager>>,
    backend_man: &Arc<Mutex<BackendManager>>,
    req: AsciiManagerRequest,
    held_backends: &mut BTreeSet<(String, Ksuid)>,
) -> Result<AsciiManagerResponse<'static>, Error> {
    match req {
        AsciiManagerRequest::Quit => unreachable!(),
        AsciiManagerRequest::PrintActiveSessions => {
            let sessions = sessman.lock().await;
            let sessions = sessions.get_sessions();
            Ok(AsciiManagerResponse::PrintActive(Cow::Owned(sessions)))
        }
        AsciiManagerRequest::DestroySession(ref sess_id) => {
            let mut sessions = sessman.lock().await;
            match sessions.destroy(sess_id) {
                Ok(()) => Ok(AsciiManagerResponse::GenericOk),
                Err(()) => Ok(AsciiManagerResponse::GenericError(Cow::Borrowed(""))),
            }
        }
        AsciiManagerRequest::DumpBackends => {
            let backends = backend_man.lock().await;
            Ok(AsciiManagerResponse::DumpBackends(BackendManager::clone(&backends)))
        }
        AsciiManagerRequest::AddHeldBackend(repl) => {
            let mut backends = backend_man.lock().await;
            let bid = backends.add_backend(&repl.hostname, repl.target_address);
            held_backends.insert((repl.hostname, bid));
            Ok(AsciiManagerResponse::GenericOk)
        }
        AsciiManagerRequest::ReplaceBackend(repl) => {
            let mut backends = backend_man.lock().await;
            backends.replace_backend(&repl.hostname, repl.target_address);
            Ok(AsciiManagerResponse::GenericOk)
        }
        AsciiManagerRequest::RemoveBackends(remo) => {
            let mut backends = backend_man.lock().await;
            backends.remove_backends(&remo.hostname);
            Ok(AsciiManagerResponse::GenericOk)
        }
    }
}
