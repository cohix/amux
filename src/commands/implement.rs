use crate::commands::agent::run_agent_with_sink;
use crate::commands::auth::resolve_auth;
use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Parse a work item string like "0001" or "1" into a u32.
pub fn parse_work_item(s: &str) -> Result<u32> {
    s.parse::<u32>()
        .with_context(|| format!("Invalid work item number: '{}'. Expected a number like 0001.", s))
}

/// Command-mode entry point.
pub async fn run(work_item_str: &str, non_interactive: bool, plan: bool) -> Result<()> {
    let work_item = parse_work_item(work_item_str)?;
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let mount_path = confirm_mount_scope_stdin(&git_root)?;
    let credentials = resolve_auth(&git_root, agent_name(&git_root)?)?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude");
    let host_settings = docker::HostSettings::prepare(agent);

    let entrypoint = if non_interactive {
        agent_entrypoint_non_interactive(agent, work_item, plan)
    } else {
        agent_entrypoint(agent, work_item, plan)
    };

    let work_item_path = find_work_item(&git_root, work_item)?;
    let status = format!(
        "Implementing work item {:04} with agent '{}': {}",
        work_item,
        agent,
        work_item_path.display()
    );

    run_agent_with_sink(
        entrypoint,
        &status,
        &OutputSink::Stdout,
        Some(mount_path),
        credentials.env_vars.clone(),
        non_interactive,
        host_settings.as_ref(),
    )
    .await
}

/// Core logic shared between command mode and TUI mode.
///
/// `mount_override`: when `Some`, skip the interactive stdin prompt and use this path.
///                   when `None`, prompt via stdin (command mode only).
/// `env_vars`: agent credential env vars to pass into the container.
/// `non_interactive`: when true, launch agent in print/non-interactive mode.
/// `plan`: when true, launch agent in plan (read-only) mode.
pub async fn run_with_sink(
    work_item: u32,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    plan: bool,
    host_settings: Option<&docker::HostSettings>,
) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude").to_string();
    let work_item_path = find_work_item(&git_root, work_item)?;

    let entrypoint = if non_interactive {
        agent_entrypoint_non_interactive(&agent, work_item, plan)
    } else {
        agent_entrypoint(&agent, work_item, plan)
    };

    let status = format!(
        "Implementing work item {:04} with agent '{}': {}",
        work_item,
        agent,
        work_item_path.display()
    );

    run_agent_with_sink(
        entrypoint,
        &status,
        out,
        mount_override,
        env_vars,
        non_interactive,
        host_settings,
    )
    .await
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

/// The prompt given to the code agent when implementing a work item.
const IMPLEMENT_PROMPT_TEMPLATE: &str = "Implement work item {work_item}. Iterate until the build \
    succeeds. Implement tests as described in the work item and the project aspec. Iterate until \
    tests are comprehensive and pass. Write documentation as described in the project aspec. \
    Ensure final build and test success.";

/// Build the prompt string for the given work item number.
pub fn implement_prompt(work_item: u32) -> String {
    IMPLEMENT_PROMPT_TEMPLATE.replace("{work_item}", &format!("{:04}", work_item))
}

pub fn agent_entrypoint(agent: &str, work_item: u32, plan: bool) -> Vec<String> {
    let prompt = implement_prompt(work_item);

    let mut args = match agent {
        "claude" => vec![
            "claude".to_string(),
            prompt,
        ],
        "codex" => vec![
            "codex".to_string(),
            prompt,
        ],
        "opencode" => vec![
            "opencode".to_string(),
            "run".to_string(),
            prompt,
        ],
        _ => vec![
            agent.to_string(),
            prompt,
        ],
    };
    append_plan_flags(&mut args, agent, plan);
    args
}

/// Build the entrypoint command for the implement agent in non-interactive (print) mode.
pub fn agent_entrypoint_non_interactive(agent: &str, work_item: u32, plan: bool) -> Vec<String> {
    let prompt = implement_prompt(work_item);

    let mut args = match agent {
        "claude" => vec![
            "claude".to_string(),
            "-p".to_string(),
            prompt,
        ],
        "codex" => vec![
            "codex".to_string(),
            "--quiet".to_string(),
            prompt,
        ],
        "opencode" => vec![
            "opencode".to_string(),
            "run".to_string(),
            prompt,
        ],
        _ => vec![
            agent.to_string(),
            prompt,
        ],
    };
    append_plan_flags(&mut args, agent, plan);
    args
}

/// Append agent-specific plan mode flags to the argument list.
///
/// - Claude: `--permission-mode plan`
/// - Codex: `--approval-mode plan`
/// - Opencode: no plan mode available (flag is silently ignored)
fn append_plan_flags(args: &mut Vec<String>, agent: &str, plan: bool) {
    if !plan {
        return;
    }
    match agent {
        "claude" => {
            args.push("--permission-mode".to_string());
            args.push("plan".to_string());
        }
        "codex" => {
            args.push("--approval-mode".to_string());
            args.push("plan".to_string());
        }
        // Opencode and unknown agents have no plan mode.
        _ => {}
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
        let args = agent_entrypoint("claude", 1, false);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "claude");
        assert!(args[1].contains("work item 0001"));
    }

    #[test]
    fn agent_entrypoint_codex() {
        let args = agent_entrypoint("codex", 2, false);
        assert_eq!(args[0], "codex");
        assert!(args[1].contains("work item 0002"));
    }

    #[test]
    fn agent_entrypoint_opencode() {
        let args = agent_entrypoint("opencode", 3, false);
        assert_eq!(args[0], "opencode");
        assert_eq!(args[1], "run");
        assert!(args[2].contains("work item 0003"));
    }

    #[test]
    fn implement_prompt_includes_work_item_number() {
        let prompt = implement_prompt(42);
        assert!(prompt.contains("work item 0042"));
        assert!(prompt.contains("Iterate until the build succeeds"));
        assert!(prompt.contains("Ensure final build and test success"));
    }

    #[test]
    fn parse_work_item_valid_inputs() {
        assert_eq!(parse_work_item("1").unwrap(), 1);
        assert_eq!(parse_work_item("0001").unwrap(), 1);
        assert_eq!(parse_work_item("42").unwrap(), 42);
        assert_eq!(parse_work_item("0042").unwrap(), 42);
    }

    #[test]
    fn parse_work_item_invalid_inputs() {
        assert!(parse_work_item("abc").is_err());
        assert!(parse_work_item("").is_err());
        assert!(parse_work_item("-1").is_err());
    }

    #[test]
    fn agent_entrypoint_non_interactive_claude() {
        let args = agent_entrypoint_non_interactive("claude", 1, false);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
        assert!(args[2].contains("work item 0001"));
    }

    #[test]
    fn agent_entrypoint_non_interactive_codex() {
        let args = agent_entrypoint_non_interactive("codex", 2, false);
        assert_eq!(args[0], "codex");
        assert_eq!(args[1], "--quiet");
        assert!(args[2].contains("work item 0002"));
    }

    #[test]
    fn agent_entrypoint_non_interactive_opencode() {
        let args = agent_entrypoint_non_interactive("opencode", 3, false);
        assert_eq!(args[0], "opencode");
        assert_eq!(args[1], "run");
        assert!(args[2].contains("work item 0003"));
    }

    // --- Plan mode tests ---

    #[test]
    fn agent_entrypoint_plan_claude() {
        let args = agent_entrypoint("claude", 1, true);
        assert_eq!(args[0], "claude");
        assert!(args[1].contains("work item 0001"));
        assert_eq!(args[2], "--permission-mode");
        assert_eq!(args[3], "plan");
    }

    #[test]
    fn agent_entrypoint_plan_codex() {
        let args = agent_entrypoint("codex", 2, true);
        assert_eq!(args[0], "codex");
        assert!(args[1].contains("work item 0002"));
        assert_eq!(args[2], "--approval-mode");
        assert_eq!(args[3], "plan");
    }

    #[test]
    fn agent_entrypoint_plan_opencode() {
        // Opencode has no plan mode; flag is silently ignored.
        let args = agent_entrypoint("opencode", 3, true);
        assert_eq!(args.len(), 3); // opencode, run, prompt — no extra flags
        assert_eq!(args[0], "opencode");
        assert_eq!(args[1], "run");
    }

    #[test]
    fn agent_entrypoint_plan_unknown_agent() {
        let args = agent_entrypoint("custom", 1, true);
        assert_eq!(args.len(), 2); // agent, prompt — no extra flags
    }

    #[test]
    fn agent_entrypoint_non_interactive_plan_claude() {
        let args = agent_entrypoint_non_interactive("claude", 1, true);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
        assert!(args[2].contains("work item 0001"));
        assert_eq!(args[3], "--permission-mode");
        assert_eq!(args[4], "plan");
    }

    #[test]
    fn agent_entrypoint_non_interactive_plan_codex() {
        let args = agent_entrypoint_non_interactive("codex", 2, true);
        assert_eq!(args[0], "codex");
        assert_eq!(args[1], "--quiet");
        assert!(args[2].contains("work item 0002"));
        assert_eq!(args[3], "--approval-mode");
        assert_eq!(args[4], "plan");
    }

    #[test]
    fn agent_entrypoint_non_interactive_plan_opencode() {
        let args = agent_entrypoint_non_interactive("opencode", 3, true);
        assert_eq!(args.len(), 3); // opencode, run, prompt — no extra flags
    }
}
