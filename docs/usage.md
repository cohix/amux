# aspec Usage Guide

## Overview

`aspec` is a CLI tool for managing predictable, secure agentic coding environments.
Every agent action runs inside a Docker container — never directly on your machine.

---

## Installation

```sh
make install          # builds and installs to /usr/local/bin/aspec
# or specify a different path:
INSTALL_PATH=~/bin make install
```

---

## Execution Modes

### Interactive Mode (TUI)

Running `aspec` with no arguments opens the interactive REPL:

```sh
aspec
```

The TUI displays a persistent command input box at the bottom of the screen.
Type any subcommand and press **Enter** to run it. Suggestions appear as you type.

### Command Mode

Running `aspec` with a subcommand executes it and exits immediately:

```sh
aspec init
aspec ready
aspec ready --refresh
aspec ready --build
aspec ready --build --no-cache
aspec implement 0001
aspec implement 0001 --plan
aspec chat
aspec chat --plan
aspec new
```

---

## Subcommands

### `aspec init [--agent=<name>]`

Initialises the current Git repository for use with `aspec`.

- Detects the Git root directory
- Writes `aspec/.aspec-cli.json` (repository config)
- Downloads the `aspec/` folder from GitHub (`github.com/cohix/aspec`) if it does
  not already exist in the Git root. This provides the project specification
  templates, work item template, and architecture documents.
- Downloads and writes `Dockerfile.dev` from the agent template hosted on GitHub
  (`github.com/cohix/aspec-cli`). Falls back to the bundled template if the
  download fails (e.g. no network). Only writes if one does not already exist.
- Logs download URLs, file sizes, and destinations for all downloaded content

**Flags**

| Flag | Values | Default |
|------|--------|---------|
| `--agent` | `claude`, `codex`, `opencode` | `claude` |

**Example**

```sh
aspec init --agent=claude
```

---

### `aspec ready [--refresh] [--build] [--no-cache] [--non-interactive]`

Checks that your environment is ready for agentic development.

1. Verifies the Docker daemon is running
2. Checks that `Dockerfile.dev` exists — if missing, initialises it from the
   agent template (same as `init`) and **always rebuilds the image** (even if
   one with the correct name already exists)
3. Checks for an existing `aspec-{projectname}:latest` image — builds one if
   it does not exist yet (with streaming output)
4. Presents a summary table showing the status of each step

When `--refresh` is passed, `ready` also runs the Dockerfile agent audit:

4. Launches a container with the configured code agent to scan the project
   and update `Dockerfile.dev` with any missing build/test tools
5. Rebuilds the image with the updated `Dockerfile.dev`

Without `--refresh`, the audit is skipped and a tip is shown suggesting its use.

When `--build` is passed (without `--refresh`), `ready` forces a rebuild of the
dev container image from the current `Dockerfile.dev`, even if an image already
exists. This is useful when you have manually edited the Dockerfile and want a
fresh image without re-running the agent audit.

When `--no-cache` is passed, `docker build` is invoked with `--no-cache`,
forcing Docker to rebuild every layer from scratch. This flag applies to all
build operations — whether triggered by `--build`, `--refresh`, or a missing
image. Combine with `--build` to get a completely fresh image.

The image tag is derived from the Git root folder name (e.g. `aspec-myapp:latest`).

Before launching the audit container, `ready` applies the same mount scope and
agent authentication flow as `implement` (see [Agent Auth](#agent-authentication)).

**Flags**

| Flag | Description |
|------|-------------|
| `--refresh` | Run the Dockerfile agent audit (skipped by default) |
| `--build` | Force rebuild the dev container image (ignored if `--refresh` is also set) |
| `--no-cache` | Pass `--no-cache` to `docker build` for all build operations |
| `--non-interactive` | Run the agent in print/non-interactive mode |

**Flag interactions**

- `--refresh` + `--build`: `--build` is ignored because `--refresh` always
  rebuilds the image after the agent audit completes.
- `--refresh` + `--no-cache`: `--no-cache` is applied to the post-audit
  image rebuild.
- `--build` + `--no-cache`: The image is rebuilt without Docker layer caching.

**Docker Build Output**

All Docker build commands stream their output line-by-line as they run, so you
can see progress in real time instead of waiting for the build to complete.

**Summary Table**

At the end of every `ready` run, a summary table is displayed showing the status
of each step:

```
┌──────────────────────────────────────────────────┐
│                  Ready Summary                   │
├───────────────────┬──────────────────────────────┤
│    Docker daemon  │ ✓ running                    │
│    Dockerfile.dev │ ✓ exists                     │
│         Dev image │ ✓ exists                     │
│   Refresh (audit) │ – use --refresh to run       │
│     Image rebuild │ – no refresh                 │
└───────────────────┴──────────────────────────────┘
```

**Examples**

```sh
aspec ready                                # quick check — skips audit
aspec ready --refresh                      # full check with Dockerfile audit
aspec ready --refresh --non-interactive    # audit in non-interactive mode
aspec ready --build                        # rebuild image from current Dockerfile.dev
aspec ready --build --no-cache             # full rebuild without Docker cache
aspec ready --refresh --no-cache           # audit + rebuild without cache
```

---

### `aspec implement <NNNN> [--non-interactive] [--plan]`

Launches the dev container to implement a work item.

```sh
aspec implement 0001    # implements aspec/work-items/0001-*.md
aspec implement 0003    # implements aspec/work-items/0003-*.md
```

The work item number is a 4-digit identifier (e.g. `0001`). Both `0001` and
`1` are accepted as input.

- Finds the matching work item file in `aspec/work-items/`
- Prompts to confirm the Docker mount scope (Git root vs CWD) on first run
- Optionally passes agent credentials via environment variable (see [Agent Auth](#agent-authentication))
- Launches a container with the configured agent

**Flags**

| Flag | Description |
|------|-------------|
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--plan` | Run the agent in plan mode (read-only, no file modifications) |

**Interactive Mode (default)**

By default, the agent launches in **interactive mode**. Before the agent starts,
a large ASCII-art notice is displayed informing you that:

- The agent is launching in interactive mode
- You will need to quit the agent (via Ctrl+C or exit) when its work is complete

When Claude is the configured agent, the container starts an interactive Claude
session. The initial prompt instructs Claude to implement the work item, iterate
on builds and tests, write documentation, and ensure final success. After the
initial prompt, you can interact with Claude directly — type follow-up
instructions, review output, and guide the implementation just as you would in
a normal terminal session.

In **command mode**, the container's stdin/stdout/stderr are fully connected to
your terminal. In **TUI mode**, the execution window acts as a full terminal
emulator: all keyboard input (including arrow keys, Ctrl+O, and other shortcuts)
is forwarded to the running agent process.

**Non-Interactive Mode (`--non-interactive`)**

When `--non-interactive` is passed, the agent runs in print/batch mode:

- Claude: uses `-p` flag (print mode)
- Codex: uses `--quiet` flag
- Opencode: uses `run` subcommand (same as interactive)

The agent's output is captured and displayed. No user interaction is required.

**Plan Mode (`--plan`)**

When `--plan` is passed, the agent runs in read-only plan mode — it can analyse
code and suggest changes, but cannot modify files. How plan mode is activated
depends on the agent:

| Agent | Plan mode flag |
|-------|----------------|
| Claude | `--plan` |
| Codex | `--approval-mode plan` |
| Opencode | Not supported (flag is silently ignored) |

`--plan` can be combined with `--non-interactive`.

---

### `aspec chat [--non-interactive] [--plan]`

Starts a freeform chat session with the configured agent in a container.

Unlike `implement`, which sends an initial prompt to the agent, `chat` launches
the agent with no pre-configured prompt — giving you a clean interactive session.

```sh
aspec chat                      # start interactive chat
aspec chat --non-interactive    # start in non-interactive mode
aspec chat --plan               # start in plan mode (read-only)
```

- Prompts to confirm the Docker mount scope (Git root vs CWD) if needed
- Passes agent credentials via environment variable (see [Agent Auth](#agent-authentication))
- Launches a container with the configured agent (no initial prompt)

**Flags**

| Flag | Description |
|------|-------------|
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--plan` | Run the agent in plan mode (read-only, no file modifications) |

**Interactive Mode (default)**

By default, the agent launches in **interactive mode**. This is identical to
`implement`, but without an initial prompt — you start with a blank slate and
can type any instructions or questions directly.

In **command mode**, the container's stdin/stdout/stderr are fully connected to
your terminal. In **TUI mode**, the container window opens with full keyboard
passthrough, just like `implement`.

**Non-Interactive Mode (`--non-interactive`)**

When `--non-interactive` is passed, the agent launches in print/batch mode
(Claude uses `-p`, Codex uses `--quiet`). Since there is no initial prompt,
the agent reads from stdin in non-interactive mode.

**Plan Mode (`--plan`)**

Same as `implement --plan` — see the [implement](#aspec-implement-nnnn---non-interactive---plan)
section for details on how each agent's plan mode is activated.

**Shared Implementation**

`chat` and `implement` share the same underlying container-launching code
(`commands/agent.rs`). The only difference is:
- `implement` passes the work item implementation prompt as the agent entrypoint
- `chat` passes no prompt — just the agent command itself

---

### `aspec new`

Creates a new work item from the template (`aspec/work-items/0000-template.md`).

1. Scans the `aspec/work-items/` directory to determine the next sequential number
2. Prompts for the work item type: **Feature**, **Bug**, or **Task**
3. Prompts for a title
4. Creates a new file using the naming pattern `XXXX-title-of-item.md`
5. Replaces the template's header and title lines with the user's choices
6. If running inside a VS Code terminal, opens the new file in the editor

**In TUI mode**, the type and title are collected via dialog overlays instead of
stdin prompts.

**Filename generation**: The title is lowercased, spaces are replaced with
hyphens, and all non-alphanumeric characters (except hyphens) are removed.

**Edge case**: If no template is found in the current Git root, `aspec` will
automatically download the `aspec/` folder from GitHub (`github.com/cohix/aspec`)
to provide the template. If the download also fails, an error message is displayed.

**Example**

```sh
aspec new
# Select work item type:
#   1) Feature
#   2) Bug
#   3) Task
# Choice [1/2/3]: 1
# Work item title: Add user authentication
# Created work item: /path/to/repo/aspec/work-items/0007-add-user-authentication.md
```

---

## Interactive TUI Reference

### Layout

The TUI has two types of windows:

- **Outer window** — displays plain-text streaming output from commands (Docker
  builds, status messages, etc.)
- **Container window** — appears as an overlay whenever a Docker container runs
  a coding agent. Optimized for interactive experiences (TUI apps, spinners,
  carriage returns, full-screen output).

```
┌─── ● running: implement 0001 ────────────────────────────┐
│ $ docker run --rm -it --name aspec-12345 ...               │
│ ╔══════════════════════════════════════════════════╗        │
│ ║  Agent 'claude' is launching in INTERACTIVE mode ║        │
│ ╚══════════════════════════════════════════════════╝        │
│                                                             │
│ ╭─ 🔒 Claude Code (containerized) ── name | 5% | 200mb ──╮│
│ │                                                          ││
│ │ [Interactive agent output here...]                       ││
│ │                                                          ││
│ ╰──────────────────────────────────────────────────────────╯│
└─────────────────────────────────────────────────────────────┘
 Press Esc to minimize the container window
┌─── command (inactive) ───────────────────────────────────────┐
│ > _                                                           │
└──────────────────────────────────────────────────────────────┘
  init  ·  ready  ·  implement  ·  chat  ·  new
```

### Container Window

Whenever `aspec` launches a Docker container to run a coding agent (via
`implement` or `ready --refresh`), a **container window** appears overlaying
95% of the outer window's width and height, centered. This window is dedicated
to the interactive agent session.

**Visual indicators:**

- Green border (rounded)
- Title bar (top left): `🔒 Claude Code (containerized)`
- Title bar (top right): `container_name | CPU% | memory | runtime`

Docker stats (container name, CPU, memory, total runtime) are polled from the
Docker daemon every 5 seconds and displayed in the title bar.

**When the container window is maximized (default):**

- All keyboard input is forwarded to the running container process
- Arrow keys, Ctrl+C, Ctrl+O, and all other shortcuts work natively
- **Mouse scroll wheel** scrolls through the container's terminal scrollback
  history, allowing you to review recent output from the agent. A centered
  yellow indicator ("scrollback (N lines up)") appears in the title bar when
  scrolled up. Scroll back to the bottom to return to the live view.
- Press **Esc** to minimize the container window
- A status hint shows: "Esc minimize  ·  scroll ↕ history"

**When the container window is minimized:**

The container collapses to a 1-line green-bordered bar below the outer window,
showing the agent name and Docker stats. The outer window becomes fully visible
for reading its output.

| Key | Action |
|-----|--------|
| **↑** / **↓** | Scroll the outer window |
| **b** / **e** | Jump to beginning/end of outer window |
| **c** | Restore the container window to maximized |
| **Esc** | Return focus to the command text box |

When the command text box is focused, pressing **↑** focuses the outer window
(not the container window), regardless of whether the container is minimized
or maximized.

**When the container process exits:**

The container window closes and focus returns to the command box. A summary
bar appears below the outer window with a dashed-line border showing:

- Agent used, container name, average CPU, average memory, total time, exit code

This summary persists until a new container is launched.

### Command Box

| Key | Action |
|-----|--------|
| Type | Update command, show autocomplete suggestions |
| **Enter** | Execute command |
| **Shift+Enter** | Insert newline (multi-line input) |
| **←** / **→** | Move cursor |
| **↑** | Focus the execution window (for scrolling) |
| **Backspace** / **Delete** | Edit input |
| **q** (on empty input) | Show quit confirmation |
| **Ctrl+C** | Show quit confirmation |

### Outer Execution Window

The outer window displays plain-text streaming output (Docker builds, status
messages, command output). It is optimized for simple text — ANSI escape
sequences are stripped and content is rendered as plain lines.

| State | Focus | Border colour |
|-------|-------|--------------|
| Running | Selected | Blue |
| Running | Unselected | Grey |
| Done (success) | Selected | Green |
| Done (success) | Unselected | Grey |
| Done (error) | Selected | Red |
| Done (error) | Unselected | Red |

When the window is **selected while running** (blue border, no container
window active):

- All keypresses are forwarded directly to the running process
- Arrow keys, typing, and keyboard shortcuts (e.g. Ctrl+O, Ctrl+C) all work
  as they would in a normal terminal
- Use the mouse scroll wheel to scroll output at any time, even while the
  process is capturing keyboard input
- Press **Esc** to deselect and return focus to the command box
- A hint below the window reminds you: "Press Esc to deselect the window"

When the window is **selected after completion** (green or red border):

- **↑** / **↓** scroll through the full output — available for both success and error
- **Mouse scroll wheel** also scrolls the output
- Press **Esc** to return focus to the command box
- A hint below shows: "Press Esc to deselect  ·  ↑/↓ to scroll"

When the window is **unselected** (grey or red border):

- Press **↑** from the command box to focus the window for scrolling
- **Mouse scroll wheel** scrolls output regardless of focus
- A hint below the window reminds you: "Press ↑ to focus the window"
- Error exit codes remain visible in red even when the window is unselected

### Autocomplete

As you type, aspec shows suggestions below the command box:

```
ready
  ready
  init  ·  ready  ·  implement  ·  chat  ·  new

init --
  init --agent=claude  ·  init --agent=codex  ·  init --agent=opencode

ready --
  ready --refresh  ·  ready --build  ·  ready --no-cache  ·  ready --build --no-cache  ·  ready --non-interactive  ·  ready --refresh --non-interactive

implement --
  implement <NNNN>  e.g. implement 0001  ·  implement <NNNN> --non-interactive  ·  implement <NNNN> --plan

chat --
  chat  (start a freeform agent session)  ·  chat --non-interactive  ·  chat --plan
```

### Unknown Commands

If you type a command that is not an aspec subcommand, the error message
includes the closest known subcommand:

```
'implemnt' is not an aspec command.  Did you mean: implement
```

### Quit Confirmation

Press **q** or **Ctrl+C** when the command box is focused to open the confirmation dialog:

```
╭─── Quit aspec? ──────────────────╮
│  Are you sure you want to quit?   │
│  [y/n]                            │
╰───────────────────────────────────╯
```

Press **y** to quit, **n** or **Esc** to cancel.

---

## Agent Authentication

When running `implement` or `ready`, aspec can pass your agent's credentials
into the container so the agent is pre-authenticated — you won't have to log in
manually each time.

### Authentication methods

There are two ways to provide credentials:

**1. System keychain (default)**

By default, aspec reads the agent's OAuth access token directly from the
system keychain. On first use per repository, aspec asks for your permission:

```
Pass agent credentials (from system keychain) into container?
This will be saved for this repo. [y/n]
```

- **y** — the OAuth token is extracted from the keychain and passed into
  the container; the decision is saved in `aspec/.aspec-cli.json`
  (`"autoAgentAuthAccepted": true`)
- **n** — no credentials passed; you will need to authenticate inside
  the container manually

The decision is stored per Git repository and only asked once.

### How credentials are passed

When keychain auth is accepted, aspec:

1. Reads the OAuth credentials JSON from the macOS Keychain
   (service: `Claude Code-credentials`)
2. Extracts the inner `claudeAiOauth` object and passes it as
   `CLAUDE_CODE_OAUTH_TOKEN` via `-e` environment variable (Claude Code
   reads this env var on startup for authentication)

No credential files are mounted into the container — the
`CLAUDE_CODE_OAUTH_TOKEN` environment variable is the only credential passed.

Host agent settings (model preferences, onboarding state, plugins) are
separately mounted read-only from sanitized copies — see
[Host Settings](#host-settings) below.

### Agent keychain support

| Agent | Container env var | Keychain service (macOS) |
|-------|-------------------|--------------------------|
| `claude` | `CLAUDE_CODE_OAUTH_TOKEN` | `Claude Code-credentials` |
| `codex` | — | — |
| `opencode` | — | — |

Agent credentials are passed into the container via `-e` flags only. API key
values are **masked** (`***`) in all displayed Docker commands to prevent
accidental exposure in logs or screenshots.

**Note**: Claude Code stores its OAuth tokens in the macOS Keychain, not in
filesystem files. The keychain extraction is required for authentication.

---

## Host Settings

When launching containers for Claude, aspec mounts sanitized copies of your
host-machine Claude Code settings so the agent starts in a "set up" state
(correct model, plugins, onboarding completed) without manual configuration.

### What gets mounted

| Host file | Container path | Notes |
|-----------|----------------|-------|
| `~/.claude.json` | `/root/.claude.json:ro` | Sanitized copy: `oauthAccount` field stripped to prevent auth interference |
| `~/.claude/settings.json` | `/root/.claude/settings.json:ro` | Copied as-is (model preferences, plugins) |

### How it works

1. Before each container launch, aspec copies the relevant files to a temporary
   directory on the host
2. The `oauthAccount` field is stripped from `.claude.json` (this field causes
   Claude Code to enter a broken OAuth state when tokens aren't accessible
   from inside the container)
3. The temporary copies are mounted read-only into the container
4. When the container exits, the temporary directory is automatically cleaned up

**Your original host files are never modified or deleted.**

Only `~/.claude.json` and `~/.claude/settings.json` are copied — caches,
sessions, backups, and other large files (~50MB) are not included.

---

## Interactive Agent Notice

Whenever an interactive code agent is about to launch (in `ready --refresh` or
`implement`), aspec displays a large ASCII-art decorated notice:

```
╔══════════════════════════════════════════════════════════════╗
║                                                              ║
║     ╦╔╗╔╔╦╗╔═╗╦═╗╔═╗╔═╗╔╦╗╦╦  ╦╔═╗  ╔╦╗╔═╗╔╦╗╔═╗        ║
║     ║║║║ ║ ║╣ ╠╦╝╠═╣║   ║ ║╚╗╔╝║╣   ║║║║ ║ ║║║╣         ║
║     ╩╝╚╝ ╩ ╚═╝╩╚═╩ ╩╚═╝ ╩ ╩ ╚╝ ╚═╝  ╩ ╩╚═╝═╩╝╚═╝       ║
║                                                              ║
║  Agent 'claude' is launching in INTERACTIVE mode.            ║
║  You will need to quit the agent (Ctrl+C or exit)            ║
║  when its work is complete.                                  ║
║                                                              ║
╚══════════════════════════════════════════════════════════════╝
```

This notice is **not** shown when `--non-interactive` is used.

---

## Docker Command Visibility

Every time aspec runs a Docker command (`docker build` or `docker run`), the
full CLI command is displayed:

- **Command mode**: printed to stdout before the command runs
- **TUI mode**: included as the first line in the execution window output

This lets you see exactly what Docker invocation aspec is making, e.g.:

```
$ docker build -t aspec-myapp:latest -f Dockerfile.dev /path/to/repo
$ docker run --rm -it -v /path/to/repo:/workspace -w /workspace -e CLAUDE_CODE_OAUTH_TOKEN=*** aspec-myapp:latest claude "Implement work item 0001..."
```

---

## Configuration

### Per-repository: `GITROOT/aspec/.aspec-cli.json`

```json
{
  "agent": "claude",
  "autoAgentAuthAccepted": true
}
```

### Global: `$HOME/.aspec/config.json`

```json
{
  "default_agent": "claude"
}
```

---

## Build & Development

```sh
make all        # cargo build --release
make install    # build + install to /usr/local/bin/ (may need sudo)
make test       # cargo test
make clean      # cargo clean
```
