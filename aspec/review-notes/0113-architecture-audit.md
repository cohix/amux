# Architecture Audit — 2026-09-03

Scope: working tree at `415f780d` plus uncommitted edits. Prior audit: `aspec/review-notes/0073-architecture-audit.md` (2026-05-08, 168 files). Tree today: 280 files, 137,408 lines.

## Summary

The mechanical gates are green: `make architecture-lint` reports zero upward imports, clippy is clean, and there are no build warnings. Underneath that, the tree has grown 4x since the May audit and the spirit of the architecture has eroded in three places: the squad and API daemons are bootstrapped inside `src/frontend/`, the TUI has grown its own git engine, session policy, remote poller and `squad attach` implementation, and the catalogue is no longer the single source of truth for flag defaults or for what commands exist. A crate-wide `#![allow(dead_code)]` whose justification references the deleted `oldsrc/` hides 29 warnings, including an entire never-called sandbox backend surface.

Findings: 1 Critical, 11 High, 28 Medium, 14 Low (54 total). The two largest recommendations are (a) an L2 `Engines::build()` plus L2 daemon-bootstrap types so `main.rs`, the API server and the squad daemon stop wiring engines three separate ways, and (b) making `Dispatch` honour catalogue `FlagDefault`/`implies` generically and registering per-command constructors so the 700-line `build_command` match disappears.

| Area | Status | Findings |
|---|---|---|
| Layering (T1) | PASS | 0 (lint clean; no `use crate as`) |
| Frontend business logic (T2) | FAIL | 19 |
| Typed objects (T3) | WEAK | 7 |
| Catalogue and parity (L2, P2) | FAIL | 6 |
| Layer 0 / Layer 1 placement (L0, L1) | WEAK | 9 |
| Binary layer (L4) | WEAK | 1 |
| Security (S1, S2) | PASS | 0 (one question) |
| Spirit / consistency | WEAK | 6 |
| Simplification | — | 6 |

Baseline metrics are in `0113-architecture-audit-metrics.md`; the 29 hidden warnings are listed in `0113-architecture-audit-hidden-warnings.txt`. This report is the "what"; the fixes are specified in work items `aspec/work-items/0113-architecture-audit-critical-and-high.md` and `aspec/work-items/0114-architecture-audit-medium-and-low.md`.

---

## Findings

### F-01: `squad attach` is implemented in the frontends, twice, with no Layer 2 command
- **Rule**: T2, L2, P2
- **Severity**: Critical
- **Kind**: Violation
- **Where**: `src/frontend/tui/squad_attach.rs:341-522`, `src/frontend/tui/app.rs:631-642`, `src/frontend/cli/mod.rs:74-78`, `src/frontend/cli/per_command/squad_attach.rs`, `src/frontend/attach.rs:80-120`; catalogue entry `src/command/dispatch/catalogue.rs:1465`
- **What**: `app.rs:632` says "`squad attach` has no Layer-2 command; intercept it before the ordinary Dispatch spawn, mirroring `cli::run`'s carve-out". The TUI resolves the supervisor and gateway, probes workflow state, lists runtime containers by name prefix, diffs step transitions into attach/exit actions and calls `AgentRuntimeEngine::attach`. The CLI has a second implementation; a shared L3 helper (`frontend/attach.rs`) holds the candidate-resolution rules and error text; the API has nothing.
- **Why it matters**: The catalogue lists a command Dispatch cannot build; two frontends own the flow and the third lacks it.
- **Recommended action**: Create `SquadAttachCommand` in `src/command/commands/squad/attach.rs` owning supervisor/gateway resolution, `list_task_containers`, `resolve_attach_target`, phase detection and the slot-reconcile driver; expose `BuiltCommand::SquadAttach` from `Dispatch::build_command` returning an `AgentInstance` (same carve-out shape as `exec workflow`) and a `SquadAttachFrontend` trait. Delete `src/frontend/attach.rs` and both carve-outs.
- **Size**: L
- **Behavior change**: none intended; API gains the command

### F-02: The squad daemon is bootstrapped inside `src/frontend/squad/`
- **Rule**: T2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/squad/mod.rs:32-150` (`serve`, `serve_with`), `:156-187` (`build_engines`); `src/frontend/squad/unattended.rs:232-266, 336-383, 579-737`
- **What**: `serve_with` performs runtime admission, legacy DB relocation, store open and migrate, orphan-run reconciliation, stray-container scan, `SquadScheduler`/`LocalTaskGateway` construction and `ServerMeta` persistence; its doc comment calls the order "load-bearing". `build_engines` is a third copy of engine wiring. `unattended.rs` encodes the daemon's run policy as constant trait answers and opens per-step log files with `std::fs`.
- **Why it matters**: `SquadDaemonCommand::run_start` (L2) hands the whole runtime to a frontend trait; a fourth host would reimplement all of it.
- **Recommended action**: Add `SquadDaemonRuntime::bootstrap(config, engines)` in `src/command/commands/squad/daemon.rs` owning every step up to the router; change `SquadCommandFrontend::serve_squad_daemon` to receive the bootstrapped runtime and only build the router and bind. Move the unattended answer table into an L2 `HeadlessDefaults` (see F-13) and the run-log file layout into `src/data/fs/squad_paths.rs`.
- **Size**: M
- **Behavior change**: none

### F-03: API server bootstrap, queue worker and session close flow live in the API frontend
- **Rule**: T2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/api/mod.rs:33-52, 66-110, 112-206, 244-261`; `src/frontend/api/queue_worker.rs:45-72, 74-320, 329-416`; `src/frontend/api/routes.rs:534-671, 732-786`
- **What**: `api::serve` rebuilds `Engines`, restores sessions from SQLite (reconstructing `SessionType::Remote` with empty placeholders at `:145-152`), marks in-progress setups failed and deletes their clones, purges closed sessions, recovers stale commands and sizes the worker pool. `QueueWorker` owns the claim loop, `derive_command_status` (maps `CommandOutcome` variants to "done"/"error"), panic conversion and the session drain/close state machine. `handle_close_session` re-implements that drain flow a second time; `resolve_setup_status` decides "assume ready" on unknown.
- **Why it matters**: These are lifecycle decisions of API mode that any HTTP host must make identically; two copies of the close rule exist inside one frontend already.
- **Recommended action**: `ApiServerRuntime::bootstrap(config, engines)` in `src/command/commands/api_server/` owning store open/migrate, session restore, restart-failure marking, stale recovery and worker spawning; move `QueueWorker` there behind a frontend-factory trait; add `CommandOutcome::exit_code()` in `src/command/dispatch/mod.rs` shared by CLI and API; add an L2 `ApiSessionLifecycle::close(id) -> CloseOutcome` and `setup_readiness(id)` that routes map to status codes only.
- **Size**: L
- **Behavior change**: none

### F-04: Squad command routing and daemon supervision are hard-coded in `cli::run` and duplicated in the TUI `App`
- **Rule**: T2, P2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/cli/mod.rs:100-116, 122-153, 522-536`; `src/frontend/tui/app.rs:96-107, 296-627`
- **What**: `cli::run` holds two `matches!` lists of squad subcommand names (duplicating the catalogue), runs the runtime-tier guard, drives `SquadSupervisor` (ensure_running, key_state, `with_squad_gateway`) and authors the missing-key text. `App` repeats the same sequence for the squad tab, classifies errors by string prefix (`:99-102`), and `squad_synthetic_session` (`:611-627`) creates a directory and a `Session` by hand.
- **Why it matters**: Business flow leaking upward into two frontends is exactly the drift the spec was written to stop.
- **Recommended action**: Add a `gateway_need: GatewayNeed::{None, Running, IfRunning}` attribute to `CommandSpec`; have `Dispatch::run_command` acquire the gateway via an L2 `SquadSupervisor::gateway_for(need)`; surface `SquadKeyState::Minted{setup}` through a `SquadCommandFrontend::show_key_setup` method and `Missing` as a typed `CommandError::SquadKeyMissing`. Add `SquadSupervisor::open_for_frontend() -> Result<SquadStartup, SquadStartError>` with a typed error enum for the TUI. Move `squad_synthetic_session` to `Session::open_squad_root(&Env)` in L0.
- **Size**: M
- **Behavior change**: none

### F-05: Engine wiring is done three times, and `main.rs` does far more than clap plus frontend selection
- **Rule**: L4, L2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/main.rs:44-152`; `src/frontend/api/mod.rs:66-110`; `src/frontend/squad/mod.rs:156-187`
- **What**: `main.rs` runs legacy-path migration, loads global config, resolves the git root, opens the session and constructs `GitEngine`, `OverlayEngine`, `AuthEngine`, `AgentEngine` and the workflow state store into `Engines`. The API server and squad daemon each repeat the construction with small differences (auth resolver source, state-store root).
- **Why it matters**: The spec says Layer 4 "builds clap, picks a frontend, delegates. That's it." Three copies of the engine graph guarantee drift (the API copy already comments that its state store path is "temporary").
- **Recommended action**: `Engines::build(&GlobalConfig, &Session)`/`Engines::for_daemon(paths)` in `src/command/dispatch/mod.rs`; a `Startup` type in L2 (or L0 for migration) that performs migration and session open. `main.rs` becomes clap, `Startup::run`, frontend pick. This unblocks F-02 and F-03.
- **Size**: M
- **Behavior change**: none

### F-06: The TUI git sidebar is its own git engine
- **Rule**: T2, L1
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/tui/git_sidebar.rs:96-234, 273-374` (`run_git` at `:336`, file reads at `:366-370`)
- **What**: The sidebar shells out to `git status --porcelain`, `git branch --show-current`, `git diff --numstat HEAD` and `git rev-parse`, reads untracked files to count lines, and owns the porcelain/numstat parsers plus the "no commits yet" fallback rules. `GitEngine` exposes nothing that returns per-file change type plus line counts.
- **Why it matters**: `GitEngine` is spec'd as "any and all Git operations"; a fourth frontend showing a diff summary would copy all of this.
- **Recommended action**: `GitEngine::diff_summary(&Path) -> Result<GitDiffSummary, EngineError>` in `src/engine/git/`; move the summary types and parsers there. The TUI keeps only the poll task and rendering.
- **Size**: M
- **Behavior change**: none

### F-07: The GitHub issue provider (git, gh, HTTP) is a 1,650-line engine inside Layer 0
- **Rule**: L0
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/data/issue/github.rs:191-196` (`git remote get-url`), `:253-266` (`gh issue view`), `:319-335` (blocking `reqwest` + `GITHUB_TOKEN`); `src/data/issue/mod.rs:119` (`IssueSource` trait), `router.rs`
- **What**: L0 spawns `git` and `gh` and falls back to HTTP. All consumers are L2 (`specs.rs`, `exec_prompt.rs`, `exec_workflow.rs`).
- **Why it matters**: Four-layer summary: L0 has "No git. No network."
- **Recommended action**: Move `src/data/issue/` to `src/engine/issue/` as `IssueEngine` (or keep `IssueSourceRouter` as the typed entry point). Leave `Issue`, `IssueSourceError`, `IssueSourceFlags`, `slugify` in L0 as plain data.
- **Size**: L
- **Behavior change**: none

### F-08: Session construction policy lives in the frontends; `SessionManager` is created but never used
- **Rule**: T2, L0
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/tui/key_handler.rs:983-1004`; `src/frontend/tui/mod.rs:91`, `src/frontend/tui/app.rs:116`; `src/frontend/api/mod.rs:135`; `src/frontend/api/routes.rs:51-70`; `src/frontend/squad/state.rs:13-22`
- **What**: Ctrl-T opens a `Session` and, on error, silently re-opens with the directory as git root. The API uses a different constructor (`Session::open_or_workdir_fallback`) and keeps its own `HashMap<String, Arc<RwLock<Session>>>`. `SessionManager::in_memory()` is threaded into `App` and never called.
- **Why it matters**: The spec names `SessionManager` as the multi-session owner; the non-git fallback policy already differs between TUI and API.
- **Recommended action**: `SessionManager::open_or_create(dir, opts)` in L0 using one fallback policy; TUI tabs and the API state hold a session id into it.
- **Size**: M
- **Behavior change**: TUI non-git fallback becomes identical to the API's

### F-09: Remote workflow polling, route paths and job-status semantics live in the TUI
- **Rule**: T2, L2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/frontend/tui/per_command/remote.rs:23-108` (`WorkflowStateSource` trait, the only frontend-owned trait), `:110-222` (`RemoteWorkflowPoller`)
- **What**: The two impls hard-code the squad route `["tasks", task, "workflow"]` (`:100`) and the API terminal statuses `"done" | "error"` (`:75`); the poller owns the 500 ms interval, the final-refresh rule and the errors-freeze-view rule.
- **Why it matters**: Route knowledge in a frontend drifts from the server; the CLI would need the same poller for parity.
- **Recommended action**: `TaskGateway::workflow_state(name)` and `RemoteClient::job_status(id) -> JobStatus` in L2; move the poller to L2 as a typed object taking an `on_state` callback.
- **Size**: M
- **Behavior change**: none

### F-10: `Dispatch::build_command` re-types catalogue defaults by hand; `implies` is never honoured
- **Rule**: L2
- **Severity**: High
- **Kind**: Violation
- **Where**: `src/command/dispatch/mod.rs:384-1080` (697-line match), `:403-406, 554-557, 688-691, 712-717, 941-944, 1005-1008`; `:419-422, 543-546, 1342`; `:1092-1180` (second 15-arm match)
- **What**: `unwrap_or_else(|| "claude")` at `:406` restates `FlagDefault::Str("claude")` (catalogue `:464`); the same for `9876`, `"6h"`, `"gitroot"`, `"local"`, `"toml"`. `if flags.json { non_interactive = true }` and `if yolo || auto { worktree = true }` restate the catalogue's `implies`, which has no non-test consumer anywhere. Four commands already use the better `read_*_flags` pattern (`:1280-1367`).
- **Why it matters**: The catalogue is not actually the source of truth for defaults or implications; clap uses one value, TUI/API dispatch another.
- **Recommended action**: Resolve `FlagDefault` and `implies` once and generically into a `ResolvedFlags` view; give each `*Command` a `from_input(&ResolvedFlags, &Engines, Session) -> Result<Self>` registered beside its `CommandSpec`; `build_command` collapses to lookup plus call.
- **Size**: L
- **Behavior change**: none (the six literals equal today's catalogue defaults)

### F-11: Docker and Apple backends are one backend written twice
- **Rule**: P1
- **Severity**: High
- **Kind**: Simplification
- **Where**: `src/engine/container/docker.rs:563-937` vs `src/engine/container/apple.rs:542-935`
- **What**: `spawn_piped_docker` vs `spawn_piped_apple` differ in 51 of ~92 lines, every one of which is `"docker"`↔`"container"` or a type name (`diff -w`). Six 7-parameter spawn functions, two execution backends, two instance structs and two bridge configs exist in parallel. The only real Apple-specific code is the attach-socket server (`apple.rs:601-640`). `apple.rs:770` documents that ACP was added "for parity" as a copy.
- **Why it matters**: Every spawn fix is a two-file change with a diff review to prove parity.
- **Recommended action**: `src/engine/container/process.rs` with `ContainerCli { bin, label, start_delay }`, one `ContainerInstance`/`ContainerExecution`, one `SpawnRequest` (collapsing the 7-param signatures), and `spawn_piped`/`spawn_piped_interactive`/`spawn_pty_bridged(…, post_bridge: Option<AttachHook>)`. Backends keep their own list/stats/stop parsing.
- **Size**: M
- **Behavior change**: none

### F-12: Crate-wide `#![allow(dead_code)]` with a stale justification hides 29 warnings
- **Rule**: P1
- **Severity**: High
- **Kind**: Spirit
- **Where**: `src/lib.rs:15` ("until oldsrc/ is deleted" — `oldsrc/` no longer exists); `src/data/mod.rs:1` (`#![allow(unused_imports)]`)
- **What**: Compiling a scratch copy with both attributes removed produces 29 warnings (list in `hidden-warnings.txt`), including: `SandboxBackend::{start_sandbox, restart_sandbox, exec_in_sandbox, remove}` never called and `SandboxId` never constructed (`src/engine/sandbox/backend.rs:19-58`); `WorkflowEngine.git_engine`/`overlay_engine` never read (`src/engine/workflow/mod.rs:194-195`); `image_exists_locally` never used and shelling to `docker` directly (`src/engine/agent/mod.rs:908`); five dead functions in `src/frontend/cli/output.rs:11-48`; `DockerBackend::is_available` unused (`docker.rs:39`); an unused import at `src/data/fs/daemon_guard.rs:19`.
- **Why it matters**: The spec forbids leaving legacy cruft; a layer-wide lint suppression is where cruft accumulates unseen.
- **Recommended action**: Delete both attributes, delete the dead items (or wire the sandbox trait methods to real callers if they are planned), and keep `-D warnings` green.
- **Size**: S
- **Behavior change**: none

### F-13: Three different headless answer tables (API, squad, CLI non-TTY)
- **Rule**: T2, P2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/api/command_frontend.rs:574-585, 843-903, 1023-1031`; `src/frontend/squad/unattended.rs:605-614, 640-690, 726-737`; `src/frontend/cli/per_command/exec_workflow.rs:110-120`
- **What**: `ask_agent_setup`: API returns `Setup` only if the default is available else `Abort`; squad always `Setup`. Merge mode: API `Squash`, squad `LeaveBranch`. Pre-worktree uncommitted files: API `Commit`, squad `UseLastCommit`. `confirm_resume`: API `true`, squad `false`.
- **Why it matters**: The same `exec workflow` behaves differently depending on which headless host ran it.
- **Recommended action**: One L2 `HeadlessDefaults` with named profiles (`::api()`, `::unattended_daemon()`) if the daemon legitimately needs a stricter one; all three frontends call it.
- **Size**: M
- **Behavior change**: none if profiles preserve today's answers

### F-14: Bearer-key verification is re-implemented in the API frontend, weaker than `AuthEngine`
- **Rule**: T2, L1
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/api/serve.rs:43-87` vs `src/engine/auth/mod.rs:413-431`
- **What**: `check_bearer_auth` hashes with SHA-256 and compares with `ct_eq`, without the sentinel-hash timing defence that `AuthEngine::verify_api_key` has. `AuthEngine` is built in `api::serve` but never used for request auth. `AuthMode` is defined in `routes.rs`.
- **Why it matters**: The spec assigns "authentication logic for the API server" to `AuthEngine`; two implementations of a security check already differ.
- **Recommended action**: `AuthEngine::request_auth_mode(skip)` and `AuthEngine::verify_bearer(mode, header)`; the frontend keeps header extraction and the HTTP envelope. The squad daemon builds an `AuthEngine` over `SquadPaths::daemon()`.
- **Size**: S
- **Behavior change**: the sentinel comparison now applies on the request path

### F-15: TUI `spawn_command` derives business facts from command and flag names
- **Rule**: T2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/app.rs:26-38, 772-775, 777-798, 800-817, 845-853`
- **What**: `is_containerized = matches!(path.first(), Some("chat" | "exec"))`; an 11-line ASCII "INTERACTIVE MODE" banner that no other frontend shows; `yolo_mode = yolo || auto` re-derived from flag names; `agent_name_from_parsed` calls L2 `resolve_agent` then hard-codes a `"claude"` fallback (tested at `:1444-1471` as a frontend test asserting a business default); gateway injection keyed on `path.first() == "squad"`.
- **Why it matters**: Adding a containerized command means editing the TUI; the `--auto`⇒yolo rule already lives in L2.
- **Recommended action**: `CommandSpec` attributes `runs_agent_container`, `wants_squad_gateway`; `Dispatch::yolo_effective(&parsed)`; make the built command expose the resolved agent display name. Decide whether the banner is a `ChatCommand`/`ExecPromptCommand` message (see Q6).
- **Size**: S
- **Behavior change**: none

### F-16: Container stats polling policy in the TUI with a fabricated `AgentHandle`
- **Rule**: T2, T3
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/app.rs:1024-1094, 1180-1213`
- **What**: The TUI owns the 3 s cadence, the in-flight dedupe set, the "name known → `stats()`, else first of `list_running_all()`" strategy, and builds `AgentHandle { started_at: now, image_tag: "" }` to satisfy the L1 signature.
- **Recommended action**: An L2 `ContainerStatsSampler` (or `AgentRuntimeEngine::stats_by_name`) that emits typed `StatsSample { tab_key, step_name, stats }`; replaces the `(usize, String, AgentStats)` tuple channels behind the `type_complexity` allows.
- **Size**: M
- **Behavior change**: none

### F-17: Squad daemon health classification in the TUI indicator
- **Rule**: T2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/squad_indicator.rs:53-78, 115-135`
- **What**: `probe_once` drives `SquadSupervisor` (pidfile → sidecar → gateway list → 401) and `classify` encodes the six-state precedence.
- **Recommended action**: `SquadSupervisor::health() -> SquadHealth` in `src/command/commands/squad/daemon.rs`; the TUI keeps the 10 s loop and the coloured dot.
- **Size**: S
- **Behavior change**: none

### F-18: The workflow control board's "simple advance" case is computed from step states in the TUI
- **Rule**: T2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/per_command/workflow_frontend.rs:23-125` (esp. `:35-45`), `:455-470`
- **What**: The TUI scans `step_states` for failures and pending steps to pick the lightweight confirm vs the full board, and re-applies permission guards the engine already encoded in `AvailableActions`.
- **Recommended action**: Add `simple_advance: Option<SimpleAdvance>` and `current_step_name` to `AvailableActions` (engine); TUI picks the dialog from those fields.
- **Size**: S
- **Behavior change**: none

### F-19: Dialog option sets, labels and defaults are authored per frontend and have already drifted
- **Rule**: L3, P2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/per_command/init.rs:57-61` vs `src/frontend/cli/per_command/init.rs:88` (labels differ); `"6h"` literal in `src/frontend/tui/per_command/squad.rs:172-177`, `src/frontend/cli/command_frontend.rs:161`, `src/command/dispatch/mod.rs:691`, `src/command/dispatch/catalogue.rs:1126`; `src/frontend/tui/per_command/exec_workflow.rs:40-46`, `worktree_lifecycle.rs:158-161`, `specs.rs:37-40`; CLI `command_frontend.rs:156-176, 300-310`, `per_command/mount_scope.rs:141-157`
- **What**: Hotkeys, labels, the Esc→default policy and fallback values ("6h", cwd, `Dockerfile.dev`, `GitRoot`) are written in each frontend. Only `PostWorkflowWorktreePrompt` uses the L2-supplied-prompt pattern. Non-test TUI+CLI long user-facing strings: 120.
- **Recommended action**: Generalise `PostWorkflowWorktreePrompt` into `Prompt<D> { title, body, choices: Vec<Choice<D>>, default_on_dismiss }` in L2 and pass it to every `ask_*`; read the interval default from the catalogue.
- **Size**: M
- **Behavior change**: none (labels converge)

### F-20: Config dialog validates keys and uses a tab-separated string protocol between two TUI files
- **Rule**: T2, T3
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/dialog_router.rs:70-80, 86-170, 193-260` (`:134` builds `"field\tvalue\trepo"`); `src/frontend/tui/per_command/config.rs:51-64` re-parses it
- **What**: `is_valid_map_key` mirrors `AgentName` rules; the router knows which fields are maps/arrays/secrets and computes the next guidance index.
- **Recommended action**: Extend `ConfigFieldRow` with a typed `kind`; return `DialogResponse::ConfigEdit(ConfigEditRequest)`; let `ConfigCommand` reject bad keys.
- **Size**: M
- **Behavior change**: none

### F-21: Startup command policy decided by a raw `.git` probe, duplicated
- **Rule**: T2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/mod.rs:135-162`; `src/frontend/tui/key_handler.rs:1008-1035`
- **What**: `if git_root.join(".git").exists() { "ready" } else { "status --watch" }`, copied verbatim.
- **Recommended action**: `Session::is_git_repo()` in L0 and `Dispatch::startup_command(&Session)` in L2.
- **Size**: S
- **Behavior change**: none

### F-22: `Tab` carries its own execution/workflow/container state instead of viewing `SessionState`
- **Rule**: T2 (spec: "TabState replaced by Session")
- **Severity**: Medium
- **Kind**: Spirit
- **Where**: `src/frontend/tui/tabs.rs:31-37, 94-150, 470-590`; `src/frontend/tui/tabs/overlay_lifecycle.rs:112-201`; `src/frontend/tui/workflow_view.rs:254-323`; vs `src/data/session.rs:211-217`
- **What**: `Tab` owns a cloned `Session` plus ~50 fields (`execution_phase`, `workflow_state`, `stuck`, `yolo_mode`, …) while `SessionState.current_command/current_workflow/current_container` are never written by anyone. `WorkflowStepView.status` is a `String` matched on "pending"/"running"/"done"/"error".
- **Recommended action**: Split `Tab` into view state plus a session id into `SessionManager`; have commands update `SessionState`; type the step status. Depends on F-08 and the answer to Q3.
- **Size**: L
- **Behavior change**: none intended

### F-23: The TUI imports the CLI, and a box renderer lives in Layer 0
- **Rule**: P1, L0
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/mod.rs:21` (`use crate::frontend::cli::RuntimeContext`); `src/frontend/tui/per_command/ready.rs:60` (CLI `render_summary_box`); `src/data/step_status.rs:34-99`; `src/command/commands/remote.rs:720-731`
- **What**: `RuntimeContext` (engines + session bundle) is defined in the CLI and consumed by the TUI; `render_summary_box` draws a Unicode box in L0 and is called from L2 `remote.rs`.
- **Recommended action**: Move `RuntimeContext` to `src/command/dispatch/`; move `render_summary_box` and glyphs to a shared frontend render helper; `remote.rs` returns the `ReadySummary` for the frontend to render.
- **Size**: S
- **Behavior change**: none

### F-24: `TuiCommandFrontend::new` takes 19 parameters; tuple channels hide simple structs
- **Rule**: T3
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/frontend/tui/command_frontend.rs:64, 108-130`; `src/frontend/tui/app.rs:135-149, 713-733`; five test copies (`per_command/init.rs:135-184`, `ready.rs`, `clean.rs`, `mount_scope.rs`, `workflow_frontend.rs`)
- **What**: 15 of the 19 arguments are `tab.x.clone()` shared slots; three `type_complexity` allows hide `(usize, String, AgentStats)` and `Result<(RemoteTaskGateway, SquadKeyState), String>`.
- **Recommended action**: `TabSharedState` (Clone) on `Tab`, `DialogChannels`, `StatsSample`, `SquadStartupResult`; `new(parsed, dialogs, io, shared)`; `TabSharedState::for_tests()`.
- **Size**: M
- **Behavior change**: none

### F-25: The catalogue and `aspec/uxui/cli.md` have drifted in both directions
- **Rule**: L2, P2
- **Severity**: Medium
- **Kind**: Violation (spec drift)
- **Where**: `src/command/dispatch/catalogue.rs:402-437` (`clean`, absent from cli.md), `:1270` (`squad edit`), `:1449` (`squad trigger`), `:1069` (`--agent-models`), `:1511, 1527` (`remote exec workflow|prompt`), `:1708-1775`, `:1876` (`toml|yaml` only); `aspec/uxui/cli.md:85-158` (`remote run <cmd>`, `session start <dir>`, `--format md`)
- **What**: The spec documents commands the binary does not have and omits ones it does. `requires_runtime` is also hard-coded as `!matches!(path.first(), Some("config"))` (`:221-223`) instead of a `CommandSpec` field.
- **Recommended action**: Generate the per-command reference in `cli.md` from a `CommandCatalogue::markdown_reference()` projection, checked by a test; add `requires_runtime: bool` to `CommandSpec`. Needs Q8/Q9 answered first.
- **Size**: S
- **Behavior change**: none

### F-26: Two parallel catalogue-driven token parsers with divergent semantics
- **Rule**: P1, P2
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/command/dispatch/parsed_input.rs:35-243` (TUI path) vs `src/command/dispatch/projections/raw_args.rs:265-500` (API path)
- **What**: Both walk `CommandSpec` for long/short flags, `--`, trailing args, conflicts and positionals (~200 lines each). The TUI parser does not validate enum values or coerce numbers; error kinds differ.
- **Recommended action**: Reduce `parsed_input::parse` to tokenise plus path resolution, then delegate to `raw_args::parse_against_spec`.
- **Size**: M
- **Behavior change**: TUI gains enum/number validation at parse time (parity)

### F-27: `commands/mod.rs` holds the overlay grammar and config-source merge (L0 work) as free functions
- **Rule**: L0, T3
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/command/commands/mod.rs:52-130, 142-166, 188-270, 473-605, 607-764`; tests `:765-1804` (90% of the file)
- **What**: `parse_overlay_spec`/`parse_overlay_list` (the `dir()/ssh()/env()/skill()/context()` DSL), `collect_all_overlay_specs` (merges global, repo, `AWMAN_OVERLAYS`, CLI, workflow and step values), `resolve_launch_mode`, `resolve_agent`, `resolve_context_overlays`. Merging config sources is spec'd as L0; `RepoConfig.overlays` is an opaque `Vec<String>` only L2 can validate.
- **Recommended action**: `src/data/config/overlays.rs` for the grammar types, parser and `collect_all_overlay_specs` (as `EffectiveConfig::collected_overlays(cli, workflow, step)`); keep `resolve_agent`/`resolve_launch_mode`/`resolve_context_overlays` in L2 on a `LaunchPolicy` struct in its own module; `mod.rs` becomes declarations.
- **Size**: L
- **Behavior change**: none

### F-28: HTTP transport (`HttpCore`, SSE client) is a network primitive living in Layer 2
- **Rule**: L1
- **Severity**: Medium
- **Kind**: Spirit
- **Where**: `src/command/commands/http_core.rs:1-14, 60-91`; `src/command/commands/remote_client.rs:265-457`
- **What**: `HttpCore` is the tree's single `reqwest::Client` builder (timeouts, bearer, cert pinning). Engine-layer HTTP users (`engine/agent/download.rs`, `engine/workflow/poll_ci.rs`) cannot reuse it because it sits above them.
- **Recommended action**: Move `HttpCore`, `RemoteClient`, `ExecutionEventSink`/`RemoteEventSink` to `src/engine/remote/` returning `EngineError`; `RemoteCommand` stays L2.
- **Size**: M
- **Behavior change**: none

### F-29: Git, network and process supervision inside Layer 0 beyond the issue provider
- **Rule**: L0
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/data/fs/context_dirs.rs:91` (`git remote get-url` in a path resolver); `src/data/network/aspec_tarball.rs:21-44` (`reqwest` download); `src/data/fs/daemon_process.rs:256-746` (`systemd-run`, `launchctl`, `ps`, `tasklist`, `taskkill`, spawning the daemon binary), `src/data/fs/daemon_guard.rs`
- **What**: The remote-URL parser is also duplicated three times in L0 (`context_dirs.rs:106-144`, `issue/github.rs:214-237`, `fs/skill_library.rs:74-112`). PID/meta file handling in `daemon_process.rs:60-245` is legitimate L0; the process-management half is not. `api_server.rs:274-275` calls the free `is_process_alive`/`pid_is_awman` directly.
- **Recommended action**: `ContextDirResolver::repo_dir(remote_url: Option<&str>, git_root)` with `GitEngine` supplying the URL and one shared `parse_owner_repo`; `AspecDownloader` in `src/engine/aspec/`; `DaemonSupervisor` in `src/engine/daemon/` owning spawn/kill/liveness while `DaemonPaths`/`ServerMeta` stay in L0. Gate on Q1.
- **Size**: L
- **Behavior change**: none

### F-30: A dead second workflow-state model and a duplicated agent-precedence rule in Layer 0
- **Rule**: P1
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/data/session.rs:137-180` (`StepStatus`, `WorkflowStepRecord`, `WorkflowInvocation`), `:211-217`; `src/data/fs/workflow_state.rs:19-107` vs `src/data/workflow_state_store.rs:16-102` (two `pub struct WorkflowStateStore`); `src/data/mod.rs:50` (`EngineWorkflowStateStore` re-export); `src/data/session.rs:490-505` vs `src/data/config/effective.rs:86-95`
- **What**: Nothing outside `session.rs` reads `WorkflowInvocation` or the three `SessionState` fields. The `fs/workflow_state.rs` store is used only for its `sha256_hex`/`sanitize_name_for_filename` helpers. `resolve_default_agent` re-encodes the flag > repo > global chain that `EffectiveConfig::agent()` calls its single source of truth.
- **Recommended action**: Delete the dead types (or wire them per Q3), keep one `WorkflowStateStore` under its plain name, derive `default_agent` from `EffectiveConfig`.
- **Size**: M
- **Behavior change**: none

### F-31: Config is loaded from disk ad hoc in frontends, engines and commands instead of through `Session`
- **Rule**: T2, L2
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/frontend/tui/per_command/init.rs:49`, `src/frontend/cli/per_command/init.rs:83` (`RepoConfig::load(...).unwrap_or_default()` to build a prompt title, plus `"Dockerfile.dev"` default); `src/command/commands/api_server.rs:191-196` (re-implements `EffectiveConfig::api_work_dirs()`); `src/engine/init/mod.rs:161, 201, 273, 436, 496`; `src/command/commands/squad/commands.rs:538, 597`; `squad/gateway.rs:305`; `src/frontend/api/mod.rs:71`; `src/frontend/squad/mod.rs:160`; `src/main.rs:54`
- **What**: Each site does an independent `GlobalConfig::load().unwrap_or_default()`, swallowing parse errors `Session::open` would surface; two frontends read repo config to learn a path the trait could pass.
- **Recommended action**: `InitFrontend::ask_dockerfile_setup` receives the display path; `api_server.rs` uses `session.effective_config().api_work_dirs()`; `InitEngine` holds its `RepoConfig`; the rest route through the owned `Session`.
- **Size**: M
- **Behavior change**: config parse errors may surface where they are silently defaulted today

### F-32: Runtime and agent identity are branched on by string in Layer 1, contrary to the modules' own invariants
- **Rule**: L1, P1
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/engine/container/runtime.rs:82-88` and four more `"apple-containers" =>` arms (`:121, 197, 283, 317`) though `ContainerBackend::cli_binary()` exists; `src/engine/agent/mod.rs:4-5` ("All agent-name branching lives in `agent_matrix.rs`") vs `:360, 567, 616, 666`; `src/engine/overlay/mod.rs:409-598, 663-682`, `src/engine/auth/keychain.rs:46-70`, `src/engine/ready/mod.rs:98-110` (four per-agent `match agent.as_str()` tables)
- **What**: `agent_settings_overlays_with_credentials` (211 lines) is five identical 10-line arms plus two special cases; `AgentMatrix` has no mount, credential-source or ping fields, so per-agent facts are spread over four files.
- **Recommended action**: Delegate to `backend.cli_binary()` and add `display_name()`/probe args to `ContainerBackend`; extend `AgentMatrix` with `settings_mount`, `skills_mount`, `credential_source`, `ping_argv`, `static_env`; collapse the tables.
- **Size**: M
- **Behavior change**: none

### F-33: Two different traits named `AgentFrontend`, plus re-export shims that give L0 types an engine path
- **Rule**: P1
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/engine/agent/frontend.rs:12-22` vs `src/engine/agent_runtime/frontend.rs:81-110`; `src/engine/step_status.rs:1`, `src/engine/ready/phase.rs:1`, `src/engine/ready/summary.rs:1` (`pub use crate::data::…::*`)
- **What**: A setup-progress trait and an I/O-binding trait share a name; `ReadyFrontend`/`InitFrontend` re-declare the same two methods inline. Twenty-plus sites import `crate::engine::step_status::StepStatus` while L0 imports the data path.
- **Recommended action**: Rename to `AgentSetupFrontend` and have `ReadyFrontend: AgentSetupFrontend`; delete the three shim files and import from `crate::data`.
- **Size**: S
- **Behavior change**: none

### F-34: `WorkflowEngine` stores engines it never calls, and setup/teardown are the same phase machine written twice
- **Rule**: P1, T3
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/engine/workflow/mod.rs:194-195, 324-326, 428-430, 452-460` (7- and 8-param constructors); `:2362-2453` vs `:2454-2550`, `:2551` (`run_shell_phase_step(phase: &str)`), `:2590` (`set_phase_step_failed(is_setup: bool)`), `:2736` vs `:2793`; `src/engine/workflow/frontend.rs:122-135` (10 callbacks = 5 identical pairs, implemented in 6 frontends and 8 test fakes)
- **What**: `git_engine`/`overlay_engine` have zero method calls in 3,273 non-test lines. `run_setup`/`run_teardown` and their remediation twins differ only by field names and callback names; the setup path discards stdout/stderr that teardown captures.
- **Recommended action**: Delete the two fields and params; introduce `WorkflowEngineDeps`/`WorkflowSpec` to drop `resume_with_state_root`; `enum PhaseKind { Setup, Teardown }` with one `run_phase(kind, …)` and `on_phase_step_*(kind, …)` callbacks (10 → 5 trait methods).
- **Size**: M
- **Behavior change**: none (setup gaining teardown's failure-file behaviour should be a deliberate follow-up)

### F-35: Dead commands and empty frontend traits inflate the trait surface every frontend must implement
- **Rule**: T3, P1
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/command/dispatch/mod.rs:324-325` (`BuiltCommand::Auth`/`Download` with no build arm and no catalogue spec); `src/command/commands/auth.rs`, `download.rs`; `src/command/commands/api_server.rs:115-117` (three empty traits with no impl), `:111` and `:122` (identical `serve_until_shutdown`); `specs.rs:257-270` (re-declares `HasAgentFrontend` methods); `set_pty_active` declared four times with two defaults; `src/command/commands/squad/commands.rs:336-372` (`SquadDaemonCommand` is a second `Command` impl re-wrapped by `SquadCommand`)
- **What**: 33 `pub trait`s in `src/command`: 16 per-command, 6 helper-flow, 4 dead or empty, 3 empty aliases, 4 non-frontend. `DispatchFrontend` requires bounds for commands nobody can invoke.
- **Recommended action**: Delete `AuthCommand`/`DownloadCommand` and their traits (or register them in the catalogue); delete the three empty `ApiServer*` traits; one `AgentLaunchFrontend: MountScope + AgentSetup + AgentAuth + HasAgent { set_pty_active; set_stuck_sender }` extended by Chat/ExecPrompt/ExecWorkflow/Specs; fold `SquadDaemonCommand` into `SquadCommand`.
- **Size**: M
- **Behavior change**: none

### F-36: `WorkflowProxy` and `AgentFrontendProxy` are 300 lines of hand-written forwarding
- **Rule**: P1
- **Severity**: Medium
- **Kind**: Simplification
- **Where**: `src/command/commands/exec_workflow.rs:222-462, 463-528`
- **What**: 37 one-line `self.0.lock().unwrap().x(args)` methods that must be updated whenever `WorkflowFrontend` changes.
- **Recommended action**: Blanket `impl<F: WorkflowFrontend + ?Sized> WorkflowFrontend for Arc<Mutex<Box<F>>>` in `src/engine/workflow/frontend.rs` (and likewise for `AgentFrontend`); delete both proxies.
- **Size**: S
- **Behavior change**: none

### F-37: Environment variables are read and parsed outside Layer 0 and are undeclared in `Env`
- **Rule**: L0
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/engine/workflow/poll_ci.rs:29` and `src/data/issue/github.rs:331` (`GITHUB_TOKEN`); `src/engine/container/attach_socket.rs:77` (`AWMAN_ATTACH_DIR`); `src/frontend/api/session_setup.rs:463-471` (`AWMAN_API_VERBOSE_SETUP`, with truthiness parsing in L3); `src/engine/container/docker.rs:1288-1292`, `options.rs:474, 480`, `sandbox/dsbx/session_config.rs:89` (passthrough values read at spawn time)
- **What**: `src/data/config/env.rs:3-4` states reads are funnelled through `Env`; none of these three names appears in `Env::from_process()` (`:178-201`). `poll_ci.rs` also builds its own `reqwest` client (`:142`) and re-shells `git rev-parse`/`git remote` (`:45-73`) instead of using `GitEngine`.
- **Recommended action**: Declare the constants and typed accessors on `EnvSnapshot`; pass resolved values in; wrap the CI poller as `CiPoller { git, token }`.
- **Size**: S
- **Behavior change**: none

### F-38: The sanctioned host-side agent ping is a pair of free functions, and the credential monitor is a process-global singleton
- **Rule**: T3, S1 (auditability)
- **Severity**: Medium
- **Kind**: Spirit
- **Where**: `src/engine/ready/mod.rs:123-160, 178-245` (`ping_local_agent`, `refresh_host_credential`), called from `src/engine/credential_refresh/monitor.rs:342`; `monitor.rs:612-625` (`static GLOBAL_MONITOR: OnceLock`) reached from `docker.rs:607, 668, 782` and `apple.rs:586, 696, 787` via `credential_refresh::global()`, installed by `src/command/dispatch/mod.rs:83`; `src/engine/squad/launcher.rs:47-70, 166-200` (`drive_unattended_agent` and friends as free fns)
- **What**: The only S1-exempt code path is callable from any module as a bare `pub async fn`; the spawn paths depend on hidden global state instead of an injected option.
- **Recommended action**: `HostAgentPinger` struct held by `ReadyEngine` and `CredentialRefreshMonitor`; carry the monitor (or a `CredentialLeaseFactory`) in `ResolvedContainerOptions`; move the squad launcher free fns onto `SquadAgentLauncher`.
- **Size**: M
- **Behavior change**: none

### F-39: Direct `git` spawns in `exec_workflow.rs`, one in the wrong directory
- **Rule**: L1
- **Severity**: Medium
- **Kind**: Violation
- **Where**: `src/command/commands/exec_workflow.rs:1532-1545` (`git config user.name/email` with no `current_dir`), `:2975-2984` (`worktree_git_status` = `git status --porcelain`, which `GitEngine::uncommitted_files` already does)
- **What**: The identity probe runs in awman's process cwd, so a repo-local identity is not honoured; the status probe swallows errors `GitEngine` would surface.
- **Recommended action**: `GitEngine::identity_configured(&Path) -> Result<GitIdentity>`; replace `worktree_git_status` with `uncommitted_files`.
- **Size**: S
- **Behavior change**: identity check becomes repo-scoped (latent bug fix; note in changelog)

### F-40: Commands switch whole flows on which concrete runtime handle is `Some`
- **Rule**: L1
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/command/commands/ready.rs:190` (`if self.engines.sandbox_runtime.is_some()` selects a different ready flow); `clean.rs:218, 404, 458`; `src/engine/sandbox/mod.rs:29-35` (`ready_sbx_agent` free fn called from `ready.rs:195`)
- **Recommended action**: Branch on `engines.runtime.capabilities().kit_declarative`; add an "image store" capability for clean; `SandboxRuntime::ready_agent(...)` method.
- **Size**: S
- **Behavior change**: none

### F-41: `SessionSetupObserver` requires the frontend to implement persistence
- **Rule**: T2
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/command/session_setup.rs:68, 74, 83` (`persist_status`, `register_session`, `persist_and_cleanup`); `src/frontend/api/session_setup.rs:22-141, 176-260`
- **Recommended action**: Split into a presentation-only `SessionSetupPresenter`; `SessionSetup` takes `SessionManager` and the status store directly; move `SessionSetupState` transition logic onto the L0 type.
- **Size**: M
- **Behavior change**: none

### F-42: Exit-code and outcome-failure policy differ between CLI and API
- **Rule**: P2
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/frontend/cli/mod.rs:446-455` (`outcome_exit_code` peeks into `NewOutcome::Skill`); `src/frontend/api/queue_worker.rs` (`derive_command_status` treats `New` as always done)
- **Recommended action**: `CommandOutcome::is_partial_failure()`/`exit_code()` in L2 (shared with F-03).
- **Size**: S
- **Behavior change**: API reports `error` for a partially failed `new skill --pull-all`

### F-43: A second Levenshtein "did you mean" in the TUI
- **Rule**: L2, P2
- **Severity**: Low
- **Kind**: Violation
- **Where**: `src/frontend/tui/command_box.rs:20-52` (`:46`); L2 already has one at `src/command/commands/config.rs:319`
- **Recommended action**: `CommandCatalogue::suggest(&[&str])` carried in `CommandError::UnknownCommand { suggestions }`.
- **Size**: S
- **Behavior change**: suggestions may improve for nested paths

### F-44: Panic-log path resolution and file append in the TUI event loop
- **Rule**: L0
- **Severity**: Low
- **Kind**: Violation
- **Where**: `src/frontend/tui/event_loop.rs:38-70`
- **Recommended action**: `data::fs::PanicLog { path(&Env), append(report) }`; the hook formats and calls it.
- **Size**: S
- **Behavior change**: none

### F-45: `WorkflowFrontend` pushes engine channels into frontends; ready/init steps are keyed by free-form strings; engines render transcript text
- **Rule**: T1, T3, L3
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/engine/workflow/frontend.rs:104-121, 181-190` (`set_engine_sender`, `set_stuck_sender`, `set_parallel_step_io`, …); `src/engine/ready/frontend.rs:23`, `init/frontend.rs:33`, `agent/frontend.rs:15` (`step: &str`); `src/engine/git/mod.rs:18-48` (`"$ {cmd}"` echo), `src/engine/ready/mod.rs:619-627` (`"> greeting"`/`"< response"`), `src/engine/workflow/mod.rs:2629-2639`
- **Recommended action**: One `attach_engine(EngineHandles)` method; `ReadyStep`/`InitStep` enums with `Display` in L0; typed events (`command_started`, `report_ping`, `report_ci_poll`) with default impls that keep today's text.
- **Size**: M
- **Behavior change**: none

### F-46: Agent/model default logic and a permission policy inside Layer 1
- **Rule**: L1, L2
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/engine/squad/scheduler.rs:397-425` (parses `agent::model` leader syntax and picks the first `agentsToModels` entry for log labels, duplicating the L2 evaluator); `src/engine/acp/session.rs:66-72` (`auto_approve = yolo || auto`)
- **Recommended action**: Have `TaskEvaluator` report the resolved pair; pass an explicit `PermissionPolicy` into `AcpSession::new`.
- **Size**: S
- **Behavior change**: none

### F-47: Serialization contracts defined in Layer 1 and raw filesystem writes in Layer 2
- **Rule**: L0
- **Severity**: Low
- **Kind**: Violation
- **Where**: `src/engine/squad/verdict.rs:43-50, 81-109` (`RunVerdict`, written by the leader agent into the container — an external file contract); `src/command/commands/squad/gateway.rs:359-473` (task config writes), `squad/daemon.rs:467` (`std::env::set_var`), `squad/evaluation.rs:1002, 1022` (build-log files); `src/command/commands/api_server/banner.rs:5-13` (box-drawing in L2)
- **Recommended action**: `RunVerdict` to `src/data/fs/squad_verdict.rs`; `TaskStore`/`SquadPaths` helpers for the writes; emit the API key as data and render the banner in the CLI.
- **Size**: S
- **Behavior change**: TUI/API stop receiving box-drawing text unless they render it

### F-48: Stringly-typed session kind and step status crossing layer and wire boundaries
- **Rule**: T3, L0
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/command/session_create.rs:103-109`, `src/command/commands/remote.rs:534-536` (`match "local" | "remote"` though `data::session::SessionType` exists); `src/data/fs/api_db.rs` `insert_session_full` (7 `&str` params); `src/data/execution_event.rs:26-31` (`from_status: String, to_status: String`), `src/frontend/api/command_frontend.rs:651-658`, `queue_worker.rs:438, 501`
- **Recommended action**: `SessionKind` enum with `FromStr`; `NewSessionRow` struct; serde-tagged `StepStatusKind` in `EventPayload` (same snake_case strings on the wire; gate on Q10).
- **Size**: S
- **Behavior change**: none on the wire if names are preserved

### F-49: Three commands are built without a `Session` and probe frontend identity at run time
- **Rule**: L2
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/command/commands/status.rs:179-181, 213` (`frontend.tui_context()`); `squad/commands.rs:308-320, 752` (`frontend.is_local_user_session()`); `api_server.rs:132`
- **Recommended action**: Pass a `CallerContext { local_user, tui_tab }` from `Dispatch` at construction; give all three the `Session`.
- **Size**: S
- **Behavior change**: none

### F-50: Non-interactive flag has two sources of truth inside the CLI; `flags_applied` is hard-coded in a route
- **Rule**: T2
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `src/frontend/mod.rs:25-31`, `src/frontend/cli/command_frontend.rs:357-373` (reads `--non-interactive` from clap directly and caches it) while `mount_scope.rs:141`, `agent_setup.rs:182` call `stdin_is_tty()` directly; `src/frontend/api/routes.rs:952-955` (`flags_applied: {yolo:true, non_interactive:true}` vs catalogue `API_FLAG_DEFAULTS` at `raw_args.rs:158`); `src/frontend/squad/routes.rs:282-289` (squad-subtree rule)
- **Recommended action**: `CommandFrontend::input_available()` (TTY fact) with `Dispatch` computing effective non-interactive; serialise the catalogue profile; `FrontendKind::SquadDaemon` visibility in the catalogue.
- **Size**: S
- **Behavior change**: `--non-interactive` on a TTY consistently suppresses all prompts

### F-51: Oversized functions and files that should be split (behaviour-preserving)
- **Rule**: P1
- **Severity**: Low
- **Kind**: Simplification
- **Where**: `src/engine/workflow/mod.rs` (7,622 lines, 57% tests; 7 responsibilities) → `workflow/{mod, single_step, parallel, control, phases, queries}.rs` + `tests/`; `src/command/commands/exec_workflow.rs` (5,943; `run_with_frontend` 423 lines, `execute_prepared` 544, `run_dynamic` 361, `drive_leader_agent` 11 params) → `exec_workflow/{mod, proxies, factory, prepare, execute, dynamic, image, issue}.rs` plus a neutral `commands/workflow_preflight.rs` for the eight helpers `squad/evaluation.rs:21-26` imports from it; `src/engine/overlay/mod.rs` → `{agent_settings, claude, skills, credential_file}.rs`; `src/command/dispatch/catalogue.rs` → per-group files with `EXEC_PROMPT_FLAGS`/`SQUAD_EDIT` derived by const fn from `AGENT_RUN_FLAGS_NO_WORKTREE`/`SQUAD_ADD` (93% / 64% identical today); `src/frontend/tui/render/dialog.rs:7-870` (one 864-line match) → per-variant `render_*`; `src/frontend/tui/key_handler.rs:23-759` (737 lines) → `focus_context`, `intercept_dialog_keys`, grouped action handlers; `src/frontend/tui/app.rs::tick_all_tabs` (310 lines, six labelled sections); `clean.rs::discover` (186 lines, four numbered categories) and `new.rs::run_with_frontend` (532 lines, three arms) → per-category/per-subcommand fns; `config.rs:23-530` seven string-keyed lookups over one field list → a `ConfigFieldSpec` table in `src/data/config/fields.rs`; `src/engine/agent/mod.rs:214-236` vs `:515-543` etc. (three verbatim validation/mode-flag blocks) → `AgentMatrix::validate_run`/`mode_flags`
- **Size**: L in aggregate (each piece S–M)
- **Behavior change**: none

### F-52: Test fixture duplication
- **Rule**: P1
- **Severity**: Low
- **Kind**: Simplification
- **Where**: `fn make_engines` copied into 9 test modules (identical 33-line body in `dispatch/mod.rs:1436`, `exec_workflow.rs:3684`, `config.rs:1838`, `tui/app.rs:1374`, …); `fn make_session` defined 23 times; `TuiCommandFrontend::new(…19 args…)` copied in 5 TUI test modules; `tests/helpers/mod.rs` has `TestEnv` but no `Engines` builder; 7 `#[ignore]` tests whose bodies are `todo!()` at `exec_workflow.rs:5846-5894`
- **Recommended action**: `Engines::for_tests(root)` under `cfg(test)`; `TestEnv::engines()`; `TabSharedState::for_tests()` (F-24); delete or implement the seven stubs.
- **Size**: S
- **Behavior change**: none

### F-53: Stale documentation pointers
- **Rule**: —
- **Severity**: Low
- **Kind**: Spirit
- **Where**: `aspec/architecture/four-layer-summary.md:497` points to `docs/10-architecture-overview.md`, which does not exist (`docs/10-github-integration.md` does; `docs/architecture.md` is the real page); `src/data/message.rs:25-27` says `UserMessageSink` is "Defined by Layer 1" (it is L0); `src/engine/agent/mod.rs:110-111` says production callers pass `image_exists_locally` (they do not)
- **Recommended action**: Fix the three references.
- **Size**: S
- **Behavior change**: none

### F-54: API keeps a raw session map instead of `SessionManager`; `InitialTab` carries a redundant `Session`
- **Rule**: L0, P1
- **Severity**: Low
- **Kind**: Simplification
- **Where**: `src/frontend/api/routes.rs:51-70`, `src/frontend/squad/state.rs:13-22`; `src/frontend/tui/mod.rs:68-72` (`#[allow(clippy::large_enum_variant)]` on `InitialTab::Normal(Session)` while `ctx.session` already carries it)
- **Recommended action**: Fold into F-08; `InitialTab { Normal, Squad }` built from `ctx.session`.
- **Size**: S
- **Behavior change**: none

---

## Large refactor proposals

### R-A: Engine assembly and daemon bootstrap in Layer 2 (F-05, F-02, F-03, F-04)
Replaces: three hand-wired `Engines` constructions and two daemon bootstraps in `src/frontend/`. Target: `Engines::build(&GlobalConfig, &Session)`, `Engines::for_daemon(paths)`, `Startup::run()` (migration + session open), `ApiServerRuntime::bootstrap`, `SquadDaemonRuntime::bootstrap`, `QueueWorker` under `src/command/commands/api_server/`. Order: F-05 first (pure move), then F-02, then F-03 (largest), then F-04. Proof: `make pre-push`, `tests/api_parity/live_server.rs`, `tests/squad_daemon_e2e.rs`, `tests/squad_daemon_http.rs`, plus `make architecture-lint` after each move. Needs a work item.

### R-B: Catalogue-owned defaults and per-command constructors (F-10, F-26, F-35, F-25)
Replaces: the 697-line `build_command` match, the second `run_command` match, six duplicated default literals, unconsumed `implies`, and the second token parser. Target: `ResolvedFlags` produced once by `Dispatch` from `FlagDefault`/`implies`; `CommandSpec.build: fn(&BuildContext) -> Result<BuiltCommand>`; `parsed_input::parse` delegating to `raw_args`. Proof: `tests/cli_parity/*`, `src/command/dispatch/projections/parity_test.rs`, TUI key-handler tests. Needs a work item.

### R-C: `squad attach` as a Layer 2 command (F-01)
Replaces: `frontend/attach.rs`, `tui/squad_attach.rs` driver logic, `cli/per_command/squad_attach.rs` flow. Target: `SquadAttachCommand` + `SquadAttachFrontend` + `BuiltCommand::SquadAttach`. Proof: `tests/squad_attach.rs`, `tests/squad_attach_frontends.rs`, `tests/squad_tui_tab.rs`. Needs a work item.

### R-D: Issue engine and overlay grammar relocation (F-07, F-27, F-29)
Replaces: `src/data/issue/` (L0 → L1), overlay DSL and source merge (L2 → L0), `aspec_tarball`/`DaemonProcess` process half (L0 → L1). Each is a mechanical move guarded by `make architecture-lint`. Gate on Q1. Needs a work item.

### R-E: Single container process module (F-11)
Replaces: six 7-parameter spawn functions across `docker.rs`/`apple.rs`. Target: `container/process.rs` with `ContainerCli`, `SpawnRequest`, one instance/execution type. Proof: `tests/engine/container_docker.rs`, `container_io.rs`, `credential_argv_docker.rs`, and the Apple path under `AWMAN_DOCKER_INTEGRATION`. Needs a work item.

### R-F: `Tab` as a view of `SessionState` (F-22, F-08)
Depends on Q3. Not recommended before R-A and F-08 land.

---

## Questions for the developer

1. **Layer 0 scope.** The grand architecture says L0 holds "external storage, api types, filesystem interaction" and "handling the API mode directories"; the four-layer summary says "No git. No network." Which governs for (a) the aspec tarball download, (b) the GitHub issue provider, (c) daemon process supervision (`systemd-run`/`launchctl`/spawn)? F-07 and F-29 assume the stricter reading.
2. **Serializable outcomes.** 53 `Serialize` types in `src/command` (`*Outcome`, `CommandOutcome`) are the `--json` and API-response contracts. Are they an "external contract" that must move to `data::outcomes`, or L2-owned results the frontends merely serialise? This decides whether ~19 files change.
3. **`SessionState`.** Nothing outside L0 reads or writes `current_command`/`current_workflow`/`current_container`. Should it be wired as the ruling in-flight state (and `Tab` derive from it, F-22), or deleted so `WorkflowEngine` owns state (F-30)?
4. **`frontend/squad`.** Is it intended as a fourth frontend, or as the daemon's runtime host? The doc header says "frontend and bootstrap"; the code is ~90% bootstrap.
5. **Headless profiles.** Are the differing API vs squad answers (squash-merge + auto-commit vs leave-branch + never-commit; agent setup) deliberate per-host policy or drift? Either way F-13 puts them in L2, but as one table or two named profiles.
6. **Interactive banner.** Is the TUI-only "INTERACTIVE MODE" banner (`app.rs:777-798`) intentional, or should `ChatCommand`/`ExecPromptCommand` emit it via `UserMessageSink` so the CLI shows it too?
7. **`api_allowed: false`** for clean/init/ready/chat/specs/status/config/new/remote: long-term parity policy, or transitional? Today P2 parity is enforced by exclusion.
8. **`remote` surface.** `cli.md` documents `remote run <command…>` and `session start <dir>`; the code has `remote exec workflow|prompt` and `session start --type/--workdir/--repo-url`. Which is right?
9. **`new workflow --format md`** is documented in `cli.md:90` but the catalogue accepts only `toml|yaml`. Intentional removal?
10. **API wire schema.** Is the `EventPayload` string schema (`"pending"`/`"running"`/`"done"`, `"failed"`) frozen for external clients? F-48 must preserve exact names if so.
11. **`docker.sock` mount** under `allow_docker` (`docker.rs:1308-1325`) is the only host path outside the S2 set. Should `security.md` list it as sanctioned?
12. **`ReadyEngine`/`InitEngine`** build and run audit agents through a concrete `Arc<ContainerRuntime>` (`ready/mod.rs:738-745`, `init/mod.rs:391-402`), so they cannot run under `SandboxRuntime`; sandbox readiness is routed separately via `ready_sbx_agent`. Intended?
13. **Cross-command imports.** `squad/evaluation.rs:21-26` imports eight helpers from `exec_workflow.rs`. Acceptable within L2, or should they move to a neutral `commands/workflow_preflight.rs` (F-51)?

## Decisions — 2026-09-04

Answers from the developer to the questions above, plus three edge-case
decisions raised by the work items. These are binding for WI 0113 and 0114.

| # | Decision |
|---|---|
| Q1 | **Strict.** No git, network or process spawning in Layer 0. F-07 (issue provider), F-28 (HTTP client) and F-29 (tarball download, daemon supervision) all move to Layer 1. |
| Q2 | **L2-owned.** `*Outcome`/`CommandOutcome` stay beside their commands. Only on-disk / cross-process contracts move to L0 (F-47 `RunVerdict`). |
| Q3 | **Wire it.** `SessionState` becomes the ruling in-flight state; commands update it and the TUI `Tab` derives from it (F-22). F-30 deletes only the parallel dead types. |
| Q4 | **All squad business logic moves down, the majority into the engine layer.** `frontend/squad` keeps only presentation and I/O (router, bind, trait impls) and calls down through traits like every other frontend. Squad must adhere to the grand architecture with no exception. This is stronger than the F-02 recommendation: the daemon runtime (store, scheduler, reconciliation, gateway) is an engine, and the L2 command wires frontend traits into it. |
| Q5 | **Deliberate.** Two named L2 profiles reproducing today's answers: `HeadlessDefaults::api()` and `HeadlessDefaults::squad()` (not `::unattended_daemon()`). |
| Q6 | **Delete the banner entirely.** No frontend shows the "INTERACTIVE MODE" banner. |
| Q7 | **Long-term policy.** `api_allowed: false` stays for interactive/PTY commands; record it as a sanctioned P2 exception in the catalogue doc comment and the grand architecture. |
| Q8 | **The code is right**, and `aspec/uxui/cli.md` should not document specific commands and flags at all. It becomes the source of truth for generalised UI/UX standards and best practices; the per-command reference is user documentation and belongs in `docs/`, generated from the catalogue. |
| Q9 | **Intentional.** `--format md` is gone; the reference lists `toml|yaml`. |
| Q10 | **Not frozen.** The `EventPayload` strings may be renamed; the typed enums use whatever names read best. |
| Q11 | **Yes.** Document the `docker.sock` mount under `--allow-docker` in `security.md` as a sanctioned exception to S2. |
| Q12 | **Make `ReadyEngine`/`InitEngine` runtime-agnostic in WI 0114** (`Arc<dyn AgentRuntimeEngine>`; `ready_sbx_agent` folds into the trait). |
| Q13 | **Move to a neutral module.** The eight helpers `squad/evaluation.rs` imports from `exec_workflow.rs` go to `commands/workflow_preflight.rs`; `LeaderSpec` to `data/config`. |
| F-12 | **Delete** the never-called `SandboxBackend` methods and `SandboxId`. |
| F-35 | **Delete both** `AuthCommand` and `DownloadCommand`; keep the tarball logic as a private helper of `InitCommand`. |
| F-19 | **Abort on Esc everywhere** in the `new` interviews; the TUI stops writing a file named `workflow`. |

---

## Proposed execution order

Small safe wins first, then the two refactors that unblock the rest.

1. F-12 (remove stale lint suppressions, delete dead items) — S, no dependencies
2. F-53, F-52 (docs pointers, test stubs and `Engines::for_tests`) — S
3. F-33, F-36, F-35 (trait renames, blanket proxies, dead commands and empty traits) — S/M
4. F-34 (drop dead `WorkflowEngine` fields, `PhaseKind`) — M
5. F-37, F-39, F-40, F-44, F-46, F-47 (env vars, git spawns, capability branches, small L0/L1 moves) — S each
6. F-14, F-42, F-43, F-21, F-17, F-18 (small T2 pull-downs into L2/L1) — S each
7. F-05 (`Engines::build`, `Startup`) — M; unblocks 8–9
8. F-02 then F-03 then F-04 (daemon bootstraps and squad routing into L2) — R-A, work item first
9. F-06, F-09, F-15, F-16, F-19, F-20, F-24, F-31 (TUI/CLI business logic into L2/L1) — M each; F-24 first since it shrinks the others' diffs
10. F-10 then F-26 then F-25 (catalogue-owned defaults, single parser, regenerate `cli.md`) — R-B, work item first; F-25 needs Q8/Q9
11. F-01 (`SquadAttachCommand`) — R-C, work item first; after F-04
12. F-08, F-13, F-41, F-49, F-50, F-54 (session manager, headless defaults, observer split) — M
13. F-07, F-27, F-29, F-30, F-28, F-38 (layer relocations) — R-D, after Q1
14. F-11, F-32 (container process module, agent matrix consolidation) — R-E
15. F-51, F-45, F-48 (file splits, trait shape, typed statuses) — as capacity allows
16. F-22 (Tab as SessionState view) — R-F, after Q3 and item 12
