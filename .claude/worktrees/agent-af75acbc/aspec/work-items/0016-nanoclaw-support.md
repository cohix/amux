# Work Item: Feature

Title: nanoclaw support
Issue: issuelink

## Summary:
- allow the user to configure a machine-global installation of nanoclaw to run persistent background agents

## User Stories

### User Story 1:
As a: user

I want to:
run `aspec claws ready` to be guided through setting up nanoclaw within a container

So I can:
run persistent secure agents on my machine


## Implementation Details:

### Command structure
- `aspec claws` is a new top-level command (parent for all claw agent subcommands); on its own it does nothing.
- `aspec claws ready` is the first subcommand, implemented in `src/commands/claws.rs`.
  - CLI entry point: `commands::claws::run(ClawsAction::Ready)` → `run_claws_ready(&OutputSink::Stdout)`.
  - TUI entry point: `PendingCommand::ClawsReady` dispatches into the same `run_claws_ready` logic via a spawned async task.

### Installation path
nanoclaw is installed at **`$HOME/.nanoclaw`** (not `/usr/local/nanoclaw` as originally planned).

Rationale: Docker Desktop on macOS only file-shares paths under `/Users` by default; `/usr/local` is outside that scope and the bind mount would silently fail. Using `$HOME/.nanoclaw` keeps the host path and the in-container path identical on all platforms.

The path is computed by `nanoclaw_path()` / `nanoclaw_path_str()` (falls back to `/root/.nanoclaw` when `$HOME` is unset).

### Config file
`$HOME/.nanoclaw/.aspec.json` stores `{ "nanoclawContainerID": "<id>" }` (key is camelCase for JSON, snake_case in Rust via `serde rename`). Helpers: `load_nanoclaw_config()` / `save_nanoclaw_config()`.

### First-run wizard (when `$HOME/.nanoclaw` does not exist)

Steps in order, as implemented:

1. **Fork check** — ask if the user has already forked nanoclaw.
   - **Yes** → prompt for GitHub username → confirm → `clone_nanoclaw(username)` (tries SSH then HTTPS).
   - **No** → explain `gh repo fork` command and offer to run it via `fork_and_clone_nanoclaw()`.
     - If the user declines, bail with manual instructions.
     - If `gh` is not installed or fails, bail with an error (no offer to install `gh`; the user must install it themselves).
   - On `CloneOutcome::PermissionDenied` from either path, offer to retry with sudo (`clone_nanoclaw_sudo` / `fork_and_clone_nanoclaw_sudo`).
   - After any successful clone, call `chmod_nanoclaw_permissive()` (`chmod -R u+rwX`).

2. **`/setup` explanation** — before any image build or container launch, warn the user they must run `/setup` inside the agent after first launch. Require explicit acceptance to proceed.

3. **Image build + audit (Phase 1)** via `build_nanoclaw_image()`:
   - Check Docker daemon.
   - Ensure `Dockerfile.dev` exists in the nanoclaw repo (reuses `write_dockerfile`).
   - Build initial `aspec-nanoclaw:latest` image.
   - Run the Dockerfile.dev audit agent in the foreground **without** Docker socket access (audit only needs to read/write `Dockerfile.dev`).
   - Rebuild `aspec-nanoclaw:latest` with the audit-updated `Dockerfile.dev`.

4. **Docker socket warning** — after the image is built, warn the user that the nanoclaw container will be mounted to the host Docker socket. Require explicit acceptance.

5. **Container launch (Phase 2)** via `launch_nanoclaw_container()`:
   - Start the container detached (`docker run -d`) with:
     - Bind mount: `$HOME/.nanoclaw` on host → `$HOME/.nanoclaw` in container (direct bind mount; **no Docker named volume**).
     - Docker socket mounted (`--allow-docker` equivalent).
   - Wait up to 5 seconds for the container to reach running state.
   - Persist the container ID to `.aspec.json`.

6. **Attach** via `attach_to_nanoclaw()`:
   - Runs `docker exec` with the chat entrypoint (no premade prompt — user interacts directly to run `/setup`).
   - On ctrl+c, the container continues running in the background; aspec exits cleanly.

### Subsequent runs (when `$HOME/.nanoclaw` already exists)

1. Read `.aspec.json` for the saved container ID.
2. If a container ID exists and Docker reports it running → show the summary table; done.
3. If not running (or no saved ID):
   - Check Docker daemon is running (bail if not).
   - Offer to start the container.
   - If accepted: start detached (same flags as first run), wait 5 s, persist new container ID, attach.
   - If declined: mark container as `Skipped` in summary.

### TUI dialogs (parity with CLI)
All user confirmations and inputs use popup dialogs in the TUI, matching numbered-list prompts in the CLI:

| Dialog variant | Purpose |
|---|---|
| `ClawsReadyHasForked` | Fork check (yes/no) |
| `ClawsReadyUsernameInput` | GitHub username entry |
| `ClawsReadySetupExplain` | `/setup` explanation accept |
| `ClawsReadyDockerSocketWarning` | Docker socket warning accept |
| `ClawsReadySudoConfirm` | Collect sudo password for retry (TUI); CLI uses natural sudo prompt |
| `ClawsReadyOfferStart` | Subsequent run: offer to start stopped container |

### Subprocess output routing
`stream_child_output()` wraps `std::process::Child` to pipe stdout/stderr through `OutputSink`, ensuring all git/gh subprocess output appears in the TUI execution window rather than leaking to the raw terminal.

### Reuse from existing code
- `docker::is_daemon_running`, `docker::run_container_detached`, `docker::build_image_streaming`, `docker::run_container_at_path`, `docker::run_container_captured_at_path`, `docker::build_exec_args_pty`
- `commands::auth::agent_keychain_credentials`
- `commands::chat::chat_entrypoint`
- `commands::ready::audit_entrypoint`, `StepStatus`
- `commands::init::write_dockerfile`
- `config::load_repo_config`


## Edge Case Considerations:
- **Permission denied on clone/move**: detected via `is_nanoclaw_parent_permission_denied()` (probe file creation); offers sudo retry.
- **SSH clone fallback**: tries SSH first, falls back to HTTPS automatically.
- **`gh` not installed**: fails with a clear error; no offer to install `gh` automatically.
- **Container start timeout**: waits 5 seconds via `wait_for_container()`; bails with a clear error.
- **Error messages**: printed in red (`\x1b[31m...\x1b[0m`) with retry offers where applicable.
- **TUI/CLI parity**: all wizard steps have equivalent popup dialogs (TUI) and numbered-list prompts (CLI).

## Test Considerations:
- Unit tests in `claws.rs`: `CloneOutcome` variants, `NanoclawConfig` serialization/deserialization, `ClawsSummary` defaults, `print_claws_summary` table output, `attach_to_nanoclaw` uses `chat_entrypoint` (not audit prompt).
- Integration tests: first-run wizard flow, subsequent-run flow (container running vs. stopped), permission-denied sudo retry paths.
- TUI/CLI parity: each dialog variant maps to the corresponding CLI prompt.

## Codebase Integration:
- New module: `src/commands/claws.rs`.
- `src/commands/mod.rs`: `pub mod claws`.
- `src/cli.rs`: `Command::Claws { action: ClawsAction }`, `ClawsAction::Ready`.
- `src/tui/state.rs`: `Dialog` variants for claws wizard steps, `PendingCommand::ClawsReady`, `ClawsPhase`.
- `src/tui/input.rs`, `src/tui/render.rs`, `src/tui/mod.rs`: claws wizard dialog handling and rendering.
- `docs/usage.md`: `aspec claws ready` documented.
