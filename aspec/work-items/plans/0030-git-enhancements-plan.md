# Implementation Plan: 0030 — Git Enhancements (Worktree Isolation & SSH Key Mounting)

## Overview

Two independent features that share a single PR:

1. **`--worktree`** on `amux implement` — creates an isolated Git worktree, mounts it into every container, and prompts to merge/discard after completion.
2. **`--allow-ssh`** on `amux chat` and `amux implement` — mounts `~/.ssh` read-only into every container.

No new external crate dependencies are required.

---

## Phase 1 — New `src/git.rs` Module

Create `/workspace/src/git.rs` to encapsulate all `git worktree` subprocess calls. Register it in `lib.rs` / `main.rs` with `pub mod git;` or `mod git;`.

### 1.1 — `git_version_check()`

```rust
/// Verify that `git` is installed and version >= 2.5 (worktree support).
/// Returns Err with a clear message if not.
pub fn git_version_check() -> Result<()>
```

- Run `git --version`, parse `"git version X.Y.Z"`, compare major/minor.
- Error: `"git >= 2.5 is required for --worktree support (found: <version>)"`.

### 1.2 — `worktree_path(git_root: &Path, work_item: u32) -> PathBuf`

```rust
/// Returns ~/.amux/worktrees/<repo-name>/<NNNN>/
/// <repo-name> = last path component of git_root
pub fn worktree_path(git_root: &Path, work_item: u32) -> Result<PathBuf>
```

- Uses `dirs::home_dir()` (already a dependency).
- Work item zero-padded to four digits: `format!("{:04}", work_item)`.

### 1.3 — `worktree_branch_name(work_item: u32) -> String`

```rust
/// Returns "amux/work-item-NNNN"
pub fn worktree_branch_name(work_item: u32) -> String
```

### 1.4 — `branch_exists(git_root: &Path, branch: &str) -> bool`

```rust
/// Returns true if `git rev-parse --verify refs/heads/<branch>` exits 0.
pub fn branch_exists(git_root: &Path, branch: &str) -> bool
```

### 1.5 — `is_detached_head(git_root: &Path) -> bool`

```rust
/// Returns true if git_root is in detached HEAD state.
pub fn is_detached_head(git_root: &Path) -> bool
```

- Run `git symbolic-ref --quiet HEAD`; exit code != 0 means detached.

### 1.6 — `create_worktree(git_root, worktree_path, branch) -> Result<()>`

```rust
/// Create a new worktree.
/// - If branch does not exist: `git worktree add <path> -b <branch>`
/// - If branch exists but no worktree dir: `git worktree add <path> <branch>`
/// Caller handles the "worktree dir already exists" case before calling this.
pub fn create_worktree(git_root: &Path, worktree_path: &Path, branch: &str) -> Result<()>
```

- Uses `std::process::Command`.
- All git commands run with `.current_dir(git_root)`.

### 1.7 — `remove_worktree(git_root, worktree_path) -> Result<()>`

```rust
/// `git worktree remove --force <worktree_path>`
pub fn remove_worktree(git_root: &Path, worktree_path: &Path) -> Result<()>
```

### 1.8 — `merge_branch(git_root, branch) -> Result<()>`

```rust
/// `git merge --no-ff <branch>` run from git_root.
/// Returns Err if merge fails (e.g. conflicts).
pub fn merge_branch(git_root: &Path, branch: &str) -> Result<()>
```

### 1.9 — `delete_branch(git_root, branch) -> Result<()>`

```rust
/// `git branch -d <branch>`
pub fn delete_branch(git_root: &Path, branch: &str) -> Result<()>
```

### 1.10 — Unit Tests in `src/git.rs`

Each function has unit tests. Since the tests invoke real `git` commands, they require a temp dir initialized as a git repo (use `tempfile::TempDir` + `git init`). Test cases:

- `worktree_path`: correct path format under `~/.amux/worktrees/`.
- `worktree_branch_name`: zero-padded to 4 digits.
- `branch_exists`: true after branch creation, false otherwise.
- `is_detached_head`: true in detached state, false on a branch.
- `create_worktree`: new dir exists after call; errors when path already occupied.
- `remove_worktree`: dir removed after call.
- `merge_branch`: fast-forward merge succeeds; conflict returns Err.
- `git_version_check`: passes on the CI environment.

---

## Phase 2 — CLI Changes (`src/cli.rs`)

### 2.1 — Add `worktree: bool` to `Implement`

```rust
// In the Implement variant:
#[arg(long, help = "Run in an isolated Git worktree under ~/.amux/worktrees/")]
worktree: bool,
```

### 2.2 — Add `mount_ssh: bool` to `Implement`

```rust
#[arg(long, help = "Mount host ~/.ssh read-only into the agent container")]
mount_ssh: bool,
```

### 2.3 — Add `mount_ssh: bool` to `Chat`

```rust
#[arg(long, help = "Mount host ~/.ssh read-only into the agent container")]
mount_ssh: bool,
```

### 2.4 — Thread through `main.rs` / `commands/mod.rs`

Pass the new fields into the command dispatch:

```rust
Command::Implement { work_item, non_interactive, plan, allow_docker, workflow, worktree, mount_ssh } =>
    implement::run(&work_item, non_interactive, plan, allow_docker, workflow.as_deref(), worktree, mount_ssh).await?,

Command::Chat { non_interactive, plan, allow_docker, mount_ssh } =>
    chat::run(non_interactive, plan, allow_docker, mount_ssh).await?,
```

### 2.5 — `PendingCommand` in `src/tui/state.rs`

Add the new fields to `PendingCommand::Implement` and `PendingCommand::Chat`:

```rust
Implement { work_item, non_interactive, plan, allow_docker, workflow: Option<PathBuf>, worktree: bool, mount_ssh: bool },
Chat { non_interactive, plan, allow_docker, mount_ssh: bool },
```

---

## Phase 3 — Docker Changes (`src/docker/mod.rs`)

### 3.1 — Add `ssh_dir: Option<PathBuf>` to all `build_run_args*` helpers

Affected functions (all take similar arg lists):
- `build_run_args`
- `build_run_args_display`
- `build_run_args_pty`
- `build_run_args_pty_display`
- `build_run_args_pty_at_path`

In each, after the `allow_docker` socket mount block, append:

```rust
if let Some(ref ssh_path) = ssh_dir {
    args.push("-v".to_string());
    args.push(format!("{}:/root/.ssh:ro", ssh_path.display()));
}
```

`build_run_args_display` shows the actual path (not masked) per spec.

### 3.2 — Propagate `ssh_dir` through `run_container` and `run_container_captured`

Both call `build_run_args`; add `ssh_dir: Option<PathBuf>` parameter and forward it.

`run_container_at_path` and `run_container_captured_at_path` are used for non-standard mounts (nanoclaw); add the parameter there too for consistency, even if `--allow-ssh` is never set for those paths.

### 3.3 — No new structs needed

`ssh_dir` is a simple optional path, not a settings object.

---

## Phase 4 — Agent Runner (`src/commands/agent.rs`)

### 4.1 — Add `mount_ssh: bool` parameter to `run_agent_with_sink`

New signature:

```rust
pub async fn run_agent_with_sink(
    entrypoint: Vec<String>,
    status_message: &str,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
    mount_ssh: bool,                      // NEW
    container_name_override: Option<String>,
) -> Result<()>
```

### 4.2 — SSH warning

When `mount_ssh == true`, print before launching:

```
WARNING: --allow-ssh: mounting host ~/.ssh into container (read-only). Ensure you trust the agent image.
```

Use the same `out.println(...)` approach as the existing `--allow-docker` warning.

### 4.3 — Resolve `~/.ssh` and pass to Docker helpers

```rust
let ssh_dir: Option<PathBuf> = if mount_ssh {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Cannot resolve home directory"))?;
    let ssh = home.join(".ssh");
    if !ssh.exists() {
        bail!("Host ~/.ssh directory not found; cannot use --allow-ssh");
    }
    Some(ssh)
} else {
    None
};
```

Pass `ssh_dir` to every `run_container` / `run_container_captured` call.

---

## Phase 5 — `src/commands/implement.rs`

### 5.1 — Update `run()` signature

```rust
pub async fn run(
    work_item_str: &str,
    non_interactive: bool,
    plan: bool,
    allow_docker: bool,
    workflow_path: Option<&Path>,
    worktree: bool,       // NEW
    mount_ssh: bool,      // NEW
) -> Result<()>
```

### 5.2 — Worktree setup in `run()`

At the start of `run()`, after resolving `git_root`:

```rust
if worktree {
    git::git_version_check()?;
    if git::is_detached_head(&git_root) {
        eprintln!("WARNING: You are in detached HEAD state. The worktree branch will be created from the current commit. \
                   Consider checking out a branch first.");
    }
}
```

### 5.3 — `mount_path` resolution in `run()`

Replace the current `confirm_mount_scope_stdin` call location:

```rust
let (mount_path, worktree_info) = if worktree {
    let wt_path = git::worktree_path(&git_root, work_item)?;
    let branch = git::worktree_branch_name(work_item);
    let wt_path = prepare_worktree_cmd(&git_root, &wt_path, &branch)?;
    (wt_path, Some((branch,)))   // carry branch name for post-run prompt
} else {
    let mp = confirm_mount_scope_stdin(&git_root)?;
    (mp, None)
};
```

Where `prepare_worktree_cmd` (private helper):
1. If `wt_path` exists and is a directory → prompt: `"Worktree already exists at <path>. [r]esume / [R]ecreate? "` (stdin). `r` → reuse as-is. `R` → `remove_worktree` + `create_worktree`.
2. Else → `std::fs::create_dir_all(wt_path.parent())` + `create_worktree`.
3. Returns the worktree path.

### 5.4 — Post-run merge prompt in `run()` (command mode)

After the container/workflow completes (regardless of success/error):

```rust
if let Some((branch,)) = worktree_info {
    post_run_merge_prompt_stdin(&git_root, &mount_path, &branch)?;
}
```

`post_run_merge_prompt_stdin` (private helper):
```
Worktree branch `amux/work-item-NNNN` is ready. Merge into current branch? [y/n/s(kip-and-keep)]
```
- `y` → `git::merge_branch(&git_root, &branch)?`; on merge conflict error: print clear message (see edge cases), return without removing worktree. On success: `git::remove_worktree(&git_root, &wt_path)?` + `git::delete_branch(&git_root, &branch)?`.
- `n` → `git::remove_worktree(&git_root, &wt_path)?` + `git::delete_branch(&git_root, &branch)?`.
- `s` → print `"Worktree kept at: <wt_path>"`. Return.

### 5.5 — Update `run_with_sink()` signature

```rust
pub async fn run_with_sink(
    work_item: u32,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    plan: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
    worktree: bool,       // NEW
    mount_ssh: bool,      // NEW
) -> Result<()>
```

When `worktree == true` in `run_with_sink`:
- Call `git::git_version_check()?`.
- Compute worktree path and call `prepare_worktree_cmd` (using TUI dialog instead of stdin — see Phase 7).
- Set `mount_override = Some(wt_path)`.
- After the container exits, fire the TUI merge dialog (see Phase 7).

### 5.6 — Update `run_workflow()` signature

```rust
async fn run_workflow(
    work_item: u32,
    workflow_path: &Path,
    git_root: &Path,
    mount_path: PathBuf,
    env_vars: Vec<(String, String)>,
    agent: &str,
    host_settings: Option<docker::HostSettings>,
    non_interactive: bool,
    plan: bool,
    allow_docker: bool,
    mount_ssh: bool,      // NEW
) -> Result<()>
```

Propagate `mount_ssh` to every `run_agent_with_sink` call inside the loop. The worktree mount path is already passed as `mount_path`, so no further changes to `run_workflow` are needed for `--worktree`.

### 5.7 — Propagate `mount_ssh` to every `run_agent_with_sink` call

Both the single-container path and each iteration of the workflow loop must pass `mount_ssh`.

---

## Phase 6 — `src/commands/chat.rs`

### 6.1 — Update `run()` signature

```rust
pub async fn run(non_interactive: bool, plan: bool, allow_docker: bool, mount_ssh: bool) -> Result<()>
```

### 6.2 — Update `run_with_sink()` signature

```rust
pub async fn run_with_sink(
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    plan: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
    mount_ssh: bool,      // NEW
) -> Result<()>
```

### 6.3 — Propagate `mount_ssh` to `run_agent_with_sink`

---

## Phase 7 — TUI Changes

### 7.1 — New `Dialog` variant: `WorktreeMergePrompt`

In `src/tui/state.rs`, add to the `Dialog` enum:

```rust
WorktreeMergePrompt {
    branch: String,
    worktree_path: PathBuf,
    git_root: PathBuf,
    had_error: bool,   // true if container exited with error (show note)
},
```

### 7.2 — New `TabState` fields

In `TabState`:

```rust
pub worktree_branch: Option<String>,      // set when --worktree, cleared after merge/discard
pub worktree_path: Option<PathBuf>,       // ~/ .amux/worktrees/<repo>/<NNNN>/
pub worktree_git_root: Option<PathBuf>,   // for merge/remove calls
```

### 7.3 — Worktree path in status bar (render.rs)

In the status bar rendering section of `src/tui/render.rs`:

- When `tab.worktree_path.is_some()` and a container is currently active for that tab:
  - Replace the `CWD: ...` text with a blue-colored `Using Worktree: <path>`.
- When no container is active, show normal CWD.

Implementation note: check `matches!(tab.phase, ExecutionPhase::Running { .. })` and `tab.worktree_path.is_some()`. Use `Style::default().fg(Color::Blue)` to render it.

### 7.4 — `WorktreeMergePrompt` dialog rendering (render.rs)

Add `draw_worktree_merge_prompt(frame, area, branch, worktree_path, had_error)`:

- Popup style: rounded border, yellow title `"Worktree Ready"`.
- Width: ~60 chars. Height: ~10 lines (12 if `had_error`).
- Content:
  ```
  Branch: amux/work-item-NNNN

  [y] Merge into current branch
  [n] Discard worktree and branch
  [s] Skip — keep worktree for manual review

  (optional if had_error: "Note: container exited with an error;")
  (optional:              "partial changes may still be worth reviewing.")
  ```
- Footer: `" [y/n/s] select "`

Call `draw_worktree_merge_prompt` from the main `draw_dialog` match on `Dialog::WorktreeMergePrompt`.

### 7.5 — `WorktreeMergePrompt` key handling (input.rs)

Add `handle_worktree_merge_prompt(tab, key) -> Action` and new `Action` variants:

```rust
// New Action variants:
WorktreeMerge,
WorktreeDiscard,
WorktreeSkip,
```

Handler logic:
- `'y'` → `tab.dialog = Dialog::None`; return `Action::WorktreeMerge`.
- `'n'` → `tab.dialog = Dialog::None`; return `Action::WorktreeDiscard`.
- `'s'` → `tab.dialog = Dialog::None`; return `Action::WorktreeSkip`.
- `Esc` → dismiss (same as `s` — keep worktree).

In the TUI event loop (`src/tui/mod.rs`), handle these actions:

**`WorktreeMerge`:**
```rust
let tab = app.active_tab();
if let (Some(branch), Some(wt_path), Some(git_root)) =
    (tab.worktree_branch.clone(), tab.worktree_path.clone(), tab.worktree_git_root.clone())
{
    match git::merge_branch(&git_root, &branch) {
        Ok(()) => {
            let _ = git::remove_worktree(&git_root, &wt_path);
            let _ = git::delete_branch(&git_root, &branch);
            tab.worktree_branch = None;
            tab.worktree_path = None;
            tab.worktree_git_root = None;
            // append success message to output
        }
        Err(e) => {
            // show error in a new dialog or output line:
            // "Merge failed with conflicts — resolve manually in <git_root>,
            //  then run: git branch -d amux/work-item-NNNN && git worktree remove <wt_path>"
            tab.dialog = Dialog::WorktreeMergePrompt { ..., had_error: true };
        }
    }
}
```

**`WorktreeDiscard`:**
```rust
let _ = git::remove_worktree(&git_root, &wt_path);
let _ = git::delete_branch(&git_root, &branch);
tab.worktree_branch = None; tab.worktree_path = None; tab.worktree_git_root = None;
```

**`WorktreeSkip`:**
```rust
// print path to output, clear fields
out.println(format!("Worktree kept at: {}", wt_path.display()));
tab.worktree_branch = None; tab.worktree_path = None; tab.worktree_git_root = None;
```

### 7.6 — Triggering the dialog from `run_with_sink`

After the container (or workflow) finishes in `implement::run_with_sink`, send a message to the TUI event loop through an existing channel (e.g. `out.send_event(...)` or equivalent) that triggers `Dialog::WorktreeMergePrompt`. The specific mechanism should follow the same pattern used by `WorkflowStepConfirm` — inspect how that dialog is triggered from the workflow loop in `src/tui/mod.rs` and replicate for the worktree case.

### 7.7 — `PendingCommand` changes

Update the existing `PendingCommand::Implement` and `PendingCommand::Chat` arms in `src/tui/mod.rs` where `run_with_sink` / `chat::run_with_sink` are called, to pass the new `worktree` and `mount_ssh` parameters.

---

## Phase 8 — Tests

### 8.1 — `src/git.rs` unit tests

(Detailed in Phase 1.10 above.)

### 8.2 — `src/cli.rs` unit tests

Add test cases (follow existing CLI parse test pattern):

```
amux implement 0001 --worktree                 → worktree: true, mount_ssh: false
amux implement 0001                            → worktree: false, mount_ssh: false
amux implement 0001 --worktree --workflow wf.md → both flags correctly parsed
amux implement 0001 --allow-ssh                → mount_ssh: true
amux implement 0001 --worktree --allow-ssh     → both true
amux chat --allow-ssh                          → mount_ssh: true
amux chat                                      → mount_ssh: false
```

### 8.3 — `src/docker/mod.rs` unit tests

```rust
// ssh_dir: Some(path) → args include "-v <path>:/root/.ssh:ro"
// ssh_dir: None       → args do NOT contain "/.ssh"
```

Test both `build_run_args_display` and `build_run_args`.

### 8.4 — Integration tests (`tests/`)

- `run_agent_with_sink` with `mount_ssh: true` → verify docker helpers receive `ssh_dir: Some(...)`.
- `run_agent_with_sink` with `mount_ssh: false` → verify `ssh_dir: None`.
- Workflow loop: verify `mount_ssh` propagated to every step container call (mock or capture args).

### 8.5 — End-to-end tests (`tests/`)

These require a real git repo and optionally Docker:
- `amux implement 0001 --worktree` → worktree exists at expected path; container launched with that as mount path; merge prompt appears.
- `amux implement 0001 --allow-ssh` → SSH warning printed; Docker command includes the SSH volume arg.
- `amux chat --allow-ssh` → SSH warning printed; SSH volume arg present.

---

## Implementation Order

The two features are fully independent. Implement in this order to maintain compilability throughout:

1. **`src/git.rs`** — no dependencies on other changed files; all tests pass immediately.
2. **`src/docker/mod.rs`** — add `ssh_dir` param; update all call sites with `None`; tests pass.
3. **`src/commands/agent.rs`** — add `mount_ssh` param; pass `None` for now; all callers compile.
4. **`src/cli.rs`** — add new fields; update dispatch in `main.rs`/`commands/mod.rs`.
5. **`src/commands/chat.rs`** — thread `mount_ssh` through.
6. **`src/commands/implement.rs`** — thread `mount_ssh` through; add worktree setup and post-run prompt.
7. **`src/tui/state.rs`** — add `Dialog::WorktreeMergePrompt`, new `Action` variants, `TabState` fields.
8. **`src/tui/render.rs`** — add status-bar override and dialog rendering.
9. **`src/tui/input.rs`** — add key handler for the new dialog.
10. **`src/tui/mod.rs`** — handle new `Action` variants; trigger merge dialog after worktree run.
11. **Tests** — add all unit, integration, and E2E tests.

---

## Edge Cases and Their Handling Locations

| Edge Case | Location | Handling |
|---|---|---|
| `git worktree` unavailable (git < 2.5) | `git::git_version_check()` called from `implement::run` and `run_with_sink` | Bail with version message |
| Worktree path already exists | `prepare_worktree_cmd()` in `implement.rs` | stdin prompt: resume / recreate |
| Branch exists, no worktree dir | `git::create_worktree()` | Call without `-b` flag |
| Detached HEAD | `implement::run()` before `prepare_worktree_cmd` | Warning printed; proceed |
| Merge conflict after agent run | `post_run_merge_prompt_stdin()` / `WorktreeMerge` action | Print manual resolution instructions; keep worktree |
| `~/.ssh` not found | `run_agent_with_sink()` when `mount_ssh: true` | Bail with clear error |
| `--allow-ssh` + `--worktree` combined | Both flags thread independently | Both mounts applied; no conflict |
| `--allow-ssh` + `--workflow` | `mount_ssh` propagated into every `run_agent_with_sink` call in loop | Each step gets SSH mount |
| Process killed mid-workflow with worktree | Next invocation detects existing worktree dir | resume/recreate prompt |
| `~/.amux/worktrees/` parent doesn't exist | `prepare_worktree_cmd()` | `std::fs::create_dir_all` |
| Windows `$HOME/.ssh` path | `run_agent_with_sink()` | Use `dirs::home_dir()` |

---

## Files Changed Summary

| File | Change Type | Summary |
|---|---|---|
| `src/git.rs` | **New** | All git worktree helper functions |
| `src/cli.rs` | Modify | Add `worktree` + `mount_ssh` fields |
| `src/docker/mod.rs` | Modify | Add `ssh_dir: Option<PathBuf>` to all `build_run_args*` and `run_container*` |
| `src/commands/agent.rs` | Modify | Add `mount_ssh: bool`; warn; resolve `~/.ssh`; pass to docker |
| `src/commands/implement.rs` | Modify | Add `worktree` + `mount_ssh`; worktree setup; post-run prompt |
| `src/commands/chat.rs` | Modify | Add `mount_ssh`; thread through |
| `src/tui/state.rs` | Modify | New dialog variant; new `TabState` fields; update `PendingCommand` |
| `src/tui/render.rs` | Modify | Worktree status-bar override; merge dialog rendering |
| `src/tui/input.rs` | Modify | New `Action` variants; `handle_worktree_merge_prompt` |
| `src/tui/mod.rs` | Modify | Handle new actions; trigger merge dialog; update `PendingCommand` dispatch |
| `tests/` | Modify | New unit, integration, and E2E tests |
