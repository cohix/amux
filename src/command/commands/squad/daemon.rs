//! Daemon lifecycle and the shared daemon-discovery supervisor for squad.

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::command::commands::http_core::HttpCore;
use crate::command::commands::squad::commands::{SquadCommandFrontend, SquadServeConfig};
use crate::command::commands::squad::gateway::{DaemonStatus, RemoteTaskGateway};
use crate::command::commands::squad::key_setup;
use crate::command::commands::Command;
use crate::command::dispatch::Engines;
use crate::command::error::CommandError;
use crate::data::config::env::{Env, EnvSnapshot};
use crate::data::fs::daemon_process::{SQUAD_PLIST_LABEL, SQUAD_UNIT_NAME};
use crate::data::fs::{
    AcquireError, DaemonGuard, DaemonKind, DaemonProcess, SquadPaths, Termination,
};
use crate::data::message::{MessageLevel, UserMessage};
use crate::engine::auth::ApiKey;

#[derive(Debug, Clone)]
pub struct SquadStartFlags {
    pub port: u16,
    pub background: bool,
    pub refresh_key: bool,
    pub dangerously_skip_auth: bool,
}
#[derive(Debug, Clone)]
pub struct SquadStopFlags;
#[derive(Debug, Clone)]
pub struct SquadStatusFlags;
#[derive(Debug, Clone)]
pub struct SquadLogsFlags {
    pub follow: bool,
}

#[derive(Debug, Clone)]
pub enum SquadDaemonSubcommand {
    Start(SquadStartFlags),
    Stop(SquadStopFlags),
    Status(SquadStatusFlags),
    Logs(SquadLogsFlags),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SquadDaemonOutcome {
    Started {
        port: u16,
        background: bool,
        refreshed_key: bool,
    },
    Stopped {
        stopped_pid: Option<u32>,
    },
    Status(DaemonStatus),
    Logs {
        log_path: String,
    },
}

pub struct SquadDaemonCommand {
    sub: SquadDaemonSubcommand,
    engines: Engines,
}

impl SquadDaemonCommand {
    pub fn new(sub: SquadDaemonSubcommand, engines: Engines) -> Self {
        Self { sub, engines }
    }
}

#[async_trait]
impl Command for SquadDaemonCommand {
    type Frontend = Box<dyn SquadCommandFrontend>;
    type Outcome = SquadDaemonOutcome;

    async fn run_with_frontend(
        self,
        mut frontend: Self::Frontend,
    ) -> Result<Self::Outcome, CommandError> {
        let env = Env::from_process();
        let paths = SquadPaths::from_env(&env)?;
        let process = squad_process(&paths);
        let guard = DaemonGuard::for_daemon(DaemonKind::Squad, &env)?;
        let outcome = match self.sub {
            SquadDaemonSubcommand::Start(flags) => {
                run_start(
                    flags,
                    &self.engines,
                    &paths,
                    &process,
                    &guard,
                    &mut *frontend,
                )
                .await?
            }
            SquadDaemonSubcommand::Stop(_) => run_stop(&process, &mut *frontend)?,
            SquadDaemonSubcommand::Status(_) => run_status(&process).await?,
            SquadDaemonSubcommand::Logs(flags) => run_logs(&process, flags, &mut *frontend).await?,
        };
        frontend.replay_queued();
        Ok(outcome)
    }
}

/// What this process holds to authenticate to the squad daemon with, decided
/// once a daemon is running.
///
/// The squad key is disclosed exactly once — by the process that mints it — and
/// lives on disk only as a hash. That makes "I have no key" a state a frontend
/// has to be able to report, rather than something it discovers as a 401 on
/// every subsequent request.
#[derive(Debug, Clone)]
pub enum SquadKeyState {
    /// A key is in hand: `AWMAN_SQUAD_KEY` was set, or the running daemon
    /// serves unauthenticated and needs none.
    Ready,
    /// This process minted the key just now, so it must be shown — the
    /// plaintext exists nowhere else, and no later process can recover it.
    Minted {
        /// The rendered banner plus shell snippet, ready to display.
        setup: String,
        /// The key itself, for callers that need to act on it rather than
        /// print it.
        key: String,
    },
    /// A key hash exists on disk but this process holds no key. Every request
    /// will be refused with 401 until one is supplied or a new one is minted.
    Missing,
}

pub struct SquadSupervisor {
    process: DaemonProcess,
    guard: DaemonGuard,
    paths: SquadPaths,
    env: EnvSnapshot,
    /// A bearer key minted by this process because none existed yet. It is
    /// deliberately never printed from here — see `provision_key`. Callers that
    /// own a terminal drain it via [`SquadSupervisor::take_generated_key_setup`].
    generated_key: std::sync::Mutex<Option<ApiKey>>,
    /// Set once the minted key has been handed to a frontend for display.
    key_disclosed: std::sync::atomic::AtomicBool,
}

impl SquadSupervisor {
    pub fn from_env(env: &EnvSnapshot) -> Result<Self, CommandError> {
        let paths = SquadPaths::from_env(env)?;
        Ok(Self {
            process: squad_process(&paths),
            guard: DaemonGuard::for_daemon(DaemonKind::Squad, env)?,
            paths,
            env: env.clone(),
            generated_key: std::sync::Mutex::new(None),
            key_disclosed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// The key this supervisor minted during `ensure_running`, if any. A caller
    /// that owns a terminal (the CLI, the TUI) displays it; nothing else may.
    pub fn generated_key(&self) -> Option<ApiKey> {
        self.generated_key
            .lock()
            .expect("squad generated-key mutex poisoned")
            .clone()
    }

    /// Take the setup snippet for a key this supervisor minted, if it minted
    /// one and has not handed it out yet. `None` on every later call, so a
    /// caller may print the result unconditionally.
    ///
    /// The key itself stays in place — this supervisor still needs it to
    /// authenticate — but the *disclosure* happens exactly once, so two
    /// frontends sharing a supervisor cannot both print the same secret.
    pub fn take_generated_key_setup(&self) -> Option<String> {
        let key = self.generated_key()?;
        if self
            .key_disclosed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return None;
        }
        Some(key_setup::render_key_setup(
            key.as_str(),
            key_setup::ShellFlavor::from_env(&self.env),
        ))
    }

    /// Resolve the bearer key this process will authenticate with.
    ///
    /// On a first run there is no `squad_key.hash` yet. The key MUST be minted
    /// here, in the process that is about to spawn the daemon — never inside
    /// the detached child, whose stdout is redirected to `~/.awman/squad/awman.log`
    /// (launchd) or the journal (systemd-run) and would persist the plaintext
    /// key in a file `awman squad logs` prints verbatim.
    fn provision_key(&self) -> Result<Option<ApiKey>, CommandError> {
        if let Some(key) = self.env.squad_key() {
            return Ok(Some(ApiKey::from_string(key.to_string())));
        }
        if let Some(key) = self.generated_key() {
            return Ok(Some(key));
        }
        // A daemon started with `--dangerously-skip-auth` checks no bearer
        // token, so minting one here would write an `squad_key.hash` whose
        // plaintext nobody holds — and the next auth-enabled start would then
        // demand a key the user was never shown.
        if self.daemon_auth_disabled()? {
            return Ok(None);
        }
        if self.process.paths().read_key_hash()?.is_some() {
            // A hash exists but this process was given no key; the request will
            // be refused by the daemon with the standard auth error.
            return Ok(None);
        }
        let auth_engine = crate::engine::auth::AuthEngine::with_paths(
            crate::data::fs::AuthPathResolver::from_process_env()?,
            crate::data::fs::ApiPaths::from_process_env()?,
        );
        let key = auth_engine.generate_api_key()?;
        let hash = auth_engine.hash_api_key(&key);
        self.process.paths().write_key_hash(hash.as_str())?;
        *self
            .generated_key
            .lock()
            .expect("squad generated-key mutex poisoned") = Some(key.clone());
        publish_key_to_process_env(&key);
        tracing::info!("squad daemon key minted for automatic daemon startup");
        Ok(Some(key))
    }

    /// What this process can authenticate with, now that a daemon is running.
    ///
    /// Call once per startup: the `Minted` arm consumes the one-shot
    /// disclosure, so a second call reports `Ready` for a key this process
    /// already showed rather than showing it twice.
    pub fn key_state(&self) -> Result<SquadKeyState, CommandError> {
        // Gathering the five facts is all this does; the decision itself is
        // `decide_key_state`, which is pure and therefore exhaustively
        // testable without a daemon, a keyring, or a filesystem.
        //
        // `take_generated_key_setup` is called first because it is the only
        // one-shot among them: it consumes the disclosure, so asking for it
        // must not depend on the order the other four are read in.
        let minted = self
            .generated_key()
            .map(|key| (self.take_generated_key_setup(), key.as_str().to_string()));
        Ok(decide_key_state(
            minted,
            self.env.squad_key().is_some(),
            self.daemon_auth_disabled()?,
            self.process.paths().read_key_hash()?.is_some(),
        ))
    }

    /// Mint a fresh bearer key and restart the daemon onto it.
    ///
    /// The recovery from [`SquadKeyState::Missing`]: the previous key is
    /// unrecoverable — only its hash was ever stored — so the only way back to
    /// a working client is a new key, which means a new hash, which means the
    /// running daemon has to be replaced.
    ///
    /// The key is minted **here**, in this process, and the daemon is then
    /// started normally rather than with `--refresh-key`. The two produce the
    /// same on-disk state, but `--refresh-key` would mint inside the detached
    /// child, whose stdout is `~/.awman/squad/awman.log` — a file
    /// `awman squad logs` prints verbatim. See [`Self::provision_key`]: the
    /// plaintext key must never reach that log.
    ///
    /// Every other process still exporting the old key stops being able to
    /// reach squad; callers are expected to say so before offering this.
    pub async fn refresh_key(&self) -> Result<RemoteTaskGateway, CommandError> {
        self.guard.check()?;
        // Stop first: the running daemon authenticates against the old hash,
        // and `run_start` refuses to start a second one anyway.
        self.process.terminate()?;
        self.process.clear_meta()?;

        let auth_engine = crate::engine::auth::AuthEngine::with_paths(
            crate::data::fs::AuthPathResolver::from_process_env()?,
            crate::data::fs::ApiPaths::from_process_env()?,
        );
        let key = auth_engine.generate_api_key()?;
        let hash = auth_engine.hash_api_key(&key);
        self.process.paths().write_key_hash(hash.as_str())?;
        *self
            .generated_key
            .lock()
            .expect("squad generated-key mutex poisoned") = Some(key.clone());
        // The new key has never been shown, whatever was shown for the old one.
        self.key_disclosed
            .store(false, std::sync::atomic::Ordering::SeqCst);
        publish_key_to_process_env(&key);
        tracing::info!("squad daemon key refreshed; restarting the daemon on the new key");

        self.ensure_running().await
    }

    /// Whether the daemon that is currently running published a sidecar saying
    /// it serves unauthenticated. `false` when no daemon is running, when it
    /// published no sidecar, or when the sidecar predates the flag — every one
    /// of which means "assume auth is required".
    fn daemon_auth_disabled(&self) -> Result<bool, CommandError> {
        if self.process.running_pid()?.is_none() {
            return Ok(false);
        }
        Ok(self
            .process
            .read_meta()?
            .is_some_and(|meta| meta.auth_disabled))
    }

    /// Whether a squad daemon is already running for this squad root.
    ///
    /// The same liveness check [`ensure_running`](Self::ensure_running) makes
    /// before deciding to spawn, exposed so a caller can *ask first* rather
    /// than discovering after the fact that it started a background process —
    /// the TUI's open-squad-tab confirmation (WI 0110). It mints no key,
    /// writes nothing, and starts nothing.
    pub fn daemon_is_running(&self) -> Result<bool, CommandError> {
        Ok(self.process.running_pid()?.is_some())
    }

    /// Discover the existing daemon endpoint, if its metadata sidecar is present.
    pub fn gateway_from_meta(&self) -> Result<Option<RemoteTaskGateway>, CommandError> {
        let key = self.provision_key()?;
        self.gateway_from_meta_with(key.as_ref())
    }

    fn gateway_from_meta_with(
        &self,
        key: Option<&ApiKey>,
    ) -> Result<Option<RemoteTaskGateway>, CommandError> {
        let Some(meta) = self.process.read_meta()? else {
            return Ok(None);
        };
        let address = format!("{}://{}:{}", meta.scheme, meta.bind_ip, meta.port);
        Ok(Some(RemoteTaskGateway::new(HttpCore::new(
            &address, "v1", key,
        )?)))
    }

    /// Drop an endpoint sidecar left behind by a daemon that is no longer
    /// running.
    ///
    /// Called only from [`ensure_running`](Self::ensure_running), and only
    /// after its PID check has established that nothing is listening. A daemon
    /// killed with SIGKILL — or lost with the machine — never clears
    /// `server.json`, so without this the wait for the daemon we are about to
    /// spawn returns the *dead* one's port on its very first iteration, and
    /// hands back a gateway whose every request is refused with a connection
    /// error. Clearing it first makes that wait mean what it says: block until
    /// the daemon being started publishes an endpoint of its own.
    fn discard_stale_endpoint(&self) -> Result<(), CommandError> {
        if self.process.read_meta()?.is_some() {
            tracing::info!("squad supervisor discarded a stale daemon endpoint sidecar");
            self.process.clear_meta()?;
        }
        Ok(())
    }

    /// Return a remote gateway, starting the daemon only when needed. The
    /// cross-daemon guard is intentionally first, before a PID check or spawn.
    pub async fn ensure_running(&self) -> Result<RemoteTaskGateway, CommandError> {
        tracing::info!("squad supervisor ensure-running requested");
        self.guard.check()?;
        // Mint the key here, before any spawn, so the detached child always
        // finds a hash already on disk and never emits a key to its log.
        let key = self.provision_key()?;
        if self.process.running_pid()?.is_some() {
            tracing::info!("squad supervisor found an already-running daemon");
            return self.gateway_from_meta_with(key.as_ref())?.ok_or_else(|| {
                CommandError::Other(format!(
                    "squad daemon is running but has not published its endpoint; check {}",
                    self.paths.daemon().log_file().display()
                ))
            });
        }
        let binary = std::env::current_exe()
            .map_err(|e| CommandError::Other(format!("cannot determine awman binary: {e}")))?;
        self.discard_stale_endpoint()?;
        self.process
            .spawn_detached(&binary, &["squad".into(), "start".into()])?;
        tracing::info!("squad supervisor spawned daemon process");
        // Whether a daemon process ever existed at all. The OS process managers
        // report a *request* accepted, not a process started — launchd will
        // happily accept a bootstrap that runs nothing — so "spawn succeeded"
        // is not evidence. The pidfile is: `run_start` claims it before it
        // serves, and the PID check above guarantees no pidfile survives into
        // this loop, so anything seen here was written by the child we started.
        let mut saw_process = false;
        for _ in 0..100 {
            if let Some(gateway) = self.gateway_from_meta_with(key.as_ref())? {
                return Ok(gateway);
            }
            saw_process = saw_process || self.process.read_pid()?.is_some();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(CommandError::Other(
            self.startup_timeout_message(saw_process),
        ))
    }

    /// Explain a start that timed out, in terms of how far it actually got.
    ///
    /// The two failures need different answers and used to share one message.
    /// A daemon that started and then failed leaves its reason in the log. A
    /// daemon that never started leaves nothing there — so pointing at the log
    /// is worse than useless, and the useful instruction is to run the daemon
    /// in the foreground, where its output has somewhere to go.
    fn startup_timeout_message(&self, saw_process: bool) -> String {
        let log = self.paths.daemon().log_file().display().to_string();
        if saw_process {
            return format!(
                "squad daemon started but did not publish its endpoint within 10 seconds; \
                 check {log}"
            );
        }
        format!(
            "the squad daemon process never started: nothing claimed {pid} within 10 seconds. \
             The OS process manager accepted the request but ran nothing, so {log} has nothing \
             to show. Run `awman squad start` in a terminal to see why{platform}.",
            pid = self.paths.daemon().pid_file().display(),
            platform = if cfg!(target_os = "macos") {
                ", and check `launchctl print gui/$(id -u)/io.awman.squad`"
            } else if cfg!(target_os = "linux") {
                ", and check `systemctl --user status awman-squad`"
            } else {
                ""
            },
        )
    }
}

/// Publish a freshly minted squad key into this process's own environment.
///
/// The key is displayed once and then exists only as a hash, so the user is
/// told to export it — but they cannot export it into a process that is
/// *already running*, and the process that mints the key is exactly that: the
/// TUI that started the daemon. Without this, the one process guaranteed to
/// have seen the key is the one process that cannot use it for anything it
/// builds later (`squad attach`, a Dispatch command, a second supervisor),
/// because each of those reads `AWMAN_SQUAD_KEY` from the live environment.
///
/// This is a `setenv` in a process that already has threads running, which is
/// only safe because of when it happens: during daemon startup, before any
/// squad gateway exists to read the variable, and exactly once per minted key.
fn publish_key_to_process_env(key: &ApiKey) {
    std::env::set_var(crate::data::config::env::AWMAN_SQUAD_KEY, key.as_str());
}

/// Decide what a process can authenticate to squad with, from the four facts
/// that determine it.
///
/// `minted` is `Some((setup, key))` when *this* process minted the key, with
/// `setup` present only if it has not been disclosed yet. The remaining three
/// are read from the environment snapshot, the running daemon's sidecar, and
/// the key-hash file.
///
/// Separated from [`SquadSupervisor::key_state`] so the precedence between
/// them — which is what decides whether a user sees their key, sees nothing,
/// or sees the missing-key recovery — is testable in isolation.
fn decide_key_state(
    minted: Option<(Option<String>, String)>,
    env_key_present: bool,
    auth_disabled: bool,
    hash_present: bool,
) -> SquadKeyState {
    // A key this process minted outranks everything: it is in hand, and if it
    // has not been shown yet then showing it is the whole point.
    if let Some((setup, key)) = minted {
        return match setup {
            Some(setup) => SquadKeyState::Minted { setup, key },
            // Another frontend sharing this supervisor already displayed it;
            // the key is still in hand, so nothing is missing.
            None => SquadKeyState::Ready,
        };
    }
    if env_key_present || auth_disabled {
        return SquadKeyState::Ready;
    }
    if hash_present {
        return SquadKeyState::Missing;
    }
    // No hash, no key, and auth is on: nothing has minted yet, which
    // `provision_key` resolves on the next start.
    SquadKeyState::Ready
}

fn squad_process(paths: &SquadPaths) -> DaemonProcess {
    DaemonProcess::new(paths.daemon(), SQUAD_UNIT_NAME, SQUAD_PLIST_LABEL)
}

async fn run_start(
    flags: SquadStartFlags,
    engines: &Engines,
    paths: &SquadPaths,
    process: &DaemonProcess,
    guard: &DaemonGuard,
    frontend: &mut dyn SquadCommandFrontend,
) -> Result<SquadDaemonOutcome, CommandError> {
    // Must precede every actual daemon launch, including a detached launch.
    guard.check()?;
    if let Some(pid) = process.running_pid()? {
        return Err(CommandError::Other(format!(
            "squad daemon is already running (PID {pid})"
        )));
    }
    tracing::info!(
        port = flags.port,
        background = flags.background,
        refresh_key = flags.refresh_key,
        "squad administrator requested daemon start"
    );
    // `--dangerously-skip-auth` mints nothing and writes no hash: the flag is
    // for this run only, so a later plain `start` still finds whatever hash was
    // on disk before. It is tolerable because squad binds 127.0.0.1 exclusively.
    if flags.dangerously_skip_auth {
        frontend.write_message(UserMessage {
            level: MessageLevel::Warning,
            text: "Authentication is DISABLED (--dangerously-skip-auth). Any process \
                   on this machine can drive squad. The daemon still binds to loopback \
                   (127.0.0.1) only, so it is unreachable from the network."
                .into(),
        });
    }
    if flags.refresh_key
        || (!flags.dangerously_skip_auth && process.paths().read_key_hash()?.is_none())
    {
        let key = engines.auth_engine.generate_api_key()?;
        let hash = engines.auth_engine.hash_api_key(&key);
        process.paths().write_key_hash(hash.as_str())?;
        tracing::info!(
            refresh = flags.refresh_key,
            "squad daemon key minted or refreshed"
        );
        // The plaintext key is disclosed exactly here, in the foreground
        // process that owns a terminal — never in the detached child, whose
        // stdout lands in a log file `awman squad logs` prints verbatim.
        frontend.write_message(UserMessage {
            level: MessageLevel::Info,
            text: key_setup::render_key_setup(
                key.as_str(),
                key_setup::ShellFlavor::from_env(&Env::from_process()),
            ),
        });
        if flags.refresh_key {
            return Ok(SquadDaemonOutcome::Started {
                port: flags.port,
                background: false,
                refreshed_key: true,
            });
        }
    }
    if flags.background {
        let binary = std::env::current_exe()
            .map_err(|e| CommandError::Other(format!("cannot determine awman binary: {e}")))?;
        let mut args = vec![
            "squad".into(),
            "start".into(),
            "--port".into(),
            flags.port.to_string(),
        ];
        if flags.dangerously_skip_auth {
            args.push("--dangerously-skip-auth".into());
        }
        let pid = process.spawn_detached(&binary, &args)?;
        tracing::info!(pid, port = flags.port, "squad daemon started in background");
        frontend.write_message(UserMessage {
            level: MessageLevel::Success,
            text: format!("squad daemon started in background (PID {pid})."),
        });
        return Ok(SquadDaemonOutcome::Started {
            port: flags.port,
            background: true,
            refreshed_key: false,
        });
    }
    guard
        .acquire(std::process::id())
        .map_err(|error| match error {
            AcquireError::AlreadyRunning { pid } => {
                CommandError::Other(format!("squad daemon is already running (PID {pid})"))
            }
            other => CommandError::Data(other.into_data_error()),
        })?;
    tracing::info!(port = flags.port, "squad daemon starting in foreground");
    let result = frontend
        .serve_squad_daemon(SquadServeConfig {
            port: flags.port,
            dangerously_skip_auth: flags.dangerously_skip_auth,
        })
        .await;
    let _ = process.clear_meta();
    let _ = guard.release();
    result?;
    let _ = paths;
    Ok(SquadDaemonOutcome::Started {
        port: flags.port,
        background: false,
        refreshed_key: false,
    })
}

fn run_stop(
    process: &DaemonProcess,
    frontend: &mut dyn SquadCommandFrontend,
) -> Result<SquadDaemonOutcome, CommandError> {
    tracing::info!("squad administrator requested daemon stop");
    let pid = match process.terminate_running()? {
        Termination::Terminated { pid } => pid,
        // Absent, stale, or another process' pidfile: the pidfile has been
        // cleaned up either way, so the daemon is simply not running.
        _ => return Err(CommandError::Other("squad daemon is not running".into())),
    };
    let _ = process.clear_meta();
    tracing::info!(pid, "squad daemon stopped");
    frontend.write_message(UserMessage {
        level: MessageLevel::Success,
        text: format!("squad daemon (PID {pid}) stopped."),
    });
    Ok(SquadDaemonOutcome::Stopped {
        stopped_pid: Some(pid),
    })
}

async fn run_logs(
    process: &DaemonProcess,
    flags: SquadLogsFlags,
    frontend: &mut dyn SquadCommandFrontend,
) -> Result<SquadDaemonOutcome, CommandError> {
    let path = process.paths().log_file();
    // Initial dump: emit every existing line and remember where the file ends
    // so follow-mode only streams what is appended after this point.
    let mut offset = match tail_new_lines(&path, 0)? {
        Some((lines, end)) => {
            for line in lines {
                frontend.write_message(UserMessage {
                    level: MessageLevel::Info,
                    text: line,
                });
            }
            end
        }
        None => {
            frontend.write_message(UserMessage {
                level: MessageLevel::Warning,
                text: format!("Log file not found: {}", path.display()),
            });
            0
        }
    };

    // Follow mode: tail appended lines every 250 ms until the user interrupts
    // (Ctrl-C). This is a local file read — the daemon still exposes no log
    // route on the network.
    if flags.follow {
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if let Some((lines, end)) = tail_new_lines(&path, offset)? {
                        for line in lines {
                            frontend.write_message(UserMessage {
                                level: MessageLevel::Info,
                                text: line,
                            });
                        }
                        offset = end;
                    }
                }
            }
        }
    }

    Ok(SquadDaemonOutcome::Logs {
        log_path: path.display().to_string(),
    })
}

/// Read complete lines appended to `path` after byte `from`. Returns the lines
/// and the new byte offset (advanced only past the last complete line, so a
/// partial trailing line is re-read on the next call), or `None` when the file
/// does not exist yet.
fn tail_new_lines(
    path: &std::path::Path,
    from: u64,
) -> Result<Option<(Vec<String>, u64)>, CommandError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::data::error::DataError::io(path, error).into()),
    };
    let len = file
        .metadata()
        .map_err(|error| crate::data::error::DataError::io(path, error))?
        .len();
    // A truncated/rotated file (shorter than our offset) restarts from 0.
    let start = if from > len { 0 } else { from };
    if start == len {
        return Ok(Some((Vec::new(), len)));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|error| crate::data::error::DataError::io(path, error))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|error| crate::data::error::DataError::io(path, error))?;
    // Advance only past the last newline so a partial final line is not emitted.
    let consumed = match buf.rfind('\n') {
        Some(idx) => idx + 1,
        None => 0,
    };
    let lines = buf[..consumed].lines().map(str::to_string).collect();
    Ok(Some((lines, start + consumed as u64)))
}

async fn run_status(process: &DaemonProcess) -> Result<SquadDaemonOutcome, CommandError> {
    let pid = process.running_pid()?;
    let meta = process.read_meta()?;
    let bound_addr = meta
        .as_ref()
        .map(|m| format!("{}://{}:{}", m.scheme, m.bind_ip, m.port));
    Ok(SquadDaemonOutcome::Status(DaemonStatus {
        running: pid.is_some(),
        pid,
        bound_addr,
        task_count: 0,
        active_count: 0,
        last_tick: None,
        in_flight: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::{tail_new_lines, SquadKeyState, SquadSupervisor};
    use crate::data::config::env::{EnvSnapshot, AWMAN_SQUAD_ROOT};
    use crate::engine::auth::ApiKey;

    /// A daemon that died without clearing its endpoint sidecar must not be
    /// handed out as a live one.
    ///
    /// This is what made "start the squad daemon" look like it silently did
    /// nothing: `ensure_running` found the dead daemon's `server.json` on its
    /// first poll, returned a gateway pointing at a port nothing was listening
    /// on, and every request through it was refused — while the daemon it had
    /// just spawned came up on a different port and was never used.
    #[test]
    fn a_dead_daemons_endpoint_is_discarded_rather_than_handed_out() {
        let tmp = tempfile::tempdir().unwrap();
        let env = EnvSnapshot::with_overrides([(AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap())]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();

        supervisor
            .process
            .write_meta(&crate::data::fs::daemon_process::ServerMeta {
                port: 40253,
                bind_ip: "127.0.0.1".into(),
                scheme: "http".into(),
                auth_disabled: true,
            })
            .unwrap();
        assert!(
            supervisor.gateway_from_meta().unwrap().is_some(),
            "the stale sidecar is exactly what a naive wait would accept"
        );

        supervisor.discard_stale_endpoint().unwrap();

        assert!(
            supervisor.process.read_meta().unwrap().is_none(),
            "the stale sidecar must be gone, so the wait blocks for a real one"
        );
        assert!(supervisor.gateway_from_meta().unwrap().is_none());
        // Idempotent: nothing to discard is not an error.
        supervisor.discard_stale_endpoint().unwrap();
    }

    /// A start that timed out has to say which of the two things happened,
    /// because they need opposite responses.
    ///
    /// This is the bug the message itself caused: an OS process manager that
    /// accepts a start request and then runs nothing produced "check <log>" —
    /// naming a file that, by construction, no process had ever written to.
    #[test]
    fn a_timeout_that_never_started_a_process_does_not_send_the_user_to_an_empty_log() {
        let tmp = tempfile::tempdir().unwrap();
        let env = EnvSnapshot::with_overrides([(AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap())]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();

        let never_started = supervisor.startup_timeout_message(false);
        assert!(
            never_started.contains("never started"),
            "the user must be told no process exists: {never_started:?}"
        );
        assert!(
            never_started.contains("awman squad start"),
            "the foreground run is the only way to see the reason: {never_started:?}"
        );

        // A daemon that did start and then failed left its reason in the log,
        // so that is where the message must point.
        let started_then_failed = supervisor.startup_timeout_message(true);
        assert!(
            started_then_failed.contains("awman.log"),
            "{started_then_failed:?}"
        );
        assert!(
            !started_then_failed.contains("never started"),
            "{started_then_failed:?}"
        );
    }

    /// The precedence that decides whether a user is shown their key, shown
    /// nothing, or shown the missing-key recovery.
    #[test]
    fn the_key_state_precedence_covers_every_combination_that_matters() {
        use super::decide_key_state;
        let minted = || Some((Some("SETUP".to_string()), "abc123".to_string()));
        let already_shown = || Some((None, "abc123".to_string()));

        // A key this process minted and has not shown yet must be shown, even
        // though a hash now exists on disk (it wrote it) and even if the
        // environment happens to carry a different one.
        assert!(matches!(
            decide_key_state(minted(), true, false, true),
            SquadKeyState::Minted { .. }
        ));
        // Shown once, never twice: a second read is "we hold a key", not a
        // second disclosure of the same secret.
        assert!(matches!(
            decide_key_state(already_shown(), false, false, true),
            SquadKeyState::Ready
        ));
        // The two ways to be fine without minting anything.
        assert!(matches!(
            decide_key_state(None, true, false, true),
            SquadKeyState::Ready
        ));
        assert!(matches!(
            decide_key_state(None, false, true, true),
            SquadKeyState::Ready
        ));
        // The reported case: a hash on disk, nothing in the environment, and
        // auth on. Every request would be refused with 401.
        assert!(matches!(
            decide_key_state(None, false, false, true),
            SquadKeyState::Missing
        ));
        // No hash at all is not "missing" — nothing has minted yet, and the
        // next start will. Reporting it would put the recovery dialog in front
        // of a user on their very first run.
        assert!(matches!(
            decide_key_state(None, false, false, false),
            SquadKeyState::Ready
        ));
    }

    /// The end the supervisor actually reaches: a hash on disk that this
    /// process holds no key for.
    #[test]
    fn a_hash_with_no_key_in_the_environment_reports_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let env = EnvSnapshot::with_overrides([(AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap())]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();

        assert!(
            matches!(supervisor.key_state().unwrap(), SquadKeyState::Ready),
            "before anything is minted there is nothing to recover from"
        );

        supervisor
            .process
            .paths()
            .write_key_hash("0123456789abcdef")
            .unwrap();
        assert!(matches!(
            supervisor.key_state().unwrap(),
            SquadKeyState::Missing
        ));

        // The same root, with the key exported: nothing missing.
        let env = EnvSnapshot::with_overrides([
            (AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap()),
            (crate::data::config::env::AWMAN_SQUAD_KEY, "a-real-key"),
        ]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();
        assert!(matches!(
            supervisor.key_state().unwrap(),
            SquadKeyState::Ready
        ));
    }

    /// The minting process is the one process that cannot be told to export
    /// the key afterwards, so it publishes it to its own environment.
    #[test]
    fn a_minted_key_is_published_into_this_process_environment() {
        use crate::data::config::env::AWMAN_SQUAD_KEY;
        let previous = std::env::var(AWMAN_SQUAD_KEY).ok();

        super::publish_key_to_process_env(&ApiKey::from_string("published-key".to_string()));
        assert_eq!(
            std::env::var(AWMAN_SQUAD_KEY).unwrap(),
            "published-key",
            "a later `Env::from_process()` in this process must find the key"
        );

        match previous {
            Some(value) => std::env::set_var(AWMAN_SQUAD_KEY, value),
            None => std::env::remove_var(AWMAN_SQUAD_KEY),
        }
    }

    #[test]
    fn tail_reads_the_whole_file_on_the_first_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("awman.log");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let (lines, end) = tail_new_lines(&path, 0).unwrap().unwrap();
        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(end, 8);
    }

    #[test]
    fn tail_streams_only_lines_appended_after_the_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("awman.log");
        std::fs::write(&path, "one\n").unwrap();
        let (_, end) = tail_new_lines(&path, 0).unwrap().unwrap();
        // Append more; a follow tick from `end` yields only the new line.
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let (lines, end2) = tail_new_lines(&path, end).unwrap().unwrap();
        assert_eq!(lines, vec!["two".to_string()]);
        assert_eq!(end2, 8);
    }

    #[test]
    fn tail_does_not_emit_a_partial_final_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("awman.log");
        // No trailing newline: the partial line must be withheld and re-read.
        std::fs::write(&path, "complete\npartial").unwrap();
        let (lines, end) = tail_new_lines(&path, 0).unwrap().unwrap();
        assert_eq!(lines, vec!["complete".to_string()]);
        assert_eq!(end, 9, "offset advances only past the last newline");
        // Once the line is completed, the next tick emits it in full.
        std::fs::write(&path, "complete\npartial done\n").unwrap();
        let (lines, _) = tail_new_lines(&path, end).unwrap().unwrap();
        assert_eq!(lines, vec!["partial done".to_string()]);
    }

    #[test]
    fn tail_reports_a_missing_file_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.log");
        assert!(tail_new_lines(&path, 0).unwrap().is_none());
    }
}
