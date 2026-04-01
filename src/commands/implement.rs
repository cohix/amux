use crate::commands::agent::run_agent_with_sink;
use crate::commands::auth::resolve_auth;
use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use crate::workflow::{self, StepStatus, WorkflowState};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Parse a work item string like "0001" or "1" into a u32.
pub fn parse_work_item(s: &str) -> Result<u32> {
    s.parse::<u32>()
        .with_context(|| format!("Invalid work item number: '{}'. Expected a number like 0001.", s))
}

/// Command-mode entry point.
pub async fn run(
    work_item_str: &str,
    non_interactive: bool,
    plan: bool,
    allow_docker: bool,
    workflow_path: Option<&Path>,
    worktree: bool,
    mount_ssh: bool,
) -> Result<()> {
    let work_item = parse_work_item(work_item_str)?;
    let git_root = find_git_root().context("Not inside a Git repository")?;

    // Worktree pre-checks.
    if worktree {
        crate::git::git_version_check()?;
        if crate::git::is_detached_head(&git_root) {
            eprintln!(
                "WARNING: You are in detached HEAD state. The worktree branch will be created \
                 from the current commit. Consider checking out a branch first."
            );
        }
    }

    let (mount_path, worktree_branch) = if worktree {
        let wt_path = crate::git::worktree_path(&git_root, work_item)?;
        let branch = crate::git::worktree_branch_name(work_item);
        let wt_path = prepare_worktree_cmd(&git_root, &wt_path, &branch)?;
        (wt_path, Some(branch))
    } else {
        (confirm_mount_scope_stdin(&git_root)?, None)
    };

    let credentials = resolve_auth(&git_root, agent_name(&git_root)?)?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude");
    let host_settings = docker::HostSettings::prepare(agent);

    if let Some(wf_path) = workflow_path {
        // Resolve relative paths against the process's working directory so that
        // paths like ./aspec/workflows/implement-feature.md work as expected.
        let resolved_wf: PathBuf = if wf_path.is_absolute() {
            wf_path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_else(|_| git_root.clone()).join(wf_path)
        };
        let result = run_workflow(
            work_item,
            &resolved_wf,
            &git_root,
            mount_path.clone(),
            credentials.env_vars,
            agent,
            host_settings,
            non_interactive,
            plan,
            allow_docker,
            mount_ssh,
        )
        .await;
        if let Some(ref branch) = worktree_branch {
            let _ = post_run_merge_prompt_stdin(&git_root, &mount_path, branch);
        }
        return result;
    }

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

    let result = run_agent_with_sink(
        entrypoint,
        &status,
        &OutputSink::Stdout,
        Some(mount_path.clone()),
        credentials.env_vars.clone(),
        non_interactive,
        host_settings.as_ref(),
        allow_docker,
        mount_ssh,
        None,
    )
    .await;

    if let Some(ref branch) = worktree_branch {
        let _ = post_run_merge_prompt_stdin(&git_root, &mount_path, branch);
    }

    result
}

/// Core logic shared between command mode and TUI mode.
///
/// `mount_override`: when `Some`, skip the interactive stdin prompt and use this path.
///                   when `None`, prompt via stdin (command mode only).
/// `env_vars`: agent credential env vars to pass into the container.
/// `non_interactive`: when true, launch agent in print/non-interactive mode.
/// `plan`: when true, launch agent in plan (read-only) mode.
/// `allow_docker`: when true, mount the host Docker daemon socket into the container.
/// `worktree`: when true, the worktree has already been set up; `mount_override` is the worktree path.
/// `mount_ssh`: when true, mount the host `~/.ssh` directory read-only into the container.
pub async fn run_with_sink(
    work_item: u32,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    plan: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
    worktree: bool,
    mount_ssh: bool,
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

    // `worktree` is handled by the TUI directly (launch_implement creates the worktree
    // and sets mount_override before calling run_with_sink). The flag is accepted here
    // for signature consistency but no extra action is needed.
    let _ = worktree;

    run_agent_with_sink(
        entrypoint,
        &status,
        out,
        mount_override,
        env_vars,
        non_interactive,
        host_settings,
        allow_docker,
        mount_ssh,
        None,
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

/// Build an agent entrypoint for a workflow step using a custom prompt.
pub fn workflow_step_entrypoint(agent: &str, prompt: &str, non_interactive: bool, plan: bool) -> Vec<String> {
    let mut args = match (agent, non_interactive) {
        ("claude", true) => vec!["claude".to_string(), "-p".to_string(), prompt.to_string()],
        ("claude", false) => vec!["claude".to_string(), prompt.to_string()],
        ("codex", true) => vec!["codex".to_string(), "--quiet".to_string(), prompt.to_string()],
        ("codex", false) => vec!["codex".to_string(), prompt.to_string()],
        ("opencode", _) => vec!["opencode".to_string(), "run".to_string(), prompt.to_string()],
        (a, _) => vec![a.to_string(), prompt.to_string()],
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

// ─── Workflow command-mode runner ────────────────────────────────────────────

/// Run a multi-step workflow in command mode (with stdin prompts between steps).
///
/// Steps are executed sequentially in the order they become ready (topological order).
/// After each step the user is prompted to advance or abort.
/// State is persisted to JSON so the workflow can be resumed after an interruption.
#[allow(clippy::too_many_arguments)]
async fn run_workflow(
    work_item: u32,
    workflow_path: &Path,
    git_root: &Path,
    mount_path: PathBuf,
    env_vars: Vec<(String, String)>,
    agent: &str,
    host_settings: Option<docker::HostSettings>,
    non_interactive: bool,
    plan: bool,
    allow_docker: bool,
    mount_ssh: bool,
) -> Result<()> {
    use std::io::{BufRead, Write};

    // Load and validate the workflow file.
    let (hash, title, steps) = workflow::load_workflow_file(workflow_path)?;

    let workflow_name = workflow_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow")
        .to_string();

    // Check for an existing state file.
    let state_path = workflow::workflow_state_path(git_root, work_item, &workflow_name);

    let mut state = if state_path.exists() {
        let existing = workflow::load_workflow_state(&state_path)?;
        resolve_resume_or_restart(existing, &hash, &steps, work_item, &workflow_name, &state_path)?
    } else {
        WorkflowState::new(title.clone(), steps.clone(), hash.clone(), work_item, workflow_name.clone())
    };

    // Persist initial state.
    workflow::save_workflow_state(git_root, &state)?;

    let title_display = state
        .title
        .clone()
        .unwrap_or_else(|| "Workflow".to_string());
    println!("\nRunning workflow: {}", title_display);
    println!("Work item: {:04}", work_item);
    println!("Steps: {}", state.steps.len());

    // Load work item content for prompt substitution.
    let work_item_path = find_work_item(&PathBuf::from(git_root), work_item)?;
    let work_item_content = std::fs::read_to_string(&work_item_path)
        .with_context(|| format!("Cannot read work item: {}", work_item_path.display()))?;

    // Handle any previously Running steps (from an interrupted run).
    let interrupted = state.interrupted_running_steps();
    for step_name in interrupted {
        println!("\nStep '{}' was running when the previous session ended.", step_name);
        print!("Start it over (s) or skip to next step (n)? [s/n]: ");
        std::io::stdout().flush()?;
        let stdin = std::io::stdin();
        let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
        if answer.trim().eq_ignore_ascii_case("n") {
            state.set_status(&step_name, StepStatus::Done);
        } else {
            state.set_status(&step_name, StepStatus::Pending);
        }
        workflow::save_workflow_state(git_root, &state)?;
    }

    // Main workflow loop.
    loop {
        let ready = state.next_ready();

        if ready.is_empty() {
            if state.all_done() {
                println!("\nAll workflow steps completed successfully.");
                let _ = std::fs::remove_file(&state_path);
                break;
            } else {
                // Some steps errored — nothing left to do automatically.
                println!("\nNo steps are ready to run. Check for errors above.");
                break;
            }
        }

        // Execute the first ready step (sequential execution).
        let step_name = ready[0].clone();
        let step_state = state
            .get_step(&step_name)
            .expect("ready step exists in state")
            .clone();

        println!("\n─── Step: {} ───", step_name);

        // Substitute template variables in the prompt.
        let prompt = workflow::substitute_prompt(
            &step_state.prompt_template,
            work_item,
            &work_item_content,
        );

        let entrypoint =
            workflow_step_entrypoint(agent, &prompt, non_interactive, plan);
        let status_msg = format!(
            "Workflow step '{}' — work item {:04} with agent '{}'",
            step_name, work_item, agent
        );

        // Generate a container name and record it for state persistence.
        let container_name = docker::generate_container_name();
        state.set_container_id(&step_name, container_name.clone());

        // Mark step as Running and save state.
        state.set_status(&step_name, StepStatus::Running);
        workflow::save_workflow_state(git_root, &state)?;

        let result = run_agent_with_sink(
            entrypoint,
            &status_msg,
            &OutputSink::Stdout,
            Some(mount_path.clone()),
            env_vars.clone(),
            non_interactive,
            host_settings.as_ref(),
            allow_docker,
            mount_ssh,
            Some(container_name),
        )
        .await;

        match result {
            Ok(_) => {
                state.set_status(&step_name, StepStatus::Done);
                workflow::save_workflow_state(git_root, &state)?;

                if state.all_done() {
                    println!("\nStep '{}' completed. Workflow finished!", step_name);
                    let _ = std::fs::remove_file(&state_path);
                    break;
                }

                println!("\nStep '{}' completed.", step_name);
                let next = state.next_ready();
                if !next.is_empty() {
                    println!("Next step(s): {}", next.join(", "));
                }
                print!("Press [Enter] to advance, or [q] to abort: ");
                std::io::stdout().flush()?;
                let stdin = std::io::stdin();
                let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
                if answer.trim().eq_ignore_ascii_case("q") {
                    println!("Workflow paused. Run again to resume.");
                    break;
                }
            }
            Err(e) => {
                state.set_status(&step_name, StepStatus::Error(e.to_string()));
                workflow::save_workflow_state(git_root, &state)?;

                println!("\nStep '{}' failed: {}", step_name, e);
                print!("Press [r] to retry, or any other key to abort: ");
                std::io::stdout().flush()?;
                let stdin = std::io::stdin();
                let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
                if answer.trim().eq_ignore_ascii_case("r") {
                    state.set_status(&step_name, StepStatus::Pending);
                    workflow::save_workflow_state(git_root, &state)?;
                    // Continue loop — the step will appear ready again.
                } else {
                    println!("Workflow paused. Run again to resume from the failed step.");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Resolve whether to resume an existing workflow state or start fresh.
///
/// Handles hash mismatch detection and interrupted-run step recovery.
fn resolve_resume_or_restart(
    existing: WorkflowState,
    new_hash: &str,
    new_steps: &[workflow::parser::WorkflowStep],
    work_item: u32,
    workflow_name: &str,
    state_path: &Path,
) -> Result<WorkflowState> {
    use std::io::{BufRead, Write};

    println!(
        "\nFound a saved workflow state for '{}' (work item {:04}).",
        workflow_name, work_item
    );

    if existing.workflow_hash != new_hash {
        println!("WARNING: The workflow file has changed since the last run.");
        print!("  1) Restart from the beginning\n  2) Continue anyway (could be dangerous)\n  [1/2]: ");
        std::io::stdout().flush()?;
        let stdin = std::io::stdin();
        let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;

        if answer.trim() == "2" {
            // Attempt to resume — validate step structure compatibility.
            match workflow::validate_resume_compatibility(&existing, new_steps) {
                Ok(_) => {
                    println!("Resuming with changed workflow file.");
                    return Ok(existing);
                }
                Err(e) => {
                    println!("Cannot resume: {}", e);
                    println!("Restarting from the beginning.");
                    // Fall through to restart.
                }
            }
        }

        // Restart: delete old state file, create fresh.
        let _ = std::fs::remove_file(state_path);
        return Ok(WorkflowState::new(
            existing.title,
            new_steps.to_vec(),
            new_hash.to_string(),
            work_item,
            workflow_name.to_string(),
        ));
    }

    // Hash matches — offer resume or restart.
    print!("  1) Resume from where you left off\n  2) Restart from the beginning\n  [1/2]: ");
    std::io::stdout().flush()?;
    let stdin = std::io::stdin();
    let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;

    if answer.trim() == "2" {
        let _ = std::fs::remove_file(state_path);
        return Ok(WorkflowState::new(
            existing.title,
            new_steps.to_vec(),
            new_hash.to_string(),
            work_item,
            workflow_name.to_string(),
        ));
    }

    println!("Resuming previous workflow run.");
    Ok(existing)
}

// ─── Worktree helpers (command mode) ─────────────────────────────────────────

/// Prepare (or reuse) a worktree at `wt_path` on `branch` using stdin prompts.
///
/// If the worktree directory already exists the user is prompted to resume or
/// recreate it.  Otherwise the worktree is created fresh.
fn prepare_worktree_cmd(git_root: &Path, wt_path: &PathBuf, branch: &str) -> Result<PathBuf> {
    use std::io::{BufRead, Write};
    if wt_path.exists() {
        println!("Worktree already exists at {}.", wt_path.display());
        print!("[r]esume / [R]ecreate? ");
        std::io::stdout().flush()?;
        let stdin = std::io::stdin();
        let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
        if answer.trim() == "R" {
            crate::git::remove_worktree(git_root, wt_path)?;
            crate::git::create_worktree(git_root, wt_path, branch)?;
        }
        // 'r' or any other key: reuse existing worktree
    } else {
        crate::git::create_worktree(git_root, wt_path, branch)?;
    }
    Ok(wt_path.clone())
}

/// After the container (or workflow) completes, ask the user whether to merge,
/// discard, or keep the worktree branch.
fn post_run_merge_prompt_stdin(git_root: &Path, wt_path: &Path, branch: &str) -> Result<()> {
    use std::io::{BufRead, Write};
    println!(
        "\nWorktree branch `{}` is ready. Merge into current branch? [y/n/s(kip-and-keep)]",
        branch
    );
    print!("> ");
    std::io::stdout().flush()?;
    let stdin = std::io::stdin();
    let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new()))?;
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" | "m" | "merge" => match crate::git::merge_branch(git_root, branch) {
            Ok(()) => {
                let _ = crate::git::remove_worktree(git_root, wt_path);
                let _ = crate::git::delete_branch(git_root, branch);
                println!("Merged and cleaned up worktree.");
            }
            Err(e) => {
                eprintln!("Merge failed with conflicts: {}", e);
                eprintln!(
                    "Resolve manually in `{}`, then run:\n  git branch -d {} && git worktree remove {}",
                    git_root.display(),
                    branch,
                    wt_path.display()
                );
            }
        },
        "n" | "no" | "d" | "discard" => {
            let _ = crate::git::remove_worktree(git_root, wt_path);
            let _ = crate::git::delete_branch(git_root, branch);
            println!("Worktree discarded.");
        }
        _ => {
            // 's', 'skip', or any other input: skip and keep
            println!("Worktree kept at: {}", wt_path.display());
        }
    }
    Ok(())
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

    // --- Workflow step entrypoint tests ---

    #[test]
    fn workflow_step_entrypoint_claude_interactive() {
        let args = workflow_step_entrypoint("claude", "my prompt", false, false);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "my prompt");
    }

    #[test]
    fn workflow_step_entrypoint_claude_non_interactive() {
        let args = workflow_step_entrypoint("claude", "my prompt", true, false);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
        assert_eq!(args[2], "my prompt");
    }

    #[test]
    fn workflow_step_entrypoint_codex_non_interactive() {
        let args = workflow_step_entrypoint("codex", "prompt", true, false);
        assert_eq!(args[0], "codex");
        assert_eq!(args[1], "--quiet");
    }

    #[test]
    fn workflow_step_entrypoint_with_plan() {
        let args = workflow_step_entrypoint("claude", "prompt", false, true);
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"plan".to_string()));
    }
}
