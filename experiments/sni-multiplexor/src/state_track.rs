use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use log::{debug, info};
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
    BackendConnecting(String),
    Connected(SocketAddrPair),
    XmitClientToBackend(u64),
    XmitBackendToClient(u64),
    ShutdownRead,
    ShutdownWrite,
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

#[derive(Clone)]
pub struct Session {
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub backend_name: Option<String>,
    pub backend_conn: Option<SocketAddrPair>,
    pub backend_connect_start: Option<Instant>,
    pub backend_connect_latency: Option<Duration>,
    pub state: SessionState,
    pub last_xmit: Instant,
    pub bytes_client_to_backend: u64,
    pub bytes_backend_to_client: u64,
}

impl Session {
    pub fn new(session_id: Ksuid, creat: &SessionCreateCommand) -> Session {
        Session {
            session_id,
            start_time: creat.start_time,
            client_conn: creat.client_conn.clone(),
            backend_name: None,
            backend_conn: None,
            backend_connect_start: None,
            backend_connect_latency: None,
            state: SessionState::Handshaking,
            last_xmit: Instant::now(),
            bytes_client_to_backend: 0,
            bytes_backend_to_client: 0,
        }
    }
}

pub struct SessionManager {
    sessions: HashMap<Ksuid, Session>,
    // removal_queue: BTreeMap<Instant, Ksuid>,
}

impl SessionManager {
    pub fn new() -> SessionManager {
        SessionManager {
            sessions: HashMap::new(),
            // removal_queue: BTreeMap::new(),
        }
    }

    pub fn get_sessions(&self) -> Vec<Session> {
        self.sessions.values().cloned().collect()
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

        let pre_state = inst.state.clone();
        let mut state_change = false;
        match cmd.data {
            SessionCommandData::Destroy
            | SessionCommandData::Create(..) => (),
            SessionCommandData::StartConnecting(ref backend_name) => {
                state_change = true;
                inst.backend_name = Some(backend_name.clone());
                inst.backend_connect_start = Some(Instant::now());
            }
            SessionCommandData::Connected(ref sap) => {
                state_change = true;
                inst.state = SessionState::Connected;
                inst.backend_conn = Some(sap.clone());
                if let Some(start) = inst.backend_connect_start {
                    inst.backend_connect_latency = Some(start.elapsed());
                }
            }
            SessionCommandData::XmitClientToBackend(bytes) => {
                inst.bytes_client_to_backend += bytes;
                inst.last_xmit = Instant::now();
            }
            SessionCommandData::XmitBackendToClient(bytes) => {
                inst.bytes_backend_to_client += bytes;
                inst.last_xmit = Instant::now();
            }
            SessionCommandData::ShutdownRead => {
                state_change = true;
                inst.state = if inst.state == SessionState::ShutdownWrite {
                    SessionState::Shutdown
                } else {
                    SessionState::ShutdownRead
                };
            }
            SessionCommandData::ShutdownWrite => {
                state_change = true;
                inst.state = if inst.state == SessionState::ShutdownRead {
                    SessionState::Shutdown
                } else {
                    SessionState::ShutdownWrite
                };
            }
        }
        if state_change {
            debug!("{} {:?} -> {:?}", cmd.session_id.fmt_base62(), pre_state, inst.state);
        }
    }
}
