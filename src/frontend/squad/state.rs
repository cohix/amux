//! State owned by the squad HTTP daemon.
//!
//! Everything of substance is Layer 2's [`SquadDaemonHandles`]. What this adds
//! is transport-local: when the listener bound, and where.

use std::sync::Mutex;
use std::time::Instant;

use crate::command::commands::squad::daemon_runtime::SquadDaemonHandles;
use crate::data::session_manager::SessionManager;

/// All daemon-local dependencies presented to the squad router.
pub struct SquadAppState {
    pub handles: SquadDaemonHandles,
    pub sessions: std::sync::Arc<SessionManager>,
    pub started_at: Instant,
    /// Filled only after the listener has successfully bound.
    pub bound_addr: Mutex<Option<String>>,
}

impl SquadAppState {
    pub fn new(handles: SquadDaemonHandles) -> Self {
        Self {
            sessions: handles.session_manager(),
            handles,
            started_at: Instant::now(),
            bound_addr: Mutex::new(None),
        }
    }

    /// The endpoint the listener bound, once it has.
    pub fn bound_addr(&self) -> Option<String> {
        self.bound_addr
            .lock()
            .expect("squad bound-address mutex poisoned")
            .clone()
    }
}
