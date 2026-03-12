use crate::config::{load_repo_config, save_repo_config};
use anyhow::Result;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Returns the host-side credential directory for the given agent, if known.
/// This directory is mounted into the dev container so the agent is pre-authenticated.
pub fn credential_path_for_agent(agent: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let rel = match agent {
        "claude" => ".claude",
        "codex" => ".openai",
        "opencode" => ".opencode",
        _ => return None,
    };
    let path = home.join(rel);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Prompt and persist the user's auth decision in command (stdin/stdout) mode.
/// Returns the credential path to mount, or None if the user declined.
pub fn prompt_auth_command_mode(git_root: &Path, agent: &str) -> Result<Option<PathBuf>> {
    let config = load_repo_config(git_root)?;

    // Already decided: honour the saved choice.
    if let Some(accepted) = config.auto_agent_auth_accepted {
        return if accepted {
            Ok(credential_path_for_agent(agent))
        } else {
            Ok(None)
        };
    }

    // No saved decision: ask the user.
    let cred_path = match credential_path_for_agent(agent) {
        Some(p) => p,
        None => {
            println!("Agent '{}' credentials not found on this machine.", agent);
            println!("Log in inside the container manually when prompted.");
            return Ok(None);
        }
    };

    print!(
        "Mount agent credentials ({}) into container? This will be saved for this repo. [y/n]: ",
        cred_path.display()
    );
    std::io::stdout().flush()?;

    let stdin = std::io::stdin();
    let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
    let accepted = matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");

    apply_auth_decision(git_root, agent, accepted)
}

/// Persists the user's auth decision and returns the credential path if accepted.
pub fn apply_auth_decision(git_root: &Path, agent: &str, accepted: bool) -> Result<Option<PathBuf>> {
    let mut config = load_repo_config(git_root)?;
    config.auto_agent_auth_accepted = Some(accepted);
    save_repo_config(git_root, &config)?;

    if accepted {
        Ok(credential_path_for_agent(agent))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn apply_decision_saves_and_returns_path() {
        let tmp = TempDir::new().unwrap();
        let result = apply_auth_decision(tmp.path(), "claude", true).unwrap();
        // Path is returned only if the directory exists on the host — conditional.
        // Always test that the config was saved correctly.
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(true));
        // If ~/.claude exists, result is Some; otherwise None.
        let expected = credential_path_for_agent("claude");
        assert_eq!(result, expected);
    }

    #[test]
    fn apply_decision_declined_saves_false() {
        let tmp = TempDir::new().unwrap();
        apply_auth_decision(tmp.path(), "claude", false).unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config.auto_agent_auth_accepted, Some(false));
    }

    #[test]
    fn credential_path_for_unknown_agent_is_none() {
        assert!(credential_path_for_agent("unknown-agent").is_none());
    }
}
