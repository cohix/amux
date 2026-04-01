# Work Item: Bug

Title: LSP recommendation suppression
Issue: issuelink

## Summary:
- Claude Code shows LSP (Language Server Protocol) installation recommendation dialogs inside agent containers. These dialogs are noise — containers have no IDE and no pre-installed language servers, so there is nothing to install. amux is supposed to suppress them via `disable_lsp_recommendations()` in `src/docker/mod.rs`, but the suppression does not work due to two compounding bugs: the wrong settings key is written, and the suppression is skipped entirely when the host has no `~/.claude.json` present (i.e., on a first-time or CI machine).

## User Stories

### User Story 1:
As a: user

I want to:
never see LSP recommendation dialogs from Claude inside agent container sessions

So I can:
work with the agent without being prompted to install language servers that are irrelevant to the container environment


## Implementation Details:

### Root Cause 1 — Wrong settings key (primary)

`disable_lsp_recommendations()` in `src/docker/mod.rs` writes `"lspRecommendationDisabled": true` to the container's `settings.json`. This key is **not a valid Claude Code setting** — it has no effect. The correct key must be determined empirically before the fix is written.

**Investigation step (must be done before coding):**

Preferred method — observe what Claude writes after the dialog is dismissed:
1. Start a fresh container with no `~/.claude/settings.json` mounted:
   ```
   docker run --rm -it -v $(pwd):/workspace <amux-image> bash
   ```
2. Run `claude` interactively inside the container.
3. Wait for or trigger the LSP recommendation dialog (open a `.py` or `.ts` file).
4. Dismiss the dialog ("Don't show again" or equivalent).
5. `cat ~/.claude/settings.json` and record the key(s) that were written.

Alternative method — search the minified Claude Code bundle for candidate key names:
```
npm pack @anthropic-ai/claude-code
tar xf *.tgz
grep -o '"[a-zA-Z]*[Ll][Ss][Pp][a-zA-Z]*"' package/cli.js | sort -u
grep -o '"[a-zA-Z]*[Rr]ecommend[a-zA-Z]*"' package/cli.js | sort -u
```

Likely candidates based on Claude Code naming conventions:
- `"hasShownLspRecommendation"` — one-time flag set after first showing
- `"lspRecommendationDismissed"` — explicit user dismissal
- `"suppressLspRecommendations"` — general suppression flag

Once the correct key is confirmed, update `disable_lsp_recommendations` in `src/docker/mod.rs`:

```rust
fn disable_lsp_recommendations(claude_dir: &Path) -> std::io::Result<()> {
    let settings_path = claude_dir.join("settings.json");
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if let Some(obj) = settings.as_object_mut() {
        // Replace with empirically confirmed key:
        obj.insert("<CORRECT_KEY>".to_string(), serde_json::json!(true));
        // Remove the dead key from previous versions (cleanup):
        obj.remove("lspRecommendationDisabled");
    }
    std::fs::write(&settings_path, serde_json::to_string(&settings)?)
}
```

### Root Cause 2 — LSP suppression skipped when host has no `~/.claude.json`

`HostSettings::prepare()` returns `None` when `~/.claude.json` is absent on the host (e.g. first-time users, CI machines). When `host_settings` is `None`, `append_settings_mounts` is never called, so no `/root/.claude/settings.json` is bind-mounted into the container. LSP suppression is entirely skipped in this case.

**Fix:** Add `HostSettings::prepare_minimal()` — a fallback that creates a settings-only mount with LSP suppression but no auth forwarding:

```rust
/// Creates a minimal HostSettings with only LSP suppression — no auth forwarding.
/// Used when the host has no `~/.claude.json` (e.g. first-time setup, CI).
pub fn prepare_minimal() -> Option<Self> {
    let temp_dir = tempfile::TempDir::new().ok()?;
    let config_path = temp_dir.path().join("claude.json");
    std::fs::write(&config_path, "{}").ok()?;
    let claude_dir_path = temp_dir.path().join("dot-claude");
    std::fs::create_dir_all(&claude_dir_path).ok()?;
    disable_lsp_recommendations(&claude_dir_path).ok()?;
    Some(HostSettings {
        _temp_dir: Some(temp_dir),
        config_path,
        claude_dir_path,
    })
}
```

Update callers in `src/commands/chat.rs` and `src/tui/mod.rs` to fall back to `prepare_minimal()`:

```rust
let host_settings = docker::HostSettings::prepare(&agent_name)
    .or_else(|| docker::HostSettings::prepare_minimal());
```

### Root Cause 3 — No test coverage for `disable_lsp_recommendations`

The function has no unit tests, so there is nothing to catch a regression if the key changes or to verify file creation when `settings.json` is absent. Tests must be added (see Test Considerations).

### Files to change

| File | Change |
|---|---|
| `src/docker/mod.rs` | Update key in `disable_lsp_recommendations`; add `HostSettings::prepare_minimal()` |
| `src/commands/chat.rs` | Fall back to `prepare_minimal()` when `prepare()` returns `None` |
| `src/tui/mod.rs` | Same fallback at all three `HostSettings::prepare(...)` call sites |


## Edge Case Considerations:

- **Wrong key silently accepted**: Claude Code ignores unknown keys in `settings.json` without error. There is no crash or warning — the only observable symptom is that the dialog still appears. The investigation step is mandatory before writing any code.
- **Existing `settings.json` with unrelated keys**: `disable_lsp_recommendations` reads and merges with the existing file contents so other user settings (e.g. `"theme"`, model preferences) are preserved.
- **Existing `settings.json` with LSP key set to `false`**: The function must overwrite the value to `true`, not skip the write.
- **`settings.json` is invalid JSON**: The function falls back to an empty object (`{}`) rather than failing, preserving the container launch. The invalid file is overwritten with valid JSON.
- **Host has no `~/.claude/` directory but does have `~/.claude.json`**: `prepare()` already handles this by creating an empty `claude_dir_path`. `disable_lsp_recommendations` is called on the empty directory and creates `settings.json` from scratch.
- **Non-Claude agents (Codex, opencode, nanoclaw)**: `HostSettings::prepare()` returns `None` for non-claude agents — the LSP recommendation mechanism is Claude-specific. No change needed for other agents.
- **`prepare_minimal()` only applies when `prepare()` returns `None`**: When `prepare()` succeeds, it already calls `disable_lsp_recommendations` in its own flow. The `or_else` chain guarantees `prepare_minimal()` is never called if `prepare()` succeeds.
- **Fallback mechanism (env var)**: If empirical testing confirms no settings key exists and suppression must be done via environment variable, inject via `env_vars` in container run args instead of via `settings.json`. This is the fallback if the investigation finds no settings key.


## Test Considerations:

### Unit tests in `src/docker/mod.rs`

1. **`disable_lsp_recommendations_creates_file_with_correct_key`**
   - Create a temp dir with no `settings.json`.
   - Call `disable_lsp_recommendations`.
   - Assert the file exists and contains the correct key set to `true`.
   - Assert the dead key `"lspRecommendationDisabled"` is **not** present.

2. **`disable_lsp_recommendations_preserves_existing_settings`**
   - Write a `settings.json` with an unrelated key (e.g. `{"theme": "dark"}`).
   - Call `disable_lsp_recommendations`.
   - Assert the original key is preserved and the LSP key is added.

3. **`disable_lsp_recommendations_overwrites_false`**
   - Write a `settings.json` with the LSP key set to `false`.
   - Call `disable_lsp_recommendations`.
   - Assert the value is overwritten to `true`.

4. **`disable_lsp_recommendations_handles_invalid_json`**
   - Write a `settings.json` containing invalid JSON (e.g. `"not json"`).
   - Call `disable_lsp_recommendations`.
   - Assert the function succeeds (returns `Ok`).
   - Assert the resulting file contains valid JSON with the LSP key set to `true`.

5. **`prepare_minimal_returns_valid_host_settings`**
   - Call `HostSettings::prepare_minimal()`.
   - Assert it returns `Some`.
   - Assert `config_path` exists and contains valid JSON.
   - Assert `claude_dir_path` exists.
   - Assert `claude_dir_path/settings.json` contains the LSP key set to `true`.

6. **`host_settings_lsp_key_present_after_prepare`** *(dev-machine gated)*
   - Gate with `if !Path::new(&dirs::home_dir()?.join(".claude.json")).exists() { return; }` (same pattern as `host_settings_prepare_sanitizes_oauth`).
   - Call `HostSettings::prepare("claude")`.
   - Read `claude_dir_path/settings.json`.
   - Assert the LSP key is `true`.

### Regression check

- Run `make test` — all existing tests must pass without change.
- Manual smoke test: start an amux session without `~/.claude.json` present (e.g. in a Docker environment or by temporarily renaming the file); confirm no LSP dialog appears.


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- The `disable_lsp_recommendations` function is private to `src/docker/mod.rs`; `prepare_minimal` is a public `impl HostSettings` method consistent with `prepare` and `prepare_to_dir`.
- All new public functions have unit tests in the same file under `#[cfg(test)] mod tests`.
- `prepare_minimal()` uses `tempfile::TempDir` consistent with `prepare()` — RAII cleanup, no leaking temp files.
- The `or_else(|| docker::HostSettings::prepare_minimal())` call chain in callers is idiomatic Rust for optional fallback — no new control flow patterns introduced.
- Do not change `prepare_to_dir` — it already calls `disable_lsp_recommendations` and the key fix in Root Cause 1 is sufficient.
- Dead key cleanup (`obj.remove("lspRecommendationDisabled")`) ensures containers launched after the fix don't carry the stale key from a previously written state file.
