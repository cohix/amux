<p align="center">
  <strong>Multi-agent manager for secure code and claw agents.</strong> <br>
  Run and coordinate agents in parallel from your terminal. <br>
  Keep your machine safe with containers.<br>
  <br>
  <img src="./docs/amux_logo_v3.svg" width="320" alt="AMUX">
</p>

<p align="center">
  <img src="https://github.com/prettysmartdev/amux/actions/workflows/test.yml/badge.svg">
</p>

## What is `amux`?

`amux` is a terminal multiplexer for AI code and claw agents. It gives you an interactive TUI where you can launch, monitor, and coordinate multiple agent sessions at the same time — each running safely inside its own container, isolated from your host machine.

Think of it like tmux, but agents: tabs, terminal emulator, multiple parallel sessions, container stats, and stuck-agent detection, all in your terminal.

![amux TUI](./docs/blog/images/tui-screenshot.png)

---

## Why `amux`?

Running agents one at a time is a waste. Running them directly on your machine is risky. `amux` solves both:

- **Parallel sessions** — open multiple tabs, each running a different agent against the same or different projects simultaneously
- **Hard isolation** — every agent runs in a container; your filesystem, credentials, and environment are never exposed to agent-generated code execution
- **Secure claw agents** — `amux` sets up and manages a fully containerized nanoclaw install that lives securely on your machine for 24/7 subagents, workflows, and messaging app chat.
- **Agent-agnostic** — supports Claude Code, Nanoclaw, Codex, and OpenCode out of the box

---

### Multi-tab agent coordination

Each tab is fully independent — its own working directory, running command, and container session. Tabs continue running in the background when you switch away.

Tab colors reflect live state:

| Color | Meaning |
|-------|---------|
| Grey | Idle |
| Blue | Command running |
| Green | Agent container active |
| Purple | Claw session running |
| Red | Exited with error |
| Yellow | stuck agent detected |

Stuck agent detection: amux detects when an agent is stuck and needs help, it will alert you with a yellow tab so you can intervene.

### Interactive container terminal

When an agent container starts, a dedicated terminal appears with:
- Full interactive terminal emulator (arrow keys, Ctrl+O, all agent shortcuts work natively)
- Mouse scroll for terminal scrollback history
- Live container stats: CPU, memory, total runtime
- Press **Esc** to minimize (agent keeps running); **c** to maximize

---

## Claw agent management

`amux claws` commands set up and manage a persistent [nanoclaw](https://github.com/qwibitai/nanoclaw) container — a machine-global background agent with Docker socket access, designed for long-running, scheduled, or cross-project work. Accessible via your messaging app of choice.

```sh
amux claws ready    # guided setup and status
```

The nanoclaw container:
- Runs persistently in the background across `amux` sessions
- Survives reboots (check status with `claws ready`)
- Has Docker socket access to build and run containers on your behalf
- Manage seamlessly via the amux TUI

---

## Security

`amux` enforces a hard boundary: **agents never execute on the host machine**.

- All agent code runs inside containers built from `Dockerfile.dev`
- `amux` will automatically scan your project to create a `Dockerfile.dev` with every tool needed for your workflow
- Only the current Git repository is mounted — never parent directories
- Your code agent is automatically configured and authenticated with secure copies of config files and OAuth tokens from your host installation.
- `amux` itself is a statically compiled Rust binary — memory-safe and unmodifiable by agents
- Every Docker command is printed in full before execution so you can see exactly what runs

---

## Quick Start

```sh
# 1. Initialize your repo (only once)
amux init

# 2. Open the TUI and start an agent session
amux

# 3. Then run an amux command (starts an agent chat session)
chat
```

See the [Getting Started Guide](docs/getting-started.md) for a full walkthrough. All commands are available directly via the CLI in addition to the TUI.

---

## Installation

```sh
curl -s https://prettysmart.dev/install/amux.sh | sh
```

That's it. The installer detects your platform and puts `amux` on your `PATH`.

<details>
<summary>Other installation options</summary>

**From GitHub Releases** — download the binary for your platform from [GitHub Releases](https://github.com/prettysmartdev/amux/releases):

| Platform | Binary |
|----------|--------|
| Linux (x86_64) | `amux-linux-amd64` |
| Linux (ARM64) | `amux-linux-arm64` |
| macOS (Intel) | `amux-macos-amd64` |
| macOS (Apple Silicon) | `amux-macos-arm64` |
| Windows (x86_64) | `amux-windows-amd64.exe` |

**From source** — requires Rust 1.94+ and make:

```sh
git clone https://github.com/prettysmartdev/amux.git
cd amux
sudo make install
```

</details>

---

## Development

```sh
make all      # build the amux binary
make install  # build + install to /usr/local/bin/
make test     # run all tests
make clean    # clean build artifacts
```

---

## Full Documentation

- [Getting Started](docs/getting-started.md) — installation and first workflow
- [Usage Guide](docs/usage.md) — all commands, flags, TUI reference, and configuration
- [Architecture](docs/architecture.md) — code structure, design patterns, and testing strategy

---

## License

See [LICENSE](LICENSE) for details.
