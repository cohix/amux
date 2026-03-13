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
aspec implement 0001
aspec new
```

---

## Subcommands

### `aspec init [--agent=<name>]`

Initialises the current Git repository for use with `aspec`.

- Detects the Git root directory
- Writes `aspec/.aspec-cli.json` (repository config)
- Writes `Dockerfile.dev` (dev container definition)

**Flags**

| Flag | Values | Default |
|------|--------|---------|
| `--agent` | `claude`, `codex`, `opencode` | `claude` |

**Example**

```sh
aspec init --agent=claude
```

---

### `aspec ready [--auth-from-env] [--refresh] [--non-interactive]`

Checks that your environment is ready for agentic development.

1. Verifies the Docker daemon is running
2. Checks that `Dockerfile.dev` exists — if missing, initialises it from the
   agent template (same as `init`)
3. Checks for an existing `aspec-{projectname}:latest` image — builds one if
   it does not exist yet (with streaming output)
4. Presents a summary table showing the status of each step

When `--refresh` is passed, `ready` also runs the Dockerfile agent audit:

4. Launches a container with the configured code agent to scan the project
   and update `Dockerfile.dev` with any missing build/test tools
5. Rebuilds the image with the updated `Dockerfile.dev`

Without `--refresh`, the audit is skipped and a tip is shown suggesting its use.

The image tag is derived from the Git root folder name (e.g. `aspec-myapp:latest`).

Before launching the audit container, `ready` applies the same mount scope and
agent authentication flow as `implement` (see [Agent Auth](#agent-authentication)).

**Flags**

| Flag | Description |
|------|-------------|
| `--auth-from-env` | Read the agent API key from host environment variables |
| `--refresh` | Run the Dockerfile agent audit (skipped by default) |
| `--non-interactive` | Run the agent in print/non-interactive mode |

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
aspec ready                            # quick check — skips audit
aspec ready --refresh                  # full check with Dockerfile audit
aspec ready --refresh --non-interactive  # audit in non-interactive mode
```

---

### `aspec implement <NNNN> [--auth-from-env] [--non-interactive]`

Launches the dev container to implement a work item.

```sh
aspec implement 0001    # implements aspec/work-items/0001-*.md
aspec implement 0003    # implements aspec/work-items/0003-*.md
```

The work item number is a 4-digit identifier (e.g. `0001`). Both `0001` and
`1` are accepted as input.

- Finds the matching work item file in `aspec/work-items/`
- Prompts to confirm the Docker mount scope (Git root vs CWD) on first run
- Optionally mounts agent credentials from the host (see [Agent Auth](#agent-authentication))
- Launches a container with the configured agent

**Flags**

| Flag | Description |
|------|-------------|
| `--auth-from-env` | Read the agent API key from host environment variables |
| `--non-interactive` | Run the agent in print/non-interactive mode |

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

**Edge case**: If no template is found in the current Git root, an error message
is displayed with a link to download the template from GitHub.

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

```
┌─── ● running: ready ──────────────────────────────┐
│ Checking Docker daemon... OK                        │
│ Checking Dockerfile.dev... OK                       │
│ Building Docker image (aspec-dev:latest)...         │
│                                                     │
└─────────────────────────────────────────────────────┘
  Done — use ↑/↓ to scroll, or type a new command
┌─── command ─────────────────────────────────────────┐
│ > _                                                  │
└─────────────────────────────────────────────────────┘
  init  ·  ready  ·  implement
```

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

### Execution Window

| State | Focus | Border colour |
|-------|-------|--------------|
| Running | Selected | Blue |
| Running | Unselected | Grey |
| Done (success) | Selected | Green |
| Done (success) | Unselected | Grey |
| Done (error) | Selected | Red |
| Done (error) | Unselected | Red |

When the window is **selected while running** (blue border):

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
  init  ·  ready  ·  implement  ·  new

init --
  init --agent=claude  ·  init --agent=codex  ·  init --agent=opencode

ready --
  ready --refresh  ·  ready --non-interactive  ·  ready --refresh --non-interactive

implement --
  implement <NNNN>  e.g. implement 0001  ·  implement <NNNN> --non-interactive
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
Pass agent credentials (ANTHROPIC_API_KEY, from system keychain) into container?
This will be saved for this repo. [y/n]
```

- **y** — the token is extracted from the keychain and passed as an environment
  variable; the decision is saved in `aspec/.aspec-cli.json`
  (`"autoAgentAuthAccepted": true`)
- **n** — no credentials passed; you will be prompted to authenticate inside
  the container

The decision is stored per Git repository and only asked once.

**2. Environment variable (`--auth-from-env`)**

Pass `--auth-from-env` to read the API key from host environment variables
instead of the keychain. This skips the prompt entirely:

```sh
aspec ready --auth-from-env
aspec implement 0001 --auth-from-env
```

This is useful in CI/CD environments or when you have the API key set
directly (e.g. `export ANTHROPIC_API_KEY=sk-ant-...`).

### Environment variables by agent

| Agent | `--auth-from-env` variables | Keychain env vars | Keychain service (macOS) |
|-------|----------------------------|-----------------|--------------------------|
| `claude` | `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` | `ANTHROPIC_API_KEY` + `CLAUDE_CODE_OAUTH_TOKEN` | `Claude Code-credentials` |
| `codex` | `OPENAI_API_KEY` | — | — |
| `opencode` | `OPENAI_API_KEY` | — | — |

Agent credentials are passed into the container via `-e` flags. API key
values are **masked** (`***`) in all displayed Docker commands to prevent
accidental exposure in logs or screenshots.

**Note**: Claude Code stores its OAuth tokens in the macOS Keychain, not in
filesystem files. Mounting `~/.claude` is insufficient for authentication.
The keychain-based extraction is the default and most reliable method.

When using the keychain, the OAuth access token is passed as both
`ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` so Claude Code picks it
up regardless of which env var it checks. The Anthropic SDK auto-detects
OAuth tokens by their `sk-ant-oat` prefix.

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
