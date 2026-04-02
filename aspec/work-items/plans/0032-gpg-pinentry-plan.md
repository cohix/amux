# Implementation Plan: 0032 — GPG Pinentry TUI Fix

## Overview

When GPG commit signing is enabled, `git commit` spawns `gpg-agent` → `pinentry`, which
claims the terminal directly.  Because Ratatui holds the terminal in raw mode with an
alternate screen active, pinentry's TTY access wipes the TUI completely and it does not
recover.

The fix is to **suspend the TUI around any git operation that may trigger a passphrase
prompt**, then restore it afterwards.  This is the standard pattern used by lazygit, vim
(`:!cmd`), and other TUI tools when ceding terminal ownership to a subprocess.

---

## Design Decision: Suspend/Restore Without Touching `Terminal<B>`

Three options were considered:

1. **Suspend TUI around git commit, using crossterm directly** ← chosen
2. Run git in a PTY rendered inside the TUI — `pinentry-curses` expects to own the
   terminal; its ANSI output inside a sub-pane would be garbled across all pinentry
   variants.
3. Detect signing and strip `--gpg-sign` then re-sign separately — fragile, requires
   parsing user git config, leaves transient unsigned commits.

Option 1 is correct because:
- Works with every pinentry variant and every signing method (GPG, SSH key signing,
  S/MIME) without special-casing any of them.
- Ratatui/crossterm already import `disable_raw_mode`, `enable_raw_mode`,
  `LeaveAlternateScreen`, `EnterAlternateScreen` in `src/tui/mod.rs` — no new deps.
- The `Terminal<B>` handle is a local in `run_app()` and is **not** stored in `App`.
  Threading it through `handle_action` → handlers would require making those functions
  generic over `B`, significantly widening the diff.  A `needs_full_redraw` flag on
  `App` achieves the same result cleanly.
- Users without GPG signing see zero behavioral change.

---

## Affected Files

| File | Change |
|---|---|
| `src/tui/state.rs` | Add `needs_full_redraw: bool` field to `App` |
| `src/tui/mod.rs` | New `run_git_interactive()` helper; update two call sites; event-loop clear |

---

## Current Code Facts (verified against source)

- `run_git_show(tab: &mut TabState, cwd: &Path, args: &[&str]) -> bool` — synchronous,
  captures stdout+stderr into the tab output pane.
- `handle_worktree_commit_files(app: &mut App, message, branch, wt_path, git_root)` —
  calls `run_git_show` for both `git add -A` and `git commit -m`.
- `handle_worktree_merge_confirmed(app: &mut App, branch, wt_path, git_root)` — calls
  `run_git_show` for `git merge --squash` and then `git commit -m`.
- `handle_action(app: &mut App, action: Action)` — dispatches to both handlers; no
  `terminal` parameter.
- `run_app<B>(terminal: &mut Terminal<B>, ...)` — `terminal` is local here; the event
  loop calls `terminal.draw()` on every iteration.
- `App` struct (in `state.rs`) has fields `tabs`, `active_tab_idx`, `should_quit`,
  `tui_tabs_shared` — no terminal, no redraw flag.
- Crossterm imports already present in `mod.rs`:
  `disable_raw_mode`, `enable_raw_mode`, `LeaveAlternateScreen`, `EnterAlternateScreen`,
  `execute`.

---

## Step-by-Step Changes

### Step 1 — Add `needs_full_redraw` to `App` (`src/tui/state.rs`)

Add a boolean field to the `App` struct and initialise it to `false`:

```rust
pub struct App {
    pub tabs: Vec<TabState>,
    pub active_tab_idx: usize,
    pub should_quit: bool,
    pub tui_tabs_shared: Arc<Mutex<Vec<TuiTabInfo>>>,
    /// Set to true after a TUI suspend/restore so the event loop can call
    /// `terminal.clear()` before the next draw, ensuring a full re-render.
    pub needs_full_redraw: bool,
}
```

In `App::new()`, add `needs_full_redraw: false`.

---

### Step 2 — Add `run_git_interactive()` (`src/tui/mod.rs`)

Add alongside the existing `run_git_show()`:

```rust
/// Run a git command that may require interactive TTY access (e.g. GPG passphrase prompt).
///
/// Suspends the Ratatui terminal before executing (leaves alternate screen, disables raw
/// mode) so that pinentry or any other TTY-based subprocess gets clean terminal ownership.
/// Restores the terminal afterwards and sets `app.needs_full_redraw` so the event loop
/// triggers a full re-render on the next tick.
///
/// Returns `true` if the command exited with status 0.
fn run_git_interactive(app: &mut App, cwd: &std::path::Path, args: &[&str]) -> bool {
    // Announce the operation so the user knows why the TUI disappeared.
    println!("\n[amux] running: git {}\n", args.join(" "));

    // Suspend: leave alternate screen then disable raw mode (order matters —
    // leaving the alternate screen while still in raw mode produces garbage output).
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();

    // Run with inherited stdio so GPG/pinentry gets full terminal access.
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status();

    // Restore unconditionally — even when the git command failed.
    let _ = enable_raw_mode();
    let _ = execute!(io::stdout(), EnterAlternateScreen);

    // Signal the event loop to call terminal.clear() before the next draw.
    app.needs_full_redraw = true;

    match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            app.active_tab_mut().push_output(format!(
                "git {} exited with code {}",
                args.join(" "),
                s.code().unwrap_or(-1)
            ));
            false
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("git {}: {e}", args.join(" ")));
            false
        }
    }
}
```

**Error-safety note:** The restore block (`enable_raw_mode` + `EnterAlternateScreen`) runs
before the `match` on `status`.  If the `Command::status()` call itself panics (highly
unlikely — it only panics on OOM), the restore would not run.  That is acceptable; the
alternative (a `Drop` guard) adds complexity disproportionate to this edge case.  The
spec's requirement is satisfied: a non-zero exit still restores the terminal.

---

### Step 3 — Update `handle_worktree_commit_files()` (`src/tui/mod.rs`)

Current code (lines ~456–459):

```rust
let tab = app.active_tab_mut();
run_git_show(tab, &wt_path, &["add", "-A"]);
run_git_show(tab, &wt_path, &["commit", "-m", &message]);
```

Replace with:

```rust
{
    let tab = app.active_tab_mut();
    run_git_show(tab, &wt_path, &["add", "-A"]);
}
if !run_git_interactive(app, &wt_path, &["commit", "-m", &message]) {
    // Error already pushed to output; stay in the current state so the user sees it.
    return;
}
```

`git add -A` never triggers signing so it stays as `run_git_show`.

The block around `run_git_show` is required because `run_git_show` borrows `app` through
`active_tab_mut()`, and `run_git_interactive` needs `&mut App` — the borrow must end
before the second call.

---

### Step 4 — Update `handle_worktree_merge_confirmed()` (`src/tui/mod.rs`)

Current code (lines ~477–481):

```rust
let tab = app.active_tab_mut();
let merge_ok = run_git_show(tab, &git_root, &["merge", "--squash", &branch]);
if merge_ok {
    run_git_show(tab, &git_root, &["commit", "-m", &commit_msg]);
}
```

Replace with:

```rust
{
    let tab = app.active_tab_mut();
    let merge_ok = run_git_show(tab, &git_root, &["merge", "--squash", &branch]);
    if !merge_ok {
        return;
    }
}
if !run_git_interactive(app, &git_root, &["commit", "-m", &commit_msg]) {
    return;
}
```

`git merge --squash` does not commit, so it never triggers signing; it stays as
`run_git_show`.

Same borrow-splitting rationale as Step 3.

---

### Step 5 — Trigger full redraw in the event loop (`src/tui/mod.rs`)

In `run_app<B>`, at the top of the main `loop { … }`, before `terminal.draw(…)`:

```rust
if app.needs_full_redraw {
    app.needs_full_redraw = false;
    let _ = terminal.clear();
}
terminal.draw(|f| render::draw(f, &mut app))?;
```

`terminal.clear()` resets Ratatui's internal diff buffer, so the subsequent `draw()`
performs a full re-render rather than sending only the incremental diff (which would be
wrong after the terminal was reset by `LeaveAlternateScreen`/`EnterAlternateScreen`).

---

## What Does NOT Change

- `run_git_show()` — unchanged; continues to be used for all non-signing git operations.
- All dialog rendering and state transitions — purely I/O layer change.
- `pty.rs` — not involved.
- Behavior for users without GPG signing — the suspend/restore round-trip is
  imperceptible when no passphrase prompt appears (~1 ms).

---

## Test Plan

### Unit tests

Add to `src/tui/mod.rs` (in a `#[cfg(test)]` module) or a dedicated test file:

| Test | What it asserts |
|---|---|
| `run_git_interactive_success` | Calls with `["--version"]`; asserts `true` returned and `needs_full_redraw` is set. |
| `run_git_interactive_nonzero_exit` | Calls with `["commit", "--no-such-flag"]` in a temp dir; asserts `false` returned, error pushed to tab output, `needs_full_redraw` still set. |
| `run_git_interactive_restore_on_failure` | After a failing command, assert that `enable_raw_mode()` was called (verify via crossterm's `is_raw_mode_enabled()` returning `true` after the call). |

Note: these tests will manipulate actual terminal raw mode.  They must be run
single-threaded (`--test-threads=1`) or each test must save/restore terminal state.
Consider wrapping with a helper that records the initial raw-mode state and restores it
in a `defer`-style pattern.

### Manual / integration test

1. Create a test repo with GPG signing: `git config commit.gpgsign true`.
2. Run an `amux` worktree workflow to completion (agent finishes, files staged).
3. Submit the commit message dialog.
4. Confirm: TUI disappears cleanly, GPG passphrase prompt appears on a normal terminal,
   passphrase entry works, TUI returns intact with a full re-render.
5. Repeat for the squash-merge commit on the main branch.
6. Repeat with SSH key signing (`git config gpg.format ssh`).
7. Confirm users without signing enabled see no visible change.

---

## Implementation Order

1. `src/tui/state.rs` — add `needs_full_redraw: bool` to `App`.
2. `src/tui/mod.rs` — add `run_git_interactive()` helper.
3. `src/tui/mod.rs` — update `handle_worktree_commit_files()`.
4. `src/tui/mod.rs` — update `handle_worktree_merge_confirmed()`.
5. `src/tui/mod.rs` — add `needs_full_redraw` check in the event loop.
6. Write unit tests.
7. Manual test with GPG signing enabled.
