use crate::config::{load_repo_config, save_repo_config};
use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

/// Credentials resolved for passing into a Docker container.
///
/// Contains environment variables (typically `CLAUDE_CODE_OAUTH_TOKEN`) to pass
/// via `-e` to `docker run`. The OAuth access token string from the host keychain
/// is the only credential passed — no files or config directories are mounted.
#[derive(Debug, Clone, Default)]
pub struct AgentCredentials {
    /// Environment variables to pass via `-e` to `docker run`.
    /// Typically `[("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-...")]`.
    pub env_vars: Vec<(String, String)>,
}

/// Returns the macOS Keychain service name and container env var for the given agent.
fn keychain_config_for_agent(agent: &str) -> Option<(&'static str, &'static str)> {
    match agent {
        // (keychain_service_name, container_env_var)
        "claude" => Some(("Claude Code-credentials", "CLAUDE_CODE_OAUTH_TOKEN")),
        _ => None,
    }
}

/// Extracts the OAuth access token from the keychain JSON blob for the given agent.
///
/// Claude stores a JSON blob with structure: `{"claudeAiOauth":{"accessToken":"...","refreshToken":"...","expiresAt":...}}`
/// Returns only the `accessToken` string value (e.g. `sk-ant-oat01-...`), which is what
/// `CLAUDE_CODE_OAUTH_TOKEN` expects as a bearer token.
fn extract_token_from_keychain_json(agent: &str, json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    match agent {
        "claude" => {
            let oauth_obj = parsed.get("claudeAiOauth")?;
            Some(oauth_obj.get("accessToken")?.as_str()?.to_string())
        }
        _ => None,
    }
}

/// Reads the raw credentials JSON blob from the system keychain for the given agent.
///
/// Uses `security find-generic-password` on macOS. Returns the raw JSON string.
pub fn read_keychain_raw(agent: &str) -> Option<String> {
    let (service, _env_var) = keychain_config_for_agent(agent)?;

    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if raw.is_empty() { None } else { Some(raw) }
        }
        _ => None,
    }
}

/// Reads the agent's OAuth token from the system keychain and prepares container credentials.
///
/// Returns an `AgentCredentials` with `CLAUDE_CODE_OAUTH_TOKEN` set to the raw
/// access token string (e.g. `sk-ant-oat01-...`). Claude Code reads this env var
/// on startup and uses it directly as the bearer token for API authentication.
///
/// No files or config directories are mounted — only the env var is passed.
pub fn agent_keychain_credentials(agent: &str) -> AgentCredentials {
    let (_service, env_var) = match keychain_config_for_agent(agent) {
        Some(cfg) => cfg,
        None => return AgentCredentials::default(),
    };

    let raw_json = match read_keychain_raw(agent) {
        Some(json) => json,
        None => return AgentCredentials::default(),
    };

    let token = match extract_token_from_keychain_json(agent, &raw_json) {
        Some(t) => t,
        None => return AgentCredentials::default(),
    };

    AgentCredentials {
        env_vars: vec![(env_var.to_string(), token)],
    }
}

/// Resolves agent credentials for the container using the system keychain.
///
/// Auto-passthrough: always reads credentials from the keychain without prompting.
/// The `git_root` parameter is retained for API compatibility but no config is checked.
pub fn resolve_auth(
    _git_root: &Path,
    agent: &str,
) -> Result<AgentCredentials> {
    Ok(agent_keychain_credentials(agent))
}

/// Persists the user's auth decision and returns the credentials if accepted.
pub fn apply_auth_decision(
    git_root: &Path,
    agent: &str,
    accepted: bool,
) -> Result<AgentCredentials> {
    let mut config = load_repo_config(git_root)?;
    config.auto_agent_auth_accepted = Some(accepted);
    save_repo_config(git_root, &config)?;

    if accepted {
        Ok(agent_keychain_credentials(agent))
    } else {
        Ok(AgentCredentials::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn apply_decision_saves_and_returns_credentials() {
        let tmp = TempDir::new().unwrap();
        let result = apply_auth_decision(tmp.path(), "claude", true).unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(true));
        // Result should match what keychain returns (may be empty on CI).
        let expected = agent_keychain_credentials("claude");
        assert_eq!(result.env_vars, expected.env_vars);
    }

    #[test]
    fn apply_decision_declined_saves_false() {
        let tmp = TempDir::new().unwrap();
        let result = apply_auth_decision(tmp.path(), "claude", false).unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(false));
        assert!(result.env_vars.is_empty());
    }

    #[test]
    fn keychain_config_known_for_claude() {
        let (service, env_var) = keychain_config_for_agent("claude").unwrap();
        assert_eq!(env_var, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(service, "Claude Code-credentials");
    }

    #[test]
    fn keychain_config_none_for_unknown() {
        assert_eq!(keychain_config_for_agent("unknown"), None);
    }

    #[test]
    fn extract_token_parses_claude_json() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-test","refreshToken":"rt","expiresAt":123}}"#;
        let token = extract_token_from_keychain_json("claude", json).unwrap();
        // Should return only the accessToken string, not the full JSON object.
        assert_eq!(token, "sk-ant-oat01-test");
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
    fn agent_keychain_credentials_unknown_agent_is_empty() {
        let creds = agent_keychain_credentials("unknown-agent");
        assert!(creds.env_vars.is_empty());
    }

    #[test]
    fn agent_keychain_credentials_sets_single_env_var_for_claude() {
        // If keychain is available (dev machine), CLAUDE_CODE_OAUTH_TOKEN should be set.
        let creds = agent_keychain_credentials("claude");
        if !creds.env_vars.is_empty() {
            assert_eq!(creds.env_vars.len(), 1);
            assert_eq!(creds.env_vars[0].0, "CLAUDE_CODE_OAUTH_TOKEN");
            // Value should be the raw access token string, not JSON.
            let val = &creds.env_vars[0].1;
            assert!(val.starts_with("sk-ant-"), "expected raw token starting with sk-ant-, got: {}", val);
        }
    }

    #[test]
    fn resolve_auth_always_returns_keychain_credentials() {
        let tmp = TempDir::new().unwrap();
        // resolve_auth is auto-passthrough: always returns keychain credentials
        // regardless of saved config preference.
        let result = resolve_auth(tmp.path(), "claude").unwrap();
        let expected = agent_keychain_credentials("claude");
        assert_eq!(result.env_vars, expected.env_vars);
    }

    #[test]
    fn read_keychain_raw_unknown_agent_returns_none() {
        assert!(read_keychain_raw("unknown-agent").is_none());
    }

    #[test]
    fn read_keychain_raw_claude_returns_json_on_dev_machine() {
        // On a dev machine with Claude logged in, this should return valid JSON.
        // On CI, it returns None (no keychain entry).
        if let Some(raw) = read_keychain_raw("claude") {
            let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert!(parsed.get("claudeAiOauth").is_some());
            assert!(parsed["claudeAiOauth"]["accessToken"].is_string());
            assert!(parsed["claudeAiOauth"]["refreshToken"].is_string());
        }
    }

    #[test]
    fn agent_credentials_default_is_empty() {
        let creds = AgentCredentials::default();
        assert!(creds.env_vars.is_empty());
    }
}
