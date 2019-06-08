use std::collections::HashMap;
use std::time::{Duration, Instant};

use log::{warn, info};
use ksuid::Ksuid;

use crate::sni::SocketAddrPair;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Handshaking,
    BackendConnecting,
    Connected,
    ShutdownRead,
    ShutdownWrite,
    Shutdown,
}

pub struct SessionCreateCommand {
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
}

pub struct SessionCommand {
    pub session_id: Ksuid,
    pub data: SessionCommandData,
}

pub enum SessionCommandData {
    Destroy,
    Create(SessionCreateCommand),
    StartConnecting,
    Connected(String, SocketAddrPair, Duration),
    XmitClientToBackend(u64),
    XmitBackendToClient(u64),
    ShutdownRead,
    ShutdownWrite,
    Shutdown,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match *self {
            SessionState::Handshaking => "handshaking",
            SessionState::BackendConnecting => "backend-connecting",
            SessionState::Connected => "connected",
            SessionState::ShutdownRead => "shutdown-read",
            SessionState::ShutdownWrite => "shutdown-write",
            SessionState::Shutdown => "shutdown",
        }
    }
}

pub struct Session {
    // use futures::future::{Empty, Abortable, AbortHandle};
    // pub client_stream_abort: Abortable<Empty<()>>,
    // pub client_stream_abort_handle: AbortHandle,
    // pub backend_stream_abort: Abortable<Empty<()>>,
    // pub backend_stream_abort_handle: AbortHandle,
    exportable: SessionExport,
}

#[derive(Clone)]
pub struct SessionExport {
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub backend_name: Option<String>,
    pub backend_conn: Option<SocketAddrPair>,
    pub backend_connect_latency: Option<Duration>,
    pub state: SessionState,
    pub last_xmit: Instant,
    pub bytes_client_to_backend: u64,
    pub bytes_backend_to_client: u64,
}

impl SessionExport {
    pub fn from_session(session: &Session) -> SessionExport {
        session.exportable.clone()
    }
}

impl Session {
    pub fn new(session_id: Ksuid, creat: &SessionCreateCommand) -> Session {
        Session {
            exportable: SessionExport {
                session_id,
                start_time: creat.start_time,
                client_conn: creat.client_conn.clone(),
                backend_name: None,
                backend_conn: None,
                backend_connect_latency: None,
                state: SessionState::Handshaking,
                last_xmit: Instant::now(),
                bytes_client_to_backend: 0,
                bytes_backend_to_client: 0,
            }
        }
    }
}

pub struct SessionManager {
    sessions: HashMap<Ksuid, Session>,
}

impl SessionManager {
    pub fn new() -> SessionManager {
        SessionManager {
            sessions: HashMap::new(),
        }
    }

    pub fn get_sessions(&self) -> Vec<SessionExport> {
        self.sessions.values().map(SessionExport::from_session).collect()
    }

    pub fn apply_command(&mut self, cmd: &SessionCommand) {
        match cmd.data {
            SessionCommandData::Destroy => {
                info!("removing session: {} => {}", cmd.session_id.fmt_base62(),
                    self.sessions.remove(&cmd.session_id).is_some()
                    );
                // self.sessions.remove(&cmd.session_id).is_some()
                return;
            },
            SessionCommandData::Create(ref creat) => {
                self.sessions.insert(cmd.session_id,
                    Session::new(cmd.session_id, creat));
                return;
            },
            _ => (),
        }

        let mut inst = self.sessions.get_mut(&cmd.session_id).unwrap();

        let pre_state = inst.exportable.state.clone();
        let mut state_change = false;
        match cmd.data {
            SessionCommandData::Destroy
            | SessionCommandData::Create(..)
            | SessionCommandData::StartConnecting => (),
            SessionCommandData::Connected(ref bn, ref sap, lat) => {
                state_change = true;
                inst.exportable.state = SessionState::Connected;
                inst.exportable.backend_name = Some(bn.clone());
                inst.exportable.backend_conn = Some(sap.clone());
                inst.exportable.backend_connect_latency = Some(lat);
            }
            SessionCommandData::XmitClientToBackend(bytes) => {
                inst.exportable.bytes_client_to_backend += bytes;
                inst.exportable.last_xmit = Instant::now();
            }
            SessionCommandData::XmitBackendToClient(bytes) => {
                inst.exportable.bytes_backend_to_client += bytes;
                inst.exportable.last_xmit = Instant::now();
            }
            SessionCommandData::ShutdownRead => {
                state_change = true;
                inst.exportable.state = if inst.exportable.state == SessionState::ShutdownWrite {
                    SessionState::Shutdown
                } else {
                    SessionState::ShutdownRead
                };
            }
            SessionCommandData::ShutdownWrite => {
                state_change = true;
                inst.exportable.state = if inst.exportable.state == SessionState::ShutdownRead {
                    SessionState::Shutdown
                } else {
                    SessionState::ShutdownWrite
                };
            }
            SessionCommandData::Shutdown => {
                state_change = true;
                warn!("this shouldn't happen - shutdown should be derived from shutdown-read and shutdown-write");
                inst.exportable.state = SessionState::Shutdown;
            }
        }
        if state_change {
            info!("{} {:?} -> {:?}", cmd.session_id.fmt_base62(), pre_state, inst.exportable.state);
        }
    }
}
