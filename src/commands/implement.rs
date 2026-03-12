use crate::commands::auth::prompt_auth_command_mode;
use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Command-mode entry point.
pub async fn run(work_item: u32) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let mount_path = confirm_mount_scope_stdin(&git_root)?;
    let cred_path = prompt_auth_command_mode(&git_root, agent_name(&git_root)?)?;
    run_with_sink(work_item, &OutputSink::Stdout, Some(mount_path), cred_path).await
}

/// Core logic shared between command mode and TUI mode.
///
/// `mount_override`: when `Some`, skip the interactive stdin prompt and use this path.
///                   when `None`, prompt via stdin (command mode only).
/// `cred_path`: when `Some`, add an extra bind-mount for agent credentials.
pub async fn run_with_sink(
    work_item: u32,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    cred_path: Option<PathBuf>,
) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let config = load_repo_config(&git_root)?;

    let agent = config.agent.as_deref().unwrap_or("claude").to_string();
    let work_item_path = find_work_item(&git_root, work_item)?;

    out.println(format!(
        "Implementing work item {:04} with agent '{}': {}",
        work_item,
        agent,
        work_item_path.display()
    ));

    let mount_path = match mount_override {
        Some(p) => p,
        None => confirm_mount_scope_stdin(&git_root)?,
    };

    let image_tag = "aspec-dev:latest";
    let entrypoint = agent_entrypoint(&agent, &work_item_path, &git_root);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    docker::run_container(image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, cred_path)
        .context("Container exited with an error")?;

    Ok(())
}

fn agent_name(git_root: &PathBuf) -> Result<&'static str> {
    let config = load_repo_config(git_root)?;
    Ok(match config.agent.as_deref().unwrap_or("claude") {
        "codex" => "codex",
        "opencode" => "opencode",
        _ => "claude",
    })
}

/// Finds the work item file for the given number, e.g. `aspec/work-items/0001-*.md`.
pub fn find_work_item(git_root: &PathBuf, work_item: u32) -> Result<PathBuf> {
    let pattern = format!("{:04}-", work_item);
    let dir = git_root.join("aspec/work-items");

    if !dir.exists() {
        bail!("Work items directory not found: {}", dir.display());
    }

    let entry = std::fs::read_dir(&dir)
        .with_context(|| format!("Cannot read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with(&pattern));

    match entry {
        Some(e) => Ok(e.path()),
        None => bail!("No work item {:04} found in {}", work_item, dir.display()),
    }
}

/// Asks the user (via stdin) whether to mount just CWD or the full Git root.
pub fn confirm_mount_scope_stdin(git_root: &PathBuf) -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    if cwd == *git_root {
        return Ok(git_root.clone());
    }

    println!(
        "Mount scope: current directory is '{}', Git root is '{}'.",
        cwd.display(),
        git_root.display()
    );
    print!("Mount the Git root (r) or current directory only (c)? [r/c]: ");

    use std::io::{BufRead, Write};
    std::io::stdout().flush()?;
    let stdin = std::io::stdin();
    let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;

    match answer.trim().to_lowercase().as_str() {
        "r" => Ok(git_root.clone()),
        _ => Ok(cwd),
    }
}

pub fn agent_entrypoint(agent: &str, work_item_path: &PathBuf, git_root: &PathBuf) -> Vec<String> {
    let relative = work_item_path
        .strip_prefix(git_root)
        .unwrap_or(work_item_path);
    let container_path = PathBuf::from("/workspace").join(relative);
    let work_item_str = container_path.to_string_lossy().to_string();

    match agent {
        "claude" => vec![
            "claude".to_string(),
            "--print".to_string(),
            format!("Implement the work item described in {}", work_item_str),
        ],
        "codex" => vec![
            "codex".to_string(),
            format!("Implement the work item described in {}", work_item_str),
        ],
        "opencode" => vec![
            "opencode".to_string(),
            "run".to_string(),
            format!("Implement the work item described in {}", work_item_str),
        ],
        _ => vec![
            agent.to_string(),
            format!("Implement the work item described in {}", work_item_str),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_work_item(dir: &PathBuf, name: &str) {
        std::fs::create_dir_all(dir.join("aspec/work-items")).unwrap();
        std::fs::write(dir.join("aspec/work-items").join(name), "# Work Item").unwrap();
    }

    #[test]
    fn find_work_item_matches_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        make_work_item(&root, "0001-add-feature.md");
        let path = find_work_item(&root, 1).unwrap();
        assert!(path.ends_with("0001-add-feature.md"));
    }

    #[test]
    fn find_work_item_errors_when_missing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("aspec/work-items")).unwrap();
        assert!(find_work_item(&root, 99).is_err());
    }

    #[test]
    fn agent_entrypoint_claude() {
        let root = PathBuf::from("/repo");
        let work_item = PathBuf::from("/repo/aspec/work-items/0001-test.md");
        let args = agent_entrypoint("claude", &work_item, &root);
        assert_eq!(args[0], "claude");
        assert!(args[2].contains("/workspace/aspec/work-items/0001-test.md"));
    }
}
