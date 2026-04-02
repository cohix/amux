# Implementation Plan: 0031 — Workflow Auto-Advance

## Overview

When a workflow agent container goes silent, automatically open the `WorkflowControlBoard` dialog on the active tab so the user can take action without manually pressing `Ctrl+W`. Reduce the stuck threshold from 30 s to 10 s. Suppress the auto-open on background (inactive) tabs until the user switches to them.

---

## Affected Files

| File | Change type |
|---|---|
| `src/tui/state.rs` | Primary: constant, struct field, `new`, `acknowledge_stuck`, `finish_command`, `tick_all`, tests |
| `src/tui/input.rs` | Secondary: maximized-PTY guard, `handle_workflow_control_board` Esc branch |

---

## Step-by-Step Changes

### 1. Reduce `STUCK_TIMEOUT` — `src/tui/state.rs` line 15

Change:
```rust
pub const STUCK_TIMEOUT: Duration = Duration::from_secs(30);
```
to:
```rust
pub const STUCK_TIMEOUT: Duration = Duration::from_secs(10);
```

---

### 2. Add `workflow_stuck_dialog_opened` field to `TabState` — `src/tui/state.rs`

In the `TabState` struct (inside the `// --- Multi-step workflow state ---` section, alongside the existing workflow fields), add:
```rust
/// Prevents the WorkflowControlBoard dialog from re-opening on every tick
/// once it has already been auto-shown for the current stuck episode.
/// Reset by acknowledge_stuck() and finish_command().
pub workflow_stuck_dialog_opened: bool,
```

In `TabState::new()`, initialize it:
```rust
workflow_stuck_dialog_opened: false,
```

---

### 3. Reset flag in `acknowledge_stuck()` — `src/tui/state.rs` line 749

Current:
```rust
pub fn acknowledge_stuck(&mut self) {
    if self.last_output_time.is_some() {
        self.last_output_time = Some(Instant::now());
    }
}
```

Updated (add reset before the existing body):
```rust
pub fn acknowledge_stuck(&mut self) {
    self.workflow_stuck_dialog_opened = false;
    if self.last_output_time.is_some() {
        self.last_output_time = Some(Instant::now());
    }
}
```

---

### 4. Reset flag in `finish_command()` — `src/tui/state.rs` line 539

Inside `finish_command`, next to the existing `self.last_output_time = None;` line (which already appears at the end of the method), add:
```rust
self.workflow_stuck_dialog_opened = false;
```

---

### 5. Auto-open logic in `tick_all()` — `src/tui/state.rs` line 1013

Current `tick_all`:
```rust
pub fn tick_all(&mut self) {
    for tab in &mut self.tabs {
        tab.tick();
    }
    // ... snapshot / shared-state update ...
}
```

After the `for tab in &mut self.tabs { tab.tick(); }` loop, and *before* the snapshot block, add an active-tab check using an index (to avoid borrow conflicts with the loop):

```rust
// Auto-open WorkflowControlBoard on the active tab when it becomes stuck.
let active = self.active_tab_idx;
if active < self.tabs.len() {
    let tab = &mut self.tabs[active];
    if tab.is_stuck()
        && tab.workflow_current_step.is_some()
        && tab.dialog == Dialog::None
        && !tab.workflow_stuck_dialog_opened
    {
        let step = tab.workflow_current_step.clone().unwrap();
        tab.dialog = Dialog::WorkflowControlBoard {
            current_step: step,
            error: None,
        };
        tab.workflow_stuck_dialog_opened = true;
    }
}
```

**Why index, not iterator**: The preceding `for tab in &mut self.tabs` loop ends before this block, so there is no active mutable borrow. Indexing into `self.tabs[active]` is safe and avoids a second mutable iteration.

> **Important — no `container_window != Maximized` guard here**: Unlike the manual Ctrl+W path, the auto-open does **not** check `container_window`. When the container is fullscreen and the agent goes silent, the dialog must still appear over the maximized window (Step 7 ensures keystrokes then route to the dialog rather than the PTY). This is an intentional divergence from Ctrl+W behaviour — see [Ctrl+W vs. auto-open](#ctrlw-vs-auto-open) below.

---

### 6. Fix `handle_workflow_control_board` Esc — `src/tui/input.rs` line ~964

Current:
```rust
KeyCode::Esc => {
    tab.dialog = Dialog::None;
    Action::None
}
```

Updated (Esc must call `acknowledge_stuck` so the cooldown timer resets and `workflow_stuck_dialog_opened` is cleared):
```rust
KeyCode::Esc => {
    tab.dialog = Dialog::None;
    tab.acknowledge_stuck();
    Action::None
}
```

This ensures that after the user dismisses the dialog with Esc, the auto-open will not immediately re-trigger; it will only fire again after another full 10 s of silence.

---

### 7. Dialog-over-maximized guard in `handle_window_key()` — `src/tui/input.rs` line ~180

Current inner maximized block:
```rust
if tab.container_window == ContainerWindowState::Maximized {
    if key.code == KeyCode::Esc {
        tab.container_window = ContainerWindowState::Minimized;
        return Action::None;
    }
    // All other keys forwarded to the PTY for full interactivity.
    if let Some(bytes) = key_to_bytes(&key) {
        return Action::ForwardToPty(bytes);
    }
    return Action::None;
}
```

Add a dialog check at the top of the maximized block, before the Esc/PTY branches:
```rust
if tab.container_window == ContainerWindowState::Maximized {
    // If a dialog is open over the maximized container, route input to the
    // dialog handler instead of the PTY.
    if tab.dialog != Dialog::None {
        return handle_dialog(tab, key);
    }
    if key.code == KeyCode::Esc {
        tab.container_window = ContainerWindowState::Minimized;
        return Action::None;
    }
    // All other keys forwarded to the PTY for full interactivity.
    if let Some(bytes) = key_to_bytes(&key) {
        return Action::ForwardToPty(bytes);
    }
    return Action::None;
}
```

> **Note**: verify the name of the general dialog-dispatch function in `input.rs` (likely `handle_dialog` or similar). If the dispatch is done via a `match tab.dialog` block rather than a named helper, replicate the same `match` inline or extract a helper. The goal is that keystrokes reach the `WorkflowControlBoard` handler, not the PTY, when the dialog is non-`None`.

---

---

## Ctrl+W vs. Auto-Open

These two paths open the same dialog but have deliberately different guards:

| Guard | Ctrl+W (manual) | Auto-open (`tick_all`) |
|---|---|---|
| `dialog == Dialog::None` | ✓ | ✓ |
| `workflow.is_some()` | ✓ | — (covered by `workflow_current_step.is_some()`) |
| `workflow_current_step.is_some()` | ✓ | ✓ |
| `phase == Running` | ✓ | ✓ (implied by `is_stuck()`) |
| `container_window != Maximized` | **✓ (kept)** | **✗ (intentionally absent)** |
| `!workflow_stuck_dialog_opened` | — | ✓ |

**Rationale for the difference**: Ctrl+W is a deliberate user keypress while interacting with the PTY in fullscreen mode; requiring the window to be non-maximized first avoids an awkward interruption of focused terminal work. The auto-open fires because the agent has silently stalled — the user is not actively typing, so overlaying the dialog over a maximized window is safe and desirable.

The existing Ctrl+W handler (lines 151–163 of `src/tui/input.rs`) is **not changed**. Its `container_window != ContainerWindowState::Maximized` guard stays intact.

---

## No Changes Required

- `src/tui/render.rs` — the `WorkflowControlBoard` dialog already renders over all content, including over a maximized container window.
- Tab-switch handler in `src/tui/mod.rs` — already calls `acknowledge_stuck()` on the new active tab (lines 302–318). On the next `tick_all()` call the `workflow_stuck_dialog_opened = false` reset (from `acknowledge_stuck`) means the auto-open will fire naturally if the tab is still stuck.
- Non-workflow stuck behavior — unchanged; the `workflow_current_step.is_some()` guard in `tick_all` keeps plain `implement`/`chat` tabs unaffected.
- Ctrl+W handler guard — `container_window != ContainerWindowState::Maximized` is left exactly as-is in the manual path.

---

## Test Plan

All new tests live in `src/tui/state.rs` inside or immediately after the `// --- Stuck tab detection tests ---` block (line ~1574), except the input-routing test which lives in `src/tui/input.rs`.

### Unit tests (`src/tui/state.rs`)

| Test name | What it asserts |
|---|---|
| `stuck_timeout_is_10s` | `STUCK_TIMEOUT == Duration::from_secs(10)` |
| `workflow_stuck_dialog_opened_initialises_false` | `TabState::new(…).workflow_stuck_dialog_opened == false` |
| `finish_command_resets_workflow_stuck_dialog_opened` | Set flag `true`, call `finish_command(0)`, assert `false` |
| `acknowledge_stuck_resets_workflow_stuck_dialog_opened` | Set flag `true`, call `acknowledge_stuck()`, assert `false` |

### Updates to existing tests

The following existing tests reference 29 s / 30 s / 31 s threshold values and must be updated:

| Old test name | Change |
|---|---|
| `is_stuck_true_when_container_silent_over_30s` | Rename to `…over_10s`; wind clock back by 11 s instead of 31 s |
| `is_stuck_false_exactly_at_timeout_boundary` | Use 9 s elapsed (just under 10 s) |
| Any other assertions referencing `30` / `29` / `31` | Update to `10` / `9` / `11` accordingly |

### Integration tests (`src/tui/state.rs`)

| Test name | Setup | Assert |
|---|---|---|
| `tick_all_opens_dialog_for_active_stuck_workflow_tab` | One tab, Running phase, `workflow_current_step = Some("step")`, `last_output_time` wound back 11 s | After `tick_all()`: `active_tab().dialog == WorkflowControlBoard { current_step: "step", error: None }` and `workflow_stuck_dialog_opened == true` |
| `tick_all_does_not_reopen_dialog_if_flag_set` | Same as above but `workflow_stuck_dialog_opened = true`, `dialog` reset to `None` | After `tick_all()`: `dialog == Dialog::None` |
| `tick_all_does_not_auto_open_for_background_stuck_workflow_tab` | Two tabs; tab 1 (index 1, inactive) stuck with workflow step; `active_tab_idx = 0` | After `tick_all()`: `tabs[1].dialog == Dialog::None` |
| `tick_all_does_not_auto_open_when_different_dialog_active` | Active tab stuck with workflow step; `dialog = Dialog::QuitConfirm` | After `tick_all()`: `dialog == Dialog::QuitConfirm` (unchanged) |
| `tick_all_does_not_auto_open_for_stuck_non_workflow_container` | Active tab stuck, `workflow_current_step = None` | After `tick_all()`: `dialog == Dialog::None` |
| `tick_all_auto_opens_dialog_when_container_maximized` | Active tab stuck with workflow step, `container_window = Maximized` | After `tick_all()`: `dialog == WorkflowControlBoard { .. }` |

### End-to-end test (`src/tui/state.rs`)

| Test name | Setup | Assert |
|---|---|---|
| `switching_to_stuck_background_tab_triggers_dialog_on_next_tick` | Two tabs; tab 1 stuck with workflow, `active_tab_idx = 0`; call `tick_all()` (confirms no dialog yet); switch `active_tab_idx = 1`, call `acknowledge_stuck()` on tab 1, then `tick_all()` again | `tabs[1].dialog == WorkflowControlBoard { .. }` |

### Input routing tests (`src/tui/input.rs`)

| Test name | Setup | Assert |
|---|---|---|
| `keys_route_to_dialog_not_pty_when_dialog_open_over_maximized_container` | Tab with `container_window = Maximized`, `dialog = WorkflowControlBoard { .. }`, simulate a key press (e.g., `KeyCode::Esc`) | Returned `Action` is NOT `ForwardToPty(_)`; dialog handler consumed the key (dialog cleared or updated) |
| `ctrl_w_does_not_open_dialog_when_container_maximized` | Running workflow tab, `container_window = Maximized`, no dialog | After Ctrl+W: `dialog == Dialog::None` (guard blocks it) — confirm existing behavior is preserved |

---

## Implementation Order

1. `STUCK_TIMEOUT` constant change (trivial, unblocks test updates).
2. Add `workflow_stuck_dialog_opened` field and initialise in `new()`.
3. Reset in `acknowledge_stuck()` and `finish_command()`.
4. Update `tick_all()` with auto-open logic.
5. Fix `handle_workflow_control_board` Esc to call `acknowledge_stuck()`.
6. Add dialog guard in `handle_window_key()` for maximized + dialog state.
7. Update existing stuck-detection tests (threshold boundary values).
8. Add all new unit, integration, and input-routing tests.
