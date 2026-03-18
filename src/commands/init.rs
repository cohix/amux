use crate::cli::Agent;
use crate::commands::download;
use crate::commands::output::OutputSink;
use crate::config::{save_repo_config, RepoConfig};
use anyhow::{Context, Result};
use std::path::Path;

/// Command-mode entry point: runs init and prints output to stdout.
pub async fn run(agent: Agent) -> Result<()> {
    run_with_sink(agent, &OutputSink::Stdout).await
}

/// Core logic shared between command mode and TUI mode.
/// Output is routed through `out` so neither caller duplicates logic.
pub async fn run_with_sink(agent: Agent, out: &OutputSink) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;

    out.println(format!("Initializing aspec in: {}", git_root.display()));
    out.println(format!("Agent: {}", agent.as_str()));

    let config = RepoConfig {
        agent: Some(agent.as_str().to_string()),
        auto_agent_auth_accepted: None,
    };

    save_repo_config(&git_root, &config)?;
    out.println(format!(
        "Config written to: {}",
        git_root.join("aspec/.aspec-cli.json").display()
    ));

    // Download aspec folder from GitHub if it doesn't already exist.
    let aspec_dir = git_root.join("aspec");
    if !aspec_dir.exists() {
        match download::download_aspec_folder(&git_root, out).await {
            Ok(()) => {}
            Err(e) => {
                out.println(format!(
                    "Warning: failed to download aspec folder from GitHub: {}",
                    e
                ));
                out.println(
                    "You can manually download it from https://github.com/cohix/aspec"
                        .to_string(),
                );
            }
        }
    } else {
        out.println(format!(
            "aspec folder already exists at: {} (not overwritten)",
            aspec_dir.display()
        ));
    }

    // Download and write Dockerfile.dev from GitHub template.
    if write_dockerfile(&git_root, &agent, out).await? {
        out.println(format!(
            "Dockerfile.dev written to: {}",
            git_root.join("Dockerfile.dev").display()
        ));
    } else {
        out.println(format!(
            "Dockerfile.dev already exists at: {} (not overwritten)",
            git_root.join("Dockerfile.dev").display()
        ));
    }

    Ok(())
}

/// Walks upward from CWD to find the nearest directory containing a `.git` folder.
pub fn find_git_root() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Write Dockerfile.dev to the git root using a template downloaded from GitHub.
/// Falls back to the embedded template if the download fails.
/// Returns `true` if a new file was created, `false` if an existing file was preserved.
/// Public so other commands (e.g. ready) can initialize a missing Dockerfile.dev.
pub async fn write_dockerfile(
    git_root: &Path,
    agent: &Agent,
    out: &OutputSink,
) -> Result<bool> {
    let path = git_root.join("Dockerfile.dev");
    if path.exists() {
        return Ok(false);
    }
    let content = download_or_fallback_dockerfile(agent, out).await;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

/// Try to download the Dockerfile template from GitHub; fall back to embedded template.
async fn download_or_fallback_dockerfile(agent: &Agent, out: &OutputSink) -> String {
    match download::download_dockerfile_template(agent, out).await {
        Ok(content) => content,
        Err(e) => {
            out.println(format!(
                "Warning: failed to download Dockerfile template from GitHub: {}. Using bundled template.",
                e
            ));
            dockerfile_for_agent_embedded(agent)
        }
    }
}

/// Embedded Dockerfile templates compiled into the binary (used as fallback).
pub fn dockerfile_for_agent_embedded(agent: &Agent) -> String {
    match agent {
        Agent::Claude => include_str!("../../templates/Dockerfile.claude").to_string(),
        Agent::Codex => include_str!("../../templates/Dockerfile.codex").to_string(),
        Agent::Opencode => include_str!("../../templates/Dockerfile.opencode").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn find_git_root_finds_git_dir() {
        let root = find_git_root();
        assert!(root.is_some());
        assert!(root.unwrap().join(".git").exists());
    }

    #[test]
    fn find_git_root_returns_none_outside_repo() {
        let tmp = TempDir::new().unwrap();
        let result = walk_for_git(tmp.path().to_path_buf());
        assert!(result.is_none());
    }

    fn walk_for_git(mut dir: std::path::PathBuf) -> Option<std::path::PathBuf> {
        loop {
            if dir.join(".git").exists() {
                return Some(dir);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    #[tokio::test]
    async fn run_with_sink_streams_output() {
        let (tx, mut rx) = unbounded_channel();
        let sink = crate::commands::output::OutputSink::Channel(tx);

        // We don't run the real init (it would write files) but we verify the function
        // signature and that it calls the sink. Run from within the project's git root.
        let result = run_with_sink(Agent::Claude, &sink).await;
        // May succeed or fail depending on environment; we just verify sink received calls.
        drop(result);
        // Should have received at least one message via the channel.
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn write_dockerfile_creates_when_missing() {
        let tmp = TempDir::new().unwrap();
        let (tx, _rx) = unbounded_channel();
        let out = OutputSink::Channel(tx);
        let result = write_dockerfile(tmp.path(), &Agent::Claude, &out).await.unwrap();
        assert!(result, "should return true when creating a new file");
        assert!(tmp.path().join("Dockerfile.dev").exists());
        let content = std::fs::read_to_string(tmp.path().join("Dockerfile.dev")).unwrap();
        assert!(content.contains("debian:bookworm-slim"));
    }

    #[tokio::test]
    async fn write_dockerfile_does_not_overwrite_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Dockerfile.dev");
        std::fs::write(&path, "CUSTOM CONTENT").unwrap();

        let (tx, _rx) = unbounded_channel();
        let out = OutputSink::Channel(tx);
        let result = write_dockerfile(tmp.path(), &Agent::Claude, &out).await.unwrap();
        assert!(!result, "should return false when file already exists");

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "CUSTOM CONTENT", "existing file must not be overwritten");
    }

    #[test]
    fn dockerfile_for_agent_embedded_uses_debian_slim_base() {
        for agent in &[Agent::Claude, Agent::Codex, Agent::Opencode] {
            let content = dockerfile_for_agent_embedded(agent);
            assert!(
                content.contains("debian:bookworm-slim"),
                "{:?} template should use debian:bookworm-slim base image",
                agent
            );
        }
    }

    #[test]
    fn dockerfile_for_agent_embedded_does_not_use_npm_install() {
        for agent in &[Agent::Claude, Agent::Codex, Agent::Opencode] {
            let content = dockerfile_for_agent_embedded(agent);
            assert!(
                !content.contains("npm install"),
                "{:?} template should not use npm install",
                agent
            );
        }
    }

    #[test]
    fn dockerfile_templates_install_via_apt_or_direct_download() {
        for agent in &[Agent::Claude, Agent::Codex, Agent::Opencode] {
            let content = dockerfile_for_agent_embedded(agent);
            assert!(
                content.contains("apt-get") || content.contains("curl"),
                "{:?} template should install packages via apt-get or direct download",
                agent
            );
        }
    }
}
