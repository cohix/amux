//! `SquadSupervisor` — the squad daemon's lifecycle, at Layer 1.
//!
//! Everything about *whether a squad daemon is running, what this process can
//! authenticate to it with, and how to start one* lives here (WI 0113 F-02,
//! decision Q4: squad is not an architectural exception). The supervisor
//! deliberately stops at the endpoint: it hands back a [`SquadEndpoint`], and
//! Layer 2 turns that into the `RemoteTaskGateway` its commands speak through.
//! When the Layer 1 HTTP client lands (WI 0114 F-28) the transport moves down
//! here too and this seam disappears.

use std::time::Duration;

use crate::data::config::env::EnvSnapshot;
use crate::data::fs::daemon_process::{SQUAD_PLIST_LABEL, SQUAD_UNIT_NAME};
use crate::data::fs::{
    ApiPaths, AuthPathResolver, DaemonGuard, DaemonKind, DaemonProcess, SquadPaths,
};
use crate::engine::auth::{ApiKey, AuthEngine};
use crate::engine::error::EngineError;
use crate::engine::squad::key_setup;

/// Where a running squad daemon is listening, and what to authenticate with.
///
/// The Layer 2 boundary object: enough to construct a client, and nothing
/// about how that client is built.
#[derive(Debug, Clone)]
pub struct SquadEndpoint {
    /// `scheme://ip:port`, as published by the running daemon.
    pub address: String,
    /// The bearer key this process holds, or `None` when it holds none (the
    /// daemon serves unauthenticated, or every request will be refused 401).
    pub key: Option<ApiKey>,
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

/// Daemon discovery, key provisioning and start-on-demand for squad.
pub struct SquadSupervisor {
    // TODO(WI 0114 F-29): spawn and probe through the Layer 1
    // `DaemonSupervisor` once it exists; `DaemonProcess` is a Layer 0 type and
    // this is the last place that drives it directly.
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
    pub fn from_env(env: &EnvSnapshot) -> Result<Self, EngineError> {
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
    fn provision_key(&self) -> Result<Option<ApiKey>, EngineError> {
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
        let key = self.auth_engine()?.generate_api_key()?;
        let hash = self.auth_engine()?.hash_api_key(&key);
        self.process.paths().write_key_hash(hash.as_str())?;
        *self
            .generated_key
            .lock()
            .expect("squad generated-key mutex poisoned") = Some(key.clone());
        publish_key_to_process_env(&key);
        tracing::info!("squad daemon key minted for automatic daemon startup");
        Ok(Some(key))
    }

    fn auth_engine(&self) -> Result<AuthEngine, EngineError> {
        Ok(AuthEngine::with_paths(
            AuthPathResolver::from_process_env()?,
            ApiPaths::from_process_env()?,
        ))
    }

    /// What this process can authenticate with, now that a daemon is running.
    ///
    /// Call once per startup: the `Minted` arm consumes the one-shot
    /// disclosure, so a second call reports `Ready` for a key this process
    /// already showed rather than showing it twice.
    pub fn key_state(&self) -> Result<SquadKeyState, EngineError> {
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
    pub async fn refresh_key(&self) -> Result<SquadEndpoint, EngineError> {
        // Typed as a conflict, not a generic data error: it is the one
        // failure whose message already names the daemon to stop, and a
        // frontend maps it to its own answer (WI 0113 F-04).
        self.guard
            .check()
            .map_err(|error| EngineError::SquadDaemonConflict(error.to_string()))?;
        // Stop first: the running daemon authenticates against the old hash,
        // and `run_start` refuses to start a second one anyway.
        self.process.terminate()?;
        self.process.clear_meta()?;

        let key = self.auth_engine()?.generate_api_key()?;
        let hash = self.auth_engine()?.hash_api_key(&key);
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
    fn daemon_auth_disabled(&self) -> Result<bool, EngineError> {
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
    pub fn daemon_is_running(&self) -> Result<bool, EngineError> {
        Ok(self.process.running_pid()?.is_some())
    }

    /// Discover the existing daemon endpoint, if its metadata sidecar is present.
    ///
    /// Checks the sidecar **before** resolving a key: `provision_key` may
    /// mint and persist a new `squad_key.hash`, and a caller resolving
    /// `GatewayNeed::IfRunning` against a daemon that isn't running must
    /// observe exactly what its own contract promises — "starts nothing and
    /// mints no key" — not a key minted and written to disk moments before
    /// this returns `Ok(None)`. Contrast [`Self::ensure_running`], which
    /// deliberately mints its key *before* checking process state for a
    /// different, equally load-bearing reason (see its own comment).
    pub fn endpoint_from_meta(&self) -> Result<Option<SquadEndpoint>, EngineError> {
        if self.process.read_meta()?.is_none() {
            return Ok(None);
        }
        let key = self.provision_key()?;
        self.endpoint_from_meta_with(key)
    }

    /// An endpoint for a read-only health probe (WI 0112's TUI indicator), or
    /// `None` when no endpoint sidecar exists.
    ///
    /// Unlike [`Self::endpoint_from_meta`] this **never mints a key**: it uses
    /// `AWMAN_SQUAD_KEY` or a key this process already minted, and otherwise
    /// carries no bearer token at all so the daemon's `401` is what the probe
    /// reports. A poller that ran every few seconds through `provision_key`
    /// could write a `squad_key.hash` nobody was ever shown; this cannot. It
    /// also never touches the one-shot key disclosure (`key_state`).
    pub fn probe_endpoint(&self) -> Result<Option<SquadEndpoint>, EngineError> {
        let key = match self.env.squad_key() {
            Some(key) => Some(ApiKey::from_string(key.to_string())),
            None => self.generated_key(),
        };
        self.endpoint_from_meta_with(key)
    }

    /// Build an endpoint from the sidecar using an already-resolved key.
    /// Callers that need "no key minted unless a sidecar exists" must check
    /// [`DaemonProcess::read_meta`] themselves *before* resolving `key`, as
    /// [`Self::endpoint_from_meta`] does — this helper reads the sidecar
    /// again only to build the address, not to gate key resolution.
    fn endpoint_from_meta_with(
        &self,
        key: Option<ApiKey>,
    ) -> Result<Option<SquadEndpoint>, EngineError> {
        let Some(meta) = self.process.read_meta()? else {
            return Ok(None);
        };
        Ok(Some(SquadEndpoint {
            address: format!("{}://{}:{}", meta.scheme, meta.bind_ip, meta.port),
            key,
        }))
    }

    /// Drop an endpoint sidecar left behind by a daemon that is no longer
    /// running.
    ///
    /// Called only from [`ensure_running`](Self::ensure_running), and only
    /// after its PID check has established that nothing is listening. A daemon
    /// killed with SIGKILL — or lost with the machine — never clears
    /// `server.json`, so without this the wait for the daemon we are about to
    /// spawn returns the *dead* one's port on its very first iteration, and
    /// hands back an endpoint whose every request is refused with a connection
    /// error. Clearing it first makes that wait mean what it says: block until
    /// the daemon being started publishes an endpoint of its own.
    fn discard_stale_endpoint(&self) -> Result<(), EngineError> {
        if self.process.read_meta()?.is_some() {
            tracing::info!("squad supervisor discarded a stale daemon endpoint sidecar");
            self.process.clear_meta()?;
        }
        Ok(())
    }

    /// Return the daemon's endpoint, starting it only when needed. The
    /// cross-daemon guard is intentionally first, before a PID check or spawn.
    pub async fn ensure_running(&self) -> Result<SquadEndpoint, EngineError> {
        tracing::info!("squad supervisor ensure-running requested");
        // Typed as a conflict, not a generic data error: it is the one
        // failure whose message already names the daemon to stop, and a
        // frontend maps it to its own answer (WI 0113 F-04).
        self.guard
            .check()
            .map_err(|error| EngineError::SquadDaemonConflict(error.to_string()))?;
        // Mint the key here, before any spawn, so the detached child always
        // finds a hash already on disk and never emits a key to its log.
        let key = self.provision_key()?;
        if self.process.running_pid()?.is_some() {
            tracing::info!("squad supervisor found an already-running daemon");
            return self.endpoint_from_meta_with(key)?.ok_or_else(|| {
                EngineError::SquadDaemonUnreachable(format!(
                    "squad daemon is running but has not published its endpoint; check {}",
                    self.paths.daemon().log_file().display()
                ))
            });
        }
        let binary = resolve_daemon_binary(std::env::current_exe().map_err(|e| {
            EngineError::SquadDaemonStartup(format!("cannot determine awman binary: {e}"))
        })?)?;
        self.discard_stale_endpoint()?;
        // TODO(WI 0114 F-29): route through the Layer 1 `DaemonSupervisor`.
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
            if let Some(endpoint) = self.endpoint_from_meta_with(key.clone())? {
                return Ok(endpoint);
            }
            saw_process = saw_process || self.process.read_pid()?.is_some();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(EngineError::SquadDaemonStartup(
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

/// Resolve the binary to re-exec as the detached `squad start` daemon, refusing
/// to do so when this process is a test/bench harness rather than the awman CLI.
///
/// `ensure_running` re-execs `current_exe()` with `["squad", "start"]`. In the
/// real binary that is `awman squad start`. But under `cargo test`/`cargo bench`
/// `current_exe()` is the libtest harness (`target/<profile>/deps/awman-<hash>`),
/// which parses `squad start` as *test-name filters* and re-runs every test
/// whose name contains "squad" or "start" — including the ones that reach this
/// spawn. Each re-run spawns another harness, detached and reparented to PID 1,
/// so the process count explodes until the host (or a container's swapless
/// guest) OOM-kills the session. Guarding here is the one choke point every
/// spawn path funnels through, so no test can trip the fork bomb regardless of
/// which one reaches `ensure_running`.
///
/// Detection is belt-and-suspenders: `cfg!(test)` catches the crate's own unit
/// tests, and a `deps` path component catches a harness from any other crate
/// (integration tests, benches) where `cfg!(test)` is not set for this code.
/// Cargo only ever places test/bench binaries under `deps/`; the installed CLI
/// and a plain `cargo build` binary live one level up, so a real daemon start is
/// never misclassified.
fn resolve_daemon_binary(binary: std::path::PathBuf) -> Result<std::path::PathBuf, EngineError> {
    let is_test_harness = cfg!(test)
        || binary
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|name| name == "deps");
    if is_test_harness {
        return Err(EngineError::SquadDaemonStartup(format!(
            "refusing to auto-start the squad daemon by re-exec'ing a test/bench harness \
             ({}): it would parse `squad start` as test filters and re-spawn itself \
             unboundedly. Start the daemon out-of-band, or stub the spawn in tests.",
            binary.display()
        )));
    }
    Ok(binary)
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

/// The squad daemon's process identity: pidfile, sidecar, unit and plist name.
pub fn squad_process(paths: &SquadPaths) -> DaemonProcess {
    DaemonProcess::new(paths.daemon(), SQUAD_UNIT_NAME, SQUAD_PLIST_LABEL)
}

#[cfg(test)]
mod tests {
    use super::{decide_key_state, resolve_daemon_binary, SquadKeyState, SquadSupervisor};
    use crate::data::config::env::{EnvSnapshot, AWMAN_SQUAD_ROOT};
    use crate::engine::auth::ApiKey;

    /// A daemon that died without clearing its endpoint sidecar must not be
    /// handed out as a live one.
    ///
    /// This is what made "start the squad daemon" look like it silently did
    /// nothing: `ensure_running` found the dead daemon's `server.json` on its
    /// first poll, returned an endpoint pointing at a port nothing was listening
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
            supervisor.endpoint_from_meta().unwrap().is_some(),
            "the stale sidecar is exactly what a naive wait would accept"
        );

        supervisor.discard_stale_endpoint().unwrap();

        assert!(
            supervisor.process.read_meta().unwrap().is_none(),
            "the stale sidecar must be gone, so the wait blocks for a real one"
        );
        assert!(supervisor.endpoint_from_meta().unwrap().is_none());
        // Idempotent: nothing to discard is not an error.
        supervisor.discard_stale_endpoint().unwrap();
    }

    /// The daemon spawn must refuse to re-exec a test/bench harness, or
    /// `ensure_running` fork-bombs the host: the harness parses `squad start`
    /// as test filters and re-runs the tests that reach the spawn, each spawning
    /// another harness until the machine (or a swapless container guest) OOMs.
    #[test]
    fn a_test_harness_binary_is_never_re_exec_as_the_daemon() {
        // `cfg!(test)` alone makes this refuse in-crate, so also prove the
        // path-based guard on a synthetic non-test path.
        let harness = std::path::PathBuf::from("/workspace/target/debug/deps/awman-deadbeef");
        let err = resolve_daemon_binary(harness).unwrap_err();
        assert!(
            err.to_string().contains("test filters"),
            "must name the fork-bomb cause: {err}"
        );

        // A real installed/plain-build binary (not under `deps/`) is allowed.
        let real = std::path::PathBuf::from("/usr/local/bin/awman");
        // Under `cfg!(test)` even this is refused, which is the point: no test
        // process ever spawns. The path check is exercised above; here we only
        // assert the real path is not the reason it would be rejected.
        assert_ne!(real.parent().unwrap().file_name().unwrap(), "deps");
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

    /// WI 0112 Part 2: the TUI indicator's probe runs every few seconds and
    /// must never mint a key as a side effect — with no hash, no env key and
    /// no sidecar it hands back nothing and writes nothing.
    #[test]
    fn the_probe_endpoint_never_mints_a_key() {
        let tmp = tempfile::tempdir().unwrap();
        let env = EnvSnapshot::with_overrides([(AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap())]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();

        assert!(
            supervisor.probe_endpoint().unwrap().is_none(),
            "no endpoint sidecar means no endpoint"
        );
        assert!(
            supervisor
                .process
                .paths()
                .read_key_hash()
                .unwrap()
                .is_none(),
            "a probe must not write squad_key.hash"
        );
        assert!(
            supervisor.generated_key().is_none(),
            "a probe must not mint a key"
        );
    }

    /// Regression: `GatewayNeed::IfRunning` (`squad status` and friends) must
    /// keep its "starts nothing and mints no key" contract on a fresh root
    /// with no daemon running at all. `endpoint_from_meta` used to call
    /// `provision_key` unconditionally *before* checking whether a sidecar
    /// existed, so a plain read-only status check minted and persisted a
    /// `squad_key.hash` whose plaintext was never shown to anyone, on a
    /// command whose whole contract is "starts nothing".
    #[test]
    fn endpoint_from_meta_never_mints_a_key_when_no_daemon_is_running() {
        let tmp = tempfile::tempdir().unwrap();
        let env = EnvSnapshot::with_overrides([(AWMAN_SQUAD_ROOT, tmp.path().to_str().unwrap())]);
        let supervisor = SquadSupervisor::from_env(&env).unwrap();

        assert!(
            supervisor.endpoint_from_meta().unwrap().is_none(),
            "no endpoint sidecar means no endpoint"
        );
        assert!(
            supervisor
                .process
                .paths()
                .read_key_hash()
                .unwrap()
                .is_none(),
            "a status check against a daemon that isn't running must not write squad_key.hash"
        );
        assert!(
            supervisor.generated_key().is_none(),
            "a status check against a daemon that isn't running must not mint a key"
        );
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
}
