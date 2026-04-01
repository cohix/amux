# Work Item: Feature

Title: Git Enhancements — Worktree Isolation and SSH Key Mounting
Issue: issuelink

## Summary:
- Add `--worktree` flag to `amux implement`: automatically creates a Git worktree under `~/.amux/worktrees/<repo>/<work-item>/`, mounts that worktree into the agent container (or all containers in a workflow), and after completion offers the user a dialog to merge the worktree branch back into the main tree.
- Add `--allow-ssh` flag to `amux chat` and `amux implement`: mounts the host `$HOME/.ssh` directory read-only into the agent container (or all containers in a workflow), enabling `git` inside the container to authenticate via the host's SSH keys. Never mounted unless the flag is explicitly passed.


## User Stories

### User Story 1:
As a: user

I want to: run `amux implement 0030 --worktree` so that my work item is implemented in an isolated Git worktree, leaving my current working branch untouched

So I can: review and selectively merge the agent's changes back into my main branch without risking unfinished or broken code landing there mid-implementation.

### User Story 2:
As a: user

I want to: run `amux implement 0030 --worktree --workflow ./aspec/workflows/my-flow.md` and have every container step in the workflow operate in the same isolated worktree

So I can: run multi-step agentic workflows that produce a coherent, reviewable diff in one place, isolated from my main working tree until I choose to merge.

### User Story 3:
As a: user

I want to: pass `--allow-ssh` to `amux chat` or `amux implement` so that the agent container can authenticate with remote Git servers using my existing SSH keys

So I can: let the agent clone private repos, push branches, or run `git fetch` inside the container without manually copying keys or re-configuring authentication.


## Implementation Details:

### Feature 1 — `--worktree` on `amux implement`

**CLI changes (`src/cli.rs`)**
- Add `worktree: bool` field with `#[arg(long)]` to the `Implement` variant in `src/cli.rs`.

**Worktree management (`src/git.rs` or inline in `src/commands/implement.rs`)**
- Introduce a helper (consider a new `src/git.rs` module) with:
  - `create_worktree(git_root, work_item) -> Result<(PathBuf, String)>` — computes a deterministic branch name (e.g. `amux/work-item-0030`) and worktree path (`~/.amux/worktrees/<repo-name>/<work-item-number>/`), then runs `git worktree add <path> -b <branch>`. Returns the path and branch name.
  - `remove_worktree(git_root, worktree_path) -> Result<()>` — runs `git worktree remove --force <path>`.
  - `merge_worktree_branch(git_root, branch) -> Result<()>` — runs `git merge --no-ff <branch>` in the git root.

**`implement::run` changes**
- When `worktree == true`:
  1. Call `create_worktree` to get `(worktree_path, branch_name)`.
  2. Use `worktree_path` as the `mount_path` instead of `git_root` or CWD. The worktree is a standalone directory — it already contains a `.git` file pointer, so the agent container sees it as a valid repo root.
  3. Pass `worktree_path` through the same code paths (single-container and workflow) that currently use `mount_path`.
  4. After the container (or workflow) completes — whether success or error — prompt the user via stdin (command mode) or TUI dialog (TUI mode):
     - "Worktree branch `amux/work-item-NNNN` is ready. Merge into current branch? [y/n/s(kip-and-keep)]"
     - `y` → call `merge_worktree_branch`, then `remove_worktree`.
     - `n` → call `remove_worktree` (discard the worktree and branch).
     - `s` → leave the worktree in place for manual inspection; print the worktree path.
  5. If the container exited with an error, still show the merge prompt (the agent may have made partial progress worth reviewing).

**Workflow path**
- The `run_workflow` function in `implement.rs` already accepts a `mount_path: PathBuf`. When `--worktree` is set, pass the worktree path instead of the git root. No structural changes to the workflow loop are needed; each step's container is started with the worktree path. The post-workflow merge prompt runs once after the loop exits.

**TUI integration**
- `implement::run_with_sink` should accept a `worktree: bool` parameter. When `true`, create the worktree before launching, then show a merge-or-discard dialog (using the existing Ratatui dialog/popup pattern, matching the `--workflow` controls dialog style) after the container exits.

**Storage**
- Worktrees live in `~/.amux/worktrees/<repo-name>/<NNNN>/`. `<repo-name>` is derived from the git root directory name (last component of `git_root`).
- On startup with `--worktree`, if the target worktree path already exists (from a previous interrupted run), prompt the user to reuse it or recreate it.

**Worktree path in TUI**
- When using a worktree, display the active worktree path at the bottom of the TUI (replace the current `CWD:...` text with a blue-colored `Using Worktree: {path}`) whenever a container running on a worktree is currently active in the current tab.

---

### Feature 2 — `--allow-ssh` on `amux chat` and `amux implement`

**CLI changes (`src/cli.rs`)**
- Add `mount_ssh: bool` field with `#[arg(long)]` to both `Implement` and `Chat` variants.

**Docker changes (`src/docker/mod.rs`)**
- Add an `ssh_dir: Option<PathBuf>` parameter to `run_container`, `run_container_captured`, and `build_run_args_display`.
- When `ssh_dir` is `Some(path)`, append to the Docker run args:
  ```
  -v <path>:/root/.ssh:ro
  ```
  The `:ro` (read-only) bind mount prevents the agent from modifying host SSH keys.
- `build_run_args_display` should show the actual path (not masked) since it is not a secret.

**Plumbing**
- `run_agent_with_sink` in `src/commands/agent.rs`: add `mount_ssh: bool` parameter. When `true`, resolve `$HOME/.ssh`, verify it exists, and pass it to the docker helpers.
- Propagate `mount_ssh` through:
  - `chat::run` → `run_agent_with_sink`
  - `chat::run_with_sink` → `run_agent_with_sink`
  - `implement::run` → `run_agent_with_sink` (single-container) and `run_workflow` → each step's `run_agent_with_sink` call
  - `implement::run_with_sink` → `run_agent_with_sink`
- Add a warning line (similar to the existing `--allow-docker` warning) when `--allow-ssh` is used:
  ```
  WARNING: --allow-ssh: mounting host ~/.ssh into container (read-only). Ensure you trust the agent image.
  ```

**Security constraint**
- `$HOME/.ssh` is NEVER mounted unless `--allow-ssh` is explicitly passed. There is no config file option for this; it must be an explicit per-invocation opt-in.


## Edge Case Considerations:

### Worktree (`--worktree`)
- **Branch already exists**: if `amux/work-item-NNNN` already exists (e.g. from a previous run), `git worktree add -b <branch>` will fail. Detect this case: if the worktree directory already exists and is a valid worktree, prompt the user to resume using it or delete and recreate. If only the branch exists (no worktree dir), use `git worktree add <path> <branch>` (without `-b`) to check out the existing branch.
- **Dirty working tree**: `git worktree add` does not require a clean working tree, so no special handling is needed.
- **Not on a branch (detached HEAD)**: warn the user that the worktree branch will be created from the current commit, and the merge prompt at the end will merge into "no branch" — advise them to `git checkout` a branch first.
- **Nested git repos / submodules**: the worktree path is in `~/.amux/`, outside the repo, so submodule complexity is avoided.
- **Merge conflicts after agent run**: `git merge` may fail with conflicts. Catch the error, print a clear message ("Merge failed with conflicts — resolve manually in `<git_root>`, then run `git branch -d amux/work-item-NNNN` and `git worktree remove ~/.amux/worktrees/...`"), and exit without deleting the worktree.
- **`git worktree` unavailable**: check the git version at startup (git ≥ 2.5 required). If not available, bail with a clear error.
- **Workflow interrupted mid-run**: if the process is killed during a workflow step, the worktree remains on disk. On the next `amux implement --worktree` invocation for the same work item, the existing worktree is detected and the resume/recreate prompt is shown.
- **Permissions on `~/.amux/worktrees/`**: create parent directories with `std::fs::create_dir_all`.

### SSH mounting (`--allow-ssh`)
- **`$HOME/.ssh` does not exist**: bail with a clear error ("Host ~/.ssh directory not found; cannot use --allow-ssh").
- **Permissions**: Docker bind mounts inherit host file permissions. SSH keys with incorrect permissions inside the container may cause `git` to refuse them. Document this in the warning message; suggest the user verify key permissions on the host.
- **`--allow-ssh` with `--worktree`**: both flags may be combined freely; the SSH directory is an additional mount alongside the worktree mount.
- **`--allow-ssh` with `--workflow`**: the SSH directory must be mounted into every workflow step container, not just the first. Ensure `mount_ssh` is propagated to every `run_agent_with_sink` call in the workflow loop.
- **Windows paths**: on Windows, `$HOME/.ssh` may be `C:\Users\<user>\.ssh`. Use `dirs::home_dir()` (already used in `docker/mod.rs` for `HostSettings`) for cross-platform resolution.


## Test Considerations:

### Unit tests

**`src/git.rs` (or wherever worktree helpers live)**
- `create_worktree` returns the expected path under `~/.amux/worktrees/<repo>/<work-item>/`.
- `create_worktree` errors gracefully when `git worktree add` fails (mock via fake git binary or error injection).
- Branch name is correctly formatted as `amux/work-item-NNNN`.

**`src/cli.rs`**
- `amux implement 0001 --worktree` parses `worktree: true`.
- `amux implement 0001` parses `worktree: false` (default).
- `amux implement 0001 --worktree --workflow wf.md` parses both flags correctly.
- `amux chat --allow-ssh` parses `mount_ssh: true`.
- `amux implement 0001 --allow-ssh` parses `mount_ssh: true`.
- `amux chat` defaults `mount_ssh: false`.
- `amux implement 0001 --worktree --allow-ssh` parses both flags correctly.

**`src/docker/mod.rs`**
- `build_run_args_display` with `ssh_dir: Some(path)` includes `-v <path>:/root/.ssh:ro`.
- `build_run_args_display` with `ssh_dir: None` does not include any `/.ssh` mount.

### Integration tests

- Verify that `run_agent_with_sink` with `mount_ssh: true` calls docker helpers with the SSH volume arg.
- Verify that `run_agent_with_sink` with `mount_ssh: false` does NOT include the SSH volume arg.
- Verify that the workflow loop passes `mount_ssh` to every step's container call.

### End-to-end tests

- `amux implement 0001 --worktree` (requires a real git repo and Docker): worktree is created at the expected path, container is launched with the worktree as the mount path, and after the run the merge prompt is presented.
- `amux implement 0001 --allow-ssh` displays the SSH warning and includes the SSH mount in the Docker command shown to the user.
- `amux chat --allow-ssh` displays the SSH warning and includes the SSH mount.


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- The `--allow-docker` flag's implementation in `src/cli.rs`, `src/commands/agent.rs`, and `src/docker/mod.rs` is the direct pattern to follow for `--allow-ssh`: add the CLI field, thread it through `run_agent_with_sink`, and handle it in the Docker run helpers.
- The `--workflow` flag's post-loop cleanup pattern in `implement::run_workflow` (state file removal, resume/restart prompts) is the pattern to follow for the post-worktree merge prompt.
- Use `dirs::home_dir()` (already a dependency via `docker/mod.rs`) for resolving `~/.amux/` and `~/.ssh` — no new dependencies needed for path resolution.
- Use `std::process::Command` (already used throughout `docker/mod.rs`) to invoke `git worktree` commands, consistent with how Docker CLI commands are run.
- The existing `confirm_mount_scope_stdin` + stdin prompt pattern (in `implement.rs`) is the model for the post-run merge/discard prompt in command mode.
- For TUI mode, follow the existing workflow controls dialog (Ratatui popup, see `src/tui/`) for the merge-or-discard dialog.
- Worktree path computation and the `git worktree` subprocess calls are candidates for a new `src/git.rs` module to keep `implement.rs` focused and the git logic independently testable.
