use std::collections::BTreeMap;
use std::collections::HashMap;
use std::mem;
use std::time::{Duration, Instant};

use ksuid::Ksuid;

use crate::context;
use crate::model::SocketAddrPair;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Handshaking,
    BackendResolving,
    BackendConnecting,
    Connected,
    ShutdownRead,
    ShutdownWrite,
    Shutdown,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match *self {
            SessionState::Handshaking => "handshaking",
            SessionState::BackendResolving => "backend-resolving",
            SessionState::BackendConnecting => "backend-connecting",
            SessionState::Connected => "connected",
            SessionState::ShutdownRead => "shutdown-read",
            SessionState::ShutdownWrite => "shutdown-write",
            SessionState::Shutdown => "shutdown",
        }
    }
}

pub struct Session {
    holder: Option<context::Holder>,
    exportable: SessionExport,
}

#[derive(Clone)]
pub struct SessionExport {
    pub session_id: Ksuid,
    pub start_time: Instant,
    pub client_conn: SocketAddrPair,
    pub backend_name: Option<String>,
    // pub backend_conn: Option<SocketAddrPair>,
    pub backend_resolve_start: Option<Instant>,
    pub backend_resolve_latency: Option<Duration>,
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
    pub fn new(
        session_id: Ksuid,
        start_time: Instant,
        client_addr: SocketAddrPair,
        holder: context::Holder,
    ) -> Session {
        Session {
            holder: Some(holder),
            exportable: SessionExport {
                session_id,
                start_time: start_time,
                client_conn: client_addr,
                backend_name: None,
                // backend_conn: None,
                backend_resolve_latency: None,
                backend_resolve_start: None,
                backend_connect_latency: None,
                backend_connect_start: None,
                state: SessionState::Handshaking,
                last_xmit: Instant::now(),
                bytes_client_to_backend: 0,
                bytes_backend_to_client: 0,
            },
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

    pub fn add_session(&mut self, session: Session) {
        self.sessions.insert(session.exportable.session_id, session);
    }

    pub fn destroy(&mut self, ksuid: &Ksuid) -> Result<(), ()> {
        if let Some(sess) = self.sessions.get_mut(ksuid) {
            if let Some(holder) = sess.holder.take() {
                drop(holder);
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
        self.sessions
            .values()
            .map(SessionExport::from_session)
            .collect()
    }

    pub fn mark_backend_resolving(&mut self, session_id: &Ksuid, backend_name: &str) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = SessionState::BackendResolving;
        inst.exportable.backend_name = Some(backend_name.to_string());
        inst.exportable.backend_resolve_start = Some(Instant::now());

        self.cleanup();
    }

    pub fn mark_backend_connecting(&mut self, session_id: &Ksuid) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = SessionState::BackendConnecting;
        inst.exportable.backend_connect_start = Some(Instant::now());
        if let Some(start) = inst.exportable.backend_resolve_start {
            inst.exportable.backend_resolve_latency = Some(start.elapsed());
        }
        self.cleanup();
    }

    pub fn mark_connected(&mut self, session_id: &Ksuid) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = SessionState::Connected;
        if let Some(start) = inst.exportable.backend_connect_start {
            inst.exportable.backend_connect_latency = Some(start.elapsed());
        }

        self.cleanup();
    }

    pub fn mark_shutdown_read(&mut self, session_id: &Ksuid) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = if inst.exportable.state == SessionState::ShutdownWrite {
            let key = (Instant::now() + Duration::new(30, 0), self.tie_break_ctr);
            self.tie_break_ctr += 1;
            self.tie_break_ctr &= 0x7FFF_FFFF;
            self.removal_queue.insert(key, *session_id);

            SessionState::Shutdown
        } else {
            SessionState::ShutdownRead
        };

        self.cleanup();
    }

    pub fn mark_shutdown_write(&mut self, session_id: &Ksuid) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = if inst.exportable.state == SessionState::ShutdownRead {
            let key = (Instant::now() + Duration::new(30, 0), self.tie_break_ctr);
            self.tie_break_ctr += 1;
            self.tie_break_ctr &= 0x7FFF_FFFF;
            self.removal_queue.insert(key, *session_id);

            SessionState::Shutdown
        } else {
            SessionState::ShutdownWrite
        };

        self.cleanup();
    }

    pub fn mark_shutdown(&mut self, session_id: &Ksuid) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.state = SessionState::Shutdown;

        let key = (Instant::now() + Duration::new(30, 0), self.tie_break_ctr);
        self.tie_break_ctr += 1;
        self.tie_break_ctr &= 0x7FFF_FFFF;
        self.removal_queue.insert(key, *session_id);

        self.cleanup();
    }

    pub fn handle_xmit_backend_to_client(&mut self, session_id: &Ksuid, bytes: u64) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.bytes_backend_to_client += bytes;
        inst.exportable.last_xmit = Instant::now();

        self.cleanup();
    }

    pub fn handle_xmit_client_to_backend(&mut self, session_id: &Ksuid, bytes: u64) {
        let inst = self.sessions.get_mut(session_id).unwrap();
        inst.exportable.bytes_client_to_backend += bytes;
        inst.exportable.last_xmit = Instant::now();

        self.cleanup();
    }
}
