# aspec Architecture

## High-level Overview

```
User
 │
 ▼
aspec binary ──► command mode  ──► commands/{init,ready,implement,new}
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
    init.rs                `aspec init` — run() + run_with_sink()
    new.rs                 `aspec new` — run() + run_with_sink()
                           WorkItemKind, slugify, apply_template,
                           find_template, next_work_item_number
    ready.rs               `aspec ready` — run() + run_with_sink()
                           ReadyOptions, ReadySummary, print_summary,
                           print_interactive_notice,
                           audit_entrypoint, audit_entrypoint_non_interactive
    implement.rs           `aspec implement` — run() + run_with_sink()
                           agent_entrypoint, agent_entrypoint_non_interactive
  docker/
    mod.rs                 is_daemon_running, image_exists, project_image_tag,
                           build_image, build_image_streaming,
                           run_container, run_container_captured,
                           build_run_args, build_run_args_pty
  tui/
    mod.rs                 run() entry point; event loop; action dispatcher
    state.rs               App struct; Focus/ExecutionPhase/Dialog enums;
                           PendingCommand (Ready/Implement with flags)
    input.rs               handle_key(); Action enum; autocomplete; key→bytes
    render.rs              draw(); draw_exec_window/command_box/dialog etc.
    pty.rs                 PtySession; PtyEvent; spawn_text_command helper
templates/
  Dockerfile.claude        Embedded via include_str! into init.rs
  Dockerfile.codex
  Dockerfile.opencode
tests/
  cli_integration.rs       Binary-level integration tests
  command_tui_parity.rs    Verifies command/TUI mode share the same logic
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
None ──[q / Ctrl+C]───────────────────► QuitConfirm ──[y]──► quit
     ──[ready|implement, cwd ≠ root]──► MountScope   ──[r/c]──► resume
     ──[ready|implement, auth=None]───► AgentAuth    ──[y/n]──► resume
     ──[new]──────────────────────────► NewKindSelect ──[1/2/3]──► NewTitleInput ──[Enter]──► create
```

Dialogs intercept all key events until dismissed. A `PendingCommand` enum
(`Ready { refresh, non_interactive }` or `Implement { work_item, non_interactive }`)
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

The host's Claude settings (`~/.claude/` and `~/.claude.json`) are mounted into
the container at `/root/.claude` and `/root/.claude.json` when they exist, so
Claude inherits the user's existing configuration and skips first-run setup.

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

`docker::build_image_streaming()` spawns `docker build` and reads stdout/stderr
line-by-line, calling an `on_line` callback for each line. This replaces the
buffered `build_image()` function for user-facing builds, so output appears in
real time instead of appearing all at once after the build completes.

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

Image tags are project-specific (`aspec-{projectname}:latest`) derived from the
Git root folder name via `docker::project_image_tag()`.

### Agent Credential Passing

Agent credentials are passed into the container as environment variables
(e.g. `-e ANTHROPIC_API_KEY=…`), not as file mounts. This is necessary
because Claude Code stores its OAuth tokens in the OS keychain, not in
filesystem files like `~/.claude` or `~/.claude.json`.

There are two credential sources, selected by the `--auth-from-env` flag:

- **Default (keychain)**: `auth::agent_keychain_vars()` calls macOS
  `security find-generic-password` to extract the OAuth access token from the
  keychain, then sets it on both `ANTHROPIC_API_KEY` and
  `CLAUDE_CODE_OAUTH_TOKEN` so that Claude Code picks it up regardless of
  which env var it checks. The Anthropic SDK auto-detects OAuth tokens by
  their `sk-ant-oat` prefix. The `auth::resolve_auth()` dispatcher checks
  the saved preference in `aspec/.aspec-cli.json` and prompts on first use.

- **`--auth-from-env`**: `auth::agent_env_vars()` reads the API key directly
  from host environment variables. No prompt is shown — credentials are passed
  unconditionally if the env var is set.

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

## Agent Auth Flow

```
ready/implement submitted
        │
        ▼
  --auth-from-env flag?
        │
   ┌────┴──────────────────┐
  yes                      no
   │                        │
   ▼                        ▼
read env var              autoAgentAuthAccepted in config?
(ANTHROPIC_API_KEY or            │
 CLAUDE_CODE_OAUTH_TOKEN)   ┌────┴──────────────────┐
   │                       None                  Some(v)
   ▼                        │                       │
pass to container           ▼                  ┌────┴────┐
                     AgentAuth dialog        true       false
                        │                     │           │
                       [y]          read keychain →    no auth
                        │           CLAUDE_CODE_OAUTH_TOKEN
                       [n]──────────────────────────────► no auth
```

The decision is saved to `GITROOT/aspec/.aspec-cli.json` so the prompt only
appears once per repository.

---

## Testing Strategy

| Layer | Location | What is tested |
|-------|----------|----------------|
| Unit — per module | inline `#[cfg(test)]` | Individual functions, data structures |
| Unit — border colors | `tui::state::tests` | All 6 combinations of phase × focus |
| Unit — PTY data | `tui::state::tests` | `\r`/`\n`/`\r\n` processing, live-line updates |
| Unit — PTY | `tui::pty::tests` | Real `echo` and `sh -c 'exit 42'` processes |
| Unit — ready | `commands::ready::tests` | Summary table, interactive notice, options, entrypoints |
| Unit — implement | `commands::implement::tests` | Entrypoints (interactive + non-interactive) |
| Unit — new | `commands::new::tests` | Slugify, numbering, template, find_template, kind parsing, run_with_sink |
| Integration — CLI | `tests/cli_integration.rs` | Binary-level: help, version, flags, work items |
| Integration — parity | `tests/command_tui_parity.rs` | Shared logic between command/TUI modes |

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
