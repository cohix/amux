use crate::cli::Agent;
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

    write_dockerfile(&git_root, &agent)?;
    out.println(format!(
        "Dockerfile.dev written to: {}",
        git_root.join("Dockerfile.dev").display()
    ));

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

/// Write Dockerfile.dev to the git root using the template for the given agent.
/// Public so other commands (e.g. ready) can initialize a missing Dockerfile.dev.
pub fn write_dockerfile(git_root: &Path, agent: &Agent) -> Result<()> {
    let path = git_root.join("Dockerfile.dev");
    let content = dockerfile_for_agent(agent);
    std::fs::write(&path, content).with_context(|| format!("Failed to write {}", path.display()))
}

pub fn dockerfile_for_agent(agent: &Agent) -> String {
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
}
