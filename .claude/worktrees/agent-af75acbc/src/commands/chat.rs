use crate::commands::agent::run_agent_with_sink;
use crate::commands::auth::resolve_auth;
use crate::commands::implement::confirm_mount_scope_stdin;
use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Command-mode entry point for `amux chat`.
pub async fn run(non_interactive: bool, plan: bool, allow_docker: bool) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let mount_path = confirm_mount_scope_stdin(&git_root)?;
    let credentials = resolve_auth(&git_root, agent_name(&git_root)?)?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude");
    let host_settings = docker::HostSettings::prepare(agent)
        .or_else(|| docker::HostSettings::prepare_minimal(agent));

    let entrypoint = if non_interactive {
        chat_entrypoint_non_interactive(agent, plan)
    } else {
        chat_entrypoint(agent, plan)
    };

    run_agent_with_sink(
        entrypoint,
        &format!("Starting chat session with agent '{}'", agent),
        &OutputSink::Stdout,
        Some(mount_path),
        credentials.env_vars.clone(),
        non_interactive,
        host_settings.as_ref(),
        allow_docker,
        None,
    )
    .await
}

/// Core logic shared between command mode and TUI mode.
///
/// `mount_override`: when `Some`, skip the interactive stdin prompt and use this path.
/// `env_vars`: agent credential env vars to pass into the container.
/// `non_interactive`: when true, launch agent in print/non-interactive mode.
/// `plan`: when true, launch agent in plan (read-only) mode.
/// `allow_docker`: when true, mount the host Docker daemon socket into the container.
pub async fn run_with_sink(
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    plan: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude").to_string();

    let entrypoint = if non_interactive {
        chat_entrypoint_non_interactive(&agent, plan)
    } else {
        chat_entrypoint(&agent, plan)
    };

    run_agent_with_sink(
        entrypoint,
        &format!("Starting chat session with agent '{}'", agent),
        out,
        mount_override,
        env_vars,
        non_interactive,
        host_settings,
        allow_docker,
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

/// Build the entrypoint command for a chat session (interactive, no prompt).
pub fn chat_entrypoint(agent: &str, plan: bool) -> Vec<String> {
    let mut args = match agent {
        "claude" => vec!["claude".to_string()],
        "codex" => vec!["codex".to_string()],
        "opencode" => vec!["opencode".to_string()],
        _ => vec![agent.to_string()],
    };
    append_plan_flags(&mut args, agent, plan);
    args
}

/// Build the entrypoint command for a chat session in non-interactive mode.
pub fn chat_entrypoint_non_interactive(agent: &str, plan: bool) -> Vec<String> {
    let mut args = match agent {
        "claude" => vec!["claude".to_string(), "-p".to_string()],
        "codex" => vec!["codex".to_string(), "--quiet".to_string()],
        "opencode" => vec!["opencode".to_string()],
        _ => vec![agent.to_string()],
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

    #[test]
    fn chat_entrypoint_claude() {
        let args = chat_entrypoint("claude", false);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "claude");
    }

    #[test]
    fn chat_entrypoint_codex() {
        let args = chat_entrypoint("codex", false);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "codex");
    }

    #[test]
    fn chat_entrypoint_opencode() {
        let args = chat_entrypoint("opencode", false);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "opencode");
    }

    #[test]
    fn chat_entrypoint_unknown_agent() {
        let args = chat_entrypoint("custom", false);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "custom");
    }

    #[test]
    fn chat_entrypoint_non_interactive_claude() {
        let args = chat_entrypoint_non_interactive("claude", false);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
    }

    #[test]
    fn chat_entrypoint_non_interactive_codex() {
        let args = chat_entrypoint_non_interactive("codex", false);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "codex");
        assert_eq!(args[1], "--quiet");
    }

    #[test]
    fn chat_entrypoint_non_interactive_opencode() {
        let args = chat_entrypoint_non_interactive("opencode", false);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "opencode");
    }

    #[test]
    fn chat_entrypoint_has_no_prompt() {
        for agent in &["claude", "codex", "opencode"] {
            let args = chat_entrypoint(agent, false);
            // Chat should have no prompt argument — just the agent command.
            for arg in &args {
                assert!(
                    !arg.contains("Implement"),
                    "Chat entrypoint for {} should not contain a prompt, found: {}",
                    agent,
                    arg
                );
            }
        }
    }

    #[test]
    fn chat_entrypoint_non_interactive_has_no_prompt() {
        for agent in &["claude", "codex", "opencode"] {
            let args = chat_entrypoint_non_interactive(agent, false);
            for arg in &args {
                assert!(
                    !arg.contains("Implement"),
                    "Chat non-interactive entrypoint for {} should not contain a prompt, found: {}",
                    agent,
                    arg
                );
            }
        }
    }

    // --- Plan mode tests ---

    #[test]
    fn chat_entrypoint_plan_claude() {
        let args = chat_entrypoint("claude", true);
        assert_eq!(args, vec!["claude", "--permission-mode", "plan"]);
    }

    #[test]
    fn chat_entrypoint_plan_codex() {
        let args = chat_entrypoint("codex", true);
        assert_eq!(args, vec!["codex", "--approval-mode", "plan"]);
    }

    #[test]
    fn chat_entrypoint_plan_opencode() {
        // Opencode has no plan mode; flag is silently ignored.
        let args = chat_entrypoint("opencode", true);
        assert_eq!(args, vec!["opencode"]);
    }

    #[test]
    fn chat_entrypoint_plan_unknown_agent() {
        // Unknown agents have no plan mode; flag is silently ignored.
        let args = chat_entrypoint("custom", true);
        assert_eq!(args, vec!["custom"]);
    }

    #[test]
    fn chat_entrypoint_non_interactive_plan_claude() {
        let args = chat_entrypoint_non_interactive("claude", true);
        assert_eq!(args, vec!["claude", "-p", "--permission-mode", "plan"]);
    }

    #[test]
    fn chat_entrypoint_non_interactive_plan_codex() {
        let args = chat_entrypoint_non_interactive("codex", true);
        assert_eq!(args, vec!["codex", "--quiet", "--approval-mode", "plan"]);
    }

    #[test]
    fn chat_entrypoint_non_interactive_plan_opencode() {
        let args = chat_entrypoint_non_interactive("opencode", true);
        assert_eq!(args, vec!["opencode"]);
    }
}
