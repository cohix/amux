# Work Item: Enhancement

Title: workflow auto-advance
Issue: issuelink

## Summary:
- When a workflow agent container becomes stuck (no output for the stuck threshold) in the TUI, automatically open the workflow control dialog so the user can take action without needing to manually press Ctrl+W.
- Reduce the stuck detection threshold from 30s to 10s.
- Only auto-open the dialog if the stuck tab is the currently active tab; if the stuck workflow is on a background tab, defer until the user switches to it.

## User Stories

### User Story 1:
As a: user

I want to:
have the workflow control dialog appear automatically when the running agent container stops producing output

So I can:
take action (advance, restart, or cancel) without having to notice the yellow stuck indicator and manually press Ctrl+W

### User Story 2:
As a: user

I want to:
the auto-advance dialog to appear faster (within 10 seconds of silence) rather than waiting 30 seconds

So I can:
recover from a stuck workflow agent more quickly and keep momentum on my work item

### User Story 3:
As a: user

I want to:
the auto-advance dialog to stay hidden while I am on a different tab, only appearing when I switch to the stuck tab

So I can:
continue working on other tabs without being interrupted by dialogs from background workflows


## Implementation Details:

- **Reduce `STUCK_TIMEOUT`**: In `src/tui/state.rs`, change `STUCK_TIMEOUT` from `Duration::from_secs(30)` to `Duration::from_secs(10)`. Update all existing test assertions that use the 30s/29s boundary to use 10s/9s instead.

- **Track whether the dialog has been auto-opened**: Add a boolean field `workflow_stuck_dialog_opened: bool` to `TabState` (default `false`) to prevent the dialog from being re-opened on every tick once it has already been shown for the current stuck episode. Reset this flag to `false` in `acknowledge_stuck()` and `finish_command()` so it is ready for the next stuck episode.

- **Auto-open logic in `tick_all()`**: After the existing per-tab `tick()` call loop in `AppState::tick_all()`, add a check for the active tab only. If the active tab:
  1. `is_stuck()` returns `true`
  2. has a `workflow_current_step` (i.e., is running a workflow)
  3. has `dialog == Dialog::None` (no dialog is already open)
  4. `workflow_stuck_dialog_opened` is `false`

  then set `tab.dialog = Dialog::WorkflowControlBoard { current_step: step, error: None }` and set `workflow_stuck_dialog_opened = true`. Because `tick_all` iterates over all tabs but the active-tab check uses `self.active_tab_idx`, access the active tab by index after the loop rather than inside the `for tab in &mut self.tabs` loop (which would require splitting the borrow).

- **Deferred auto-open on tab switch**: In the tab-switch handler inside `input::handle_key` (the branch that changes `app.active_tab_idx`), after switching, call `acknowledge_stuck` on the new active tab to reset the timer — this preserves existing behavior. Then in `tick_all`, the deferred auto-open will trigger naturally on the next tick if the newly-active tab is already in a stuck state with `workflow_stuck_dialog_opened = false`. No extra special-casing is needed beyond the active-tab-only check described above.

- **Reset flag on acknowledge**: In `TabState::acknowledge_stuck()`, also reset `workflow_stuck_dialog_opened = false` so that if the user dismisses the dialog (which calls `acknowledge_stuck` via Esc), the auto-open can fire again after the next silence window.

- **No changes to `handle_workflow_control_board`**: The dialog dismiss paths (Esc, arrow keys) already call `acknowledge_stuck` or clear the dialog. Verify that Esc calls `acknowledge_stuck` so the cooldown is properly reset.


## Edge Case Considerations:

- **Dialog already open**: If the user has manually opened a different dialog (e.g., `QuitConfirm`), the `dialog == Dialog::None` guard prevents clobbering it. The auto-open will not happen while any non-`None` dialog is active.

- **Non-workflow running containers**: If a tab is running a plain `implement` or `chat` without a workflow, `workflow_current_step` will be `None`, so the auto-open guard will not fire. Existing stuck-tab behavior (yellow color, `⚠️` label) is unchanged.

- **Maximized container window**: The auto-open check does **not** apply the maximized guard — the dialog should appear over the top of a maximized container PTY when stuck logic fires. The existing `Ctrl+W` guard (which prevents manually opening the dialog while maximized) is unchanged. When the auto-opened `WorkflowControlBoard` dialog is visible and the container window is maximized, keyboard input must be routed to the dialog handler instead of the PTY. Concretely, in `src/tui/input.rs`, the branch that normally forwards raw keystrokes to the PTY when `container_window == ContainerWindowState::Maximized` should first check whether `dialog != Dialog::None`; if a dialog is open, dispatch the key to the dialog handler and skip PTY forwarding. This ensures the user can navigate and confirm the dialog without the keystrokes leaking into the container.

- **Multiple stuck tabs**: Only the active tab is checked. If two background tabs are stuck, the auto-open fires for each one in turn as the user visits them, which is the intended deferred behavior.

- **Rapid output bursts then silence**: `last_output_time` is reset on every PTY byte, so a burst of output restarts the 10s clock. `workflow_stuck_dialog_opened` is only set after the threshold is exceeded, so a tab that was briefly stuck but recovered before the next tick will not auto-open.

- **Workflow completes while dialog is open**: `finish_command()` clears `last_output_time` and resets `workflow_stuck_dialog_opened`. The next `WorkflowStepConfirm` or similar dialog will replace `WorkflowControlBoard` through normal workflow advancement logic.

- **User dismisses dialog without acting**: Pressing Esc calls `acknowledge_stuck()`, which resets `last_output_time` to now and clears `workflow_stuck_dialog_opened`. The auto-open will only fire again after another full 10s of silence, preventing a rapid dialog loop.


## Test Considerations:

- **Unit: reduced threshold constant**: Assert `STUCK_TIMEOUT == Duration::from_secs(10)`. Update the existing `is_stuck_true_when_container_silent_over_30s` test name and boundary values to reflect the 10s threshold.

- **Unit: `workflow_stuck_dialog_opened` initialises to `false`**: `TabState::new` should produce a tab with this field set to `false`.

- **Unit: `finish_command` resets `workflow_stuck_dialog_opened`**: Set the flag to `true`, call `finish_command(0)`, assert the flag is `false`.

- **Unit: `acknowledge_stuck` resets `workflow_stuck_dialog_opened`**: Set the flag to `true`, call `acknowledge_stuck()`, assert the flag is `false`.

- **Integration: `tick_all` opens dialog for active stuck workflow tab**: Create an `App` with one tab in a running workflow state, wind `last_output_time` back past 10s, call `tick_all()`, assert `active_tab().dialog == Dialog::WorkflowControlBoard { .. }` and `workflow_stuck_dialog_opened == true`.

- **Integration: `tick_all` does not reopen dialog if `workflow_stuck_dialog_opened` is true**: After the first auto-open, change `dialog` back to `Dialog::None` but leave `workflow_stuck_dialog_opened = true`. Call `tick_all()` again. Assert that the dialog is not reopened.

- **Integration: `tick_all` does not auto-open for a non-active stuck workflow tab**: Create an `App` with two tabs; make tab index 1 (inactive) stuck with a workflow step. Call `tick_all()`. Assert that tab 1's dialog remains `Dialog::None`.

- **Integration: `tick_all` does not auto-open when a different dialog is already active**: Set `dialog = Dialog::QuitConfirm` on the active stuck workflow tab. Call `tick_all()`. Assert dialog is still `QuitConfirm`.

- **Integration: `tick_all` does not auto-open for stuck non-workflow containers**: Active tab is stuck with a plain `implement` run (`workflow_current_step = None`). Call `tick_all()`. Assert `dialog == Dialog::None`.

- **Integration: `tick_all` auto-opens dialog even when container window is Maximized**: Set `container_window = ContainerWindowState::Maximized` on the active stuck workflow tab. Call `tick_all()`. Assert `active_tab().dialog == Dialog::WorkflowControlBoard { .. }` — the maximized state must not suppress the auto-open.

- **Integration: keyboard input routes to dialog, not PTY, when dialog is open over maximized container**: Simulate a key press with `container_window == Maximized` and `dialog == Dialog::WorkflowControlBoard { .. }`. Assert that the key is handled by the dialog dispatch path and is not forwarded to the PTY writer.

- **End-to-end: switching to a stuck background tab triggers dialog on next tick**: Set up two tabs; make the inactive one stuck. Switch active tab. Call `tick_all()`. Assert the dialog opens on the now-active tab.


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- Primary changes are in `src/tui/state.rs` (threshold constant, `TabState` field, `tick_all` auto-open logic, `acknowledge_stuck` and `finish_command` resets). One targeted change is also required in `src/tui/input.rs`: in the maximized-PTY input forwarding branch, add a guard that skips PTY forwarding and dispatches to the dialog handler when `dialog != Dialog::None`. No changes to `src/tui/render.rs` or any command-mode code are required (the dialog already renders over all other content).
- The `STUCK_TIMEOUT` constant is public; update all test literals that reference 29s/30s/31s to 9s/10s/11s equivalents to keep tests accurate.
- Add new tests in `src/tui/state.rs` alongside the existing stuck-detection test block (`// --- Stuck tab detection tests ---`).
