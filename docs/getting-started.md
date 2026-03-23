# Getting Started with amux

This guide walks you through installing `amux` and running your first agentic coding session.

---

## Prerequisites

- **Git** — your project must be a Git repository
- **Docker** — a running Docker daemon ([Install Docker](https://docs.docker.com/get-docker/))
- **make** — standard on macOS and Linux; on Windows use WSL or `cargo` directly

---

## Installation

### From source

Clone the repository and build:

```sh
git clone https://github.com/cohix/amux.git
cd amux
make install    # builds and installs to /usr/local/bin/amux
```

To install to a different location:

```sh
INSTALL_PATH=~/bin make install
```

### From GitHub Releases

Download the latest binary for your platform from the
[Releases page](https://github.com/cohix/amux/releases), make it
executable, and move it to a directory on your `PATH`:

```sh
chmod +x amux-*
mv amux-* /usr/local/bin/amux
```

---

## Quick Start Workflow

### 1. Initialise your repository

Navigate to your project's Git root and run:

```sh
amux init
```

This writes `aspec/.amux.json`, downloads `Dockerfile.dev` from the agent
template (if not already present), and interactively offers to run the agent
audit container (which customizes `Dockerfile.dev` for your project's
toolchain). At the end it prints a summary table and a "What's Next?" panel.

To also download the `aspec/` folder (project specification templates):

```sh
amux init --aspec
```

To choose a different agent:

```sh
amux init --agent=codex      # or opencode
amux init --agent=claude --aspec
```

See [init reference](usage.md#amux-init---agentname---aspec) for details.

### 2. Verify your environment

```sh
amux ready
```

This checks that Docker is running, verifies the local agent is installed and
authenticated (by sending a random greeting and displaying the response),
checks `Dockerfile.dev` exists, and ensures the dev container image is built.
If the image does not exist yet, `amux ready` builds it automatically. A
summary table is shown at the end.

If `Dockerfile.dev` is missing, `ready` explains what it does and asks whether
to create it and run the agent audit. Declining fails the command.

On your first run (or after changing project dependencies), use `--refresh`
to have the agent audit your Dockerfile and add any missing tools:

```sh
amux ready --refresh
```

See [ready reference](usage.md#amux-ready---refresh---build---no-cache---non-interactive---allow-docker) for all flags and options.

### 3. Start a chat session

To explore the codebase or ask questions before implementing anything:

```sh
amux chat
```

This launches an interactive session with your agent inside a Docker container.
The agent has full access to your project files (mounted read-write) but runs
entirely inside the container — nothing executes on your host machine.

See [chat reference](usage.md#amux-chat---non-interactive---plan) for details.

### 4. Implement a work item

Create a work item describing what you want to build:

```sh
amux new
```

Follow the prompts to choose a type (Feature, Bug, or Task) and enter a title.
This creates a new file in `aspec/work-items/` from the template. Edit the file
to describe the implementation details, edge cases, and test considerations.

Then implement it:

```sh
amux implement 0001
```

The agent launches in interactive mode inside a container. It reads the work
item, implements the changes, runs tests, and writes documentation — all while
you can observe and interact with it in real time.

See [implement reference](usage.md#amux-implement-nnnn---non-interactive---plan) for details.

---

## Interactive TUI Mode

Running `amux` with no arguments opens the interactive TUI:

```sh
amux
```

The TUI provides a persistent command box where you can type any subcommand.
Autocomplete suggestions appear as you type. When an agent runs, a dedicated
container window shows the live session with Docker stats.

See the [TUI reference](usage.md#interactive-tui-reference) for keyboard
shortcuts and layout details.

---

## Plan Mode

To have the agent analyse code and suggest changes without modifying any files,
use plan mode:

```sh
amux implement 0001 --plan
amux chat --plan
```

This is useful for code review, architecture exploration, or understanding
unfamiliar code before making changes.

---

## What's Next

- Edit the spec files in `aspec/` to describe your project's architecture,
  security constraints, and design decisions — this gives the agent the context
  it needs to write better code
- Read the full [Usage Guide](usage.md) for all commands, flags, and
  configuration options
- Read the [Architecture Guide](architecture.md) to understand how `amux`
  works internally
