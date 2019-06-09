use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::mem;
use std::error::Error as StdError;

use futures::future::{empty, abortable, Empty, Shared, Abortable, AbortHandle, FutureExt};
use log::{debug, warn, info};
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
    start_time: Instant,
    client_conn: SocketAddrPair,
    abort_handle: AbortHandle,
}

pub type SessionAbortFuture = Shared<Abortable<Empty<()>>>;

impl SessionCreateCommand {
    pub fn new(start_time: Instant, client_conn: SocketAddrPair) -> (SessionCreateCommand, SessionAbortFuture) {
        let (abort_future, abort_handle) = abortable(empty());

        (SessionCreateCommand {
            start_time,
            client_conn,
            abort_handle,
        }, abort_future.shared())
    }
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

pub struct Session {
    abort_handle: Option<AbortHandle>,
    exportable: SessionExport,
}

#[derive(Clone)]
pub struct SessionExport {
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

impl SessionExport {
    pub fn from_session(session: &Session) -> SessionExport {
        session.exportable.clone()
    }
}

impl Session {
    fn new(session_id: Ksuid, creat: SessionCreateCommand) -> Session {
        Session {
            abort_handle: Some(creat.abort_handle),
            exportable: SessionExport {
                session_id,
                start_time: creat.start_time,
                client_conn: creat.client_conn.clone(),
                backend_name: None,
                backend_conn: None,
                backend_connect_latency: None,
                backend_connect_start: None,
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
    removal_queue: BTreeMap<(Instant, u32), Ksuid>,
    // who knows what kind of broken clocks could be out there...
    tie_break_ctr: u32,
}

impl SessionManager {
    pub fn new() -> SessionManager {
        SessionManager {
            sessions: HashMap::new(),
            removal_queue: BTreeMap::new(),
            tie_break_ctr: 0,
        }
    }

    pub fn destroy(&mut self, ksuid: &Ksuid) -> Result<(), ()> {
        if let Some(sess) = self.sessions.get_mut(ksuid) {
            if let Some(handle) = sess.abort_handle.take() {
                handle.abort();
                return Ok(());
            }
        }
        Err(())
    }

    fn cleanup(&mut self) {
        let key = (Instant::now(), 0xFFFF_FFFF);
        let keep_in_queue = self.removal_queue.split_off(&key);
        let old = mem::replace(&mut self.removal_queue, keep_in_queue);
        for ksuid in old.values() {
            self.sessions.remove(ksuid);
        }
    }

    pub fn get_sessions(&self) -> Vec<SessionExport> {
        self.sessions.values().map(SessionExport::from_session).collect()
    }

    pub fn apply_command(&mut self, cmd: SessionCommand) {
        self.cleanup();

        if let SessionCommandData::Create(creat) = cmd.data {
            self.sessions.insert(cmd.session_id,
                Session::new(cmd.session_id, creat));
            return;
        }

        let mut inst = self.sessions.get_mut(&cmd.session_id).unwrap();

        let pre_state = inst.exportable.state.clone();
        let mut state_change = false;
        match cmd.data {
            SessionCommandData::Create(..) => (),
            SessionCommandData::Destroy => {
                //
            }
            
            SessionCommandData::BackendConnecting(ref backend_name) => {
                state_change = true;
                inst.exportable.state = SessionState::BackendConnecting;
                inst.exportable.backend_name = Some(backend_name.clone());
                inst.exportable.backend_connect_start = Some(Instant::now());
            }
            SessionCommandData::Connected(ref sap) => {
                state_change = true;
                inst.exportable.state = SessionState::Connected;
                inst.exportable.backend_conn = Some(sap.clone());
                if let Some(start) = inst.exportable.backend_connect_start {
                    inst.exportable.backend_connect_latency = Some(start.elapsed());
                }
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
        }

        if inst.exportable.state == SessionState::Shutdown {
            debug!("scheduling removal of {}", cmd.session_id.fmt_base62());
            
            let key = (Instant::now() + Duration::new(30, 0), self.tie_break_ctr);
            self.tie_break_ctr += 1;
            self.tie_break_ctr &= 0x7FFF_FFFF;

            self.removal_queue.insert(key, cmd.session_id);
            return;
        }

        if state_change {
            info!("{} {:?} -> {:?}", cmd.session_id.fmt_base62(), pre_state, inst.exportable.state);
        }
    }
}
