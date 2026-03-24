# Getting Started with amux

`amux` is a terminal multiplexer for code and claw agents. This guide gets you from zero to running multiple containerized agent sessions in the TUI.

---

## Prerequisites

- **Git** — your project must be in a Git repository
- **Docker** — a running Docker daemon ([Install Docker](https://docs.docker.com/get-docker/))
- **A supported agent** — Claude Code, Codex, or OpenCode installed and authenticated on your machine. NanoClaw for persistent 24/7 agents.

---

## Installation

### From GitHub Releases (recommended)

Download the latest binary for your platform from the [Releases page](https://github.com/prettysmartdev/amux/releases), make it executable, and move it onto your `PATH`:

```sh
chmod +x amux-*
mv amux-* /usr/local/bin/amux
```

### From source

Requires Rust 1.94+ and make:

```sh
git clone https://github.com/prettysmartdev/amux.git
cd amux
make install    # builds and installs to /usr/local/bin/amux
```

---

## First-time setup

Navigate to your project's Git root and run:

```sh
amux init
```

This creates `aspec/.amux.json` (repo config), downloads a `Dockerfile.dev` template for your agent, and offers to run an agent audit that customizes the Dockerfile for your project's actual toolchain. It then builds the dev container image.

To verify everything is ready:

```sh
amux ready
```

This checks Docker, verifies your agent is installed and authenticated (by sending a test greeting and showing the response), and confirms the dev container image exists. Use `--refresh` to re-run the Dockerfile audit and rebuild the image:

```sh
amux ready --refresh
```

---

## Opening the TUI

```sh
amux
```

The TUI opens with a tab bar at the top, an execution window in the middle, and a command input box at the bottom. Type any subcommand and press **Enter** to run it.

---

## Starting an agent session

### Freeform chat

```sh
chat
```

Launches an interactive agent session in a Docker container against your project. The container window opens as an overlay — all keyboard input is forwarded to the agent. Mouse scroll navigates terminal scrollback history.

### Implementing a work item

```sh
implement 0001
```

Launches the agent with a prompt to implement `aspec/work-items/0001-*.md`. The agent reads the spec, writes code, runs tests, and reports back — all inside the container.

Create a new work item:

```sh
new
```

Prompts for a type (Feature, Bug, Task) and title, then creates the file in `aspec/work-items/`.

---

## Running multiple agents in parallel

Press **Ctrl+T** to open a new tab. Each tab gets its own working directory, execution window, and container session. Tabs keep running in the background when you switch away.

```
Ctrl+T          open a new tab
Ctrl+A / Ctrl+D navigate left / right between tabs
```

Tab colors show live state at a glance:

| Color | Meaning |
|-------|---------|
| Grey | Idle |
| Blue | Command running |
| Green | Agent container active |
| Purple | Claw session running |
| Red | Exited with error |
| Yellow | possible stuck agent |

A stuck tab (yellow, `⚠️` prefix) clears automatically when you switch to it or interact with it.

A practical workflow: open one tab to `implement` a work item, open another to `chat` about a different part of the codebase, and monitor both from the tab bar without leaving the TUI.

---

## Claw agent management

`amux claws ready` sets up a persistent [nanoclaw](https://github.com/qwibitai/nanoclaw) container — a long-lived, machine-global background agent with Docker socket access.

```sh
amux claws ready    # first run: guided setup (fork, clone, build, launch)
```

Unlike `chat` and `implement` sessions (ephemeral, per-project), the nanoclaw container persists across `amux` sessions. It's designed for longer-running or cross-project work where you want a continuously available agent. Press **Ctrl+C** to detach — the container keeps running; `claws ready` re-attaches next time.

In the TUI, the nanoclaw tab is shown in purple.

---

## Container window controls

| Key | Action |
|-----|--------|
| (typing) | Forwarded directly to the agent |
| **Esc** | Minimize container window (agent keeps running) |
| **c** | Restore minimized container window |
| Mouse scroll | Scroll terminal history |

When minimized, the container collapses to a 1-line status bar showing agent name, CPU, memory, and runtime. The outer execution window becomes fully visible for reading command output.

---

## Agent authentication

On first use per repository, amux asks whether to pass your agent's credentials from the system keychain into the container:

```
Pass agent credentials (from system keychain) into container?
This will be saved for this repo. [y/n]
```

Accept once; the decision is saved. Credentials are passed as environment variables only — never as mounted files. API key values are masked in all displayed Docker commands.

---

## What's next

- Read the [Usage Guide](usage.md) for all commands, flags, and TUI keyboard reference
- Use `--plan` on any session to have the agent analyse without modifying files: `amux chat --plan`
- Use `--allow-docker` to give an agent container access to the host Docker daemon when a task requires it
