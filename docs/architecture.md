# aspec Architecture

## High-level Overview

```
User
 │
 ▼
aspec binary ──► command mode  ──► commands/{init,ready,implement}
     │                                       │
     └──────► interactive mode (TUI)         │
                    │                        ▼
              tui/{mod,state,                docker::run_container
               input,render,pty}             docker::run_container_pty
                    │                              │
                    ▼                              ▼
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
    ready.rs               `aspec ready` — run() + run_with_sink()
    implement.rs           `aspec implement` — run() + run_with_sink()
  docker/
    mod.rs                 is_daemon_running, build_image, run_container,
                           build_run_args, build_run_args_pty
  tui/
    mod.rs                 run() entry point; event loop; action dispatcher
    state.rs               App struct; Focus/ExecutionPhase/Dialog enums
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

### `Dialog`

```
None ──[q / Ctrl+C]──────────────► QuitConfirm ──[y]──► quit
     ──[implement, cwd ≠ root]──► MountScope   ──[r/c]──► resume
     ──[implement, auth=None]───► AgentAuth    ──[y/n]──► resume
```

Dialogs intercept all key events until dismissed. The pending work item number
and mount path are preserved in `App` fields while a dialog is active.

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
- PTY output bytes are ANSI-stripped (`strip-ansi-escapes`) before being stored in
  `App::output_lines` for ratatui display. Full terminal emulation (cursor tracking,
  screen clearing) is a future enhancement.

For `init` and `ready` (no PTY needed), `spawn_text_command` runs a tokio task that
passes an `OutputSink::Channel` to `run_with_sink` and sends the exit code through
a `tokio::sync::oneshot` channel.

---

## Agent Auth Flow

```
implement submitted
        │
        ▼
  autoAgentAuthAccepted in config?
        │
   ┌────┴──────────────────┐
  None                  Some(v)
   │                       │
   ▼                  ┌────┴────┐
AgentAuth dialog    true       false
   │                 │           │
  [y]              mount      no mount
   │
  [n]──────────────────────────────► no mount
```

The decision is saved to `GITROOT/aspec/.aspec-cli.json` so the prompt only
appears once per repository.

---

## Testing Strategy

| Layer | Location | What is tested |
|-------|----------|----------------|
| Unit — per module | inline `#[cfg(test)]` | Individual functions, data structures |
| Unit — border colors | `tui::state::tests` | All 6 combinations of phase × focus |
| Unit — PTY | `tui::pty::tests` | Real `echo` and `sh -c 'exit 42'` processes |
| Integration — CLI | `tests/cli_integration.rs` | Binary-level: help, version, missing work item |
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
`find_work_item`, autocomplete, and auth functions produce the same results
regardless of which caller invokes them.
