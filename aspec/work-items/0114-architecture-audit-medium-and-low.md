# Work Item: Task

Title: Architecture audit remediation, part 2 — the Medium and Low findings
(F-13 through F-54) from the 2026-09-03 architecture audit
Issue: `aspec/review-notes/0113-architecture-audit.md`

## Summary:
- The 2026-09-03 architecture audit (`aspec/review-notes/0113-architecture-audit.md`,
  hereafter "the report") is the "what": evidence, `file:line` citations,
  and why each item matters. This work item is the "how" for the 28 Medium
  and 14 Low findings, **F-13 through F-54**. Every section cites the finding
  it implements; do not re-derive the evidence here.
- WI 0113 (Critical and High, F-01–F-12) lands first. Several of its steps
  absorb a piece of a finding listed here; each such finding below opens
  with an "Already done by 0113" line so implementers pick up only the
  remainder. Do not start this work item before 0113 Steps 1, 2, 5 and 9 are
  merged: F-19, F-21, F-26, F-31, F-49 and F-50 build on `CommandSpec`
  attributes and `ResolvedFlags` that 0113 introduces.
- The findings fall into six groups, worked in the report's "Proposed
  execution order": (A) small Layer 0/Layer 1 relocations and typed
  replacements, (B) small Tenet 2 pull-downs from the frontends into Layer 2,
  (C) headless parity and session lifecycle, (D) large layer relocations
  gated on report Q1, (E) engine consolidation, and (F) file splits and
  typed statuses. Each group is a separate PR series; one finding per commit.
- Implementing agents MUST read `aspec/architecture/2026-grand-architecture.md`
  and `aspec/architecture/four-layer-summary.md` in full before starting.
  When the correct layer for something is ambiguous, ask the developer; do
  not guess.

## User Stories

### User Story 1:
As a: user running the same workflow from the CLI, the TUI, the API and the
squad daemon

I want to:
identical prompts, identical option labels, identical headless answers and
identical exit codes whichever mode ran it

So I can:
trust mode parity (report F-13, F-19, F-42, F-50; P2).

### User Story 2:
As a: developer adding an agent, a runtime backend, or a config field

I want to:
one table per concept (`AgentMatrix`, `ContainerBackend`, `ConfigFieldSpec`,
`Env`) instead of string matches scattered across four files

So I can:
make the addition in one place and have the compiler tell me what I missed
(report F-32, F-33, F-37, F-51).

### User Story 3:
As a: maintainer reading `src/engine/workflow/mod.rs` or `src/command/dispatch/mod.rs`

I want to:
files under ~1,500 lines with one responsibility each, no hand-written
forwarding proxies, no duplicated phase machines, and no test fixtures
copied nine times

So I can:
read the code as the intermediate Rust developer the spec is written for
(report F-34, F-36, F-51, F-52; P1).

## Implementation Details:

Run `make pre-push` after every commit. Any commit that moves code across a
layer boundary (groups A, D and every item marked "crosses layers") also runs
`make architecture-lint` on its own; paste the result into the commit
message. Behaviour changes are called out per finding and must appear in the
release notes.

### Step 0 — Decisions (resolved 2026-09-04)

Every open question is answered; the full table is in the report under
"Decisions — 2026-09-04". The ones that shape this work item:

- **Q1 — strict Layer 0.** Group D moves the overlay grammar down to L0 and
  the HTTP client and daemon supervision up to L1, as written.
- **Q2 — outcomes are L2-owned.** Only `RunVerdict` moves (F-47).
- **Q3 — wire `SessionState`.** F-22 and F-30 are written to that decision.
- **Q4 — squad is not an exception**; all squad business logic lives in L1/L2
  (0113 Step 3 does the bulk; F-13, F-17, F-29, F-38, F-40, F-47 here finish
  it). Any squad change that leaves a decision in `src/frontend/squad/` is
  wrong.
- **Q5 — deliberate.** Two profiles: `HeadlessDefaults::api()` and
  `HeadlessDefaults::squad()`.
- **Q6 — delete the banner entirely** (F-15).
- **Q7 — `api_allowed: false` is long-term policy** for interactive/PTY
  commands; F-50's `FrontendKind::SquadDaemon` visibility follows the same
  mechanism.
- **Q8 — the code is right, and `aspec/uxui/cli.md` should not document
  specific commands and flags.** F-25 turns `cli.md` into general UI/UX
  standards and generates the per-command reference into `docs/`.
- **Q9 — `--format md` is gone** intentionally.
- **Q10 — the wire schema is not frozen.** F-48's enums may use the names
  that read best; no byte-for-byte guard is required.
- **Q12 — make `ReadyEngine`/`InitEngine` runtime-agnostic** here: new item
  F-40b in group E.
- **Q13 — move the shared helpers** to `commands/workflow_preflight.rs`
  (F-51).
- **F-35 — delete both** `AuthCommand` and `DownloadCommand`.
- **F-19 — abort on Esc everywhere** in the `new` interviews.

### Group A — small Layer 0 / Layer 1 relocations and typed replacements

#### F-33: rename the second `AgentFrontend`; delete the re-export shims
Report F-33 (`src/engine/agent/frontend.rs:12-22`, `src/engine/agent_runtime/frontend.rs:81-110`,
`src/engine/step_status.rs:1`, `src/engine/ready/phase.rs:1`, `src/engine/ready/summary.rs:1`).
1. Rename `engine::agent::AgentFrontend` to `AgentSetupFrontend`. Make
   `ReadyFrontend: AgentSetupFrontend` and `InitFrontend: AgentSetupFrontend`
   and delete their inline re-declarations of `report_step_status` /
   `container_frontend`.
2. Delete the three `pub use crate::data::…::*` shim files and
   `engine/mod.rs:29`; `sed` every `crate::engine::step_status::StepStatus`,
   `crate::engine::ready::phase::ReadyPhase`, `crate::engine::ready::summary::ReadySummary`
   import to the `crate::data::…` path.
3. Tests: compile-only; no behaviour.

#### F-36: blanket forwarding impls replace `WorkflowProxy` and `AgentFrontendProxy`
Report F-36 (`src/command/commands/exec_workflow.rs:222-528`).
1. In `src/engine/workflow/frontend.rs` add
   `impl<F: WorkflowFrontend + ?Sized> WorkflowFrontend for Arc<Mutex<Box<F>>>`
   forwarding every method (including the defaulted ones, so a frontend's
   override is not lost behind the default). Same for `AgentFrontend` in
   `src/engine/agent_runtime/frontend.rs`.
2. Delete both proxies; `exec_workflow.rs` passes the `Arc<Mutex<Box<dyn …>>>`
   directly.
3. Tests: `tests/engine/workflow_end_to_end.rs` and the `exec_workflow`
   colocated tests pass; add one test that a method overridden on the inner
   frontend is reached through the blanket impl.

#### F-35: delete dead commands and empty traits; one `AgentLaunchFrontend`
Report F-35 (`src/command/dispatch/mod.rs:324-325`, `src/command/commands/auth.rs`,
`download.rs`, `api_server.rs:111-124`, `specs.rs:257-270`, `squad/commands.rs:336-372`).
1. Delete `AuthCommand`, `DownloadCommand`, `AuthCommandFrontend`,
   `DownloadCommandFrontend`, the `BuiltCommand`/`CommandOutcome` variants
   and the `DispatchFrontend` bounds (developer decision). Keep
   `DownloadCommand`'s tarball logic as a private helper on `InitCommand`
   (`init --aspec` is its only caller); once F-29 lands it calls
   `AspecDownloader` directly.
2. Delete the three empty `ApiServer{Kill,Logs,Status}CommandFrontend`
   traits and merge `ApiServerStartCommandFrontend` into
   `ApiServerCommandFrontend`.
3. Add `pub trait AgentLaunchFrontend: MountScopeFrontend + AgentSetupFrontend + AgentAuthFrontend + HasAgentFrontend { fn set_pty_active(&mut self, active: bool); fn set_stuck_sender(&mut self, …) {} }`
   in `src/command/commands/agent_setup.rs`; `ChatCommandFrontend`,
   `ExecPromptCommandFrontend`, `ExecWorkflowCommandFrontend`,
   `SpecsCommandFrontend` extend it and drop their own copies. Pick one
   default for `set_pty_active` (required) and record why in the doc comment.
4. Fold `SquadDaemonCommand::{run_start, run_stop, run_status, run_logs}`
   into `SquadCommand` (or a plain `SquadDaemon` struct without a `Command`
   impl); delete `SquadDaemonCommand` and `SquadDaemonOutcome`.
5. Tests: the `Command`-trait impl count drops from 16 to 13; update
   `tests/cli_parity/catalogue_completeness.rs` accordingly.

#### F-34: `PhaseKind` collapses the setup/teardown twins; typed constructor args
Report F-34 (`src/engine/workflow/mod.rs:2362-2866`, `src/engine/workflow/frontend.rs:122-135`).
Already done by 0113 Step 1: the two never-read engine fields and their
constructor parameters are deleted.
1. Add `enum PhaseKind { Setup, Teardown }` to `src/data/workflow_state.rs`
   with `label()`; key phase step state by it (`phase_step_states(kind)` /
   `phase_step_states_mut(kind)` accessors over the two existing vectors are
   enough — do not change the on-disk `WorkflowState` shape).
2. Collapse `run_setup`/`run_teardown` into `run_phase(kind, …)`,
   `run_shell_phase_step(phase: &str)` into `run_single_phase_step(kind, …)`,
   `set_phase_step_failed(is_setup: bool)` into `(kind, …)`, and the two
   remediation twins into `run_phase_remediation(kind, …)`. Teardown's
   `(aborted, any_failed)` return and captured stdout/stderr become the
   common behaviour; the setup path therefore gains the failure-file
   behaviour teardown has. Call this out in the release notes as an
   intentional improvement, and add a test for it.
3. Replace the ten `on_setup_*`/`on_teardown_*` trait methods with five
   `on_phase_step_{started,output,completed,failed,fixing}(kind, …)`.
   Update the six real impls (`cli/per_command/workflow_frontend_marker.rs`,
   `cli/parallel.rs`, `api/command_frontend.rs`, `tui/per_command/workflow_frontend.rs`,
   `squad/unattended.rs`, `exec_workflow.rs`) and the eight test fakes.
4. Introduce `WorkflowEngineDeps { frontend, agent_factory }` and
   `WorkflowSpec { workflow, work_item_context, state_root: Option<PathBuf> }`
   so `new(session, spec, deps)` / `resume(session, spec, deps)` replace the
   remaining 5–6-parameter constructors and `resume_with_state_root` goes
   away. De-duplicate the struct literal shared by `new` and `resume`.
5. Tests: `tests/engine/workflow_end_to_end.rs`, `workflow_on_failure.rs`,
   `tests/api_parity/wi_0079.rs` (setup/teardown events) pass; the API event
   `phase` string must remain `"setup"`/`"teardown"`.

#### F-37: declare every environment variable in `Env`; no `std::env::var` above Layer 0
Report F-37 (`src/engine/workflow/poll_ci.rs:29`, `src/engine/container/attach_socket.rs:77`,
`src/frontend/api/session_setup.rs:463-471`, `docker.rs:1288-1292`, `options.rs:474, 480`,
`sandbox/dsbx/session_config.rs:89`). Already done by 0113 Step 11:
`GITHUB_TOKEN` declared and passed into the issue engine.
1. Declare `AWMAN_ATTACH_DIR` and `AWMAN_API_VERBOSE_SETUP` in
   `src/data/config/env.rs` with typed accessors on `EnvSnapshot`
   (`attach_dir() -> Option<PathBuf>`, `api_verbose_setup() -> bool` with the
   `0|false|no|off` falsy parse moved from the frontend). Delete
   `verbose_setup_enabled()` from `src/frontend/api/session_setup.rs`; pass
   the bool through `ApiSessionSetupObserver`.
2. Wrap `poll_ci.rs` as `CiPoller { git: Arc<GitEngine>, token: Option<String> }`
   with `poll(&self, on_message)`; branch/SHA/remote come from `GitEngine`
   (add `head_sha` if missing); the `reqwest` fallback uses the engine-level
   HTTP client once F-28 lands (until then keep the local client).
3. Env passthrough: resolve `env_passthrough` names into `EnvLiteral` values
   at L2 from an `EnvSnapshot` before building `ResolvedContainerOptions` /
   `ResolvedSandboxOptions`; delete the three spawn-time
   `std::env::var(&envvar.0)` loops. Keep the hermetic `lookup_env` closure
   seams the tests rely on.
4. Add a lint line to `tools/architecture-lint.sh`: `std::env::var` /
   `env::var(` outside `src/data/` fails, with an explicit allowlist for
   `#[cfg(test)]` modules and `src/frontend/cli/output.rs` (`NO_COLOR`,
   presentation).

#### F-39: `GitEngine` owns the identity probe and the worktree status check
Report F-39 (`src/command/commands/exec_workflow.rs:1532-1545, 2975-2984`).
1. Add `GitEngine::identity_configured(&self, path: &Path) -> Result<GitIdentity, EngineError>`
   (`GitIdentity { name: Option<String>, email: Option<String> }`) that runs
   `git -C <path> config user.name|user.email`. Call it with the worktree
   or session root. **Behaviour change**: a repo-local identity is now
   honoured; note in the release notes.
2. Replace `worktree_git_status` with `git_engine.uncommitted_files(worktree)`
   and compare `Vec<String>` baselines.
3. Tests: `tests/engine/git_engine.rs` gains identity tests (global only,
   repo-local override, none configured).

#### F-40: branch on capabilities, not on which runtime handle is `Some`
Report F-40 (`src/command/commands/ready.rs:190`, `clean.rs:218, 404, 458`,
`src/engine/sandbox/mod.rs:29-35`).
1. `ready.rs:190`: branch on `engines.runtime.capabilities().kit_declarative`.
2. Add `Capabilities::has_image_store: bool` (true for container tier);
   `clean.rs` uses it for the dangling-image category and the trait's
   `list_running`/`stop`/`remove` for the rest.
3. `ready_sbx_agent` becomes `SandboxRuntime::ready_agent(&self, agent, no_cache, sink)`;
   `ready.rs` calls it through `engines.require_sandbox_runtime()` until
   F-40b folds it into the trait.
4. `squad/runtime_guard.rs` stays as documented; add `Capabilities::squad_supported`
   and have the guard read it so the tier decision has one home.

#### F-40b (decision Q12): `ReadyEngine` and `InitEngine` become runtime-agnostic
Report question Q12 (`src/engine/ready/mod.rs:738-745`, `src/engine/init/mod.rs:391-402`,
`src/engine/agent/mod.rs:78-81`). Crosses no layer but changes the L1
surface; do it after F-40 and before F-32.
1. Add to `AgentRuntimeEngine`: `ready_agent(&self, agent, opts: ReadyAgentOptions, sink) -> Result<(), EngineError>`
   (image build or kit apply, gated by `capabilities().kit_declarative`)
   and `image_exists(&self, tag) -> Result<bool, EngineError>` /
   `image_home_dir(&self, tag)` where the sandbox tier returns a typed
   `EngineError::UnsupportedOnRuntime`. `SandboxRuntime::ready_agent` from
   F-40 is the sandbox impl; `ContainerRuntime`'s existing build path is the
   container impl.
2. `ReadyEngine`, `InitEngine` and `AgentEngine` take
   `Arc<dyn AgentRuntimeEngine>` instead of `Arc<ContainerRuntime>`; delete
   `AgentEngine::container_runtime_arc()` and the second `runtime` parameter
   on `resolve_agent_options` (report F-44's note under F-33/F-32). `ready.rs`
   loses its `sandbox_runtime.is_some()` branch entirely: one ready flow,
   branching only inside the runtime.
3. `Engines` may keep the typed `Option<Arc<ContainerRuntime>>` /
   `Option<Arc<SandboxRuntime>>` handles for genuinely paradigm-specific
   operations (the grand architecture allows this), but after this step no
   command in `src/command/` needs them for `ready`, `init`, `chat` or
   `exec prompt`.
4. Tests: `tests/engine/sbx.rs` and `tests/runtime_integration.rs` pass;
   add a ready-flow test under a fake sandbox-tier runtime.

#### F-44: panic log path and append move to Layer 0
Report F-44 (`src/frontend/tui/event_loop.rs:38-70`).
1. `src/data/fs/panic_log.rs`: `PanicLog::from_env(&Env) -> Option<PanicLog>`,
   `path()`, `append(&self, report: &str)` (best-effort, never panics).
2. The TUI hook formats the report and calls `append`.

#### F-46: no agent/model defaults or permission policy in Layer 1
Report F-46 (`src/engine/squad/scheduler.rs:397-425`, `src/engine/acp/session.rs:66-72`).
1. `TaskEvaluator::evaluate` (or the run row) reports the resolved
   `(agent, model)` back to the scheduler; `running_agent_summaries` logs
   that fact and stops parsing `agent::model` or `agentsToModels`. Make it a
   method on `SquadScheduler` (report F-38's launcher note applies).
2. `AcpSession::new` takes `PermissionPolicy::{AutoApprove, Ask}`; the
   command computes it from yolo/auto.

#### F-47: on-disk contracts in Layer 0; no raw `std::fs` or `set_var` in Layer 2; banner rendered by the frontend
Report F-47 (`src/engine/squad/verdict.rs:43-109`, `src/command/commands/squad/gateway.rs:359-473`,
`squad/daemon.rs:467`, `squad/evaluation.rs:1002, 1022`, `api_server/banner.rs`).
1. Move `RunVerdict`, `VerdictError`, `VERDICT_FILE_NAME`, `RUN_DIR_CONTAINER_PATH`
   to `src/data/fs/squad_verdict.rs` with `RunVerdict::read_from(run_dir)`;
   re-export from `engine::squad` for one release.
2. Add `TaskStore::{ensure_workspace, write_config, remove_config}` and
   `SquadPaths::build_log(...)`/`open_daemon_log()` helpers; route the gateway,
   evaluator and log-follow writes through them. Replace
   `publish_key_to_process_env`'s `std::env::set_var` with passing the key
   through `SquadServeConfig`; if child processes genuinely need it in their
   env, set it on the `Command` builder in `DaemonProcess`, not on the
   parent process.
3. Emit the API key as a typed `ApiServerOutcome::KeyGenerated { key }`
   field (or `UserMessage` kind); move `render_api_key_banner` to
   `src/frontend/cli/per_command/api_server.rs`. **Behaviour change**: the
   TUI and API no longer receive box-drawing text unless their frontend
   renders it.

### Group B — small Tenet 2 pull-downs from the frontends into Layer 2 / Layer 1

#### F-14: `AuthEngine` verifies API bearer keys
Report F-14 (`src/frontend/api/serve.rs:43-87`, `src/engine/auth/mod.rs:413-431`).
1. Move `AuthMode` to `src/engine/auth/`; add
   `AuthEngine::request_auth_mode(&self, skip: bool) -> Result<AuthMode>` and
   `AuthEngine::verify_bearer(&self, mode: &AuthMode, header: Option<&str>) -> AuthOutcome`
   using `verify_api_key`'s sentinel comparison.
2. `check_bearer_auth` keeps header extraction and the 401 envelope only.
   The squad daemon builds an `AuthEngine` over `SquadPaths::daemon()`
   instead of calling `resolve_auth_mode` on raw paths.
3. Tests: `tests/api_parity/auth_modes.rs` passes; add a timing-shape test
   that a missing hash file still performs a comparison (mirror the existing
   engine test).

#### F-17: `SquadSupervisor::health()`
Report F-17 (`src/frontend/tui/squad_indicator.rs:53-78, 115-135`).
1. Move `classify` and the probe sequence into
   `SquadSupervisor::health(&self, timeout) -> SquadHealth` in
   `src/engine/squad/supervisor.rs` (L1, where 0113 Step 3 put the
   supervisor); move the `SquadHealth` enum there.
2. `SquadIndicatorPoller` keeps its 10 s loop and colour mapping only.
   `squad status` may reuse `health()` for its summary line.
3. Tests: `tests/squad_status_marker.rs` passes; move the `classify` unit
   tests with the function.

#### F-18: the engine says when a "simple advance" applies
Report F-18 (`src/frontend/tui/per_command/workflow_frontend.rs:23-125, 455-470`).
1. Add `simple_advance: Option<SimpleAdvance { completed_step, next_step }>`
   and `current_step_name: Option<String>` to `AvailableActions`, computed in
   `WorkflowEngine` where it already computes `can_launch_next`/`can_dismiss`.
2. The TUI picks the lightweight confirm when `simple_advance.is_some()`;
   `wcb_response_to_action` keeps the key map and drops the permission
   guards (the engine validates `NextAction` against `AvailableActions`).
3. Tests: `src/frontend/tui/tests/*` dialog-selection tests move to the
   engine as `AvailableActions` tests.

#### F-21: startup command from Layer 2
Report F-21 (`src/frontend/tui/mod.rs:135-162`, `src/frontend/tui/key_handler.rs:1008-1035`).
1. `Session::is_git_repo(&self) -> bool` in L0 (resolved at open time from
   the `GitRootResolver` outcome, not a `.git` probe).
2. `Dispatch::startup_command(&Session) -> ParsedCommandBoxInput` in L2
   returning `ready` or `status --watch`; both TUI sites call it.

#### F-42: shared exit-code policy
Report F-42. Already done by 0113 Step 4 (`CommandOutcome::exit_code` /
`is_partial_failure`). Nothing remains; mark closed when 0113 lands.

#### F-43: catalogue-owned "did you mean"
Report F-43 (`src/frontend/tui/command_box.rs:20-52`, `src/command/commands/config.rs:319`).
1. Move the Levenshtein helper to `src/data/text.rs` (pure) and use it from
   both `config.rs` and a new `CommandCatalogue::suggest(path: &[&str]) -> Vec<String>`
   that considers nested paths.
2. `CommandError::UnknownCommand { path, suggestions }`; `Dispatch::parse_command_box_input`
   fills it; the TUI and CLI only format. **Behaviour change**: suggestions
   may improve for nested paths.

#### F-15: no command-name or flag-name facts in `spawn_command`
Report F-15 (`src/frontend/tui/app.rs:26-38, 772-817, 845-853`). Already
done by 0113 Step 5: gateway injection keyed on `path.first() == "squad"`
is replaced by `GatewayNeed`. Decision Q6: the banner is deleted.
1. Delete the "INTERACTIVE MODE" banner (`app.rs:777-798`) and the
   `is_containerized` match that gates it. No frontend shows it. If a later
   need for a per-command "containerized" fact arises, it becomes a
   `CommandSpec` attribute, never a name match.
2. Add `Dispatch::yolo_effective(&ParsedCommandBoxInput) -> bool` (or expose
   it on `ResolvedFlags` from 0113 Step 9); delete the TUI's `yolo || auto`.
3. `BuiltCommand` exposes `agent_display_name()` resolved by the command;
   delete `agent_name_from_parsed` and its three tests (the default-agent
   test moves to `src/command/commands/mod.rs::resolve_agent` tests).

#### F-16: a typed stats sampler
Report F-16 (`src/frontend/tui/app.rs:1024-1094, 1180-1213`).
1. Add `AgentRuntimeEngine::stats_by_name(&self, name: &str) -> Result<AgentStats, EngineError>`
   (backends look the handle up; no fabricated `AgentHandle`).
2. Add an L2 `ContainerStatsSampler` (`src/command/stats_sampler.rs`) that
   owns cadence (3 s), the in-flight set and the "named container, else
   first running" fallback, and emits `StatsSample { key, step_name, stats }`
   over a channel. `App::tick_all_tabs` drains it. This retires the
   `type_complexity` tuple channels (also F-24).

#### F-19: prompts are data supplied by Layer 2
Report F-19 (TUI `per_command/{init,squad,exec_workflow,worktree_lifecycle,specs}.rs`,
CLI `command_frontend.rs:156-176, 300-310`, `per_command/{init,mount_scope}.rs`).
1. Generalise `PostWorkflowWorktreePrompt` into
   `pub struct Prompt<D> { title: String, body: Option<String>, choices: Vec<Choice<D>>, default_on_dismiss: Option<D> }`
   with `Choice<D> { key: char, label: String, value: D }` in
   `src/command/prompts.rs` (L2 owns the copy; L0 owns only the decision
   enums).
2. Change each `ask_*` in the L2/L1 traits to take `&Prompt<D>`:
   `ask_dockerfile_setup(&Prompt<DockerfileSetupDecision>)` (with the display
   path pre-filled — also closes F-31's frontend half),
   `ask_merge_mode(&Prompt<WorktreeMergeMode>)`, `ask_spec_kind(&Prompt<WorkItemKind>)`,
   `ask_workflow_resume_or_fresh(&Prompt<ResumeChoice>)`,
   `ask_task_interval(&Prompt<…> { default: from catalogue FlagDefault })`,
   `ask_mount_scope(&Prompt<MountScope>)`, `ask_run_audit`, `ask_work_items_setup`.
   Frontends map a keystroke or index to the choice's `value` and return
   `default_on_dismiss` on Esc/blank; they contain no label, hotkey or
   default literal afterwards. Decision: every `new` interview prompt has
   `default_on_dismiss: None`, so Esc aborts (`CommandError::Aborted`) and
   the TUI stops writing a file named `workflow`; release note.
3. Delete the four copies of `"6h"` in favour of the catalogue value.
4. Tests: `tests/tui_tabs.rs` and TUI dialog tests assert against the
   `Prompt` passed in, not literals; add a parity test that the CLI and TUI
   init prompts render the same choice labels.

#### F-20: typed config edits
Report F-20 (`src/frontend/tui/dialog_router.rs:70-260`, `src/frontend/tui/per_command/config.rs:51-64`).
1. Extend `ConfigFieldRow` with `kind: ConfigFieldKind::{Scalar, MapEntry { map }, ArrayEntry { array, index }, Secret}`
   and `add_entry_hint: Option<String>`; `ConfigCommand` computes next
   indices and masks secrets.
2. `DialogResponse::ConfigEdit(ConfigEditRequest)` replaces the tab-separated
   `Text`; delete `is_valid_map_key` (the command returns
   `ConfigEditRejection`).
3. Tests: `src/frontend/tui/tests/render_tests.rs` config cases pass; add a
   `ConfigCommand` test for a rejected map key.

#### F-23: `RuntimeContext` in Layer 2; summary box rendered in the frontend
Report F-23 (`src/frontend/tui/mod.rs:21`, `src/frontend/tui/per_command/ready.rs:60`,
`src/data/step_status.rs:34-99`, `src/command/commands/remote.rs:720-731`).
1. Move `RuntimeContext` to `src/command/dispatch/runtime_context.rs`
   (0113 Step 2's `StartupOutcome` may already be this type; if so, delete
   the CLI one and use it).
2. Move `render_summary_box` and `StepStatus::glyph` to
   `src/frontend/render_helpers.rs` shared by CLI and TUI; `ReadyEngine`
   produces the credential rows in `ReadySummary` so
   `tui/per_command/ready.rs:72-90`'s classification goes away;
   `remote.rs` returns the `ReadySummary` in its outcome and the frontend
   renders it. After this step `src/frontend/tui` imports nothing from
   `src/frontend/cli`.

#### F-24: `TabSharedState` and `DialogChannels`
Report F-24 (`src/frontend/tui/command_frontend.rs:64, 108-130`, `src/frontend/tui/app.rs:135-149, 713-733`).
1. `#[derive(Clone)] pub struct TabSharedState { … the 15 Arc slots … }` as a
   field of `Tab`, `Tab::shared(&self) -> TabSharedState`;
   `pub struct DialogChannels { tx, rx }`;
   `TuiCommandFrontend::new(parsed, dialogs, io, shared)`.
2. `TabSharedState::for_tests()` replaces the five 50-line test constructors.
3. Use the existing `SharedResizeTx` alias at `command_frontend.rs:64`;
   remove all three `type_complexity` allows (F-16 removes the stats tuple).

#### F-31: config comes from `Session`, not from ad-hoc loads
Report F-31. The frontend half (dockerfile prompt) closes with F-19.
1. `api_server.rs:191-196` → `session.effective_config().api_work_dirs()`.
2. `InitEngine` holds its `RepoConfig` and updates it in place after each
   save instead of five reloads.
3. `squad/commands.rs:538, 597`, `squad/gateway.rs:305` → the owned
   `Session`'s config. **Behaviour change**: parse errors in
   `~/.awman/config.json` now surface where they were silently defaulted;
   add a test and a release note.
4. Add a lint line: `GlobalConfig::load()` / `RepoConfig::load(` outside
   `src/data/` and `src/command/startup.rs` fails.

#### F-49: `CallerContext` at construction; every command takes a `Session`
Report F-49 (`src/command/commands/status.rs:179-213`, `squad/commands.rs:308-320, 752`,
`api_server.rs:132`). Builds on 0113 Step 9's `BuildContext`.
1. `CallerContext { frontend: FrontendKind, local_user: bool, tui_tab: Option<TuiContext> }`
   populated by `Dispatch`; delete `StatusCommandFrontend::tui_context()` and
   `SquadCommandFrontend::is_local_user_session()`.
2. `StatusCommand`, `SquadCommand`, `ApiServerCommand` take the `Session`.

#### F-50: one non-interactive source of truth; catalogue-driven API profile and squad visibility
Report F-50 (`src/frontend/mod.rs:25-31`, `src/frontend/cli/command_frontend.rs:357-373`,
`src/frontend/api/routes.rs:952-955`, `src/frontend/squad/routes.rs:282-289`).
1. `CommandFrontend::input_available(&self) -> bool` (TTY fact only);
   `ResolvedFlags` (0113 Step 9) computes `non_interactive = flag || !input_available`.
   Delete `effective_non_interactive`, the cached CLI field and every direct
   `stdin_is_tty()` call inside an `ask_*`. **Behaviour change**:
   `--non-interactive` on a TTY now suppresses every prompt; release note.
2. `CommandCatalogue::frontend_profile(FrontendKind) -> &FlagDefaults`; the
   API serialises it as `flags_applied`.
3. Add `FrontendKind::SquadDaemon` to `FrontendVisibility`; the daemon
   router calls `validate_for_frontend` and drops its subtree literal.

### Group C — headless parity and session lifecycle

#### F-13: `HeadlessDefaults`
Report F-13. Decision Q5: the differences are deliberate; two named profiles.
1. `src/command/headless.rs`: `HeadlessDefaults` implementing every `ask_*`
   the API, squad and CLI non-TTY paths answer today, with
   `HeadlessDefaults::api()` and `HeadlessDefaults::squad()` constructors
   that reproduce the current answer tables exactly (agent setup, merge
   mode, pre-worktree decision, resume, post-workflow action, auth).
2. `ApiDispatchFrontend`, `UnattendedFrontend` and the CLI's non-interactive
   branches delegate to it; each frontend's `ask_*` bodies become one-liners.
3. Write the table of answers into the module doc; it is the single place
   the two policies are compared.
4. Tests: a table-driven test that each profile returns the recorded
   answers; `tests/squad_daemon_e2e.rs` and `tests/api_parity/*` pass.

#### F-08 follow-through, F-41: `SessionSetup` persists through Layer 0, not through the frontend
Report F-41 (`src/command/session_setup.rs:68-83`, `src/frontend/api/session_setup.rs`).
1. Split `SessionSetupObserver` into `SessionSetupPresenter` (`log`,
   `set_stage`, `stage_changed`, `ready_frontend`, `git_log_sink`) and give
   `SessionSetup::run` an `Arc<SessionManager>` plus the setup-status store
   for `persist_status`, `register_session`, `persist_and_cleanup`.
2. Move `SessionSetupState` mutation logic (`apply_ready_phase`,
   `set_ready`, `mark_failed`, `ready_phase_display`) onto the L0 type in
   `src/data/session_setup_event.rs`; the API bus becomes a pure broadcaster.

#### F-22: `Tab` as a view of `SessionState`
Report F-22. Decision Q3: wire `SessionState`. Depends on 0113 Step 8 and Group C above.
1. Commands update `SessionState.current_command` / `current_workflow` /
   `current_container` through the `Session` they own (0113 Step 9's
   `BuildContext` carries it); `WorkflowEngine::persist` mirrors the live
   `WorkflowState` summary into `current_workflow`.
2. Split `Tab` into `TabView { rects, scroll, parsers, dialog channels, slots, shared: TabSharedState }`
   plus `session_id`; derive `ExecutionPhase`, the workflow overview,
   `stuck`, `yolo_mode` and the container name from `SessionState` on each
   tick. Delete `Tab::workflow_agent_fallbacks` and `Tab::is_remote` if the
   grep the report mentions confirms they have no writer.
3. Replace `WorkflowStepView.status: String` with the typed step status;
   delete the string matches in `workflow_view.rs:254-323, 392-406, 560-578`.
4. `poll_command_completion` uses `CommandOutcome::exit_code()` instead of
   matching variants.
5. Tests: `src/frontend/tui/tabs/tests.rs` and `tests/tui_tabs.rs` are the
   regression suite; expect large but mechanical churn.

#### F-30: one workflow-state store, one agent-precedence rule
Report F-30. Decision Q3: wire `SessionState`.
1. Keep `SessionState` and its three fields (F-22 populates them) but
   delete the parallel `session::StepStatus` / `WorkflowStepRecord` /
   `WorkflowInvocation` types in favour of a summary derived from the live
   `WorkflowState` (`current_workflow: Option<WorkflowSummary>` built from
   `data::workflow_state`).
2. Delete `src/data/fs/workflow_state.rs::WorkflowStateStore`; move
   `sanitize_name_for_filename` / `sha256_hex` into `workflow_state_store.rs`
   (or `src/data/fs/hash.rs`, since `image_tags.rs` and `attach_socket.rs`
   use them too); rename the `EngineWorkflowStateStore` re-export to
   `WorkflowStateStore`.
3. `Session::open_at_git_root` derives `default_agent` from
   `EffectiveConfig::agent()`; delete `resolve_default_agent`.

### Group D — large layer relocations (gate: Q1)

#### F-27: overlay grammar and source merge move to Layer 0; `LaunchPolicy` in Layer 2
Report F-27 (`src/command/commands/mod.rs:52-764`).
1. `src/data/config/overlays.rs`: `TypedOverlay`, `SkillSpec`,
   `ContextOverlaySpec`, `parse_overlay_spec`, `parse_overlay_list`,
   `CollectedOverlays`, and `EffectiveConfig::collected_overlays(cli, workflow, step)`
   replacing `collect_all_overlay_specs`. Move the ~1,000 lines of tests
   with them. `RepoConfig.overlays` may then be validated at load time (add
   a `DataError::InvalidOverlaySpec`).
2. `src/command/commands/launch_policy.rs`: `LaunchPolicy` struct with
   `resolve_agent`, `resolve_launch_mode`, `acp_fallback_warning`,
   `resolve_context_overlays`, `report_session_end`, `warn_legacy_config` as
   methods. `commands/mod.rs` becomes `mod` declarations and re-exports only.
3. Crosses layers: run `make architecture-lint` alone.

#### F-28: `HttpCore` and the SSE client move to Layer 1
Report F-28 (`src/command/commands/http_core.rs`, `src/command/commands/remote_client.rs:265-457`).
1. `src/engine/remote/{http_core.rs, client.rs, events.rs}`; errors become
   `EngineError::Remote*` variants with `From` into `CommandError` (already
   exists for `EngineError`). `StartSessionRequest`, `ExecArg` and the
   route knowledge stay as the engine's typed API; `RemoteCommand` and the
   Step-7 poller (0113) stay in L2 and import from the engine.
2. `engine/agent/download.rs` and `poll_ci.rs` (F-37) switch to the shared
   client so the tree has one `reqwest::Client` builder again.

#### F-29: git and network out of Layer 0 path resolvers; process supervision to Layer 1
Report F-29 (`src/data/fs/context_dirs.rs:91`, `src/data/network/aspec_tarball.rs`,
`src/data/fs/daemon_process.rs:256-746`, `src/data/fs/daemon_guard.rs`).
1. `ContextDirResolver::repo_dir(&self, remote_url: Option<&str>, git_root: &Path)`;
   `WorkflowEngine` (its only caller) passes `GitEngine::remote_url`. Make
   `parse_owner_repo` / `normalise_slug` the single parser used by
   `skill_library.rs` and the issue engine.
2. `src/engine/aspec/mod.rs`: `AspecDownloader::new(url).download().await`
   and `extract(bytes, dest)`; `ASPEC_TARBALL_URL` stays an L0 constant.
   `InitEngine` and `init --aspec` use it.
3. `src/engine/daemon/mod.rs`: `DaemonSupervisor` owning `DaemonPaths` with
   `spawn_background`, `try_systemd_run`, `try_launchd`, `double_fork_spawn`,
   `terminate`, `status()` (replacing the free `is_process_alive` /
   `pid_is_awman`), and `DaemonGuard`. `DaemonPaths`, `ServerMeta`,
   `read_pid/write_pid/clear_pid/read_meta/write_meta` stay in L0.
   `api_server.rs:274-275` and `SquadSupervisor` call the supervisor.
   Declare `PATH`/`HOME`/`RUST_LOG` for `forwarded_env()` in `env.rs`.
4. Tests: `tests/data_layer/daemon_primitives.rs` splits into an L0 half
   (paths, PID files) and an engine half (spawn/kill, already skipped
   without a real daemon).

### Group E — engine consolidation

#### F-32: one table per backend and per agent
Report F-32 (`src/engine/container/runtime.rs:82-320`, `src/engine/agent/mod.rs:360-666`,
`src/engine/overlay/mod.rs:409-682`, `src/engine/auth/keychain.rs:46-70`,
`src/engine/ready/mod.rs:98-110`).
1. `ContainerRuntime` delegates to `backend.cli_binary()`; add
   `ContainerBackend::display_name()` and `availability_probe_args()`;
   delete the five `"apple-containers" =>` arms.
2. Extend `AgentMatrix` with `settings_mount: Option<&'static str>`,
   `skills_mount: Option<&'static str>`, `credential_source: CredentialSource`,
   `ping_argv: &'static [&'static str]`, `static_env: &'static [(&'static str, &'static str)]`,
   `sandbox_permission_mode_supported: bool`. Collapse the generic arm in
   `agent_settings_overlays_with_credentials` to one loop (keep `claude` and
   `antigravity` as explicit strategies in `overlay/claude.rs` /
   `overlay/antigravity.rs`), the skills-path match, the keychain matches,
   the ready ping match, and the four `if agent.as_str() == "copilot"|"claude"`
   checks. After this step `grep -rn 'agent.as_str() ==' src/engine` returns
   only `agent_matrix.rs`.
3. Add `AgentMatrix::validate_run(agent, run)` and `mode_flags(run)` and call
   them from both `build_options_with_credentials` and `build_sandbox_options`
   (report F-51's agent/mod.rs item).
4. Tests: `src/engine/agent/agent_matrix.rs` table tests extend to the new
   fields; `tests/engine/overlay_engine.rs` passes unchanged.

#### F-38: `HostAgentPinger`; no process-global monitor
Report F-38 (`src/engine/ready/mod.rs:123-245`, `src/engine/credential_refresh/monitor.rs:342, 612-625`,
`docker.rs:607, 668, 782`, `apple.rs:586, 696, 787`, `src/engine/squad/launcher.rs:47-200`).
1. `pub struct HostAgentPinger` in `src/engine/ready/host_agent.rs` with
   `ping(&self, agent) -> LocalAgentPingResult` and
   `refresh_credential(&self, spec, binding) -> HostRefreshOutcome`; it is the
   only type that may spawn an agent binary on the host, and the S1
   security note in `aspec/architecture/security.md` is updated to name it.
   `ReadyEngine` and `CredentialRefreshMonitor` hold one; delete the free
   functions.
2. Carry the monitor (or a `CredentialLeaseFactory`) in
   `ResolvedContainerOptions`; delete `install_global`/`global` and the
   `OnceLock`; `dispatch/mod.rs:83` passes it explicitly.
3. Move `drive_unattended_agent`, `prepare_run_log_dir`, `run_log_dir`,
   `ensure_directory_workspace_project` onto `SquadAgentLauncher` (or a
   `SquadRunPaths` value type in L0 for the path halves).
4. Tests: `tests/engine/credential_refresh_integration.rs` passes; add a
   test that constructing `ResolvedContainerOptions` without a monitor
   disables leases (today's `global().is_none()` path).

#### F-45: typed engine-to-frontend events
Report F-45 (`src/engine/workflow/frontend.rs:104-121, 181-190`, `src/engine/ready/frontend.rs:23`,
`src/engine/init/frontend.rs:33`, `src/engine/git/mod.rs:18-48`, `src/engine/ready/mod.rs:619-627`,
`src/engine/workflow/mod.rs:2629-2639`).
1. Replace the four `set_*` setters with one
   `attach_engine(&mut self, handles: EngineHandles { requests, stuck, io })`.
2. `ReadyStep`, `InitStep`, `AgentSetupStep` enums in L0 with `Display`;
   `report_step_status(step: ReadyStep, …)` replaces the `&str` keys.
3. Add `GitFrontend::command_started(&GitCommand)`,
   `ReadyFrontend::report_ping(&LocalAgentPingResult)`,
   `WorkflowFrontend::report_ci_poll(&CiPollEvent)` with default impls that
   write today's text through `write_message`, so no frontend changes output
   unless it opts in.

### Group F — file splits, parsers, typed statuses, fixtures, docs

#### F-26: one token parser
Report F-26 (`src/command/dispatch/parsed_input.rs:35-243`, `projections/raw_args.rs:265-500`).
1. `parsed_input::parse` becomes `shell_words::split` plus subcommand-path
   resolution, then `raw_args::parse_against_spec`, then a conversion to
   `ParsedCommandBoxInput` (or the TUI consumes `ParsedArgs` directly).
   Delete the duplicate loop. **Behaviour change**: the TUI now rejects bad
   enum values and non-numeric numbers at parse time with the same error
   the API gives; release note.
2. Tests: `src/frontend/tui/tests/key_handler_tests.rs` command-box cases
   pass; add cases for `--launch-mode banana` and `--port abc` in the TUI.

#### F-25: the command reference is user documentation generated from the catalogue; `cli.md` becomes UX standards
Report F-25. Decisions Q8 and Q9: the code is right, `--format md` is gone,
and `aspec/uxui/cli.md` should not document specific commands and flags at
all. It is the source of truth for generalised UI/UX standards and best
practices; per-command reference belongs in `docs/`.
1. `CommandCatalogue::markdown_reference() -> String` projection in
   `src/command/dispatch/projections/markdown.rs` rendering every command,
   subcommand, alias, flag (with kind, default, implies, conflicts, frontend
   visibility) and argument.
2. Generate `docs/14-command-reference.md` from it (a `make docs-reference`
   target and a test that fails when the committed file is stale, so the
   reference can never drift from the catalogue again). Link it from
   `docs/contents.md`; remove per-flag tables from other `docs/` pages that
   would now duplicate it, leaving prose and examples.
3. Rewrite `aspec/uxui/cli.md` as the UX standards document: flag naming and
   casing rules, positional vs flag conventions, `--json` implies
   `--non-interactive`, exit-code classes, prompt and dialog conventions
   (the `Prompt<D>` shape from F-19), hint and help-text style, parity
   rules across CLI/TUI/API, and the `api_allowed` PTY exclusion (Q7).
   Delete every per-command section. Ask the developer to review the new
   outline before writing the body.
4. Add `requires_runtime: bool` to `CommandSpec`; delete the
   `!matches!(path.first(), Some("config"))` literal.

#### F-48: typed session kind and step status on the wire
Report F-48. Decision Q10: the wire schema is not frozen; names may change.
1. `SessionKind { Local, Remote }` (serde, `FromStr`) in `src/data/session.rs`;
   the catalogue `type` flag's enum values come from it; `insert_session_full`
   takes a `NewSessionRow { id, workdir, created_at, setup_status: SetupStatus, kind: SessionKind, cloned_path }`.
2. `StepStatusKind` (serde `snake_case`) in `src/data/execution_event.rs`;
   `WorkflowStepTransition { from, to: StepStatusKind }`; `CommandStatus`
   becomes an enum too. Delete the hand maps in
   `api/command_frontend.rs:651-658` and the string matches in
   `queue_worker.rs`. Prefer today's names where they already read well;
   where a name changes, list it in the release notes under an "API
   changes" heading and update `docs/09-api-and-remote-mode.md`.
3. Tests: `tests/api_parity/wi_0079.rs` is updated to the typed schema;
   keep one raw-JSON assertion per enum so the serialised form is explicit.

#### F-51: file and function splits
Report F-51. Decision Q13: the shared helpers move to `workflow_preflight.rs`. All behaviour-preserving;
each bullet is its own commit.
1. `src/engine/workflow/mod.rs` → `workflow/{mod, single_step, parallel, control, phases, queries}.rs`;
   tests to `workflow/tests/{single, parallel, phases}.rs`.
2. `src/command/commands/exec_workflow.rs` → `exec_workflow/{mod, factory, prepare, execute, dynamic, image, issue}.rs`
   plus `commands/workflow_preflight.rs` for the eight helpers
   `squad/evaluation.rs:21-26` imports; `LeaderSpec` to `src/data/config/`
   (it is also constructed by `repo.rs`). Collapse the four worktree-lifecycle
   `match` copies at `:1085-1163` into `WorktreeName` + one call.
3. `src/engine/overlay/mod.rs` → `overlay/{mod, agent_settings, claude, skills, credential_file}.rs`.
4. `src/command/dispatch/catalogue.rs` → `catalogue/{mod, core, exec, api, squad, remote, new, shared_flags}.rs`;
   derive `EXEC_PROMPT_FLAGS`/`EXEC_WORKFLOW_FLAGS` from `AGENT_RUN_FLAGS_NO_WORKTREE`
   and `SQUAD_EDIT.flags` via `const fn all_optional` using the existing
   remote-exec const-fn technique.
5. `src/frontend/tui/render/dialog.rs::render_dialog` → per-variant
   `render_*` in `render/dialogs/{input, workflow, squad, misc}.rs`.
6. `src/frontend/tui/key_handler.rs::handle_key_event` → `focus_context`,
   `intercept_dialog_keys`, `handle_{tab,scroll,edit,squad,dialog}_action`.
7. `src/frontend/tui/app.rs::tick_all_tabs` → one function per labelled
   section; `spawn_command` likewise.
8. `clean.rs::discover` → `discover_{containers_and_images, repo_workflow_state, context_dirs}`;
   `new.rs::run_with_frontend` → `commands/new/{spec, workflow, skill}.rs`.
9. `config.rs`: one `ConfigFieldSpec { name, scope, kind, read_only, sensitive, hint, validate }`
   table in `src/data/config/fields.rs` replacing the seven string-keyed
   lookups; JSON path helpers to `config_json.rs`.
10. `QueueWorker::new` (now L2) takes `Arc<ApiRuntime>`; `apply_flag` in
    `raw_args.rs` becomes a method on a `RawArgCursor`.

#### F-52: shared test fixtures
Report F-52. Already done by 0113 Step 2 (`Engines::for_tests`) and F-24
(`TabSharedState::for_tests`).
1. `tests/helpers/mod.rs::TestEnv::engines()` and a single `make_session`
   helper in `src/data/session.rs` under `#[cfg(test)]` (or
   `tests/helpers`) replacing the 23 copies.
2. The seven `#[ignore]` `todo!()` stubs at `exec_workflow.rs:5846-5894`:
   implement them as real Docker-gated tests or delete them; a stub that can
   never run is not a test.

#### F-53: stale documentation pointers
Report F-53. Already done by 0113 Step 13 (dead link). Fix
`src/data/message.rs:25-27` ("Defined by Layer 1" → Layer 0) and any
remaining doc drift found while executing the groups above.

#### F-54
Report F-54. Already done by 0113 Step 8. Mark closed when 0113 lands.

### Close-out

1. Re-run the report's Phase 2 metrics and append a `## Remediation — WI 0114`
   section to `aspec/review-notes/0113-architecture-audit.md` with the
   before/after table and the status of each of F-13–F-54 (closed, deferred
   with reason, rejected with the developer's decision).
2. Update `aspec/architecture/security.md` for `HostAgentPinger` (F-38) and,
   per report Q11, the `docker.sock` mount under `allow_docker`.
3. Update `aspec/architecture/four-layer-summary.md` if `Prompt<D>`,
   `HeadlessDefaults`, `CallerContext` or `LaunchPolicy` change the
   documented patterns.

## Edge Case Considerations:
- **F-34**: unifying setup and teardown changes setup's remediation to keep
  stdout/stderr and write a failure file; a workflow whose setup step fails
  will now produce a log file it did not before. Ensure the file lands under
  the same run directory as teardown's and that the API event stream is
  unchanged (`phase` string preserved).
- **F-13**: profiles must reproduce today's answers exactly, including the
  API's `ask_agent_setup` dependence on `default_available`; a table-driven
  test is the contract.
- **F-19**: `default_on_dismiss` is `None` for every interview prompt
  (decision: abort on Esc everywhere) and `Some` only where a prompt has a
  documented default today (mount scope, dockerfile setup); list each
  `Some` in the `Prompt` constructor's doc comment.
- **F-26**: the TUI's `-ab` short-flag cluster is rejected today with
  `CommandBoxParse`; after delegation it becomes `UnknownFlag`. Confirm the
  hint text still reads sensibly in the command box.
- **F-29**: `DaemonSupervisor` on Windows uses `tasklist`/`taskkill`; the
  move must keep every `cfg(target_os)` block intact and compile on all three
  targets (CI covers macOS and Linux; build-check Windows locally or in CI).
- **F-31**: surfacing config parse errors may break users with a malformed
  `~/.awman/config.json` that today is silently ignored; the error message
  must name the file and the offending key.
- **F-38**: removing the global monitor changes the "no monitor installed →
  leases disabled" behaviour to an explicit option; tests that relied on the
  global being absent must construct options without a monitor.
- **F-48**: the wire schema is not frozen, but every renamed value is an
  API change for `docs/09-api-and-remote-mode.md` and the release notes;
  do not rename silently.
- **F-51**: splitting `workflow/mod.rs` must not change visibility of
  anything `exec_workflow.rs` or `squad/evaluation.rs` imports; use
  `pub(super)`/`pub(crate)` re-exports from `mod.rs`.

## Test Considerations:
- Every group ends with `make pre-push`; groups A, D and E also run
  `make test-full` with Docker at least once.
- New regression guards required by this work item: the env-var lint line
  (F-37), the config-load lint line (F-31), the headless-profile table test
  (F-13), the CLI/TUI prompt-parity test (F-19), the wire-string test
  (F-48), the `AgentMatrix` table tests (F-32), and the `markdown_reference`
  vs `cli.md` test (F-25).
- Tests move with the code they test: parser tests to the engine (F-06 in
  0113), `classify` tests to L2 (F-17), dialog-selection tests to
  `AvailableActions` (F-18), slot-driver tests to L2 (0113 F-01).
- Frontend unit tests may assert rendering and key mapping only. Any
  frontend test that asserts a default, a decision or a business outcome
  after this work item is a finding in the next audit.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- The governing document is `aspec/architecture/2026-grand-architecture.md`;
  its Tenet 3 shapes every new type here (`HeadlessDefaults`, `Prompt<D>`,
  `LaunchPolicy`, `CiPoller`, `HostAgentPinger`, `DaemonSupervisor`,
  `AspecDownloader`, `ContainerStatsSampler`, `ConfigFieldSpec`) as a struct
  with a constructor and methods.
- Layer placement rules applied here, for the record: file formats with an
  external writer live in L0 (F-47); process spawning, HTTP and git live in
  L1 (F-28, F-29, F-37); prompt copy and headless policy live in L2 (F-13,
  F-19); frontends render and map keys only.
- Findings partially or fully absorbed by WI 0113: F-15 (gateway keying),
  F-34 (dead fields), F-37 (`GITHUB_TOKEN`), F-42, F-52 (`Engines::for_tests`),
  F-53 (dead link), F-54. Confirm each is done before marking it here.

## Documentation

After implementation is complete, update user-facing documentation in `docs/` to reflect the current state of the tool:

- **Update existing feature docs**: `docs/05-workflows.md` for the setup-step
  failure log (F-34) and the repo-scoped git identity check (F-39);
  `docs/07-configuration.md` for config parse errors now surfacing (F-31)
  and the env vars newly declared (F-37); `docs/02-using-the-tui.md` for
  command-box validation of enum and numeric flags (F-26), the `--non-interactive`
  behaviour on a TTY (F-50) and any changed prompt labels (F-19);
  `docs/09-api-and-remote-mode.md` if `flags_applied` changes shape (F-50);
  `docs/12-squad.md` for the daemon health states if their wording changes
  (F-17) and the API key banner now being CLI-only (F-47).
- **Update `docs/architecture.md`**; create `docs/14-command-reference.md`
  generated from the catalogue and link it from `docs/contents.md` (F-25).
  `aspec/uxui/cli.md` becomes UX standards only and no longer lists
  commands or flags.
- **Update `aspec/architecture/security.md`** for the `docker.sock` mount
  under `--allow-docker` (decision Q11) and for `HostAgentPinger` (F-38).
- **Create new user guides only if a new user-visible feature warrants it** — none is expected.
- **Never create work-item-specific docs**.
- **Keep all technical/implementation details in work item specs or code comments**, not in `docs/`.
- **Docs are for end users**, not for developers trying to understand implementation.

See `CLAUDE.md` for more guidance on documentation standards.
