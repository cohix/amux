//! `Dispatch` — Layer 2's gateway from frontends into typed `*Command` values.
//!
//! Frontends construct a `Dispatch` with a frontend-specific
//! [`CommandFrontend`] implementation (CLI, TUI, API). Dispatch reads
//! flag values from the frontend, applies catalogue-driven validation
//! (mutually-exclusive flags, type errors, implications), and returns a typed
//! [`BuiltCommand`] enum containing the constructed `*Command` struct.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::command::commands::api_server::{ApiServerCommand, ApiServerCommandFrontend};
use crate::command::commands::auth::AuthCommandFrontend;
use crate::command::commands::chat::{ChatCommand, ChatCommandFrontend};
use crate::command::commands::clean::{CleanCommand, CleanCommandFrontend};
use crate::command::commands::config::{ConfigCommand, ConfigCommandFrontend};
use crate::command::commands::download::DownloadCommandFrontend;
use crate::command::commands::exec_prompt::{ExecPromptCommand, ExecPromptCommandFrontend};
use crate::command::commands::exec_workflow::{ExecWorkflowCommand, ExecWorkflowCommandFrontend};
use crate::command::commands::init::{InitCommand, InitCommandFrontend};
use crate::command::commands::new::{NewCommand, NewCommandFrontend};
use crate::command::commands::ready::{ReadyCommand, ReadyCommandFrontend};
use crate::command::commands::remote::{RemoteCommand, RemoteCommandFrontend};
use crate::command::commands::specs::{SpecsCommand, SpecsCommandFrontend};
use crate::command::commands::squad::attach::{
    SquadAttachCommand, SquadAttachFrontend, SquadAttachOutcome,
};
use crate::command::commands::squad::commands::{SquadCommand, SquadCommandFrontend};
use crate::command::commands::squad::gateway::TaskGateway;
use crate::command::commands::squad::runtime_guard::require_container_tier;
use crate::command::commands::squad::supervisor::SquadGatewayResolver;
use crate::command::commands::status::{StatusCommand, StatusCommandFrontend};
use crate::command::commands::Command;
use crate::command::dispatch::catalogue::{CommandCatalogue, GatewayNeed};
use crate::command::error::CommandError;
use crate::data::config::global::GlobalConfig;
use crate::data::config::EffectiveConfig;
use crate::data::fs::{ApiPaths, AuthPathResolver, DaemonKind, DataPaths, SquadPaths};
use crate::data::message::UserMessageSink;
use crate::data::session::Session;
use crate::engine::agent::AgentEngine;
use crate::engine::agent_runtime::{self, AgentRuntimeEngine, DetectedRuntime};
use crate::engine::auth::AuthEngine;
use crate::engine::container::ContainerRuntime;
use crate::engine::error::EngineError;
use crate::engine::git::GitEngine;
use crate::engine::overlay::OverlayEngine;
use crate::engine::sandbox::SandboxRuntime;

pub mod build;
pub mod catalogue;
pub mod parsed_input;
pub mod projections;
pub mod resolved;

pub use parsed_input::ParsedCommandBoxInput;
pub use resolved::{BuildContext, CallerContext, ResolvedArgs, ResolvedFlags};

/// Install the process-wide live credential monitor for this command session.
/// The container backends intentionally have no command/session dependency, so
/// this is the one command-layer bridge to their global no-op hook.
pub fn install_credential_refresh(config: &EffectiveConfig) {
    let settings = config.auth_refresh();
    if settings.enabled {
        crate::engine::credential_refresh::install_global(
            crate::engine::credential_refresh::CredentialRefreshMonitor::new(
                crate::engine::credential_refresh::MonitorConfig {
                    refresh_threshold: settings.threshold,
                    tick_interval: settings.tick,
                },
            ),
        );
    }
}

// ─── Pre-wired engines bundle ───────────────────────────────────────────────

/// All Layer 1 engine handles a `Dispatch` needs to construct a `*Command`.
/// `ReadyEngine` and `InitEngine` are NOT pre-constructed here — those
/// engines accept per-invocation flag values.
#[derive(Clone)]
pub struct Engines {
    /// Cross-paradigm trait-object handle. Used for build(), list_running(),
    /// stats(), stop(), exec_args(), is_available(), capabilities() and
    /// other operations that exist on both paradigms.
    pub runtime: Arc<dyn AgentRuntimeEngine>,

    /// Container-paradigm-specific handle, set when `runtime` is a
    /// ContainerRuntime. None when the active runtime is a SandboxRuntime.
    /// Used for image-paradigm operations (build_image, image_exists,
    /// image_home_dir, start_background) that only exist on the container
    /// side. Points at the same underlying object as `runtime`.
    pub container_runtime: Option<Arc<ContainerRuntime>>,

    /// Sandbox-paradigm-specific handle, mirror of `container_runtime` for
    /// sandbox-only operations. None when running under a ContainerRuntime.
    pub sandbox_runtime: Option<Arc<SandboxRuntime>>,

    pub git_engine: Arc<GitEngine>,
    pub overlay_engine: Arc<OverlayEngine>,
    pub auth_engine: Arc<AuthEngine>,
    pub agent_engine: Arc<AgentEngine>,
    pub workflow_state_store: Arc<crate::data::EngineWorkflowStateStore>,
}

impl Engines {
    /// Assemble the engines for a session-backed command invocation.
    ///
    /// This is the single owner of the Layer 1 graph formerly assembled by
    /// the binary entrypoint. Runtime selection is deliberately here rather
    /// than in a frontend so every session-backed host gets the same tier.
    pub fn build(global: &GlobalConfig, session: &Session) -> Result<Self, EngineError> {
        let detected = agent_runtime::detect(global)?;
        Self::from_detected(detected, session)
    }

    /// Assemble the engines used by either standalone daemon.
    ///
    /// Daemons do not have a `Session`: overlays resolve credentials from the
    /// process auth paths, auth uses the API key store, and workflow state is
    /// rooted under that daemon's own root. `paths` is the shared daemon data
    /// context retained by this common factory boundary.
    pub fn for_daemon(kind: DaemonKind, paths: &DataPaths) -> Result<Self, EngineError> {
        let auth_paths = AuthPathResolver::from_process_env()?;
        let api_paths = ApiPaths::from_process_env()?;
        let global = GlobalConfig::load().unwrap_or_default();
        let detected = agent_runtime::detect(&global)?;
        let runtime = detected.engine();
        let container_runtime = detected.container_runtime();
        let sandbox_runtime = detected.sandbox_runtime();
        let overlay_engine = Arc::new(OverlayEngine::with_auth_resolver(auth_paths.clone()));
        let agent_engine = Arc::new(AgentEngine::new(
            overlay_engine.clone(),
            container_runtime
                .clone()
                .unwrap_or_else(|| Arc::new(ContainerRuntime::docker())),
        ));

        let workflow_root = match kind {
            DaemonKind::Api => api_paths.root().to_path_buf(),
            DaemonKind::Squad => SquadPaths::from_process_env()?.root().to_path_buf(),
        };
        let _shared_data_root = paths.root();
        Ok(Self {
            runtime,
            container_runtime,
            sandbox_runtime,
            git_engine: Arc::new(GitEngine::new()),
            overlay_engine,
            auth_engine: Arc::new(AuthEngine::with_paths(auth_paths, api_paths)),
            agent_engine,
            workflow_state_store: Arc::new(crate::data::EngineWorkflowStateStore::at_git_root(
                workflow_root,
            )),
        })
    }

    pub(crate) fn from_detected(
        detected: DetectedRuntime,
        session: &Session,
    ) -> Result<Self, EngineError> {
        let runtime = detected.engine();
        let container_runtime = detected.container_runtime();
        let sandbox_runtime = detected.sandbox_runtime();
        let overlay_engine = Arc::new(OverlayEngine::new(session)?);
        let auth_engine = Arc::new(AuthEngine::new(session)?);
        // AgentEngine is container-paradigm-specific. Under a sandbox-class
        // runtime it receives an inert Docker handle that is never exercised:
        // every container-paradigm flow guards via
        // `Engines::require_container_runtime()` first (sandbox flows land in
        // WI 0090).
        let agent_engine = Arc::new(AgentEngine::new(
            overlay_engine.clone(),
            container_runtime
                .clone()
                .unwrap_or_else(|| Arc::new(ContainerRuntime::docker())),
        ));

        Ok(Self {
            runtime,
            container_runtime,
            sandbox_runtime,
            git_engine: Arc::new(GitEngine::new()),
            overlay_engine,
            auth_engine,
            agent_engine,
            workflow_state_store: Arc::new(crate::data::EngineWorkflowStateStore::at_git_root(
                session.git_root().to_path_buf(),
            )),
        })
    }

    #[cfg(test)]
    /// Assemble a hermetic container-tier engine bundle rooted at `root`.
    pub fn for_tests(root: &std::path::Path) -> Self {
        let runtime = Arc::new(ContainerRuntime::docker());
        let auth_paths = AuthPathResolver::at_home(root);
        let overlay_engine = Arc::new(OverlayEngine::with_auth_resolver(auth_paths.clone()));
        let agent_engine = Arc::new(AgentEngine::new(overlay_engine.clone(), runtime.clone()));
        Self {
            runtime: runtime.clone(),
            container_runtime: Some(runtime),
            sandbox_runtime: None,
            git_engine: Arc::new(GitEngine::new()),
            overlay_engine,
            auth_engine: Arc::new(AuthEngine::with_paths(auth_paths, ApiPaths::at_root(root))),
            agent_engine,
            workflow_state_store: Arc::new(crate::data::EngineWorkflowStateStore::at_git_root(
                root,
            )),
        }
    }

    /// The container-paradigm runtime handle, or — when the active runtime is
    /// sandbox-class — a `NotImplemented` error. Container-paradigm flows
    /// (agent setup, image builds, background containers) call this instead
    /// of unwrapping `container_runtime` so a sandbox-configured user gets an
    /// actionable error, never a panic or a silent Docker fallback.
    pub fn require_container_runtime(&self) -> Result<&Arc<ContainerRuntime>, EngineError> {
        self.container_runtime
            .as_ref()
            .ok_or(EngineError::NotImplemented(
                "this command does not yet route to the sandbox runtime \
                 (docker-sbx-experimental); set runtime to \"docker\" or \
                 \"apple-containers\" to use it here",
            ))
    }

    /// Agent-runtime detection with the documented CLI/TUI fallback policy,
    /// lifted out of `main.rs` (WI-0098 Finding B) so Layer 4 stays pure
    /// wiring. Picks the runtime named by `config`, applying three rules:
    ///
    /// * **Valid runtime** → `Ok((runtime, None))`.
    /// * **Unknown `runtime:` string** — a fatal configuration error, never a
    ///   silent Docker fallback. For a CLI invocation (`command_path`
    ///   non-empty) the [`EngineError::UnknownRuntime`] is returned so the
    ///   caller can print it and exit. For the bare-TUI invocation
    ///   (`command_path` empty) inert default (Docker) engines are still
    ///   constructed — the TUI boots only far enough to show a fatal modal —
    ///   and the error text is returned as the second tuple field for that
    ///   modal.
    /// * **Runtime unavailable on this host** (e.g. `apple-containers` on
    ///   Linux) → fatal only when the command `requires_runtime`; otherwise a
    ///   warning is printed to stderr and detection falls back to the default
    ///   Docker runtime, keeping `awman config` reachable to fix the setting.
    ///
    /// Returns the detected runtime handles paired with the optional TUI
    /// fatal-modal message (`Some` only on the unknown-runtime TUI path).
    /// `main` combines the returned [`DetectedRuntime`] with the
    /// session-derived engines to assemble the full [`Engines`] bundle.
    pub fn detect(
        catalogue: &CommandCatalogue,
        config: &GlobalConfig,
        command_path: &[&str],
    ) -> Result<(DetectedRuntime, Option<String>), EngineError> {
        match agent_runtime::detect(config) {
            Ok(detected) => Ok((detected, None)),
            Err(e @ EngineError::UnknownRuntime { .. }) => {
                // Invalid `runtime:` is fatal. CLI invocations bubble the error
                // up to be printed and exited on; the bare-TUI invocation
                // constructs inert default engines (never exercised — the
                // modal's only action is quit) and returns the message for the
                // startup modal.
                if !command_path.is_empty() {
                    return Err(e);
                }
                let fallback = agent_runtime::detect(&GlobalConfig::default())?;
                Ok((fallback, Some(e.to_string())))
            }
            Err(e) => {
                // A configured runtime this host can't construct must not lock
                // the user out of `awman config` — the documented way to switch
                // the runtime back. The catalogue decides which commands need a
                // runtime; for the rest, warn and continue on the default
                // Docker runtime, which config commands never touch.
                if catalogue.requires_runtime(command_path) {
                    return Err(e);
                }
                eprintln!(
                    "warning: configured runtime is unavailable on this host ({e}); \
                     continuing with the default Docker runtime so `awman config` \
                     can update the setting"
                );
                let fallback = agent_runtime::detect(&GlobalConfig::default())?;
                Ok((fallback, None))
            }
        }
    }
}

// ─── CommandFrontend trait ──────────────────────────────────────────────────

/// Frontend trait that supplies flag values to Dispatch. Extended by per-
/// command frontend traits (e.g. [`crate::command::commands::exec_workflow::ExecWorkflowCommandFrontend`])
/// for command-specific Q&A and reporting.
pub trait CommandFrontend: UserMessageSink + Send + Sync {
    fn flag_bool(&self, command_path: &[&str], flag: &str) -> Result<Option<bool>, CommandError>;

    fn flag_string(
        &self,
        command_path: &[&str],
        flag: &str,
    ) -> Result<Option<String>, CommandError>;

    fn flag_strings(&self, command_path: &[&str], flag: &str) -> Result<Vec<String>, CommandError>;

    fn flag_path(&self, command_path: &[&str], flag: &str)
        -> Result<Option<PathBuf>, CommandError>;

    fn flag_enum(&self, command_path: &[&str], flag: &str) -> Result<Option<String>, CommandError>;

    fn flag_u16(&self, command_path: &[&str], flag: &str) -> Result<Option<u16>, CommandError>;

    fn flag_usize(&self, command_path: &[&str], flag: &str) -> Result<Option<usize>, CommandError>;

    fn argument(&self, command_path: &[&str], name: &str) -> Result<Option<String>, CommandError>;

    fn arguments(&self, command_path: &[&str], name: &str) -> Result<Vec<String>, CommandError>;
}

// ─── Frontend supertrait ────────────────────────────────────────────────────

/// Frontend type accepted by [`Dispatch::run_command`]. A single concrete
/// frontend (CLI, TUI, or API) implements every per-command frontend
/// trait via this supertrait so that dispatch can move the frontend value
/// into the matching `Box<dyn *CommandFrontend>` for whichever variant
/// `build_command` returned. Layer 3 frontends typically derive this
/// automatically via a blanket impl over a single struct that implements
/// each trait.
pub trait DispatchFrontend:
    CommandFrontend
    + InitCommandFrontend
    + ReadyCommandFrontend
    + ChatCommandFrontend
    + StatusCommandFrontend
    + ConfigCommandFrontend
    + ExecPromptCommandFrontend
    + ExecWorkflowCommandFrontend
    + ApiServerCommandFrontend
    + SquadCommandFrontend
    + SquadAttachFrontend
    + RemoteCommandFrontend
    + NewCommandFrontend
    + AuthCommandFrontend
    + DownloadCommandFrontend
    + SpecsCommandFrontend
    + CleanCommandFrontend
    + 'static
{
}

impl<T> DispatchFrontend for T where
    T: CommandFrontend
        + InitCommandFrontend
        + ReadyCommandFrontend
        + ChatCommandFrontend
        + StatusCommandFrontend
        + ConfigCommandFrontend
        + ExecPromptCommandFrontend
        + ExecWorkflowCommandFrontend
        + ApiServerCommandFrontend
        + SquadCommandFrontend
        + SquadAttachFrontend
        + RemoteCommandFrontend
        + NewCommandFrontend
        + AuthCommandFrontend
        + DownloadCommandFrontend
        + SpecsCommandFrontend
        + CleanCommandFrontend
        + 'static
{
}

// ─── Outcome / error wrappers ───────────────────────────────────────────────

/// Catch-all outcome enum returned by `Dispatch::run_command`. Layer 3
/// inspects the variant to choose an appropriate rendering.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "payload")]
pub enum CommandOutcome {
    Init(crate::command::commands::init::InitOutcome),
    Ready(crate::command::commands::ready::ReadyOutcome),
    Chat(crate::command::commands::chat::ChatOutcome),
    Status(crate::command::commands::status::StatusOutcome),
    Config(crate::command::commands::config::ConfigOutcome),
    ExecPrompt(crate::command::commands::exec_prompt::ExecPromptOutcome),
    ExecWorkflow(crate::command::commands::exec_workflow::ExecWorkflowOutcome),
    ApiServer(crate::command::commands::api_server::ApiServerOutcome),
    Squad(crate::command::commands::squad::commands::SquadOutcome),
    SquadAttach(SquadAttachOutcome),
    Remote(crate::command::commands::remote::RemoteOutcome),
    New(crate::command::commands::new::NewOutcome),
    Specs(crate::command::commands::specs::SpecsOutcome),
    Auth(crate::command::commands::auth::AuthOutcome),
    Download(crate::command::commands::download::DownloadOutcome),
    Clean(crate::command::commands::clean::CleanOutcome),
    /// Trivial wrapper used by no-op leaf commands during the refactor.
    Empty,
}

impl CommandOutcome {
    /// The command's process/API exit code. Successful aggregate outcomes can
    /// still carry a failure: `new skill --pull-all` continues collecting
    /// libraries after one source fails, and must be visible to both CLI and
    /// API clients as a non-zero result.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ExecWorkflow(outcome) => outcome.exit_code.unwrap_or(0),
            Self::ExecPrompt(outcome) => outcome.exit_code.unwrap_or(0),
            Self::SquadAttach(outcome) => outcome.exit_code,
            _ if self.is_partial_failure() => 1,
            _ => 0,
        }
    }

    pub fn is_partial_failure(&self) -> bool {
        matches!(self, Self::New(crate::command::commands::new::NewOutcome::Skill(skill))
            if skill.libraries.iter().any(|library| library.error.is_some()))
    }
}

/// One per `*Command` struct in `src/command/commands/`. Constructed by
/// [`Dispatch::build_command`] and consumed by [`Dispatch::run_command`].
///
/// There are deliberately no `Auth` or `Download` arms: neither command is in
/// the catalogue, so neither is reachable, and WI 0114 F-35 deletes both.
pub enum BuiltCommand {
    Init(InitCommand),
    Ready(ReadyCommand),
    Chat(ChatCommand),
    Specs(SpecsCommand),
    Status(StatusCommand),
    Config(ConfigCommand),
    ExecPrompt(ExecPromptCommand),
    ExecWorkflow(ExecWorkflowCommand),
    ApiServer(ApiServerCommand),
    Squad(SquadCommand),
    SquadAttach(SquadAttachCommand),
    Remote(RemoteCommand),
    New(NewCommand),
    Clean(CleanCommand),
}

// ─── Dispatch ───────────────────────────────────────────────────────────────

pub struct Dispatch<F: CommandFrontend> {
    catalogue: &'static CommandCatalogue,
    frontend: F,
    session: Arc<RwLock<Session>>,
    engines: Engines,
    squad_gateway: Option<Arc<dyn TaskGateway>>,
}

impl<F: CommandFrontend> Dispatch<F> {
    pub fn new(frontend: F, session: Arc<RwLock<Session>>, engines: Engines) -> Self {
        // A disabled `authRefresh` leaves the monitor uninstalled, making lease
        // registration a no-op and preserving legacy env-var delivery.
        if let Ok(session_guard) = session.try_read() {
            install_credential_refresh(&session_guard.effective_config());
        }
        Self {
            catalogue: CommandCatalogue::get(),
            frontend,
            session,
            engines,
            squad_gateway: None,
        }
    }

    pub fn catalogue(&self) -> &'static CommandCatalogue {
        self.catalogue
    }

    pub fn frontend(&self) -> &F {
        &self.frontend
    }

    pub fn frontend_mut(&mut self) -> &mut F {
        &mut self.frontend
    }

    pub fn session(&self) -> Arc<RwLock<Session>> {
        Arc::clone(&self.session)
    }

    pub fn engines(&self) -> &Engines {
        &self.engines
    }

    /// Apply the catalogue-driven runtime-tier admission without performing
    /// any asynchronous gateway work. Frontends may use this before handing a
    /// command to their executor so an immediately actionable refusal can be
    /// rendered synchronously; [`Dispatch::run_command`] always repeats the
    /// same check at the authoritative execution boundary.
    pub fn validate_runtime_admission(
        engines: &Engines,
        path: &[&str],
    ) -> Result<(), CommandError> {
        let catalogue = CommandCatalogue::get();
        let canonical: Vec<&str> = catalogue.canonical_path(path).into_iter().collect();
        if catalogue
            .lookup(&canonical)
            .is_some_and(|spec| spec.requires_container_tier)
        {
            require_container_tier(engines)?;
        }
        Ok(())
    }

    /// Inject the daemon-local gateway for squad HTTP dispatch. CLI and TUI do
    /// not receive this: they obtain a remote gateway through SquadSupervisor.
    pub fn with_squad_gateway(mut self, gateway: Arc<dyn TaskGateway>) -> Self {
        self.squad_gateway = Some(gateway);
        self
    }

    /// Resolve every flag the catalogue declares for `path`.
    ///
    /// One walk over the spec's [`FlagSpec`]s: read each value through the
    /// frontend, reject mutually-exclusive pairs, apply `FlagDefault` where
    /// the frontend supplied nothing, then close over `implies` (WI 0113
    /// F-10). No command restates a default or an implication after this.
    pub fn resolve_flags(&self, path: &[&str]) -> Result<ResolvedFlags, CommandError> {
        let canonical: Vec<&str> = self.catalogue.canonical_path(path).into_iter().collect();
        let spec = self
            .catalogue
            .lookup(&canonical)
            .ok_or_else(|| CommandError::unknown_command(path))?;
        ResolvedFlags::resolve(&self.frontend, &canonical, spec)
    }

    /// Read flags from the frontend and construct the typed `*Command`. No
    /// engine work happens at this point — the command is "ready to run".
    ///
    /// Canonicalise, resolve, look up, call: every per-command decision lives
    /// behind [`CommandSpec::build`], in the command's own `from_input`.
    pub fn build_command(&self, path: &[&str]) -> Result<BuiltCommand, CommandError> {
        let canonical: Vec<&str> = self.catalogue.canonical_path(path).into_iter().collect();
        let spec = self
            .catalogue
            .lookup(&canonical)
            .ok_or_else(|| CommandError::unknown_command(path))?;
        let flags = ResolvedFlags::resolve(&self.frontend, &canonical, spec)?;
        let args = ResolvedArgs::resolve(&self.frontend, &canonical, spec.arguments)?;
        // Read the session from the shared state so every command operates
        // in the correct working directory (tab-specific in the TUI).
        let session = self
            .session
            .try_read()
            .map_err(|_| CommandError::Other("session is write-locked".into()))?
            .clone();
        let ctx = BuildContext {
            flags: &flags,
            args: &args,
            engines: &self.engines,
            session,
            gateway: self.squad_gateway.clone(),
            caller: CallerContext::new(&canonical),
        };
        (spec.build)(&ctx)
    }

    /// Tokenize a raw TUI command-box string into typed
    /// [`ParsedCommandBoxInput`]. All command-string interpretation lives
    /// here, never in the TUI.
    pub fn parse_command_box_input(raw: &str) -> Result<ParsedCommandBoxInput, CommandError> {
        parsed_input::parse(raw, CommandCatalogue::get())
    }
}

impl<F: DispatchFrontend> Dispatch<F> {
    /// Run the catalogue's pre-build admissions for `path`.
    ///
    /// The runtime tier comes first: a sandbox-class runtime cannot back
    /// squad at all, so refusing here avoids provisioning a key and then
    /// waiting ten seconds on a daemon child that was always going to refuse
    /// to start.
    ///
    /// A gateway is then resolved for whatever the spec's [`GatewayNeed`]
    /// asks for, unless one was already injected — the squad daemon supplies
    /// its own local gateway, and tests supply a double.
    async fn admit(&mut self, path: &[&str]) -> Result<(), CommandError> {
        let canonical: Vec<&str> = self.catalogue.canonical_path(path).into_iter().collect();
        let Some(spec) = self.catalogue.lookup(&canonical) else {
            // An unknown path is `build_command`'s error to report, with the
            // path the user actually typed.
            return Ok(());
        };
        Self::validate_runtime_admission(&self.engines, &canonical)?;
        if spec.gateway_need == GatewayNeed::None || self.squad_gateway.is_some() {
            return Ok(());
        }
        // The live process environment, not the session's startup snapshot: a
        // key minted earlier in this process is published into the former (see
        // `SquadSupervisor`), and the snapshot predates it.
        let resolver =
            SquadGatewayResolver::from_env(&crate::data::config::env::Env::from_process())?;
        let gateway = resolver.gateway_for(spec.gateway_need).await?;
        // The only moment the plaintext key exists outside the daemon's hash
        // file. A frontend that cannot show it drops it (see the trait's
        // default), which is why this is offered before the command runs
        // rather than folded into its output.
        if let Some(setup) = resolver.take_key_setup() {
            self.frontend.show_key_setup(&setup);
        }
        self.squad_gateway = gateway;
        Ok(())
    }

    /// Build the requested command and drive it to completion.
    ///
    /// Two admissions run before the command is built, both driven by the
    /// catalogue rather than by a frontend's own list of command names
    /// (WI 0113 F-04): the runtime-tier guard, and squad gateway resolution.
    /// Running is then one call through the [`Command`] trait.
    pub async fn run_command(mut self, path: &[&str]) -> Result<CommandOutcome, CommandError> {
        self.admit(path).await?;
        let built = self.build_command(path)?;
        built.run_with_frontend(self.frontend).await
    }
}

/// Move `$frontend` into the `Box<dyn *CommandFrontend>` each command's
/// [`Command`] impl takes, run it, and wrap the typed outcome in the matching
/// [`CommandOutcome`] variant. One line per command: the mapping is the only
/// thing that differs between arms.
macro_rules! run_built_command {
    ($built:expr, $frontend:expr, { $($variant:ident => $frontend_trait:path),* $(,)? }) => {
        match $built {
            $(
                BuiltCommand::$variant(command) => {
                    let boxed: Box<dyn $frontend_trait> = Box::new($frontend);
                    command
                        .run_with_frontend(boxed)
                        .await
                        .map(CommandOutcome::$variant)
                }
            )*
        }
    };
}

impl BuiltCommand {
    /// Run this command against `frontend`.
    ///
    /// The enum survives WI 0113 F-10 because [`Command`] carries associated
    /// `Frontend` and `Outcome` types and so cannot be made into a trait
    /// object, and because `cli::run` still needs to reach inside for the
    /// `exec workflow` carve-out. What it no longer carries is any per-command
    /// logic — only the variant-to-frontend-trait mapping below.
    pub async fn run_with_frontend<F: DispatchFrontend>(
        self,
        frontend: F,
    ) -> Result<CommandOutcome, CommandError> {
        run_built_command!(self, frontend, {
            Init => InitCommandFrontend,
            Ready => ReadyCommandFrontend,
            Chat => ChatCommandFrontend,
            Specs => SpecsCommandFrontend,
            Status => StatusCommandFrontend,
            Config => ConfigCommandFrontend,
            ExecPrompt => ExecPromptCommandFrontend,
            ExecWorkflow => ExecWorkflowCommandFrontend,
            ApiServer => ApiServerCommandFrontend,
            Squad => SquadCommandFrontend,
            SquadAttach => SquadAttachFrontend,
            Remote => RemoteCommandFrontend,
            New => NewCommandFrontend,
            Clean => CleanCommandFrontend,
        })
    }
}

pub(crate) fn parse_squad_interval(command: &[&str], raw: &str) -> Result<u64, CommandError> {
    let value = raw.trim();
    let (number, multiplier) = if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600)
    } else {
        (value, 1)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or_else(|| CommandError::InvalidFlagValue {
            command: command.iter().map(|part| (*part).to_string()).collect(),
            flag: "interval".into(),
            reason: "expected seconds or a duration such as 5m".into(),
        })
}

/// Parse `--agent-models` specs at the dispatch boundary, so Layer 2 only ever
/// sees the assembled map (WI 0110). The parse itself lives with the gateway
/// types beside the formatter that reverses it.
pub(crate) fn parse_squad_agent_models(
    command: &[&str],
    specs: &[String],
) -> Result<std::collections::BTreeMap<String, Vec<String>>, CommandError> {
    crate::command::commands::squad::gateway::parse_agent_models_specs(specs).map_err(|reason| {
        CommandError::InvalidFlagValue {
            command: command.iter().map(|part| (*part).to_string()).collect(),
            flag: "agent-models".into(),
            reason,
        }
    })
}

/// Convert the catalogue-validated launch-mode enum into the Layer 0 type.
/// Keeping this conversion at the dispatch boundary means command wiring only
/// ever sees a typed `LaunchMode`.
pub(crate) fn parse_launch_mode(
    raw: Option<String>,
    command: &[&str],
) -> Result<Option<crate::data::config::repo::LaunchMode>, CommandError> {
    match raw.as_deref() {
        None => Ok(None),
        Some("stdio") => Ok(Some(crate::data::config::repo::LaunchMode::Stdio)),
        Some("acp") => Ok(Some(crate::data::config::repo::LaunchMode::Acp)),
        Some(value) => Err(CommandError::InvalidFlagValue {
            command: command.iter().map(|part| (*part).to_string()).collect(),
            flag: "launch-mode".into(),
            reason: format!("unknown enum value {value:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording frontend used by Dispatch unit tests.
    pub(super) struct FakeCommandFrontend {
        pub bools: std::collections::HashMap<String, bool>,
        pub strings: std::collections::HashMap<String, String>,
        pub strings_vec: std::collections::HashMap<String, Vec<String>>,
        pub paths: std::collections::HashMap<String, PathBuf>,
        pub enums: std::collections::HashMap<String, String>,
        pub u16s: std::collections::HashMap<String, u16>,
        pub usizes: std::collections::HashMap<String, usize>,
        pub args: std::collections::HashMap<String, String>,
        pub args_vec: std::collections::HashMap<String, Vec<String>>,
    }

    impl FakeCommandFrontend {
        pub fn new() -> Self {
            Self {
                bools: Default::default(),
                strings: Default::default(),
                strings_vec: Default::default(),
                paths: Default::default(),
                enums: Default::default(),
                u16s: Default::default(),
                usizes: Default::default(),
                args: Default::default(),
                args_vec: Default::default(),
            }
        }
    }

    impl crate::data::message::UserMessageSink for FakeCommandFrontend {
        fn write_message(&mut self, _msg: crate::data::message::UserMessage) {}
        fn replay_queued(&mut self) {}
    }

    impl CommandFrontend for FakeCommandFrontend {
        fn flag_bool(&self, _p: &[&str], flag: &str) -> Result<Option<bool>, CommandError> {
            Ok(self.bools.get(flag).copied())
        }
        fn flag_string(&self, _p: &[&str], flag: &str) -> Result<Option<String>, CommandError> {
            Ok(self.strings.get(flag).cloned())
        }
        fn flag_strings(&self, _p: &[&str], flag: &str) -> Result<Vec<String>, CommandError> {
            Ok(self.strings_vec.get(flag).cloned().unwrap_or_default())
        }
        fn flag_path(&self, _p: &[&str], flag: &str) -> Result<Option<PathBuf>, CommandError> {
            Ok(self.paths.get(flag).cloned())
        }
        fn flag_enum(&self, _p: &[&str], flag: &str) -> Result<Option<String>, CommandError> {
            Ok(self.enums.get(flag).cloned())
        }
        fn flag_u16(&self, _p: &[&str], flag: &str) -> Result<Option<u16>, CommandError> {
            Ok(self.u16s.get(flag).copied())
        }
        fn flag_usize(&self, _p: &[&str], flag: &str) -> Result<Option<usize>, CommandError> {
            Ok(self.usizes.get(flag).copied())
        }
        fn argument(&self, _p: &[&str], name: &str) -> Result<Option<String>, CommandError> {
            Ok(self.args.get(name).cloned())
        }
        fn arguments(&self, _p: &[&str], name: &str) -> Result<Vec<String>, CommandError> {
            Ok(self.args_vec.get(name).cloned().unwrap_or_default())
        }
    }

    fn make_engines() -> Engines {
        Engines::for_tests(std::path::Path::new("/tmp"))
    }

    fn make_session() -> Arc<RwLock<Session>> {
        let tmp = tempfile::tempdir().unwrap();
        let resolver = crate::data::session::StaticGitRootResolver::new(tmp.path());
        let s = Session::open(
            tmp.path().to_path_buf(),
            &resolver,
            crate::data::session::SessionOpenOptions::default(),
        )
        .unwrap();
        Arc::new(RwLock::new(s))
    }

    #[test]
    fn build_status_command_with_no_flags() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let built = dispatch.build_command(&["status"]).unwrap();
        match built {
            BuiltCommand::Status(_) => {}
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn build_bare_squad_uses_the_status_subcommand() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let built = dispatch.build_command(&["squad"]).unwrap();
        match built {
            BuiltCommand::Squad(command) => assert!(matches!(
                command.subcommand(),
                crate::command::commands::squad::commands::SquadSubcommand::Status(_)
            )),
            _ => panic!("expected Squad"),
        }
    }

    #[test]
    fn build_unknown_command_returns_unknown_command_error() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let result = dispatch.build_command(&["bogus"]);
        match result {
            Err(CommandError::UnknownCommand { .. }) => {}
            Err(other) => panic!("expected UnknownCommand, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_specs_amend_missing_argument_errors() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let result = dispatch.build_command(&["specs", "amend"]);
        match result {
            Err(CommandError::MissingRequiredArgument { .. }) => {}
            Err(other) => panic!("expected MissingRequiredArgument, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_chat_with_yolo_and_plan_returns_mutually_exclusive() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("yolo".into(), true);
        frontend.bools.insert("plan".into(), true);
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["chat"]);
        match result {
            Err(CommandError::MutuallyExclusive { .. }) => {}
            Err(other) => panic!("expected MutuallyExclusive, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn ready_json_implies_non_interactive_in_built_command() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("json".into(), true);
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["ready"]).unwrap();
        match built {
            BuiltCommand::Ready(cmd) => {
                assert!(
                    cmd.flags().non_interactive,
                    "json should imply non_interactive"
                );
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn exec_workflow_yolo_implies_worktree_in_built_command() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("yolo".into(), true);
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "workflow"]).unwrap();
        match built {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert!(
                    cmd.flags().worktree,
                    "yolo should imply worktree on exec workflow"
                );
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn exec_workflow_auto_implies_worktree_in_built_command() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("auto".into(), true);
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "workflow"]).unwrap();
        match built {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert!(
                    cmd.flags().worktree,
                    "auto should imply worktree on exec workflow"
                );
                assert!(cmd.flags().auto);
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn build_config_show_succeeds_with_no_args() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let built = dispatch.build_command(&["config", "show"]).unwrap();
        assert!(matches!(built, BuiltCommand::Config(_)));
    }

    #[test]
    fn build_config_get_with_field_argument() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .args
            .insert("field".into(), "terminal_scrollback_lines".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["config", "get"]).unwrap();
        assert!(matches!(built, BuiltCommand::Config(_)));
    }

    #[test]
    fn build_config_get_missing_field_returns_missing_required_argument() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let result = dispatch.build_command(&["config", "get"]);
        assert!(
            matches!(result, Err(CommandError::MissingRequiredArgument { .. })),
            "missing field must return MissingRequiredArgument"
        );
    }

    #[test]
    fn build_new_workflow_with_format_flag() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.enums.insert("format".into(), "yaml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["new", "workflow"]).unwrap();
        assert!(matches!(built, BuiltCommand::New(_)));
    }

    #[test]
    fn build_api_start_with_port() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.u16s.insert("port".into(), 1234);
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["api", "start"]).unwrap();
        assert!(matches!(built, BuiltCommand::ApiServer(_)));
    }

    #[test]
    fn build_chat_default_flags_all_false() {
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let built = dispatch.build_command(&["chat"]).unwrap();
        match built {
            BuiltCommand::Chat(cmd) => {
                let f = cmd.flags();
                assert!(!f.yolo && !f.plan && !f.non_interactive && !f.allow_docker);
            }
            _ => panic!("expected Chat"),
        }
    }

    #[test]
    fn build_remote_exec_workflow_with_workflow_argument() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch
            .build_command(&["remote", "exec", "workflow"])
            .unwrap();
        assert!(matches!(built, BuiltCommand::Remote(_)));
    }

    #[test]
    fn build_remote_exec_prompt_with_prompt_argument() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.args.insert("prompt".into(), "hello".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch
            .build_command(&["remote", "exec", "prompt"])
            .unwrap();
        assert!(matches!(built, BuiltCommand::Remote(_)));
    }

    #[test]
    fn build_exec_prompt_with_prompt_argument() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.args.insert("prompt".into(), "do something".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "prompt"]).unwrap();
        assert!(matches!(built, BuiltCommand::ExecPrompt(_)));
    }

    #[test]
    fn build_exec_prompt_with_empty_prompt_builds_ok_when_issue_may_provide_input() {
        // A whitespace-only prompt is normalised to None at dispatch time;
        // final validation (prompt-or-issue required) happens at run time.
        let mut frontend = FakeCommandFrontend::new();
        frontend.args.insert("prompt".into(), "   ".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "prompt"]);
        assert!(
            result.is_ok(),
            "whitespace prompt should build OK (validation deferred to runtime)"
        );
    }

    #[test]
    fn build_exec_workflow_missing_workflow_argument_returns_missing_required_argument() {
        // workflow is required and neither flag nor positional arg is set
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        assert!(
            matches!(result, Err(CommandError::MissingRequiredArgument { .. })),
            "missing workflow must return MissingRequiredArgument"
        );
    }

    #[test]
    fn alias_wf_resolves_to_exec_workflow() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        // "wf" is a string alias under "exec"; dispatch should resolve it.
        let built = dispatch.build_command(&["exec", "wf"]).unwrap();
        assert!(
            matches!(built, BuiltCommand::ExecWorkflow(_)),
            "exec wf must dispatch to ExecWorkflow"
        );
    }

    // ─── parse_command_box_input ──────────────────────────────────────────────

    #[test]
    fn parse_command_box_input_exec_workflow_with_yolo() {
        let parsed = Dispatch::<FakeCommandFrontend>::parse_command_box_input(
            "exec workflow my-workflow.toml --yolo",
        )
        .unwrap();
        assert_eq!(parsed.path, vec!["exec", "workflow"]);
        assert!(matches!(
            parsed.flags.get("yolo"),
            Some(parsed_input::FlagValue::Bool(true))
        ));
        match parsed.arguments.get("workflow") {
            Some(parsed_input::ArgValue::Single(s)) => {
                assert_eq!(s, "my-workflow.toml");
            }
            other => panic!("expected Single workflow argument, got: {other:?}"),
        }
    }

    #[test]
    fn parse_command_box_input_rejects_unknown_top_level_command() {
        let result = Dispatch::<FakeCommandFrontend>::parse_command_box_input("not-a-command");
        assert!(
            matches!(result, Err(CommandError::UnknownCommand { .. })),
            "unknown command must return UnknownCommand, got: {result:?}"
        );
    }

    #[test]
    fn parse_command_box_input_rejects_unknown_flag() {
        let result = Dispatch::<FakeCommandFrontend>::parse_command_box_input("status --bogus");
        assert!(
            matches!(result, Err(CommandError::UnknownFlag { .. })),
            "unknown flag must return UnknownFlag, got: {result:?}"
        );
    }

    #[test]
    fn parse_command_box_input_remote_exec_workflow() {
        let parsed = Dispatch::<FakeCommandFrontend>::parse_command_box_input(
            "remote exec workflow my-workflow.toml --follow",
        )
        .unwrap();
        assert_eq!(parsed.path, vec!["remote", "exec", "workflow"]);
        match parsed.arguments.get("workflow") {
            Some(parsed_input::ArgValue::Single(s)) => {
                assert_eq!(s, "my-workflow.toml");
            }
            other => panic!("expected Single workflow argument, got: {other:?}"),
        }
        assert!(matches!(
            parsed.flags.get("follow"),
            Some(parsed_input::FlagValue::Bool(true))
        ));
    }

    #[test]
    fn parse_command_box_input_short_flag_non_interactive() {
        let parsed = Dispatch::<FakeCommandFrontend>::parse_command_box_input("ready -n").unwrap();
        assert_eq!(parsed.path, vec!["ready"]);
        assert!(matches!(
            parsed.flags.get("non-interactive"),
            Some(parsed_input::FlagValue::Bool(true))
        ));
    }

    #[test]
    fn exec_workflow_no_yolo_no_auto_worktree_false() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        // Neither yolo nor auto is set; worktree must not be implied.
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "workflow"]).unwrap();
        match built {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert!(
                    !cmd.flags().worktree,
                    "worktree must be false when neither yolo nor auto is set"
                );
                assert!(!cmd.flags().yolo);
                assert!(!cmd.flags().auto);
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn exec_workflow_yolo_plus_explicit_worktree_true_stays_true() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("yolo".into(), true);
        frontend.bools.insert("worktree".into(), true);
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "workflow"]).unwrap();
        match built {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert!(cmd.flags().yolo);
                assert!(
                    cmd.flags().worktree,
                    "worktree must be true when both yolo and --worktree are set"
                );
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    // ── Issue flag dispatch tests ─────────────────────────────────────────────

    #[test]
    fn build_exec_workflow_issue_flag_populates_issue_source() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .strings
            .insert("issue".into(), "owner/repo#84".into());
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "workflow"]).unwrap();
        match built {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert_eq!(
                    cmd.flags().issue_source.issue.as_deref(),
                    Some("owner/repo#84"),
                    "issue_source.issue must be populated from --issue flag"
                );
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn build_exec_workflow_issue_and_work_item_are_mutually_exclusive() {
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .strings
            .insert("issue".into(), "owner/repo#84".into());
        frontend
            .strings
            .insert("work-item".into(), "0084-my-item.md".into());
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        match result {
            Err(CommandError::MutuallyExclusive { .. }) => {}
            Err(other) => panic!("expected MutuallyExclusive, got {other:?}"),
            Ok(_) => panic!("expected error for mutually exclusive flags"),
        }
    }

    #[test]
    fn build_exec_prompt_issue_flag_populates_issue_source() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.strings.insert("issue".into(), "42".into());
        // No positional prompt — that's ok at dispatch time (validated at runtime).
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let built = dispatch.build_command(&["exec", "prompt"]).unwrap();
        match built {
            BuiltCommand::ExecPrompt(cmd) => {
                assert_eq!(
                    cmd.flags().issue_source.issue.as_deref(),
                    Some("42"),
                    "issue_source.issue must be populated from --issue flag"
                );
            }
            _ => panic!("expected ExecPrompt"),
        }
    }

    #[test]
    fn build_exec_prompt_issue_and_prompt_are_both_optional() {
        // When only --issue is set (no positional prompt), build_command must succeed.
        // The runtime check (at least one of prompt/issue) happens in run_with_frontend.
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .strings
            .insert("issue".into(), "owner/repo#1".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "prompt"]);
        assert!(
            result.is_ok(),
            "build_command must succeed when only --issue is set: {}",
            result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        );
        match result.unwrap() {
            BuiltCommand::ExecPrompt(cmd) => {
                assert!(
                    cmd.flags().prompt.is_none(),
                    "prompt must be None when no positional argument is provided"
                );
                assert_eq!(
                    cmd.flags().issue_source.issue.as_deref(),
                    Some("owner/repo#1")
                );
            }
            _ => panic!("expected ExecPrompt"),
        }
    }

    // ─── WI-0092: Dynamic Workflows — dispatch layer tests ───────────────────

    #[test]
    fn exec_workflow_dynamic_without_path_builds_successfully() {
        // --dynamic omits the positional workflow path; dispatch must not error.
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("dynamic".into(), true);
        frontend.strings.insert("work-item".into(), "0042".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        assert!(
            result.is_ok(),
            "--dynamic without workflow path must succeed at dispatch: {}",
            result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        );
        match result.unwrap() {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert!(cmd.flags().dynamic, "dynamic flag must be true");
                assert!(
                    cmd.flags().workflow.is_none(),
                    "workflow path must be None for --dynamic"
                );
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn exec_workflow_dynamic_without_work_item_returns_error() {
        // --dynamic requires --work-item; missing it must error at dispatch.
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("dynamic".into(), true);
        // No work-item set.
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("--dynamic requires --work-item"),
                    "error must name the missing flag, got: {msg}"
                );
            }
            Ok(_) => panic!("--dynamic without --work-item must return an error"),
        }
    }

    #[test]
    fn exec_workflow_leader_without_dynamic_returns_error() {
        // --leader is only valid with --dynamic.
        let mut frontend = FakeCommandFrontend::new();
        frontend
            .strings
            .insert("leader".into(), "claude::claude-opus-4-8".into());
        frontend
            .args
            .insert("workflow".into(), "/tmp/wf.toml".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("--leader is only valid with --dynamic"),
                    "error must state the constraint, got: {msg}"
                );
            }
            Ok(_) => panic!("--leader without --dynamic must return an error"),
        }
    }

    #[test]
    fn exec_workflow_dynamic_parses_leader_flag() {
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("dynamic".into(), true);
        frontend.strings.insert("work-item".into(), "0042".into());
        frontend
            .strings
            .insert("leader".into(), "claude::claude-opus-4-8".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        assert!(
            result.is_ok(),
            "build must succeed: {}",
            result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        );
        match result.unwrap() {
            BuiltCommand::ExecWorkflow(cmd) => {
                assert_eq!(
                    cmd.flags().leader.as_deref(),
                    Some("claude::claude-opus-4-8"),
                    "leader flag must be preserved in ExecWorkflowCommandFlags"
                );
            }
            _ => panic!("expected ExecWorkflow"),
        }
    }

    #[test]
    fn exec_workflow_dynamic_with_plan_returns_error() {
        // --dynamic enforces yolo so --plan is incompatible.
        let mut frontend = FakeCommandFrontend::new();
        frontend.bools.insert("dynamic".into(), true);
        frontend.bools.insert("plan".into(), true);
        frontend.strings.insert("work-item".into(), "0042".into());
        let dispatch = Dispatch::new(frontend, make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("--dynamic cannot be used with --plan"),
                    "error must explain the conflict, got: {msg}"
                );
            }
            Ok(_) => panic!("--dynamic --plan must return an error"),
        }
    }

    #[test]
    fn exec_workflow_static_without_path_returns_missing_required_argument() {
        // Non-dynamic invocation still requires the positional workflow path.
        let dispatch = Dispatch::new(FakeCommandFrontend::new(), make_session(), make_engines());
        let result = dispatch.build_command(&["exec", "workflow"]);
        assert!(
            matches!(result, Err(CommandError::MissingRequiredArgument { .. })),
            "static exec workflow without path must return MissingRequiredArgument"
        );
    }

    // ── WI-0098 Finding B: Engines::detect runtime-detection policy ───────────
    //
    // The three documented paths lifted out of `main.rs`: valid runtime, an
    // unknown `runtime:` string (fatal for CLI, modal for TUI), and a runtime
    // unavailable on this host (fatal only when the command requires a runtime).

    fn config_with_runtime(runtime: Option<&str>) -> GlobalConfig {
        GlobalConfig {
            runtime: runtime.map(String::from),
            ..Default::default()
        }
    }

    fn session_at(root: &std::path::Path) -> Session {
        Session::open_at_git_root(
            root.to_path_buf(),
            root.to_path_buf(),
            crate::data::session::SessionOpenOptions::default(),
        )
        .expect("open test session")
    }

    fn with_daemon_global_config<T>(config: GlobalConfig, test: impl FnOnce() -> T) -> T {
        let _guard = crate::CWD_LOCK
            .lock()
            .expect("process settings mutex poisoned");
        let home = tempfile::tempdir().expect("create global config home");
        let previous = std::env::var_os("AWMAN_CONFIG_HOME");
        std::env::set_var("AWMAN_CONFIG_HOME", home.path());
        config.save().expect("save global config");
        let result = test();
        match previous {
            Some(value) => std::env::set_var("AWMAN_CONFIG_HOME", value),
            None => std::env::remove_var("AWMAN_CONFIG_HOME"),
        }
        result
    }

    #[test]
    fn engines_build_and_for_daemon_produce_container_tier_under_default_config() {
        let root = tempfile::tempdir().expect("create test root");
        let session = session_at(root.path());
        let built = Engines::build(&GlobalConfig::default(), &session)
            .expect("default session engines build");
        assert!(built.container_runtime.is_some());
        assert!(built.sandbox_runtime.is_none());

        with_daemon_global_config(GlobalConfig::default(), || {
            let daemon = Engines::for_daemon(DaemonKind::Api, &DataPaths::at_root(root.path()))
                .expect("default daemon engines build");
            assert!(daemon.container_runtime.is_some());
            assert!(daemon.sandbox_runtime.is_none());
        });
    }

    /// `docker-sbx-experimental` is platform-gated in `SandboxRuntime::dsbx`
    /// (linux and x86_64 macos both refuse with `BackendUnsupportedOnPlatform`
    /// — see `engine::sandbox::runtime`'s own `dsbx_errors_on_*` tests, which
    /// use the same branch-on-`cfg!` pattern). `Engines::build`/`for_daemon`
    /// only assemble a sandbox-tier bundle on the platforms where the backend
    /// actually constructs.
    #[test]
    fn engines_build_and_for_daemon_produce_sandbox_tier_for_experimental_runtime() {
        let root = tempfile::tempdir().expect("create test root");
        let session = session_at(root.path());
        let config = config_with_runtime(Some("docker-sbx-experimental"));

        if cfg!(target_os = "linux") || cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            match Engines::build(&config, &session) {
                Err(EngineError::BackendUnsupportedOnPlatform { backend, .. }) => {
                    assert_eq!(backend, "docker-sbx-experimental")
                }
                Err(e) => panic!("expected BackendUnsupportedOnPlatform, got {e:?}"),
                Ok(_) => panic!("dsbx build must fail on this platform"),
            }

            with_daemon_global_config(config, || {
                match Engines::for_daemon(DaemonKind::Squad, &DataPaths::at_root(root.path())) {
                    Err(EngineError::BackendUnsupportedOnPlatform { backend, .. }) => {
                        assert_eq!(backend, "docker-sbx-experimental")
                    }
                    Err(e) => panic!("expected BackendUnsupportedOnPlatform, got {e:?}"),
                    Ok(_) => panic!("dsbx for_daemon must fail on this platform"),
                }
            });
            return;
        }

        let built = Engines::build(&config, &session).expect("sandbox session engines build");
        assert!(built.container_runtime.is_none());
        assert!(built.sandbox_runtime.is_some());

        with_daemon_global_config(config, || {
            let daemon = Engines::for_daemon(DaemonKind::Squad, &DataPaths::at_root(root.path()))
                .expect("sandbox daemon engines build");
            assert!(daemon.container_runtime.is_none());
            assert!(daemon.sandbox_runtime.is_some());
        });
    }

    #[test]
    fn detect_valid_runtime_returns_runtime_and_no_modal_message() {
        let cat = CommandCatalogue::get();
        let cfg = config_with_runtime(Some("docker"));
        let (detected, modal) = Engines::detect(cat, &cfg, &["status"])
            .expect("a valid runtime must detect successfully");
        assert_eq!(detected.engine().runtime_name(), "docker");
        assert!(
            modal.is_none(),
            "no fatal-modal message on the valid-runtime path"
        );
    }

    #[test]
    fn detect_unknown_runtime_cli_returns_unknown_runtime_error() {
        // A CLI invocation (non-empty command path) with a misspelled runtime is
        // a fatal configuration error the caller prints and exits on.
        let cat = CommandCatalogue::get();
        let cfg = config_with_runtime(Some("totally-bogus-runtime"));
        // `DetectedRuntime` is not `Debug`, so match rather than `expect_err`.
        match Engines::detect(cat, &cfg, &["status"]) {
            Err(EngineError::UnknownRuntime { .. }) => {}
            Err(other) => panic!("expected UnknownRuntime, got {other:?}"),
            Ok(_) => panic!("an unknown runtime must be an error for CLI invocations"),
        }
    }

    #[test]
    fn detect_unknown_runtime_tui_builds_default_engines_and_returns_modal_message() {
        // The bare-TUI invocation (empty command path) must still construct inert
        // default (Docker) engines so the fatal modal can render, and return the
        // error text for that modal.
        let cat = CommandCatalogue::get();
        let cfg = config_with_runtime(Some("totally-bogus-runtime"));
        let (detected, modal) =
            Engines::detect(cat, &cfg, &[]).expect("the TUI path must still yield default engines");
        assert_eq!(
            detected.engine().runtime_name(),
            "docker",
            "the TUI fallback must be the default Docker runtime"
        );
        let msg = modal.expect("the TUI path must return a fatal-modal message");
        assert!(
            msg.contains("totally-bogus-runtime"),
            "modal message must name the bad runtime; got: {msg}"
        );
    }

    // The unavailable-on-host path needs a runtime that this host cannot
    // construct. `apple-containers` is unavailable on every non-macOS host, so
    // these two tests exercise the fatal-vs-warn branch there.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn detect_unavailable_runtime_is_fatal_when_command_requires_runtime() {
        let cat = CommandCatalogue::get();
        let cfg = config_with_runtime(Some("apple-containers"));
        // `status` requires a runtime → the unavailable runtime is fatal.
        assert!(cat.requires_runtime(&["status"]));
        match Engines::detect(cat, &cfg, &["status"]) {
            Err(EngineError::UnknownRuntime { .. }) => {
                panic!("an unavailable (not unknown) runtime must not surface as UnknownRuntime")
            }
            Err(_) => {}
            Ok(_) => panic!("an unavailable runtime must be fatal for a runtime-requiring command"),
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn detect_unavailable_runtime_warns_and_falls_back_when_command_allows() {
        let cat = CommandCatalogue::get();
        let cfg = config_with_runtime(Some("apple-containers"));
        // `config` does not require a runtime → warn on stderr and fall back to
        // the default Docker runtime so `awman config` stays reachable.
        assert!(!cat.requires_runtime(&["config", "show"]));
        let (detected, modal) = Engines::detect(cat, &cfg, &["config", "show"])
            .expect("config commands must fall back rather than fail");
        assert_eq!(
            detected.engine().runtime_name(),
            "docker",
            "the fallback must be the default Docker runtime"
        );
        assert!(
            modal.is_none(),
            "the unavailable-but-tolerated path yields no TUI modal message"
        );
    }

    #[test]
    fn command_outcome_exit_code_covers_execution_and_partial_skill_failure() {
        assert_eq!(
            CommandOutcome::ExecWorkflow(
                crate::command::commands::exec_workflow::ExecWorkflowOutcome {
                    workflow: "workflow.toml".into(),
                    exit_code: Some(17),
                    worktree_used: false,
                }
            )
            .exit_code(),
            17
        );
        assert_eq!(
            CommandOutcome::ExecPrompt(crate::command::commands::exec_prompt::ExecPromptOutcome {
                agent: None,
                exit_code: Some(3)
            })
            .exit_code(),
            3
        );
        let partial = CommandOutcome::New(crate::command::commands::new::NewOutcome::Skill(
            crate::command::commands::new::NewSkillOutcome {
                interview: false,
                global: false,
                path: None,
                pull: true,
                libraries: vec![crate::command::commands::new::PullLibraryOutcome {
                    slug: "broken".into(),
                    dir: "broken".into(),
                    updated: false,
                    skills_found: Vec::new(),
                    error: Some("unreachable".into()),
                }],
            },
        ));
        assert!(partial.is_partial_failure());
        assert_eq!(partial.exit_code(), 1);
        assert_eq!(CommandOutcome::Empty.exit_code(), 0);
    }
}
