# Multi-Agent Workflows

amux supports **multi-step workflows** — reusable Markdown files that define a sequence of agent runs, each focused on a discrete phase of work (e.g. plan → implement → review → docs).

amux parses the workflow into a dependency graph (DAG), runs each step inside a Docker container in order, pauses between steps for your review, and shows a live status strip in the TUI so you always know where things stand.

---

## Quick start

```bash
# Run the bundled example workflow against work item 0027
amux implement 0027 --workflow aspec/workflows/implement-feature.md
```

The TUI shows a strip of coloured step boxes. Pressing `Enter` after each step advances to the next one. At any point you can press `q` to pause — the state is saved to disk so you can resume later.

---

## Workflow file format

A workflow is a plain Markdown file you write and own. amux looks for:

| Element | Description |
|---|---|
| `# Title` | Optional top-level heading used for display only |
| `## Step: <name>` | Defines a step (the name must be unique in the file) |
| `Depends-on: <step>[, <step>…]` | Declares upstream dependencies (omit for root steps) |
| `Prompt:` | Everything after this keyword (until the next heading) is the step's prompt template |

### Example

```markdown
# Implement Feature Workflow

## Step: plan
Prompt: Read the following work item and produce an implementation plan.

{{work_item_content}}

## Step: implement
Depends-on: plan
Prompt: Implement work item {{work_item_number}} according to the plan.

Follow the spec: {{work_item_section:[Implementation Details]}}

## Step: review
Depends-on: implement
Prompt: Review the changes from the implement step for correctness and style.

## Step: docs
Depends-on: implement
Prompt: Write documentation for work item {{work_item_number}}.
```

In this example `review` and `docs` both depend only on `implement` — they form a **parallel group** and will be executed sequentially in the order they appear in the file.

---

## Prompt template variables

| Variable | Replaced with |
|---|---|
| `{{work_item_number}}` | Zero-padded four-digit work item number (e.g. `0027`) |
| `{{work_item_content}}` | Full text of the work item Markdown file |
| `{{work_item_section:[Name]}}` | Content of the named H1/H2 section from the work item file (case-insensitive) |

If a variable is unknown or the named section is not found, the placeholder is left in place with a warning.

---

## Running a workflow

### CLI (command mode)

```bash
amux implement <work-item> --workflow <path/to/workflow.md> [flags]
```

Flags work identically to the regular `implement` command:

| Flag | Description |
|---|---|
| `--non-interactive` | Run each step's agent in print/non-interactive mode |
| `--plan` | Launch each step's agent in plan (read-only) mode |
| `--allow-docker` | Mount the host Docker socket into each step's container |
| `--worktree` | Run all steps in an isolated Git worktree (see [Worktree Isolation](usage.md#worktree-isolation)) |
| `--mount-ssh` | Mount host `~/.ssh` read-only into each step's container (see [SSH Key Mounting](usage.md#ssh-key-mounting)) |

Between steps amux prints the step summary and prompts:

```
Step 'plan' completed.
Next step(s): implement
Press [Enter] to advance, or [q] to abort:
```

On agent failure:

```
Step 'implement' failed: Container exited with code 1
Press [r] to retry, or any other key to abort:
```

### TUI (interactive mode)

In the TUI type:

```
implement 0027 --workflow=aspec/workflows/implement-feature.md
```

A **workflow status strip** appears between the execution window and the command box, showing each step as a coloured box:

| Colour | Status |
|---|---|
| Grey / dim | Pending |
| Blue / bold | Running |
| Green | Done |
| Red / bold | Error |
| Yellow / bold | Stuck (idle for >10 s) |

When a step completes, a confirmation dialog appears. Press `Enter` or `y` to advance, `q` or `Esc` to pause.

---

## Workflow control board (TUI only)

While a workflow step is **running**, press **Ctrl+W** (with the container window minimized) to open the **workflow control board** — a diamond-shaped popup that lets you redirect execution in real time without waiting for the current step to finish.

```
╭──────── Workflow Control ────────╮
│ Step: implement                  │
│                                  │
│         ↑ Restart current step   │
│                                  │
│ ← Cancel to prev   → Next: new   │
│                                  │
│         ↓ Next: same container   │
│                                  │
│  [Arrow] select   [Esc] dismiss  │
╰──────────────────────────────────╯
```

### Controls

| Key | Action |
|-----|--------|
| **↑** | **Restart current step** — resets the running step to Pending and relaunches it in a fresh container |
| **←** | **Cancel to previous step** — marks the current step Pending and rolls back the most recently completed step so it runs again |
| **→** | **Next step: new container** — marks the current step Done and advances to the next step in a brand-new container |
| **↓** | **Next step: same container** — marks the current step Done and sends the next step's prompt to the existing container via its open PTY session |
| **Esc** | Dismiss the control board without changing anything |

Each action persists workflow state to disk before launching any new execution, so an unexpected exit mid-action leaves the state consistent.

### Auto-advance when stuck

If a running workflow step produces no container output for **10 seconds**, amux considers it stuck and **automatically opens the workflow control board** so you can take action without having to notice the yellow indicator and manually press Ctrl+W.

The auto-open fires only when all of the following are true:

- The stuck tab is the **currently active tab** (background tabs are deferred)
- A workflow step is currently running
- No other dialog is already open
- The control board has not already been auto-opened for this stuck episode

The auto-open works even when the container window is **maximized** — the dialog appears over the full-screen terminal view and keyboard input is routed to the dialog rather than the PTY.

**After you dismiss with Esc**, the stuck timer resets. If the container remains silent for another 10 seconds, the dialog re-opens. This prevents a rapid re-open loop while still keeping you informed.

**Background tab deferral** — if a workflow step goes silent on a tab you are not currently viewing, the auto-open is suppressed. When you switch to that tab, the control board opens on the next tick (within ~100 ms) if the step is still stuck.

### When Ctrl+W is available (manual)

Press **Ctrl+W** to open the control board manually. This requires **all** of the following:

- A workflow is active in the current tab
- A step is currently running
- The container window is **minimized** (not maximized / full-screen)
- No other dialog is already open

Ctrl+W is deliberately blocked when the container window is maximized — the auto-open path handles that case instead. When a workflow is running and the container window is minimized, a hint appears below the execution window:

```
Press Ctrl+w for workflow controls
```

When the container window is maximized (PTY has full keyboard focus), the hint changes to:

```
Press Esc to minimize container, then Ctrl+w for workflow controls
```

### Next step: same container

The **↓ Next: same container** action reuses the already-running Docker container instead of spinning up a new one. The next step's prompt is written directly to the container's PTY stdin, exactly as if you had typed it at the shell prompt. The container window is automatically maximized so you can follow along.

This is useful when:
- The container has already installed dependencies or built an intermediate artifact that the next step needs
- You want to avoid the overhead of starting a new Docker container

If the PTY session has already closed by the time you press **↓**, amux falls back to launching a new container and shows a status message:

```
PTY session ended — starting new container
```

### Edge cases

| Situation | Behaviour |
|---|---|
| **↑ Restart** when current step has no next step | Relaunches the current step; no workflow-complete transition triggered |
| **← Cancel** on the first step (no predecessor) | Dialog stays open with an error: `No previous step to return to` |
| **← Cancel** with parallel predecessors | Rolls back the most recently completed step (last `Done` step by file order) |
| **→ / ↓ Next** when current step is the final step | Transitions to workflow-complete state; no new launch |
| **↓ Next: same container** with closed PTY | Falls back to new container; shows status message |
| Container window maximized (manual Ctrl+W) | Ctrl+W is suppressed; hint guides you to minimize first |
| Container window maximized (auto-open) | Dialog opens over the maximized terminal; input routes to the dialog |
| Another dialog already open | Both Ctrl+W and auto-open are suppressed until the open dialog is dismissed |
| Step goes silent on a background tab | Auto-open is deferred; control board appears when you switch to that tab |
| Esc dismissed; container still silent | Timer resets; dialog re-opens after another 10 s of silence |
| Output resumes before the 10 s threshold | Stuck state clears; auto-open does not trigger |

---

## Parallel groups

Steps that share the same `Depends-on` set form a **parallel group**. amux v0.3 executes them **sequentially** in file order (true parallel container execution is a future enhancement). In the TUI they are rendered stacked vertically with slight indentation to indicate sequential-within-group ordering.

If a group has more than two steps, the third box shows `+ N more…` to keep the strip compact (max 3 rows).

---

## Workflow state persistence

amux saves workflow state to:

```
$GITROOT/.amux/workflows/<repo-hash8>-<work-item>-<workflow-name>.json
```

The file stores the status of every step, the container ID used for each step, and a SHA-256 hash of the workflow file.

### Resuming

If amux finds an existing state file when you run `implement --workflow`, it offers:

```
Found a saved workflow state for 'implement-feature' (work item 0027).
  1) Resume from where you left off
  2) Restart from the beginning
  [1/2]:
```

Choosing `2` deletes the old state file and starts fresh.

### Workflow file changed

If the workflow file has been modified since the state was saved (the hash differs), amux warns you:

```
WARNING: The workflow file has changed since the last run.
  1) Restart from the beginning
  2) Continue anyway (could be dangerous)
  [1/2]:
```

If you choose `2`, amux verifies that the step names and `Depends-on` values are identical. If they are not, it forces a restart.

### Interrupted steps

If a step was `Running` when amux last exited, on the next run you are asked:

```
Step 'implement' was running when the previous session ended.
Start it over (s) or skip to next step (n)? [s/n]:
```

---

## Bundled example workflow

`aspec/workflows/implement-feature.md` defines a four-step plan → implement → review + docs workflow suitable for most feature work items. Copy and customise it for your project.

---

## Edge cases

| Situation | Behaviour |
|---|---|
| Cycle in `Depends-on` graph | Error before any agent runs |
| Unknown `Depends-on` step name | Error at parse time |
| Empty workflow file (no steps) | Rejected with a helpful message |
| Work item file not found | Error before loading the workflow |
| Workflow file not found / unreadable | Clear error with the file path |
| Agent failure mid-workflow | Step marked `Error`; user prompted to retry or abort |
| Very long step names | Truncated to 12 characters with `…` in the TUI strip |
| Large number of parallel steps | Capped at 3 visible rows; extra shown as `+ N more…` |
| Large number of sequential steps | `+ N more…` box at the far right of the strip |

---

## Limitations (v0.3)

- **Sequential only**: steps in a parallel group run one at a time in file order. True concurrent container execution is not yet supported.
- **TUI resume dialogs**: in TUI mode, hash-mismatch and resume prompts use the simpler auto-restart behaviour rather than a full dialog.
