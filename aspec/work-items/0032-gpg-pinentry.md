# Work Item: Bug

Title: GPG pinentry destroys TUI during worktree commit signing
Issue: N/A

## Summary:
- When a user has GPG commit signing enabled and the worktree merge flow runs `git commit`, the GPG `pinentry` program claims the terminal, completely wiping the amux TUI. The user is left staring at a blank or corrupted screen until the git operation completes, at which point the TUI does not automatically recover.

## User Stories

### User Story 1:
As a: user

I want to:
sign my git commits with my GPG key while using amux worktree workflows

So I can:
maintain my standard commit signing practices without my terminal session being destroyed by the passphrase prompt

## Implementation Details:
- The bug occurs in `handle_worktree_commit_files()` and `handle_worktree_merge_confirmed()` in `src/tui/mod.rs`, both of which call `run_git_show()` to execute `git commit` synchronously via `std::process::Command`
- Ratatui holds the terminal in raw mode with an alternate screen active; when git spawns `gpg-agent` → `pinentry`, pinentry opens `/dev/tty` directly and renders its own curses/text UI over the same terminal, fighting with Ratatui's rendering and wiping the TUI
- The fix is to suspend the TUI (exit alternate screen, disable raw mode) immediately before executing any git command that may trigger a GPG passphrase prompt, then restore it (re-enter alternate screen, enable raw mode, force full redraw) after the command returns
- This is the standard pattern used by TUI tools (lazygit, vim `:!cmd`) when they need to cede terminal ownership to a subprocess
- The suspend/restore must wrap both `git commit` call sites: the worktree commit of uncommitted files and the squash-merge commit on the main branch
- A short informational line should be printed to stdout before suspending so the user understands why the TUI disappeared

## Edge Case Considerations:
- Users without GPG signing enabled must see no behavioral change — the suspend/restore is invisible when no passphrase prompt appears
- SSH-based commit signing (`gpg.format=ssh`) triggers the same class of problem and is fixed by the same approach
- S/MIME signing via `smimesign` similarly uses a TTY-based passphrase prompt and is also fixed
- If the git command fails (non-zero exit), the TUI must still be restored before surfacing the error to the user
- The terminal restore must happen even if the git command panics or returns an error mid-way; use Rust's `Drop` or an explicit guard to ensure cleanup

## Test Considerations:
- Unit test: `run_git_interactive()` (or equivalent) suspends and restores terminal state around a no-op subprocess
- Integration test: simulate a git commit command that exits non-zero; assert TUI is restored before error is propagated
- Manual test: enable GPG signing in a test repo, trigger a worktree workflow to completion, confirm the passphrase prompt appears cleanly and the TUI returns intact afterwards

## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
