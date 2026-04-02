# amux Usage Guide

## Overview

`amux` is a containerized code and claw agent manager.
Every agent action runs inside a Docker container — never directly on your machine.

---

## Installation

```sh
curl -s https://prettysmart.dev/install/amux.sh | sh
```

See the [Getting Started guide](getting-started.md) for other installation options.

---

## Execution Modes

### Interactive Mode (TUI)

Running `amux` with no arguments opens the interactive REPL:

```sh
amux
```

The TUI displays a persistent command input box at the bottom of the screen.
Type any subcommand and press **Enter** to run it. Suggestions appear as you type.

### Command Mode

Running `amux` with a subcommand executes it and exits immediately:

```sh
amux init
amux ready
amux ready --refresh
amux ready --build
amux ready --build --no-cache
amux implement 0001
amux implement 0001 --plan
amux implement 0001 --allow-docker
amux implement 0001 --workflow aspec/workflows/implement-feature.md
amux implement 0001 --worktree
amux implement 0001 --worktree --mount-ssh
amux chat
amux chat --plan
amux chat --allow-docker
amux chat --mount-ssh
amux new
amux specs new
amux specs new --interview
amux specs amend 0025
amux claws init
amux claws ready
amux claws chat
amux status
amux status --watch
```

---

## Subcommands

### `amux init [--agent=<name>] [--aspec]`

Initialises the current Git repository for use with `amux`.

1. Writes `aspec/.amux.json` (repository config) with the chosen agent
2. Downloads the `aspec/` folder from GitHub if `--aspec` is passed and the
   folder does not yet exist. Skipped (noted in summary) without `--aspec`.
3. Writes `Dockerfile.dev` from the agent template — **never overwrites** an
   existing file. Falls back to a bundled template if the download fails.
4. Offers to run the agent audit container (explains what it does and asks
   for confirmation). Only proceeds if you accept; skipped if you decline.
5. Builds the dev container image (before audit, and again after if audit ran).
6. Prints a summary table of every step.
7. Prints a "What's Next?" panel explaining how to use the TUI.

**Flags**

| Flag | Values | Default |
|------|--------|---------|
| `--agent` | `claude`, `codex`, `opencode` | `claude` |
| `--aspec` | (flag, no value) | off |

**`--aspec`**

When passed, downloads the `aspec/` folder from `github.com/prettysmartdev/aspec` to
provide project specification templates and work item scaffolding. If the
folder already exists it is left untouched. Without `--aspec`, the folder
download is skipped and noted in the summary table.

**Summary table**

At the end of every `init` run a summary table is printed:

```
┌──────────────────────────────────────────────────┐
│              Init Summary (       claude)         │
├───────────────────┬──────────────────────────────┤
│            Config │ ✓ saved                       │
│      aspec folder │ – use --aspec to download     │
│    Dockerfile.dev │ ✓ created                     │
│       Agent audit │ ✓ completed                   │
│      Docker image │ ✓ built                       │
└───────────────────┴──────────────────────────────┘
```

**What's Next?**

After the summary, a decorated panel reminds you to run `amux` to open the
interactive TUI and explains the `chat`, `new`, and `implement` commands.

**Examples**

```sh
amux init                       # init with claude agent, skip aspec folder
amux init --agent=codex         # use codex agent
amux init --aspec               # also download the aspec/ folder
amux init --agent=claude --aspec
```

---

### `amux ready [--refresh] [--build] [--no-cache] [--non-interactive] [--allow-docker]`

Checks that your environment is ready for agentic development.

1. Verifies the Docker daemon is running
2. Checks the `aspec/` folder — notes if missing and suggests `amux init --aspec`
3. Checks the local agent installation by sending a random greeting (non-containerized)
   and displaying the greeting sent and the agent's response
4. Checks that `Dockerfile.dev` exists — if missing, explains what it does and
   asks whether to create it and run the audit. Declines result in a failure.
   If the content matches the default template, offers to run the audit.
5. Checks for an existing `amux-{projectname}:latest` image — builds one if
   it does not exist yet (with streaming output)
6. Presents a summary table showing the status of each step

When `--refresh` is passed, `ready` also runs the Dockerfile agent audit:

- Launches a container with the configured code agent to scan the project
  and update `Dockerfile.dev` with any missing build/test tools
- Rebuilds the image with the updated `Dockerfile.dev`

Without `--refresh`, the audit is skipped and a tip is shown suggesting its use.

When `--build` is passed (without `--refresh`), `ready` forces a rebuild of the
dev container image from the current `Dockerfile.dev`, even if an image already
exists. This is useful when you have manually edited the Dockerfile and want a
fresh image without re-running the agent audit.

When `--no-cache` is passed, `docker build` is invoked with `--no-cache`,
forcing Docker to rebuild every layer from scratch. This flag applies to all
build operations — whether triggered by `--build`, `--refresh`, or a missing
image. Combine with `--build` to get a completely fresh image.

The image tag is derived from the Git root folder name (e.g. `amux-myapp:latest`).

Before launching the audit container, `ready` uses the same mount scope and
passes host agent credentials into the container automatically (see [Agent Auth](#agent-authentication)).

**Local Agent Check**

Every `ready` run sends a random greeting to the configured agent on the host
machine (not inside a container) to verify it is installed and authenticated.
The greeting and the agent's first response line are printed, e.g.:

```
Checking local claude agent...
  > Howdy
  < Howdy! How can I help you today?
  claude: installed & authenticated: OK
```

If the agent is not installed or not authenticated, the check is noted in the
summary but does not fail the command.

**Dockerfile.dev handling**

- **Missing**: `ready` explains what `Dockerfile.dev` does and asks whether to
  create it and run the audit. Declining results in a command failure (the dev
  image cannot be built without it).
- **Matches default template**: `ready` offers to run the audit container to
  customize the Dockerfile for your project's actual toolchain.
- **Custom content**: The audit offer is skipped; use `--refresh` explicitly.

**Flags**

| Flag | Description |
|------|-------------|
| `--refresh` | Run the Dockerfile agent audit (skipped by default) |
| `--build` | Force rebuild the dev container image (ignored if `--refresh` is also set) |
| `--no-cache` | Pass `--no-cache` to `docker build` for all build operations |
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--allow-docker` | Mount the host Docker daemon socket into the audit container (see [Docker Socket Access](#docker-socket-access)) |

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
│      aspec folder │ ✓ present                    │
│       Local agent │ ✓ claude: installed & ...    │
│         Dev image │ ✓ exists                     │
│   Refresh (audit) │ – use --refresh to run       │
│     Image rebuild │ – no refresh                 │
└───────────────────┴──────────────────────────────┘
```

**Examples**

```sh
amux ready                                # quick check — skips audit
amux ready --refresh                      # full check with Dockerfile audit
amux ready --refresh --non-interactive    # audit in non-interactive mode
amux ready --build                        # rebuild image from current Dockerfile.dev
amux ready --build --no-cache             # full rebuild without Docker cache
amux ready --refresh --no-cache           # audit + rebuild without cache
amux ready --refresh --allow-docker       # audit with Docker daemon access in container
```

---

### `amux implement <NNNN> [--non-interactive] [--plan] [--allow-docker] [--workflow=<path>]`

Launches the dev container to implement a work item.

```sh
amux implement 0001    # implements aspec/work-items/0001-*.md
amux implement 0003    # implements aspec/work-items/0003-*.md
amux implement 0027 --workflow aspec/workflows/implement-feature.md
amux implement 0030 --worktree
amux implement 0030 --worktree --mount-ssh
```

The work item number is a 4-digit identifier (e.g. `0001`). Both `0001` and
`1` are accepted as input.

- Finds the matching work item file in `aspec/work-items/`
- Prompts to confirm the Docker mount scope (Git root vs CWD) on first run (skipped when `--worktree` is used)
- Passes host agent credentials into the container automatically (see [Agent Auth](#agent-authentication))
- Launches a container with the configured agent

**Flags**

| Flag | Description |
|------|-------------|
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--plan` | Run the agent in plan mode (read-only, no file modifications) |
| `--allow-docker` | Mount the host Docker daemon socket into the container (see [Docker Socket Access](#docker-socket-access)) |
| `--workflow=<path>` | Path to a workflow Markdown file for multi-step execution (see [Workflows](workflows.md)) |
| `--worktree` | Run in an isolated Git worktree under `~/.amux/worktrees/` (see [Worktree Isolation](#worktree-isolation)) |
| `--mount-ssh` | Mount host `~/.ssh` read-only into the container (see [SSH Key Mounting](#ssh-key-mounting)) |

When `--workflow` is provided, amux runs the work item through a series of agent steps defined in the workflow file, pausing between each step for your review. State is persisted so interrupted workflows can be resumed.

When `--worktree` is provided, the agent runs against an isolated Git worktree instead of your main working tree. After the agent finishes you are prompted to merge, discard, or keep the worktree branch.

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

### `amux chat [--non-interactive] [--plan] [--allow-docker] [--mount-ssh]`

Starts a freeform chat session with the configured agent in a container.

Unlike `implement`, which sends an initial prompt to the agent, `chat` launches
the agent with no pre-configured prompt — giving you a clean interactive session.

```sh
amux chat                      # start interactive chat
amux chat --non-interactive    # start in non-interactive mode
amux chat --plan               # start in plan mode (read-only)
amux chat --allow-docker       # start with Docker daemon access in container
amux chat --mount-ssh          # start with host ~/.ssh mounted read-only
```

- Prompts to confirm the Docker mount scope (Git root vs CWD) if needed
- Passes agent credentials via environment variable (see [Agent Auth](#agent-authentication))
- Launches a container with the configured agent (no initial prompt)

**Flags**

| Flag | Description |
|------|-------------|
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--plan` | Run the agent in plan mode (read-only, no file modifications) |
| `--allow-docker` | Mount the host Docker daemon socket into the container (see [Docker Socket Access](#docker-socket-access)) |
| `--mount-ssh` | Mount host `~/.ssh` read-only into the container (see [SSH Key Mounting](#ssh-key-mounting)) |

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

Same as `implement --plan` — see the [implement](#amux-implement-nnnn---non-interactive---plan)
section for details on how each agent's plan mode is activated.

**Shared Implementation**

`chat` and `implement` share the same underlying container-launching code
(`commands/agent.rs`). The only difference is:
- `implement` passes the work item implementation prompt as the agent entrypoint
- `chat` passes no prompt — just the agent command itself

---

### `amux claws` — Persistent nanoclaw agent management

The `claws` commands manage a persistent nanoclaw agent container — a
machine-global installation of [nanoclaw](https://github.com/qwibitai/nanoclaw)
that runs a background agent accessible from anywhere on the machine.

Unlike `implement` and `chat` (per-project, ephemeral containers), the nanoclaw
container is persistent and machine-global. It lives at `$HOME/.nanoclaw`
and survives across `amux` sessions.

All `claws` subcommands show as **purple/magenta** tabs in the TUI.

---

### `amux claws init`

Runs the first-time setup wizard to install and launch nanoclaw.

1. **Fork check** — asks whether you have already forked nanoclaw on GitHub.
   - **Yes** — prompts for your GitHub username and clones
     `github.com/<username>/nanoclaw` to `$HOME/.nanoclaw`.
   - **No** — offers to fork and clone using the GitHub CLI (`gh repo fork`).
     If you decline, provides manual instructions.
2. **Docker daemon** — verifies the Docker daemon is running.
3. **Dockerfile setup** — writes or verifies `Dockerfile.dev` inside the
   nanoclaw repo and builds the `amux-nanoclaw:latest` image.
4. **Agent audit** — runs a nanoclaw-specific audit agent in a foreground,
   interactive, prompted container (no Docker socket). The agent receives a
   specialized prompt that covers two tasks: (a) updating `Dockerfile.dev` with
   all tools needed to build, run, and test nanoclaw at their newest compatible
   versions; and (b) modifying the nanoclaw codebase to replace any
   `host.docker.internal` references with container-to-container networking so
   that the controller, onecli, and workers communicate directly over the Docker
   network. This prompt is distinct from the standard audit prompt used by
   `amux init` and `amux ready --refresh`. The container is named with an
   `amux-` prefix and runs `--rm -it`. In the **TUI**, the audit opens in the
   tab's container window exactly like `chat` or `implement`. In **command
   mode**, it inherits stdin/stdout.
5. **Docker socket warning** — explains (and requires explicit acceptance) that
   the nanoclaw container will be mounted to the host Docker socket, granting
   elevated access identical to `--allow-docker`.
6. **`/setup` explanation** — reminds you to run `/setup` inside the agent
   after launching, and requires explicit acceptance before proceeding (CLI
   only; the TUI shows this as a dialog during the wizard).
7. **Container launch** — starts the nanoclaw container in the background
   (`-d`, detached), waits for it to reach running state, and saves the
   container ID to `$HOME/.nanoclaw/.amux.json`.
8. **Attach** — attaches to the running container and launches the configured
   code agent interactively (identical to `amux chat`).

In the **TUI**, the wizard is presented via modal dialogs, and the audit agent
runs in the tab's PTY container window. In **command mode**, the wizard runs
interactively on stdin.

**Examples**

```sh
amux claws init    # run first-time setup wizard
```

---

### `amux claws ready`

Checks whether the nanoclaw container is running and shows a status table.
Does **not** run the first-time wizard.

- **Nanoclaw not installed** (`$HOME/.nanoclaw` missing) — prints a message
  suggesting `amux claws init` and exits without error.
- **Container running** — shows a status summary table and exits immediately.
- **Container stopped** — interactively offers to start the container in the
  background (`-d`). If accepted, starts it and saves the new container ID.
  Use `claws chat` afterwards to attach.

**Examples**

```sh
amux claws ready    # check status; start container if stopped
```

---

### `amux claws chat`

Attaches to the running nanoclaw container for a freeform agent chat session.

- If nanoclaw is not installed → error, suggests `claws init`.
- If the container is not running → error, suggests `claws ready`.
- If the container is running → attaches interactively (identical to `amux chat`).

In the **TUI**, the container window opens with full keyboard passthrough. In
**command mode**, stdin/stdout/stderr are fully connected to the agent.

**Agent session behavior**

- Press **Ctrl+C** to detach from the agent — the container **continues
  running in the background**. Run `claws chat` again to re-attach.

**Examples**

```sh
amux claws chat    # attach to running nanoclaw container
```

---

#### Authentication (all claws commands)

The nanoclaw container is auto-authenticated using the same keychain
passthrough as `chat` and `implement` — no manual login required.

#### Docker Socket Access

The nanoclaw container always mounts the host Docker socket. This is required
for nanoclaw to manage Docker containers on your behalf. A warning is shown and
explicit acceptance is required during `claws init`.

#### Configuration

The container ID is stored at `$HOME/.nanoclaw/.amux.json`:

```json
{
  "nanoclawContainerID": "abc123..."
}
```

---

### `amux status [--watch]`

Shows the status of all running code-agent and nanoclaw containers in two tables.

**CODE AGENTS** — every running container with an `amux-` name prefix (excluding nanoclaw system containers). For each container the table shows:

- **Project** — the host Git repository mounted into the container (`/workspace` bind-mount source)
- **Agent** — the configured agent name (read from `GITROOT/aspec/.amux.json`)
- **CPU** — live CPU usage percentage from `docker stats`
- **Memory** — live memory usage from `docker stats`

**NANOCLAW** — the `amux-claws-controller` container and any container whose name contains `nanoclaw`. Shows Container name, CPU, and Memory.

When no containers are running in a section, an empty-state message is shown with the command needed to start that type of agent.

**Flags**

| Flag | Description |
|------|-------------|
| `--watch` | Refresh the output every 3 seconds |

**Watch mode behaviour**

| Context | Behaviour |
|---------|-----------|
| Command mode (CLI) | Uses ANSI cursor-up + clear-to-end to overwrite in place |
| TUI mode | Clears the execution window content before each refresh so tables appear to update in place |

**TUI startup**: If `amux` is launched outside of any Git repository, `status --watch` runs automatically instead of `ready`.

**Examples**

```sh
amux status              # one-shot snapshot
amux status --watch      # auto-refreshing dashboard
```

**Example output**

```
CODE AGENTS
┌────────────────────────────┬────────┬───────┬─────────┐
│ Project                    │ Agent  │ CPU   │ Memory  │
├────────────────────────────┼────────┼───────┼─────────┤
│ /home/user/myproject       │ claude │ 5.23% │ 210MiB  │
└────────────────────────────┴────────┴───────┴─────────┘

NANOCLAW
┌──────────────────────────┬───────┬─────────┐
│ Container                │ CPU   │ Memory  │
├──────────────────────────┼───────┼─────────┤
│ amux-claws-controller    │ 2.10% │ 150MiB  │
└──────────────────────────┴───────┴─────────┘
```

---

### `amux new`

Creates a new work item from the template (`aspec/work-items/0000-template.md`).

1. Scans the `aspec/work-items/` directory to determine the next sequential number
2. Prompts for the work item type: **Feature**, **Bug**, **Task**, or **Enhancement**
3. Prompts for a title
4. Creates a new file using the naming pattern `XXXX-title-of-item.md`
5. Replaces the template's header and title lines with the user's choices
6. If running inside a VS Code terminal, opens the new file in the editor

**In TUI mode**, the type and title are collected via dialog overlays instead of
stdin prompts.

**Filename generation**: The title is lowercased, spaces are replaced with
hyphens, and all non-alphanumeric characters (except hyphens) are removed.

**Edge case**: If no template is found in the current Git root, `amux` will
automatically download the `aspec/` folder from GitHub (`github.com/prettysmartdev/aspec`)
to provide the template. If the download also fails, an error message is displayed.

**Example**

```sh
amux new
# Select work item type:
#   1) Feature
#   2) Bug
#   3) Task
#   4) Enhancement
# Choice [1/2/3/4]: 1
# Work item title: Add user authentication
# Created work item: /path/to/repo/aspec/work-items/0007-add-user-authentication.md
```

---

### `amux specs new [--interview]`

Creates a new work item from the template. This is the preferred entry point for
spec-related operations. Without `--interview`, behaves identically to `amux new`.

**With `--interview`**:

1. Prompts for work item type and title
2. Creates the work item file (same as `amux new`, does **not** open in VS Code yet)
3. Prompts for a brief summary of the work item
4. Launches the configured code agent in an interactive container with a prompt to
   complete the work item details (user stories, implementation plan, edge cases, test plan)
5. After the agent finishes, opens the work item file in VS Code (if inside a VS Code terminal)

The agent prompt (filled in with your inputs):

```
Work item {number} template has been created for {kind}: {title}.
Help complete the work item based on the following summary...

Summary:
{your summary}
```

**In TUI mode**, after entering the type and title, a large freeform text box
dialog opens for entering the summary. Use **Ctrl+Enter** to submit or **Esc** to cancel.
The text box supports multi-line input with full cursor movement (arrow keys, Home/End).

**Flags**

| Flag | Description |
|------|-------------|
| `--interview` | After creating the file, prompt for a summary and launch the agent to complete the spec |

**Examples**

```sh
amux specs new               # same as amux new
amux specs new --interview   # create file + run agent to complete spec
```

---

### `amux specs amend <NNNN> [--non-interactive] [--allow-docker]`

Launches the code agent to review a completed work item and amend it to match the
final implementation.

```sh
amux specs amend 0025    # amends aspec/work-items/0025-*.md
```

The agent receives the following prompt:

```
Work item {number} is complete. Review the work that has been done in the codebase
and compare it against the work item markdown file. If needed, amend the work item
to ensure it matches the final implementation...
Add new details if needed. Summarize the implementation and any corrections
or changes that were needed to achieve the desired result in a new
`Agent implementation notes` section at the bottom of the file.
```

- Finds the matching work item file in `aspec/work-items/`
- Prompts to confirm the Docker mount scope (Git root vs CWD) if needed
- Passes host agent credentials automatically
- Launches a container with the configured agent

**Flags**

| Flag | Description |
|------|-------------|
| `--non-interactive` | Run the agent in print/non-interactive mode |
| `--allow-docker` | Mount the host Docker daemon socket into the container |

**Examples**

```sh
amux specs amend 0025                     # interactive agent reviews work item 0025
amux specs amend 0025 --non-interactive   # non-interactive mode
amux specs amend 0025 --allow-docker      # with Docker daemon access in container
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
│ $ docker run --rm -it --name amux-12345 ...               │
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

Whenever `amux` launches a Docker container to run a coding agent (via
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

### Multi-Tab Support

The TUI supports multiple simultaneous sessions — each tab runs its own
independent project context (working directory, command, container).

**Tab bar** — displayed at the top of the screen. Each tab shows:

- Project name (folder name of the tab's working directory, ≤14 characters)
- Subcommand currently running (or last run), truncated if needed
- Arrow `➡` in the active tab's title

**Active tab appearance** — the active tab renders with its bottom border suppressed, making it appear to open into the content area below (like a raised tab in a physical tabbed folder). Inactive tabs retain a full closed border, visually receding so focus is drawn to the active one. This visual distinction works regardless of the tab's assigned colour.

**Tab colours** reflect the state of each tab's current command:

| Colour | Meaning |
|--------|---------|
| Grey | Idle or completed |
| Blue | Running (no container) |
| Green | Running with an active container |
| Magenta/Purple | Running a claws session |
| Red | Exited with an error |
| **Yellow** | Container has been **silent for 10 seconds** (stuck warning) |

**Stuck tab detection** — if a running container produces no output for more
than 10 seconds, the tab turns **yellow** and the subcommand label gains a
**⚠️** prefix (e.g. `⚠️ implement 0001`). This alerts you that the container
may be waiting for input or has stalled.

For **workflow tabs**, amux goes a step further: the [workflow control board](workflows.md#auto-advance-when-stuck)
opens automatically so you can restart, advance, or cancel the step without
having to notice the yellow indicator and manually press Ctrl+W. The dialog
is suppressed on background tabs and appears as soon as you switch to them.

The yellow warning is automatically cleared as soon as you:
- Switch to the yellow tab (**Ctrl+A** / **Ctrl+D**)
- Press any key while the tab is active
- Scroll with the mouse wheel

**Tab keyboard shortcuts:**

| Key | Action |
|-----|--------|
| **Ctrl+T** | Open a new tab (prompts for working directory) |
| **Ctrl+A** | Switch to the previous tab |
| **Ctrl+D** | Switch to the next tab |
| **Ctrl+C** (in command box, multiple tabs) | Close the current tab |
| **Ctrl+W** (workflow running, container minimized) | Open the [workflow control board](workflows.md#workflow-control-board-tui-only) |

Tabs are independent — each has its own output, phase, and container state.
Background tabs continue running even when not active.

---

### Autocomplete

As you type, amux shows suggestions below the command box:

```
ready
  ready
  init  ·  ready  ·  implement  ·  chat  ·  new  ·  specs  ·  claws  ·  status

init --
  init --agent=claude  ·  init --agent=codex  ·  init --agent=opencode

ready --
  ready --refresh  ·  ready --build  ·  ready --no-cache  ·  ready --build --no-cache  ·  ready --non-interactive  ·  ready --refresh --non-interactive  ·  ready --refresh --allow-docker

implement --
  implement <NNNN>  e.g. implement 0001  ·  implement <NNNN> --non-interactive  ·  implement <NNNN> --plan  ·  implement <NNNN> --allow-docker  ·  implement <NNNN> --worktree  ·  implement <NNNN> --mount-ssh

chat --
  chat  (start a freeform agent session)  ·  chat --non-interactive  ·  chat --plan  ·  chat --allow-docker  ·  chat --mount-ssh

status --
  status         (show all running agents and nanoclaw containers)
  status --watch (refresh every 3 seconds)
```

### Unknown Commands

If you type a command that is not an amux subcommand, the error message
includes the closest known subcommand:

```
'implemnt' is not an amux command.  Did you mean: implement
```

### Quit Confirmation

Press **q** or **Ctrl+C** when the command box is focused to open the confirmation dialog:

```
╭─── Quit amux? ──────────────────╮
│  Are you sure you want to quit?   │
│  [y/n]                            │
╰───────────────────────────────────╯
```

Press **y** to quit, **n** or **Esc** to cancel.

---

## Agent Authentication

When running `implement`, `chat`, or `ready`, amux automatically passes your
agent's credentials into the container so it is pre-authenticated — you won't
have to log in manually each time.

If your host agent is installed and authenticated, its credentials are
securely and transparently passed into the container so your account and global
settings are maintained. No prompting or configuration is required.

### How credentials are passed

amux:

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

When launching containers for Claude, amux mounts sanitized copies of your
host-machine Claude Code settings so the agent starts in a "set up" state
(correct model, plugins, onboarding completed) without manual configuration.

### What gets mounted

| Host file | Container path | Notes |
|-----------|----------------|-------|
| `~/.claude.json` | `/root/.claude.json:ro` | Sanitized copy: `oauthAccount` field stripped to prevent auth interference |
| `~/.claude/settings.json` | `/root/.claude/settings.json:ro` | Copied as-is (model preferences, plugins) |

### How it works

1. Before each container launch, amux copies the relevant files to a temporary
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
`implement`), amux displays a large ASCII-art decorated notice:

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

## Docker Socket Access

The `--allow-docker` flag is available on `implement`, `chat`, and `ready`. When
passed, amux mounts the host Docker daemon socket into the agent container,
giving the agent the ability to build and run Docker images itself.

### When to use it

Use `--allow-docker` when your work item or chat session requires the agent to:

- Build Docker images as part of its task
- Run Docker containers (e.g. test environments, services)
- Interact with the local Docker daemon in any way

### What happens

Before launching the container, amux:

1. Verifies the Docker socket exists and is accessible (fails with a clear
   error if the daemon is not running)
2. Prints a warning message so you are aware of the elevated access:

```
Docker socket: /var/run/docker.sock (found)
WARNING: --allow-docker: mounting host Docker socket into container
(/var/run/docker.sock:/var/run/docker.sock). This grants the agent elevated host access.
```

3. Mounts the socket into the container:
   - Linux / macOS: `-v /var/run/docker.sock:/var/run/docker.sock`
   - Windows: `--mount type=npipe,source=\\.\pipe\docker_engine,target=\\.\pipe\docker_engine`

### Security note

Mounting the Docker socket grants the agent the ability to run arbitrary
containers on the host. Only use `--allow-docker` when you trust the agent and
the work item requires it. This is an intentional capability escalation —
`amux` will never mount the Docker socket without this explicit flag.

### Examples

```sh
amux implement 0005 --allow-docker    # implement a work item that needs Docker
amux chat --allow-docker              # start a chat session with Docker access
amux ready --refresh --allow-docker   # run audit with Docker access in container
```

---

## Worktree Isolation

The `--worktree` flag on `amux implement` runs the agent against an isolated
Git worktree instead of your main working tree. The worktree is a separate
checkout of your repository stored in `~/.amux/worktrees/<repo>/<NNNN>/`, so
the agent's changes are completely isolated from your current branch until you
decide to merge them.

### Why use it

- The agent can make large changes without risking unstable commits landing on
  your working branch mid-implementation
- You can review the full diff in one place before it touches your main tree
- If the agent's output isn't what you wanted, discard it with a single key
  press — no `git reset` required
- Works with `--workflow`: every step in the workflow operates in the same
  isolated worktree, producing one coherent diff to review at the end

### How it works

1. amux verifies that `git` ≥ 2.5 is installed (required for worktree support)
2. A branch named `amux/work-item-NNNN` is created from your current `HEAD`
3. A worktree is checked out at `~/.amux/worktrees/<repo-name>/<NNNN>/`
4. The agent container is launched with the worktree as its mounted directory
5. After the agent finishes (success or error), a prompt asks what to do with
   the branch

### Post-run prompt (command mode)

```
Worktree branch `amux/work-item-0030` is ready. Merge into current branch? [y/n/s(kip-and-keep)]
```

| Key | Action |
|-----|--------|
| `y` | Merge the branch into the current branch (`git merge --no-ff`), then remove the worktree and delete the branch |
| `n` | Discard — remove the worktree and delete the branch |
| `s` | Skip — keep the worktree and branch as-is for manual review; prints the path |

### Post-run dialog (TUI mode)

After the container exits, a dialog appears:

```
╭─── Worktree: Merge or Discard? ───────────────────────╮
│                                                        │
│  Branch 'amux/work-item-0030' completed.               │
│                                                        │
│  [m/y] Merge into current branch                       │
│  [d]   Discard (delete branch + worktree)              │
│  [s/Esc] Keep worktree branch as-is                    │
│                                                        │
╰────────────────────────────────────────────────────────╯
```

If the container exited with an error the dialog notes it: "Branch
`amux/work-item-0030` finished with errors." — you can still choose to merge
partial progress.

### Resuming an interrupted run

If a worktree already exists at the target path (e.g. because the previous run
was interrupted), amux detects it and prompts:

```
Worktree already exists at ~/.amux/worktrees/myrepo/0030.
[r]esume — reuse existing worktree
[R]ecreate — remove it and start fresh
```

### Merge conflicts

If the merge fails due to conflicts, amux prints a recovery message and leaves
the worktree in place:

```
Merge failed with conflicts — resolve manually in /path/to/repo,
then run: git branch -d amux/work-item-0030 && git worktree remove ~/.amux/worktrees/myrepo/0030
```

### Commit signing (GPG, SSH, S/MIME)

When you have Git commit signing enabled (`commit.gpgsign = true`,
`gpg.format = ssh`, or `gpg.format = x509`), the `git commit` steps in the
worktree merge flow require a passphrase prompt. To allow this to work cleanly,
amux **suspends the TUI** around every `git commit` it runs, then restores it
once the command returns.

What you see when the TUI suspends:

```
[amux] running: git commit -m "Implement amux/work-item-0030"
```

At this point the terminal is in normal (non-TUI) mode. Your configured
passphrase prompt — `pinentry-curses`, `pinentry-mac`, an SSH `ssh-askpass`,
or whichever variant your system uses — appears and operates normally. After
you enter your passphrase (or if no prompt is needed because the key is cached
in the agent), the TUI is restored and rendering resumes.

This affects two points in the worktree flow:

| Step | What amux runs |
|------|----------------|
| **Commit uncommitted files** — the optional step before merge, when the agent left files unstaged | `git commit -m "<message>"` in the worktree directory |
| **Squash-merge commit** — after `git merge --squash` succeeds on the main branch | `git commit -m "Implement <branch>"` in the Git root |

If the `git commit` fails (e.g. wrong passphrase, cancelled prompt), the TUI
is still restored and the error is shown in the execution window output. The
merge flow stops at the failed step so you can retry from the dialog.

Users without commit signing configured see no change — the suspend/restore
round-trip is invisible when no passphrase prompt appears.

### Edge cases

| Situation | Behaviour |
|-----------|-----------|
| `git` < 2.5 installed | Error before launch: "git ≥ 2.5 is required for --worktree support" |
| Detached HEAD state | Warning printed; worktree created from current commit; proceed |
| Branch already exists, no worktree directory | Worktree created using existing branch (no `-b` flag) |
| Merge conflict | Error printed with manual resolution instructions; worktree kept |
| Combined with `--workflow` | All workflow-step containers use the same worktree |
| Combined with `--mount-ssh` | Both flags apply independently; SSH mount added alongside worktree mount |
| GPG / SSH / S-MIME commit signing enabled | TUI suspends for each `git commit`, passphrase prompt works normally, TUI restores after |
| Commit signing passphrase wrong or cancelled | TUI restored before error is shown; merge flow stops at the failed step |

### Worktree storage

Worktrees are stored at `~/.amux/worktrees/<repo-name>/<NNNN>/`. The
`<repo-name>` is the last component of the Git root path (e.g. `myproject`).
Parent directories are created automatically.

### Examples

```sh
amux implement 0030 --worktree                          # isolated run; prompt to merge after
amux implement 0030 --worktree --workflow wf.md         # multi-step workflow in one worktree
amux implement 0030 --worktree --mount-ssh              # worktree + SSH keys in container
```

---

## SSH Key Mounting

The `--mount-ssh` flag mounts your host `~/.ssh` directory read-only into the
agent container, so the agent can authenticate with remote Git servers using
your existing SSH keys.

### When to use it

Use `--mount-ssh` when your work item or chat session requires the agent to:

- Clone private repositories over SSH
- Push branches or tags to a remote
- Run `git fetch` / `git pull` against SSH remotes
- Authenticate with any other service that uses SSH keys

### What happens

Before launching the container, amux:

1. Resolves `~/.ssh` using the platform home directory (cross-platform safe)
2. Verifies the directory exists — if not, the command fails with a clear error:
   ```
   Host ~/.ssh directory not found; cannot use --mount-ssh
   ```
3. Prints a warning so you are aware of the mount:
   ```
   WARNING: --mount-ssh: mounting host ~/.ssh into container (read-only). Ensure you trust the agent image.
   ```
4. Mounts the directory read-only into the container:
   ```
   -v /home/user/.ssh:/root/.ssh:ro
   ```

The `:ro` flag prevents the agent from modifying your host SSH keys.

### Security notes

- `~/.ssh` is **never** mounted unless `--mount-ssh` is explicitly passed. There
  is no config file option — it must be an explicit per-invocation opt-in.
- The mount is read-only: the agent can use your keys but cannot change them.
- SSH key permissions must be correct on the host (`600` for private keys).
  Docker bind mounts inherit host permissions; keys with loose permissions may
  be rejected by `ssh` inside the container.
- Only use `--mount-ssh` when you trust the agent image. A compromised image
  could read your SSH keys during the run.

### Combining flags

`--mount-ssh` can be combined freely with any other flag:

```sh
amux implement 0030 --mount-ssh                      # SSH keys in implement container
amux chat --mount-ssh                                # SSH keys in chat container
amux implement 0030 --worktree --mount-ssh           # worktree isolation + SSH keys
amux implement 0030 --mount-ssh --workflow wf.md     # every workflow-step container gets SSH keys
amux implement 0030 --mount-ssh --allow-docker       # SSH keys + Docker socket
```

When used with `--workflow`, the SSH directory is mounted into **every**
workflow-step container, not just the first.

### Examples

```sh
amux implement 0030 --mount-ssh       # agent can push/pull over SSH
amux chat --mount-ssh                 # freeform session with SSH access
```

---

## Docker Command Visibility

Every time amux runs a Docker command (`docker build` or `docker run`), the
full CLI command is displayed:

- **Command mode**: printed to stdout before the command runs
- **TUI mode**: included as the first line in the execution window output

This lets you see exactly what Docker invocation amux is making, e.g.:

```
$ docker build -t amux-myapp:latest -f Dockerfile.dev /path/to/repo
$ docker run --rm -it -v /path/to/repo:/workspace -w /workspace -e CLAUDE_CODE_OAUTH_TOKEN=*** amux-myapp:latest claude "Implement work item 0001..."
```

---

## Configuration

### Per-repository: `GITROOT/aspec/.amux.json`

```json
{
  "agent": "claude"
}
```

### Global: `$HOME/.amux/config.json`

```json
{
  "default_agent": "claude"
}
```

---

## Build & Development

```sh
make all                      # cargo build --release
make install                  # build + install to /usr/local/bin/ (may need sudo)
make test                     # cargo test
make clean                    # cargo clean
make release VERSION=v1.0.0   # create and publish a release (see below)
```

### Releasing

`make release VERSION=vx.y.z` automates the full release process:

1. Switches to `main`, pulls latest, and verifies a clean working tree
2. Creates `docs/releases/vx.y.z.md` with a release notes template
3. Launches `amux chat` so you can prompt the agent to write release notes
4. Runs all tests locally
5. Commits the release notes and tags the commit with the version
6. Pushes the commit and tag to `main`
7. Creates a GitHub Release with the release notes via `gh`

The tag push triggers the release CI pipeline, which builds binaries for all
platforms and uploads them to the GitHub Release.
