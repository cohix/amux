//! The bottom-row squad health indicator (WI 0112 Part 2).
//!
//! An app-level poller — independent of the squad tab, which may never have
//! been opened, and of the tab's own `SquadTaskPoller`, which fetches only
//! while that tab is focused — probes the squad daemon every ten seconds and
//! publishes one of six states. The renderer paints a coloured `●` for it on
//! every tab. The poller holds no policy: [`classify`] is the whole decision
//! and is a pure function.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::command::commands::squad::gateway::TaskGateway;
use crate::command::commands::squad::supervisor::SquadGatewayResolver;
use crate::command::error::CommandError;
use crate::data::config::env::Env;
use crate::data::fs::task_store::{RunStatus, Task};

/// How often the indicator re-probes the daemon.
pub const SQUAD_INDICATOR_INTERVAL: Duration = Duration::from_secs(10);
/// How long one probe may take before it is reported as unreachable. Keeps a
/// hung daemon from stacking probes: at most one is ever in flight.
pub const SQUAD_INDICATOR_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the indicator reports. Ordered by precedence, most severe first —
/// [`classify`] returns the first that applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquadIndicator {
    /// No probe has completed yet. Rendered like `NotRunning`.
    Unknown,
    /// No squad daemon process is running (pidfile check).
    NotRunning,
    /// A daemon is running but this TUI got no successful answer from it: no
    /// endpoint sidecar, no bearer key (a 401), connection refused, timeout,
    /// or any other transport or HTTP error.
    Unreachable,
    /// Reachable, and at least one task's most recent run failed.
    Failed,
    /// Reachable, and at least one task is executing right now.
    Running,
    /// Reachable, nothing failed, nothing running.
    Healthy,
}

/// Cross-thread handle: the poller writes, the renderer reads.
pub type SharedSquadIndicator = Arc<Mutex<SquadIndicator>>;

/// Map one probe's result onto an indicator state. `daemon_running` is the
/// pidfile answer; `probe` is the task list the daemon returned, or why it
/// did not. Pure, so every row of the state table is unit-tested without a
/// daemon.
pub fn classify(daemon_running: bool, probe: Result<&[Task], &CommandError>) -> SquadIndicator {
    if !daemon_running {
        return SquadIndicator::NotRunning;
    }
    let Ok(tasks) = probe else {
        return SquadIndicator::Unreachable;
    };
    // Red beats blue: a failure needs attention and persists; a running task
    // is transient and shows once the failure is cleared or another run
    // starts.
    if tasks
        .iter()
        .any(|task| task.last_run_status == Some(RunStatus::Failed))
    {
        return SquadIndicator::Failed;
    }
    if tasks
        .iter()
        .any(|task| task.last_run_status == Some(RunStatus::Running))
    {
        return SquadIndicator::Running;
    }
    SquadIndicator::Healthy
}

/// The background probe. Started once per TUI process from `tui::run`, never
/// from `App::new`, so unit-test apps never touch `~/.awman/squad`.
pub struct SquadIndicatorPoller {
    shared: SharedSquadIndicator,
}

impl SquadIndicatorPoller {
    pub fn new(shared: SharedSquadIndicator) -> Self {
        Self { shared }
    }

    /// Ticks every [`SQUAD_INDICATOR_INTERVAL`] until `cancel` fires. The
    /// first probe runs immediately so the indicator leaves `Unknown` on the
    /// first tick rather than ten seconds in.
    pub fn start(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SQUAD_INDICATOR_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let state = tokio::time::timeout(SQUAD_INDICATOR_PROBE_TIMEOUT, probe_once())
                            .await
                            .unwrap_or(SquadIndicator::Unreachable);
                        if let Ok(mut guard) = self.shared.lock() {
                            *guard = state;
                        }
                    }
                }
            }
        })
    }
}

/// One probe: pidfile, then a keyless-or-existing-key gateway, then `list`.
/// `Env` is re-read every time so a key minted mid-session (published into
/// the process environment by the supervisor) is picked up without a restart.
async fn probe_once() -> SquadIndicator {
    let supervisor = match SquadGatewayResolver::from_env(&Env::from_process()) {
        Ok(supervisor) => supervisor,
        Err(_) => return SquadIndicator::Unreachable,
    };
    match supervisor.daemon_is_running() {
        Ok(true) => {}
        Ok(false) => return SquadIndicator::NotRunning,
        Err(_) => return SquadIndicator::Unreachable,
    }
    let gateway = match supervisor.probe_gateway() {
        Ok(Some(gateway)) => gateway,
        Ok(None) | Err(_) => return SquadIndicator::Unreachable,
    };
    match gateway.list().await {
        Ok(tasks) => classify(true, Ok(&tasks)),
        Err(error) => classify(true, Err(&error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::fs::task_store::{MountScope, TaskStatus};
    use chrono::Utc;
    use std::path::PathBuf;

    fn task(name: &str, last_run_status: Option<RunStatus>) -> Task {
        let now = Utc::now();
        Task {
            id: name.to_string(),
            name: name.to_string(),
            description: "test".into(),
            repo_scope: PathBuf::from("/workspace"),
            mount_scope: MountScope::Directory,
            overlays: Vec::new(),
            interval_secs: 60,
            status: TaskStatus::Active,
            agent: None,
            model: None,
            backoff_until: None,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            trigger_requested_at: None,
            last_run_status,
        }
    }

    #[test]
    fn a_daemon_that_is_not_running_is_grey_whatever_the_probe_says() {
        let tasks = [task("a", Some(RunStatus::Failed))];
        assert_eq!(classify(false, Ok(&tasks)), SquadIndicator::NotRunning);
        assert_eq!(
            classify(false, Err(&CommandError::RemoteTimeout)),
            SquadIndicator::NotRunning
        );
    }

    #[test]
    fn every_probe_error_is_unreachable() {
        for error in [
            CommandError::RemoteTimeout,
            CommandError::RemoteConnectionRefused("refused".into()),
            CommandError::RemoteHttpStatus {
                status: 401,
                body: "Invalid API key.".into(),
            },
            CommandError::RemoteHttpStatus {
                status: 500,
                body: "boom".into(),
            },
            CommandError::RemoteTransport("eof".into()),
        ] {
            assert_eq!(
                classify(true, Err(&error)),
                SquadIndicator::Unreachable,
                "{error}"
            );
        }
    }

    #[test]
    fn a_reachable_daemon_with_no_tasks_is_healthy() {
        assert_eq!(classify(true, Ok(&[])), SquadIndicator::Healthy);
    }

    #[test]
    fn a_failed_last_run_is_red_and_beats_a_running_task() {
        let tasks = [
            task("a", Some(RunStatus::Running)),
            task("b", Some(RunStatus::Failed)),
        ];
        assert_eq!(classify(true, Ok(&tasks)), SquadIndicator::Failed);
    }

    #[test]
    fn a_running_task_is_blue() {
        let tasks = [
            task("a", Some(RunStatus::WorkflowExecuted)),
            task("b", Some(RunStatus::Running)),
        ];
        assert_eq!(classify(true, Ok(&tasks)), SquadIndicator::Running);
    }

    #[test]
    fn interrupted_and_ordinary_outcomes_are_healthy() {
        let tasks = [
            task("a", Some(RunStatus::Interrupted)),
            task("b", Some(RunStatus::NotTriggered)),
            task("c", Some(RunStatus::WorkflowExecuted)),
            task("d", None),
        ];
        assert_eq!(classify(true, Ok(&tasks)), SquadIndicator::Healthy);
    }
}
