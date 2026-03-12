use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-repository configuration stored at `GITROOT/aspec/.aspec-cli.json`.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RepoConfig {
    pub agent: Option<String>,
    /// Whether the user has consented to mounting agent credentials into containers.
    /// Saved once per Git root; None means the user has not been asked yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_agent_auth_accepted: Option<bool>,
}

/// Global configuration stored at `$HOME/.aspec/config.json`.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct GlobalConfig {
    pub default_agent: Option<String>,
}

pub fn repo_config_path(git_root: &Path) -> PathBuf {
    git_root.join("aspec").join(".aspec-cli.json")
}

#[allow(dead_code)]
pub fn global_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    Ok(home.join(".aspec").join("config.json"))
}

pub fn load_repo_config(git_root: &Path) -> Result<RepoConfig> {
    let path = repo_config_path(git_root);
    if !path.exists() {
        return Ok(RepoConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("Invalid JSON in {}", path.display()))
}

pub fn save_repo_config(git_root: &Path, config: &RepoConfig) -> Result<()> {
    let path = repo_config_path(git_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

#[allow(dead_code)]
pub fn load_global_config() -> Result<GlobalConfig> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("Invalid JSON in {}", path.display()))
}

#[allow(dead_code)]
pub fn save_global_config(config: &GlobalConfig) -> Result<()> {
    let path = global_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn repo_config_path_is_correct() {
        let root = PathBuf::from("/some/repo");
        let path = repo_config_path(&root);
        assert_eq!(path, PathBuf::from("/some/repo/aspec/.aspec-cli.json"));
    }

    #[test]
    fn global_config_path_is_under_home() {
        let path = global_config_path().unwrap();
        assert!(path.ends_with(".aspec/config.json"));
    }

    #[test]
    fn save_and_load_repo_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = RepoConfig {
            agent: Some("claude".to_string()),
            auto_agent_auth_accepted: None,
        };
        save_repo_config(tmp.path(), &config).unwrap();
        let loaded = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn load_repo_config_returns_default_when_absent() {
        let tmp = TempDir::new().unwrap();
        let config = load_repo_config(tmp.path()).unwrap();
        assert_eq!(config, RepoConfig::default());
    }
}
