# Work Item: Enhancement

Title: Workflow Keyboard Control Board
Issue: issuelink

## Summary:
- When a TUI tab has a running workflow, pressing Ctrl+W opens a "workflow control board" dialog — a diamond-shaped popup with four arrow-key-mapped controls for directing workflow execution in real time.

## User Stories

### User Story 1:
As a: user

I want to:
press Ctrl+W at any point during a running workflow to open a control dialog

So I can:
quickly decide how the workflow should proceed without waiting for the current step to finish

### User Story 2:
As a: user

I want to:
navigate the control board with arrow keys and confirm my choice with Enter (or dismiss with Esc)

So I can:
restart a misbehaving step, branch to a new container, or cancel back to the previous step without leaving the TUI

### User Story 3:
As a: user

I want to:
see the four workflow control options arranged in a diamond shape, labeled clearly

So I can:
understand at a glance which direction maps to which action before I commit to a keypress


## Implementation Details:

### 1. New `Dialog` variant — `WorkflowControlBoard`
Add to the `Dialog` enum in `src/tui/state.rs`:
```rust
WorkflowControlBoard {
    current_step: String,
    // selected option tracked in render/input, not stored here
},
```

### 2. New `Action` variants
Add to the `Action` enum in `src/tui/input.rs`:
```rust
WorkflowRestartStep,          // Up arrow — restart current step
WorkflowCancelToPrevious,     // Left arrow — cancel and return to previous step
WorkflowNextInNewContainer,   // Right arrow — start next step in a new container
WorkflowNextInCurrentContainer, // Down arrow — start next step in current container
```

### 3. Ctrl+W key binding (input.rs)
In `handle_key()`, before the existing dialog dispatch block, add:
- If `key == Ctrl+W` and `tab.phase == ExecutionPhase::Running` and `tab.workflow.is_some()` and `tab.workflow_current_step.is_some()`:
  - Set `tab.dialog = Dialog::WorkflowControlBoard { current_step: step.clone() }`
  - Return `Action::None`
- This binding must be suppressed when the container window is maximized (PTY has full focus), consistent with other Ctrl shortcuts.
- When a workflow is running in the current tab, show a hint in the TUI (bold yellow text) "Press ctrl-w when container is minimized for workflow controls"

### 4. Dialog input handler — `handle_workflow_control_board()` (input.rs)
New function called from the dialog dispatch block at the top of `handle_key()`:
- `KeyCode::Up`    → dismiss dialog, return `Action::WorkflowRestartStep`
- `KeyCode::Left`  → dismiss dialog, return `Action::WorkflowCancelToPrevious`
- `KeyCode::Right` → dismiss dialog, return `Action::WorkflowNextInNewContainer`
- `KeyCode::Down`  → dismiss dialog, return `Action::WorkflowNextInCurrentContainer`
- `KeyCode::Esc`   → dismiss dialog, return `Action::None`
- All other keys   → return `Action::None` (dialog stays open)

### 5. Render the dialog (render.rs)
New function `draw_workflow_control_board(f, area, step_name)`:
- Centered popup, ~30 cols × 11 rows
- Title: `" Workflow Control "` (bordered block)
- Current step name shown at top inside the box
- Diamond layout (using spaces + label text):
  ```
          ↑ Restart current step
  ← Cancel to prev step      → Next in new container
          ↓ Next in current container
  ```
- Arrow keys highlighted with bold/cyan style; labels in normal style
- Bottom hint line: `"[Arrow] select  [Esc] cancel"`
- Call this renderer from the dialog rendering section of `draw_ui()`, similar to existing dialog popups.

### 6. Action dispatch in `src/tui/mod.rs`
In the main event loop where `Action` variants are matched (ensure running containers are killed where appropriate):

- `WorkflowRestartStep`:
  - Reset current step status to `Pending` via `workflow.set_status(step, StepStatus::Pending)`
  - Persist state
  - Call existing `launch_next_workflow_step()` (reuses retry logic — same as `WorkflowRetry`)

- `WorkflowCancelToPrevious`:
  - Mark current step `Pending` (undo Running status)
  - Find the predecessor step: the last step in `workflow.steps` whose status is `Done` (simple linear scan; for parallel graphs, pick the most recently completed)
  - Mark that predecessor step `Pending`
  - Persist state
  - Call `launch_next_workflow_step()` which will pick up the now-ready predecessor

- `WorkflowNextInNewContainer`:
  - Mark current step `Done` (treat as success, skip confirmation dialog)
  - Persist state
  - Call `launch_next_workflow_step()` — existing logic already spawns each step in its own container

- `WorkflowNextInCurrentContainer`:
  - Mark current step `Done`
  - Persist state
  - Call a new variant of `launch_next_workflow_step()` (or a flag parameter `reuse_container: bool`) that, when `true`, reuses `tab.container_info` instead of starting a new `docker run`; the next step's prompt is sent to the existing PTY session via stdin and "return" key is sent.

### 7. "Reuse container" execution path
`launch_next_workflow_step_in_current_container()` in `src/tui/mod.rs`:
- Asserts `tab.pty.is_some()` and `tab.container_info.is_some()`
- Substitutes the next step's prompt template
- Sends the prompt string directly to the existing PTY via `tab.pty.as_mut().unwrap().write(prompt)`, then send enter/return key.
- Updates `tab.workflow_current_step` and step status to `Running`
- Persists state
- Does NOT spawn a new Docker container, automatically maximizes existing container window/pty to show what was done.

### 8. Ctrl+W guard conditions
Do not open the dialog if:
- No workflow is active (`tab.workflow.is_none()`)
- No step is currently running (`tab.workflow_current_step.is_none()`)
- Container window is maximized (PTY has exclusive focus)
- Another dialog is already open (`tab.dialog != Dialog::None`)


## Edge Case Considerations:
- **First step / no predecessor**: `WorkflowCancelToPrevious` when the current step is the first step has no valid predecessor. Show a brief inline error message inside the dialog ("No previous step to return to") and keep the dialog open rather than panicking or silently doing nothing.
- **Parallel steps**: When multiple steps share the same `depends_on` set, "previous step" is ambiguous. Use the most recently transitioned-to-Done step (by scanning `workflow.steps` in reverse). If none, treat as first-step case above.
- **No next step**: `WorkflowNextInNewContainer` and `WorkflowNextInCurrentContainer` when `workflow.next_ready()` is empty after marking current step Done should close the dialog and show the normal workflow-complete state rather than panicking.
- **PTY already closed**: If the PTY session has ended before Ctrl+W is pressed, the `WorkflowNextInCurrentContainer` path must detect the closed PTY and fall back to launching a new container, notifying the user with a status line message.
- **Workflow state file conflict**: All four actions must persist workflow state to disk before launching any new execution, so a crash mid-action leaves the state consistent.
- **Container window maximized**: Ctrl+W must be fully suppressed in maximized mode to avoid fighting the PTY for keyboard control. The guard in step 8 above handles this. The visually prominent hint in the TUI guides the user to minimize the container window to enable ctrl-w.
- **Rapid keypresses**: The dialog dismiss-and-dispatch sequence must be atomic within the event loop tick to avoid double-firing actions if the user presses keys quickly.


## Test Considerations:
- **Unit — dialog state transitions** (`src/tui/state.rs` / `src/tui/input.rs`):
  - `handle_workflow_control_board()` returns correct `Action` for each arrow key
  - Esc returns `Action::None` and clears dialog
  - Non-arrow keys leave dialog open
- **Unit — Ctrl+W guard conditions** (`src/tui/input.rs`):
  - No dialog opens when workflow is `None`
  - No dialog opens when `workflow_current_step` is `None`
  - No dialog opens when container window is maximized
  - No dialog opens when another dialog is already active
- **Unit — `WorkflowCancelToPrevious` predecessor resolution** (`src/tui/mod.rs`):
  - Returns correct predecessor for linear workflow (step N-1)
  - Returns most-recent Done step for parallel workflow
  - Returns error/noop when current step is the first step
- **Unit — `WorkflowNextInCurrentContainer`** (`src/tui/mod.rs`):
  - Prompt is sent to existing PTY rather than spawning a new container
  - Falls back gracefully when PTY is closed
- **Integration — full workflow control board flow**:
  - Simulate running workflow tab, press Ctrl+W, verify `Dialog::WorkflowControlBoard` is set
  - Simulate each arrow key, verify correct `Action` and resulting workflow state changes
  - Verify state file is persisted after each action
- **Integration — no-next-step boundary**:
  - On the final step, advance actions should transition to workflow-complete state without panic
- **Render — diamond layout**:
  - Snapshot test (or visual inspection test) confirming the four labels appear in diamond positions within the popup area


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- New `Dialog` variant goes in `src/tui/state.rs` alongside existing workflow dialogs (`WorkflowStepConfirm`, `WorkflowStepError`).
- New `Action` variants go in `src/tui/input.rs` alongside `WorkflowAdvance`, `WorkflowAbort`, `WorkflowRetry`.
- The Ctrl+W guard mirrors the existing pattern for Ctrl+T/A/D tab controls — check `container_window` state before registering the keypress.
- Dialog rendering follows the same centered-popup pattern used for `QuitConfirm` and workflow dialogs in `src/tui/render.rs`.
- The "reuse container" execution path in `mod.rs` should live near `launch_next_workflow_step()` (around line 2315) and share its prompt-substitution and state-persistence logic to avoid duplication.
- Workflow state persistence (`save_workflow_state()`) must be called after every status mutation, consistent with the existing step-completion and retry paths.
