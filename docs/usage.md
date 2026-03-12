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
aspec implement 1
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

### `aspec ready`

Checks that your environment is ready for agentic development:

1. Verifies the Docker daemon is running
2. Checks that `Dockerfile.dev` exists in the Git root
3. Builds the `aspec-dev:latest` Docker image

Run this after `init` and whenever you update `Dockerfile.dev`.

---

### `aspec implement <work-item-number>`

Launches the dev container to implement a work item.

```sh
aspec implement 1       # implements aspec/work-items/0001-*.md
```

- Finds the matching work item file in `aspec/work-items/`
- Prompts to confirm the Docker mount scope (Git root vs CWD) on first run
- Optionally mounts agent credentials from the host (see [Agent Auth](#agent-authentication))
- Launches `aspec-dev:latest` with the agent's entrypoint, passing the work item path

The container's stdin/stdout/stderr are fully connected — you can interact with
the agent just as if it were running in your own terminal.

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
- Use arrow keys, type commands, interact exactly as in a terminal
- Press **Esc** to deselect and return focus to the command box
- A hint below the window reminds you: "Press Esc to deselect the window"

When the window is **selected after completion** (green or red border):

- **↑** / **↓** scroll through the full output — available for both success and error
- Press **Esc** to return focus to the command box
- A hint below shows: "Press Esc to deselect  ·  ↑/↓ to scroll"

When the window is **unselected** (grey or red border):

- Press **↑** from the command box to focus the window for scrolling
- A hint below the window reminds you: "Press ↑ to focus the window"
- Error exit codes remain visible in red even when the window is unselected

### Autocomplete

As you type, aspec shows suggestions below the command box:

```
ready
  ready
  init  ·  ready  ·  implement

init --
  init --agent=claude  ·  init --agent=codex  ·  init --agent=opencode
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

When running `implement`, aspec can mount your agent's local credentials into the
container so the agent is pre-authenticated — you won't have to log in manually
each time.

On first use per repository, aspec asks for your permission:

```
Mount agent credentials (~/.claude) into container?
(saved for this repo: /my/repo)
[y/n]
```

- **y** — credentials are mounted read-only; the decision is saved in `aspec/.aspec-cli.json`
  (`"autoAgentAuthAccepted": true`)
- **n** — no credential mounting; you will be prompted to authenticate inside the container

The decision is stored per Git repository and only asked once.

### Credential directories by agent

| Agent | Host credential path |
|-------|---------------------|
| `claude` | `~/.claude/` |
| `codex` | `~/.openai/` |
| `opencode` | `~/.opencode/` |

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
