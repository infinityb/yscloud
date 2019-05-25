use std::collections::HashMap;
use std::time::{Duration, Instant};

use ksuid::Ksuid;

use crate::sni::SocketAddrPair;

#[derive(Clone)]
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
    Connected(SocketAddrPair, Duration),
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

#[derive(Clone)]
pub struct Session {
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub backend_conn: Option<SocketAddrPair>,
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
            backend_conn: None,
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
}

impl SessionManager {
    pub fn new() -> SessionManager {
        SessionManager {
            sessions: HashMap::new(),
        }
    }

    pub fn get_sessions(&self) -> Vec<Session> {
        self.sessions.values().cloned().collect()
    }

    pub fn apply_command(&mut self, cmd: &SessionCommand) {
        match cmd.data {
            SessionCommandData::Destroy => {
                self.sessions.remove(&cmd.session_id);
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

        match cmd.data {
            SessionCommandData::Destroy
            | SessionCommandData::Create(..)
            | SessionCommandData::StartConnecting => (),
            SessionCommandData::Connected(ref sap, lat) => {
                inst.state = SessionState::Connected;
                inst.backend_conn = Some(sap.clone());
                inst.backend_connect_latency = Some(lat);
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
                inst.state = SessionState::ShutdownRead;
            }
            SessionCommandData::ShutdownWrite => {
                inst.state = SessionState::ShutdownWrite;
            }
            SessionCommandData::Shutdown => {
                inst.state = SessionState::Shutdown;
            }
        }
    }
}
