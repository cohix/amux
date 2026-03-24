# amux Architecture

## High-level Overview

```
User
 │
 ▼
amux binary ──► command mode  ──► commands/{init,ready,implement,chat,new}
     │                                       │
     └──────► interactive mode (TUI)         │
                    │                        ▼
              tui/{mod,state,                docker::run_container
               input,render,pty}             docker::run_container_captured
                    │                        docker::build_image_streaming
                    ▼                              │
              Docker Daemon ──────────────► Managed Container
                                           (agent runs here)
```

---

## Source Layout

```
src/
  main.rs                  Entry point: dispatch TUI or command mode
  lib.rs                   Re-exports public API for integration tests
  cli.rs                   clap CLI: Cli, Command, Agent enums
  config/
    mod.rs                 RepoConfig, GlobalConfig, load/save helpers
  commands/
    mod.rs                 Public run() dispatcher
    output.rs              OutputSink: routes output to stdout or TUI channel
    auth.rs                Agent credential path resolution, auth prompts
    agent.rs               Shared agent launching: run_agent_with_sink()
                           Used by both implement and chat
    download.rs            GitHub downloads: Dockerfile templates (raw files)
                           and aspec folder (tarball extraction)
    init.rs                `amux init` — run() + run_with_sink()
                           Downloads aspec/ folder and Dockerfile templates
                           from GitHub, falls back to embedded templates
    new.rs                 `amux new` — run() + run_with_sink()
                           WorkItemKind, slugify, apply_template,
                           find_template, next_work_item_number
                           Auto-downloads aspec/ if template is missing
    ready.rs               `amux ready` — run() + run_with_sink()
                           ReadyOptions, ReadySummary, print_summary,
                           print_interactive_notice,
                           audit_entrypoint, audit_entrypoint_non_interactive
    implement.rs           `amux implement` — run() + run_with_sink()
                           agent_entrypoint, agent_entrypoint_non_interactive
    chat.rs                `amux chat` — run() + run_with_sink()
                           chat_entrypoint, chat_entrypoint_non_interactive
  docker/
    mod.rs                 is_daemon_running, image_exists, project_image_tag,
                           build_image, build_image_streaming,
                           run_container, run_container_captured,
                           build_run_args, build_run_args_pty,
                           HostSettings (sanitized config mount),
                           ContainerStats, generate_container_name,
                           query_container_stats
  tui/
    mod.rs                 run() entry point; event loop; action dispatcher
    state.rs               App struct; Focus/ExecutionPhase/Dialog enums;
                           PendingCommand (Ready/Implement/Chat with flags);
                           ContainerWindowState, ContainerInfo,
                           LastContainerSummary
    input.rs               handle_key(); Action enum; autocomplete; key→bytes
    render.rs              draw(); draw_exec_window/command_box/dialog etc.
    pty.rs                 PtySession; PtyEvent; spawn_text_command helper
templates/
  Dockerfile.claude        Bundled fallback via include_str! (debian:bookworm-slim base)
  Dockerfile.codex         Bundled fallback (debian:bookworm-slim base)
  Dockerfile.opencode      Bundled fallback (debian:bookworm-slim base)
                           Primary source: downloaded from github.com/prettysmartdev/aspec-cli
tests/
  cli_integration.rs       Binary-level integration tests
  command_tui_parity.rs    Verifies command/TUI mode share the same logic
  dockerfile_build.rs      Builds each agent template Dockerfile to verify validity
  download_integration.rs  GitHub download tests: templates, aspec folder, fallback
docs/
  usage.md                 End-user reference
  architecture.md          This file
```

---

## The `OutputSink` Abstraction

Every command function (`init::run_with_sink`, `ready::run_with_sink`, etc.) accepts
an `OutputSink` instead of calling `println!` directly:

```rust
pub enum OutputSink {
    Stdout,                               // command mode
    Channel(UnboundedSender<String>),     // TUI mode
}
```

`OutputSink` implements `Clone`, allowing it to be passed to streaming callbacks
like `docker::build_image_streaming`.

This is the core mechanism that allows zero code duplication between the two
execution modes. The command logic is identical — only the destination of output differs.

In command mode, `run()` wraps `run_with_sink(…, &OutputSink::Stdout)`.
In TUI mode, `execute_command()` passes `OutputSink::Channel(app.output_tx.clone())`.

---

## Working Directory Contract

All `run_with_sink` functions accept an explicit `cwd: &Path` parameter that
determines where the Git root is searched from. This ensures correctness for
both execution modes:

| Mode | `cwd` value | Behaviour |
|------|-------------|-----------|
| CLI (command mode) | `std::env::current_dir()` | Uses the directory where `amux` was launched |
| TUI (interactive mode) | `app.active_tab().cwd` | Uses the tab's working directory |

**Rule:** No command implementation may call `find_git_root()` (which reads the
process CWD). All callers must use `find_git_root_from(cwd)` with an explicitly
provided `cwd`. This prevents TUI tabs from accidentally operating on the wrong
repository when a tab's working directory differs from the process's launch
directory.

The `find_git_root()` helper (which reads `std::env::current_dir()`) exists only
for the CLI `run()` entry points, which call it once to determine the `cwd` to
pass down.

---

## TUI State Machine

The TUI state is split across three orthogonal enums plus the `App` struct:

### `Focus`

```
CommandBox  ←──── Esc ────── ExecutionWindow
    │                                ▲
    └─────── ↑ arrow / running ──────┘
```

### `ExecutionPhase`

```
Idle ──[Submit]──► Running ──[exit 0]──► Done
                      │
                      └──[exit ≠ 0]──► Error
```

`Done` and `Error` are both read-only scroll states. Any non-scroll key press
in the window, or any new Submit, transitions back through `Idle → Running`.

Mouse scrolling is enabled via `crossterm::EnableMouseCapture` and works in all
phases and focus states. Scroll events adjust `App::scroll_offset` by 3 lines
per tick, allowing the user to navigate output even while a process is running
and capturing keyboard input.

### `Dialog`

```
None ──[q / Ctrl+C]──────────────────────────► QuitConfirm ──[y]──► quit
     ──[ready|implement|chat, cwd ≠ root]──► MountScope   ──[r/c]──► resume
     ──[ready|implement|chat, auth=None]───► AgentAuth    ──[y/n]──► resume
     ──[new]───────────────────────────────► NewKindSelect ──[1/2/3]──► NewTitleInput ──[Enter]──► create
```

Dialogs intercept all key events until dismissed. A `PendingCommand` enum
(`Ready { refresh, non_interactive }`, `Implement { work_item, non_interactive, plan }`,
or `Chat { non_interactive, plan }`)
and the mount path are preserved in `App` fields while a dialog is active, so
the correct command resumes after the dialog is dismissed.

---

## Ready Command

The `ready` command has two modes based on the `--refresh` flag:

### Without `--refresh` (default)

1. Check Docker daemon
2. Check `Dockerfile.dev` exists (init from template if missing)
3. Check project image exists (build if missing, with streaming output)
4. Print skip message and tip about `--refresh`
5. Display summary table

### With `--refresh`

1–3: Same as above
4. Launch agent to audit `Dockerfile.dev` (interactive or non-interactive)
5. Rebuild image with updated `Dockerfile.dev` (streaming output)
6. Display summary table

### `ReadyOptions`

```rust
pub struct ReadyOptions {
    pub refresh: bool,
    pub non_interactive: bool,
}
```

Shared between command mode and TUI mode. Defaults to `refresh: false, non_interactive: false`.

### `ReadySummary`

```rust
pub struct ReadySummary {
    pub docker_daemon: StepStatus,
    pub dockerfile: StepStatus,
    pub dev_image: StepStatus,
    pub refresh: StepStatus,
    pub image_rebuild: StepStatus,
}
```

Each step status is one of `Pending`, `Ok(msg)`, `Skipped(msg)`, or `Failed(msg)`.
The summary table is rendered via `print_summary()` at the end of every ready run.

### Interactive Notice

Before launching any interactive agent (in `ready --refresh` or `implement`),
`print_interactive_notice()` displays a large ASCII-art banner alerting the user
that:
- The agent is in interactive mode
- They need to quit the agent when done

This notice is suppressed when `--non-interactive` is used.

---

## Implement Command

The `implement` command accepts a 4-digit work item number (e.g. `0001`) and
launches the configured agent to implement it. The agent receives a structured
prompt that instructs it to implement the work item, iterate on builds and tests,
write documentation, and ensure final success.

### Interactive Mode (default)

Uses `agent_entrypoint()` which launches the agent in interactive mode. An
ASCII-art interactive notice is shown before launch.

### Non-Interactive Mode (`--non-interactive`)

Uses `agent_entrypoint_non_interactive()` which adds print-mode flags:
- Claude: `-p` flag
- Codex: `--quiet` flag
- Opencode: same `run` subcommand

Output is captured via `docker::run_container_captured()` and displayed.
A tip suggests removing `--non-interactive` for direct interaction.

### Plan Mode (`--plan`)

When `--plan` is passed, the agent is initialized in read-only plan mode.
Plan flags are appended after the regular entrypoint arguments via
`append_plan_flags()`:
- Claude: `--plan`
- Codex: `--approval-mode plan`
- Opencode: no plan mode (flag is silently ignored)

`--plan` can be combined with `--non-interactive`.

Host agent settings are mounted read-only into the container via
`docker::HostSettings::prepare()`, which copies sanitized versions of
`~/.claude.json` (with `oauthAccount` stripped) and `~/.claude/settings.json`
into a temporary directory. These are mounted at `/root/.claude.json:ro` and
`/root/.claude:ro`. The temp directory is cleaned up automatically when the
`HostSettings` struct is dropped (after the container exits).

Authentication is handled entirely via the `CLAUDE_CODE_OAUTH_TOKEN` environment
variable — the host settings mount provides agent configuration (onboarding
state, model preferences, plugins) without interfering with auth.

---

## Chat Command

The `chat` command starts a freeform agent session with no pre-configured prompt.
It shares the same underlying container-launching logic as `implement` via the
`commands/agent.rs` module.

### Shared Agent Launching (`commands/agent.rs`)

The `run_agent_with_sink()` function is the shared code path for both `implement`
and `chat`. It handles:

- Git root detection and config loading
- Mount path resolution
- Docker image tag derivation
- Docker command display (with masked secrets)
- Interactive notice display
- Container launching (interactive or captured)

The only differences between `chat` and `implement` are:
- **Entrypoint**: `chat` passes just the agent command (e.g. `["claude"]`);
  `implement` passes the agent command + a structured prompt
- **Status message**: `chat` shows "Starting chat session"; `implement` shows
  the work item being implemented

### Chat Entrypoints

| Agent | Interactive | Non-Interactive | Plan (appended) |
|-------|-----------|-----------------|-----------------|
| `claude` | `["claude"]` | `["claude", "-p"]` | `["--plan"]` |
| `codex` | `["codex"]` | `["codex", "--quiet"]` | `["--approval-mode", "plan"]` |
| `opencode` | `["opencode"]` | `["opencode"]` | (none) |

---

## New Command

The `new` command creates a new work item from the `0000-template.md` template.

1. Locates the template at `GITROOT/aspec/work-items/0000-template.md`
2. Scans existing work item files to determine the next sequential number
3. Collects the work item kind (Feature/Bug/Task) and title
4. Generates a slug from the title (lowercase, spaces→hyphens, strip non-alphanumeric)
5. Writes the new file with template substitutions applied
6. Opens the file in VS Code if running in the VS Code terminal

In **command mode**, kind and title are collected via stdin prompts.
In **TUI mode**, two dialog overlays (`NewKindSelect` → `NewTitleInput`) collect
the information, then `run_with_sink` is called with the pre-supplied values.

---

## Docker Build Streaming

`docker::build_image_streaming()` spawns `docker build` and reads stdout and
stderr concurrently in separate background threads. Both threads send lines
through a shared `std::sync::mpsc` channel, and the calling thread receives
lines from the channel and forwards them to the `on_line` callback as they
arrive. This ensures real-time streaming of Docker build output — including
stderr, where Docker emits most of its build progress — rather than buffering
stderr until after stdout finishes.

The `OutputSink`'s `Clone` implementation enables passing it into the streaming
callback closure.

---

## PTY Architecture

For `implement`, the container process must have a real terminal (PTY) so that
interactive agent CLIs (Claude, Codex, etc.) work correctly.

```
App::pty (PtySession)
    │
    ├── master (Box<dyn MasterPty>)       ← held for resize()
    └── input_tx (SyncSender<Vec<u8>>)    ← TUI keypresses → writer thread
                                                           → PTY master
                                                           → container stdin

PtyEvent channel (std::sync::mpsc)
    ├── reader thread → Data(Vec<u8>)     ← PTY master → strip ANSI → output_lines
    └── wait thread   → Exit(i32)         ← child.wait() → finish_command()
```

Key design decisions:
- `master` stays on the main thread (no `Send` required); only `resize()` is called on it
- The writer (`Box<dyn Write + Send>`) is moved to a dedicated `std::thread` and communicated
  with via a bounded `std::sync::mpsc::sync_channel`
- The child (`Box<dyn Child + Send>`) is moved to a wait thread; its exit code is sent
  back via `std::sync::mpsc`
- PTY output bytes are processed for `\r` (carriage return) and `\n` (newline) from
  the raw byte stream *before* ANSI stripping, because `strip_ansi_escapes::strip`
  removes `\r` characters. A bare `\r` clears the line buffer (overwrite from start),
  `\r\n` is treated as a newline, and content segments between control characters are
  ANSI-stripped before appending. A "live line" at the end of `output_lines` is updated
  in-place until finalized by `\n`, enabling correct display of terminal spinners and
  progress indicators. Full terminal emulation (cursor tracking, screen clearing) is
  a future enhancement.

For `init` and `ready` (no PTY needed), `spawn_text_command` runs a tokio task that
passes an `OutputSink::Channel` to `run_with_sink` and sends the exit code through
a `tokio::sync::oneshot` channel.

### Dockerfile Audit (ready --refresh)

The `ready --refresh` command runs a three-phase workflow:

1. **Pre-audit** (text command via `OutputSink`): checks Docker daemon, ensures
   `Dockerfile.dev` exists, builds the image (streaming). Returns a `ReadyContext`
   with the image tag, mount path, agent name, and env vars.
2. **Audit** (interactive PTY or captured): launches the agent to scan the project
   and update `Dockerfile.dev`. In command mode with interactive: uses
   `docker::run_container()` with inherited stdio. In command mode with
   `--non-interactive`: uses `docker::run_container_captured()`. In TUI mode:
   uses a PTY session (interactive) or captured command (non-interactive).
3. **Post-audit** (text command): rebuilds the Docker image with streaming output.

Without `--refresh`, only phase 1 runs, followed by the summary table.

In TUI mode, `ReadyPhase` tracks which phase is active. When a phase completes,
`check_ready_continuation()` automatically launches the next phase.

Image tags are project-specific (`amux-{projectname}:latest`) derived from the
Git root folder name via `docker::project_image_tag()`.

### Agent Credential Passing

Agent credentials are extracted from the macOS system keychain and passed
into the container via a single environment variable:

- **`CLAUDE_CODE_OAUTH_TOKEN`**: The OAuth credential JSON (containing
  `accessToken`, `refreshToken`, `expiresAt`), passed via `-e`. Claude Code
  reads this env var on startup for authentication.

No credential files are mounted. The environment variable is the only
credential passed to the container. Host agent settings (model preferences,
onboarding state) are mounted separately via `HostSettings` — see the
Implement Command section above.

The credential extraction flow:

1. `auth::read_keychain_raw()` calls macOS `security find-generic-password`
   to read the full JSON blob from the keychain (service: `Claude Code-credentials`)
2. `auth::extract_token_from_keychain_json()` parses the JSON and extracts
   the `claudeAiOauth` inner object as a JSON string
3. The JSON is returned and passed as the `CLAUDE_CODE_OAUTH_TOKEN` env var

`auth::resolve_auth()` always returns keychain credentials (auto-passthrough)
without prompting. No opt-in dialog is needed.

`docker::append_env_args()` translates `(key, value)` pairs into
`-e KEY=VALUE` Docker flags.

For display purposes (CLI output, TUI window), `build_run_args_display()`
masks env var values as `KEY=***` to prevent accidental secret exposure.

### Docker Command Visibility

Every `docker build` and `docker run` invocation is formatted as a CLI string
via `docker::format_build_cmd()` / `docker::format_run_cmd()` and printed
through the `OutputSink` before execution. In command mode this appears on
stdout; in TUI mode it appears in the execution window output.

---

## Container Window

When `implement`, `chat`, or `ready --refresh` launches an interactive agent, the TUI
displays a dedicated **container window** overlaying the outer execution window.

### State Machine

```
Hidden ──[start_container()]──► Maximized ──[Esc]──► Minimized ──['c']──► Maximized
                                     │                    │
                                     └────[finish]────────┘──► Hidden + Summary bar
```

`ContainerWindowState` is an enum with three variants: `Hidden`, `Maximized`,
and `Minimized`. The state transitions are:

- **Hidden → Maximized**: `start_container()` is called when an agent launches.
  It sets the container name, agent display name, start time, and initializes
  the stats channel receiver.
- **Maximized → Minimized**: User presses `Esc`. The outer window becomes
  visible and scrollable while the container continues running in the background.
  A 1-line green-bordered bar shows the agent name and live stats.
- **Minimized → Maximized**: User presses `c`. The container window re-overlays
  the outer window and keyboard input is forwarded to the container again.
- **Maximized/Minimized → Hidden**: `finish_command()` transitions the container
  window to `Hidden` and generates a `LastContainerSummary` with average CPU,
  peak memory, and total runtime.

### Layout

When **maximized**, the container window covers 95% of the outer execution
window's width and height, centered. It has a green border with:
- Left title: `🔒 {agent} (containerized)` (e.g. `🔒 Claude Code (containerized)`)
- Right title: `{container_name} | CPU {cpu}% | Mem {mem}MB | {runtime}`

When **minimized**, a 1-line bar with green border appears between the outer
execution window and the command box, showing agent name and live stats.

After the container **exits**, a summary bar with dashed border shows:
`{agent} exited | avg CPU {cpu}% | peak mem {mem}MB | runtime {duration}`

### Container Scrollback

When the container window is maximized, the mouse scroll wheel allows the user
to scroll through the vt100 terminal's scrollback buffer, viewing recent output
that has scrolled off the screen. This is implemented using the vt100 crate's
`set_scrollback()` API:

- **Scroll up**: increases `container_scroll_offset`, which calls
  `parser.set_scrollback(offset)` to shift the view into scrollback
- **Scroll down**: decreases the offset; at 0 the live screen is shown
- **Indicator**: a centered yellow title ("↑ scrollback (N lines up)") appears
  on the container border when scrolled

The maximum scroll depth is capped at the screen row count due to a limitation
in the vt100 crate's `set_scrollback()` implementation (its internal
`visible_rows()` iterator underflows when offset > rows_len). The cursor is
hidden when viewing scrollback. Scrollback state resets to 0 when a new
container starts.

### PTY Output Routing

PTY output bytes are routed to different line buffers depending on the container
window state:

- **Container window active** (`Maximized` or `Minimized`): PTY data goes to
  `container_output_lines`, displayed inside the container window.
- **Container window hidden**: PTY data goes to `output_lines`, displayed in
  the outer execution window (original behavior).

The routing decision is made in `process_pty_data()` using `pty_uses_container()`,
which returns `true` when `container_window` is not `Hidden`. This avoids a
mutable borrow conflict by returning a boolean flag instead of a mutable
reference to the target buffer.

### Docker Stats Polling

When a container starts, `spawn_stats_poller()` creates a tokio task that polls
Docker stats every 5 seconds:

```
tokio::spawn ──► loop {
    interval.tick().await           (5s)
    spawn_blocking(query_container_stats)
    tx.send(stats)
}
```

`query_container_stats()` runs `docker stats --no-stream --format` and parses
the JSON output into a `ContainerStats` struct (name, cpu_percent, memory).
The stats are sent via `tokio::sync::mpsc::unbounded_channel` and drained in
`App::tick()` each render cycle.

Each polled stats snapshot is appended to `ContainerInfo::stats_history` for
computing averages and peaks when the container exits.

### Container Naming

`generate_container_name()` produces a deterministic name (`amux-{pid}-{nanos}`)
passed to `docker run --name`. This allows `query_container_stats()` to query
stats for the specific container by name.

---

## Agent Auth Flow

```
ready/implement submitted
        │
        ▼
  autoAgentAuthAccepted in config?
        │
   ┌────┴──────────────────┐
  None                  Some(v)
   │                       │
   ▼                  ┌────┴────┐
Auto-passthrough:
   read_keychain_raw() → extract OAuth JSON → CLAUDE_CODE_OAUTH_TOKEN env var
```

Credentials are always sourced from the macOS system keychain and passed
automatically (no opt-in dialog needed).

---

## Testing Strategy

| Layer | Location | What is tested |
|-------|----------|----------------|
| Unit — per module | inline `#[cfg(test)]` | Individual functions, data structures |
| Unit — border colors | `tui::state::tests` | All 6 combinations of phase × focus |
| Unit — PTY data | `tui::state::tests` | `\r`/`\n`/`\r\n` processing, live-line updates |
| Unit — container window | `tui::state::tests` | Container state transitions, PTY routing, summary generation |
| Unit — container render | `tui::render::tests` | Container window overlay, minimized bar, summary bar |
| Unit — container input | `tui::input::tests` | Key handling in maximized/minimized/hidden states |
| Unit — docker build streaming | `docker::tests` | Incremental line delivery, stderr capture, failure handling |
| Unit — docker stats | `docker::tests` | Stats parsing, container name generation |
| Unit — PTY | `tui::pty::tests` | Real `echo` and `sh -c 'exit 42'` processes |
| Unit — ready | `commands::ready::tests` | Summary table, interactive notice, options, entrypoints |
| Unit — implement | `commands::implement::tests` | Entrypoints (interactive + non-interactive) |
| Unit — chat | `commands::chat::tests` | Entrypoints, no-prompt verification |
| Unit — agent | `commands::agent::tests` | Shared agent launching |
| Unit — new | `commands::new::tests` | Slugify, numbering, template, find_template, kind parsing, run_with_sink |
| Integration — CLI | `tests/cli_integration.rs` | Binary-level: help, version, flags, work items |
| Integration — parity | `tests/command_tui_parity.rs` | Shared logic between command/TUI modes, container lifecycle, tab-cwd correctness |
| Unit — download | `commands::download::tests` | Tarball extraction, file counting, empty tarball error |
| Integration — download | `tests/download_integration.rs` | GitHub template downloads, aspec folder download, init integration, fallback |
| Integration — Docker | `tests/dockerfile_build.rs` | Builds each agent template Dockerfile to verify validity |

### Window Border Color Matrix

| Phase | Focus | Color |
|-------|-------|-------|
| Running | ExecutionWindow (selected) | Blue |
| Running | CommandBox (unselected) | Grey |
| Done | ExecutionWindow (selected) | Green |
| Done | CommandBox (unselected) | Grey |
| Error | ExecutionWindow (selected) | Red |
| Error | CommandBox (unselected) | Red |
| Idle | any | DarkGray |

The parity tests are the most important: they verify that `run_with_sink`,
`find_work_item`, autocomplete, auth functions, summary table, interactive notice,
and non-interactive entrypoints produce the same results regardless of which
caller invokes them.
