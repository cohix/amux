use crate::config::{load_repo_config, save_repo_config};
use anyhow::{Context, Result};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Returns the environment variable names that carry API credentials for the given agent.
///
/// For `--auth-from-env`, these are checked in order and the first one found is used.
/// Claude Code accepts either `ANTHROPIC_API_KEY` (for direct API keys) or
/// `CLAUDE_CODE_OAUTH_TOKEN` (for OAuth access tokens from the keychain).
pub fn env_var_names_for_agent(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        "codex" => &["OPENAI_API_KEY"],
        "opencode" => &["OPENAI_API_KEY"],
        _ => &[],
    }
}

/// Returns the container env var names and macOS Keychain service for the given agent.
///
/// The keychain stores OAuth tokens. We pass the token via multiple env vars so that
/// Claude Code picks it up regardless of which env var it checks:
/// - `ANTHROPIC_API_KEY` — the Anthropic SDK auto-detects OAuth tokens by their
///   `sk-ant-oat` prefix, so this works for both direct API keys and OAuth tokens.
/// - `CLAUDE_CODE_OAUTH_TOKEN` — the dedicated env var for OAuth access tokens.
fn keychain_config_for_agent(agent: &str) -> Option<(&'static [&'static str], &'static str)> {
    match agent {
        // (container_env_vars, keychain_service_name)
        "claude" => Some((&["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"], "Claude Code-credentials")),
        _ => None,
    }
}

/// Extracts the API token from the keychain JSON blob for the given agent.
///
/// Claude stores a JSON blob with structure: `{"claudeAiOauth":{"accessToken":"..."}}`
fn extract_token_from_keychain_json(agent: &str, json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    match agent {
        "claude" => parsed
            .get("claudeAiOauth")?
            .get("accessToken")?
            .as_str()
            .map(String::from),
        _ => None,
    }
}

/// Reads the agent's API token from the system keychain.
///
/// Uses `security find-generic-password` on macOS. Returns `(ENV_VAR_NAME, token)` pairs
/// using the correct container env var for the agent's auth method.
pub fn agent_keychain_vars(agent: &str) -> Vec<(String, String)> {
    let (env_names, service) = match keychain_config_for_agent(agent) {
        Some(cfg) => cfg,
        None => return vec![],
    };

    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let raw = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return vec![],
    };

    match extract_token_from_keychain_json(agent, &raw) {
        Some(token) => env_names
            .iter()
            .map(|name| (name.to_string(), token.clone()))
            .collect(),
        None => vec![],
    }
}

/// Returns `(VAR_NAME, value)` pairs for each agent credential env var that is set on the host.
///
/// Used when `--auth-from-env` is passed. Checks all known env var names for the agent
/// and returns the first one found.
pub fn agent_env_vars(agent: &str) -> Vec<(String, String)> {
    env_var_names_for_agent(agent)
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|val| (name.to_string(), val)))
        .take(1) // Use only the first match to avoid sending duplicate credentials.
        .collect()
}

/// Resolves agent credentials for the container.
///
/// - `auth_from_env = true`: reads from host environment variables directly (no prompt).
/// - `auth_from_env = false`: reads from system keychain, subject to saved preference / prompt.
pub fn resolve_auth(
    git_root: &Path,
    agent: &str,
    auth_from_env: bool,
) -> Result<Vec<(String, String)>> {
    if auth_from_env {
        let vars = agent_env_vars(agent);
        if vars.is_empty() {
            let names = env_var_names_for_agent(agent);
            if !names.is_empty() {
                eprintln!(
                    "Warning: --auth-from-env specified but none of {} are set.",
                    names.join(", ")
                );
            }
        }
        return Ok(vars);
    }

    prompt_auth_command_mode(git_root, agent)
}

/// Prompt and persist the user's auth decision in command (stdin/stdout) mode.
///
/// When the user accepts, credentials are read from the system keychain.
/// Returns the env vars to pass into the container, or empty vec if declined.
pub fn prompt_auth_command_mode(git_root: &Path, agent: &str) -> Result<Vec<(String, String)>> {
    let config = load_repo_config(git_root)?;

    // Already decided: honour the saved choice.
    if let Some(accepted) = config.auto_agent_auth_accepted {
        return if accepted {
            Ok(agent_keychain_vars(agent))
        } else {
            Ok(vec![])
        };
    }

    // No saved decision: check if keychain has credentials before asking.
    let keychain_vars = agent_keychain_vars(agent);
    if keychain_vars.is_empty() {
        println!(
            "No credentials found in system keychain for agent '{}'.",
            agent
        );
        println!("Log in inside the container manually, or re-run with --auth-from-env.");
        return Ok(vec![]);
    }

    let display: Vec<&str> = keychain_vars.iter().map(|(k, _)| k.as_str()).collect();
    print!(
        "Pass agent credentials ({}, from system keychain) into container? This will be saved for this repo. [y/n]: ",
        display.join(", ")
    );
    std::io::stdout().flush()?;

    let stdin = std::io::stdin();
    let answer = stdin
        .lock()
        .lines()
        .next()
        .unwrap_or(Ok(String::new()))
        .context("Failed to read stdin")?;
    let accepted = matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");

    apply_auth_decision(git_root, agent, accepted)
}

/// Persists the user's auth decision and returns the env vars if accepted.
pub fn apply_auth_decision(
    git_root: &Path,
    agent: &str,
    accepted: bool,
) -> Result<Vec<(String, String)>> {
    let mut config = load_repo_config(git_root)?;
    config.auto_agent_auth_accepted = Some(accepted);
    save_repo_config(git_root, &config)?;

    if accepted {
        Ok(agent_keychain_vars(agent))
    } else {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn apply_decision_saves_and_returns_vars() {
        let tmp = TempDir::new().unwrap();
        let result = apply_auth_decision(tmp.path(), "claude", true).unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(true));
        let expected = agent_keychain_vars("claude");
        assert_eq!(result, expected);
    }

    #[test]
    fn apply_decision_declined_saves_false() {
        let tmp = TempDir::new().unwrap();
        let result = apply_auth_decision(tmp.path(), "claude", false).unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(false));
        assert!(result.is_empty());
    }

    #[test]
    fn env_var_names_for_unknown_agent_is_empty() {
        assert!(env_var_names_for_agent("unknown-agent").is_empty());
    }

    #[test]
    fn agent_env_vars_unknown_agent_is_empty() {
        assert!(agent_env_vars("unknown-agent").is_empty());
    }

    #[test]
    fn env_var_names_correct_for_each_agent() {
        assert_eq!(
            env_var_names_for_agent("claude"),
            &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]
        );
        assert_eq!(env_var_names_for_agent("codex"), &["OPENAI_API_KEY"]);
        assert_eq!(env_var_names_for_agent("opencode"), &["OPENAI_API_KEY"]);
    }

    #[test]
    fn keychain_config_known_for_claude() {
        let (env_vars, service) = keychain_config_for_agent("claude").unwrap();
        assert_eq!(env_vars, &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]);
        assert_eq!(service, "Claude Code-credentials");
    }

    #[test]
    fn keychain_config_none_for_unknown() {
        assert_eq!(keychain_config_for_agent("unknown"), None);
    }

    #[test]
    fn extract_token_parses_claude_json() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-test","refreshToken":"rt","expiresAt":123}}"#;
        let token = extract_token_from_keychain_json("claude", json);
        assert_eq!(token, Some("sk-ant-oat01-test".into()));
    }

    #[test]
    fn extract_token_returns_none_for_invalid_json() {
        assert_eq!(extract_token_from_keychain_json("claude", "not json"), None);
    }

    #[test]
    fn extract_token_returns_none_for_missing_field() {
        let json = r#"{"other":{}}"#;
        assert_eq!(extract_token_from_keychain_json("claude", json), None);
    }

    #[test]
    fn extract_token_returns_none_for_unknown_agent() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-test"}}"#;
        assert_eq!(extract_token_from_keychain_json("codex", json), None);
    }

    #[test]
    fn agent_keychain_vars_unknown_agent_is_empty() {
        assert!(agent_keychain_vars("unknown-agent").is_empty());
    }

    #[test]
    fn agent_keychain_vars_sets_both_env_vars_for_claude() {
        // If keychain is available (dev machine), both env vars should be set
        let vars = agent_keychain_vars("claude");
        if !vars.is_empty() {
            assert_eq!(vars.len(), 2);
            assert_eq!(vars[0].0, "ANTHROPIC_API_KEY");
            assert_eq!(vars[1].0, "CLAUDE_CODE_OAUTH_TOKEN");
            // Both should have the same token value.
            assert_eq!(vars[0].1, vars[1].1);
        }
    }

    #[test]
    fn resolve_auth_from_env_reads_env_vars() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_auth(tmp.path(), "claude", true).unwrap();
        let expected = agent_env_vars("claude");
        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_auth_keychain_respects_saved_decline() {
        let tmp = TempDir::new().unwrap();
        apply_auth_decision(tmp.path(), "claude", false).unwrap();
        let result = resolve_auth(tmp.path(), "claude", false).unwrap();
        assert!(result.is_empty());
    }
}
