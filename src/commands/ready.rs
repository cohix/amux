use crate::cli::Agent;
use crate::commands::auth::resolve_auth;
use crate::commands::implement::confirm_mount_scope_stdin;
use crate::commands::init::{find_git_root, write_dockerfile};
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// The prompt sent to the agent for Dockerfile.dev audit.
const AUDIT_PROMPT: &str = "scan this project and determine every tool needed to build, run, \
    and test it per the local development workflows defined in the aspec. Modify Dockerfile.dev \
    to ensure that all of those tools, at the correct version, get installed when the Dockerfile \
    is built";

/// Context produced by the pre-audit phase, needed by the audit and post-audit phases.
#[derive(Clone)]
pub struct ReadyContext {
    pub image_tag: String,
    pub dockerfile_str: String,
    pub git_root_str: String,
    pub mount_path: String,
    pub agent_name: String,
    pub env_vars: Vec<(String, String)>,
}

/// Options controlling ready command behavior. Shared between command and TUI modes.
#[derive(Clone, Debug, Default)]
pub struct ReadyOptions {
    /// When true, run the Dockerfile agent audit. When false, skip it.
    pub refresh: bool,
    /// When true, launch the agent in non-interactive (print) mode.
    pub non_interactive: bool,
}

/// Tracks the status of each step for the summary table.
#[derive(Clone, Debug)]
pub struct ReadySummary {
    pub docker_daemon: StepStatus,
    pub dockerfile: StepStatus,
    pub dev_image: StepStatus,
    pub refresh: StepStatus,
    pub image_rebuild: StepStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepStatus {
    Pending,
    Ok(String),
    Skipped(String),
    Failed(String),
}

impl Default for ReadySummary {
    fn default() -> Self {
        Self {
            docker_daemon: StepStatus::Pending,
            dockerfile: StepStatus::Pending,
            dev_image: StepStatus::Pending,
            refresh: StepStatus::Pending,
            image_rebuild: StepStatus::Pending,
        }
    }
}

/// Prints the summary table to the output sink.
pub fn print_summary(out: &OutputSink, summary: &ReadySummary) {
    out.println(String::new());
    out.println("┌──────────────────────────────────────────────────┐");
    out.println("│                  Ready Summary                   │");
    out.println("├───────────────────┬──────────────────────────────┤");
    print_summary_row(out, "Docker daemon", &summary.docker_daemon);
    print_summary_row(out, "Dockerfile.dev", &summary.dockerfile);
    print_summary_row(out, "Dev image", &summary.dev_image);
    print_summary_row(out, "Refresh (audit)", &summary.refresh);
    print_summary_row(out, "Image rebuild", &summary.image_rebuild);
    out.println("└───────────────────┴──────────────────────────────┘");
}

fn print_summary_row(out: &OutputSink, label: &str, status: &StepStatus) {
    let (symbol, text) = match status {
        StepStatus::Pending => ("-", "pending".to_string()),
        StepStatus::Ok(msg) => ("✓", msg.clone()),
        StepStatus::Skipped(msg) => ("–", msg.clone()),
        StepStatus::Failed(msg) => ("✗", msg.clone()),
    };
    out.println(format!(
        "│ {:>17} │ {} {:<27} │",
        label, symbol, text
    ));
}

/// Large ASCII-art notice printed before launching an interactive agent.
pub fn print_interactive_notice(out: &OutputSink, agent_name: &str) {
    out.println(String::new());
    out.println("╔══════════════════════════════════════════════════════════════╗");
    out.println("║                                                              ║");
    out.println("║     ╦╔╗╔╔╦╗╔═╗╦═╗╔═╗╔═╗╔╦╗╦╦  ╦╔═╗  ╔╦╗╔═╗╔╦╗╔═╗        ║");
    out.println("║     ║║║║ ║ ║╣ ╠╦╝╠═╣║   ║ ║╚╗╔╝║╣   ║║║║ ║ ║║║╣         ║");
    out.println("║     ╩╝╚╝ ╩ ╚═╝╩╚═╩ ╩╚═╝ ╩ ╩ ╚╝ ╚═╝  ╩ ╩╚═╝═╩╝╚═╝       ║");
    out.println("║                                                              ║");
    out.println(format!(
        "║  Agent '{}' is launching in INTERACTIVE mode.{}║",
        agent_name,
        " ".repeat(46usize.saturating_sub(agent_name.len() + 43))
    ));
    out.println("║  You will need to quit the agent (Ctrl+C or exit)            ║");
    out.println("║  when its work is complete.                                  ║");
    out.println("║                                                              ║");
    out.println("╚══════════════════════════════════════════════════════════════╝");
    out.println(String::new());
}

/// Command-mode entry point: prompts for mount scope and auth, then runs ready phases.
/// The audit phase is only run when `--refresh` is passed.
pub async fn run(auth_from_env: bool, refresh: bool, non_interactive: bool) -> Result<()> {
    let opts = ReadyOptions { refresh, non_interactive };
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let mount_path = confirm_mount_scope_stdin(&git_root)?;
    let config = load_repo_config(&git_root)?;
    let agent_name = config.agent.as_deref().unwrap_or("claude");
    let env_vars = resolve_auth(&git_root, agent_name, auth_from_env)?;
    let out = &OutputSink::Stdout;

    let mut summary = ReadySummary::default();
    let ctx = run_pre_audit(out, mount_path, env_vars, &mut summary).await?;

    if opts.refresh {
        if !opts.non_interactive {
            print_interactive_notice(out, &ctx.agent_name);
        }
        // Run audit interactively (inherited stdio → user can interact with agent).
        let entrypoint = if opts.non_interactive {
            audit_entrypoint_non_interactive(&ctx.agent_name)
        } else {
            audit_entrypoint(&ctx.agent_name)
        };
        let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
        let config_dir = docker::claude_config_dir();

        if opts.non_interactive {
            let (_cmd, audit_output) = docker::run_container_captured(
                &ctx.image_tag,
                &ctx.mount_path,
                &entrypoint_refs,
                &ctx.env_vars,
                config_dir.as_deref(),
            )
            .context("Dockerfile audit container failed")?;
            for line in audit_output.lines() {
                out.println(line);
            }
        } else {
            docker::run_container(
                &ctx.image_tag,
                &ctx.mount_path,
                &entrypoint_refs,
                &ctx.env_vars,
                config_dir.as_deref(),
            )
            .context("Dockerfile audit container failed")?;
        }

        summary.refresh = StepStatus::Ok("completed".into());
        run_post_audit(out, &ctx, &mut summary).await?;
    } else {
        out.println("Skipping Dockerfile audit (use --refresh to run it).");
        summary.refresh = StepStatus::Skipped("use --refresh to run".into());
        summary.image_rebuild = StepStatus::Skipped("no refresh".into());
    }

    print_summary(out, &summary);

    if !opts.refresh {
        out.println(String::new());
        out.println("Tip: use `aspec ready --refresh` to run the Dockerfile audit agent.");
    }

    out.println(String::new());
    out.println("aspec is ready.");
    Ok(())
}

/// Phase 1 — Pre-audit: Docker checks, Dockerfile init, image build.
///
/// Returns the context needed to launch the audit and post-audit phases.
pub async fn run_pre_audit(
    out: &OutputSink,
    mount_path: PathBuf,
    env_vars: Vec<(String, String)>,
    summary: &mut ReadySummary,
) -> Result<ReadyContext> {
    // 1. Docker daemon check
    out.print("Checking Docker daemon... ");
    if docker::is_daemon_running() {
        out.println("OK");
        summary.docker_daemon = StepStatus::Ok("running".into());
    } else {
        out.println("FAILED");
        summary.docker_daemon = StepStatus::Failed("not running".into());
        bail!("Docker daemon is not running or not accessible. Start Docker and try again.");
    }

    // 2. Git root + project-specific image tag
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let image_tag = docker::project_image_tag(&git_root);
    let dockerfile = git_root.join("Dockerfile.dev");
    let config = load_repo_config(&git_root)?;
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();

    // 3. Initialize Dockerfile.dev from template if missing
    out.print("Checking Dockerfile.dev... ");
    if dockerfile.exists() {
        out.println(format!("OK ({})", dockerfile.display()));
        summary.dockerfile = StepStatus::Ok("exists".into());
    } else {
        out.println("MISSING — initializing from template...");
        let agent = agent_from_str(&agent_name);
        write_dockerfile(&git_root, &agent)?;
        out.println(format!("Dockerfile.dev created at {}", dockerfile.display()));
        summary.dockerfile = StepStatus::Ok("created".into());
    }

    // 4. Check if project image exists; build if missing
    let dockerfile_str = dockerfile.to_str().unwrap().to_string();
    let git_root_str = git_root.to_str().unwrap().to_string();
    let mount_path_str = mount_path.to_str().unwrap().to_string();

    if !docker::image_exists(&image_tag) {
        out.println(format!("Image {} not found. Building...", image_tag));
        out.println(format!(
            "$ {}",
            docker::format_build_cmd(&image_tag, &dockerfile_str, &git_root_str)
        ));
        let out_clone = out.clone();
        docker::build_image_streaming(
            &image_tag,
            &dockerfile_str,
            &git_root_str,
            |line| { out_clone.println(line); },
        )
        .context("Failed to build Docker image")?;
        out.println(format!("Image {} built successfully.", image_tag));
        summary.dev_image = StepStatus::Ok("built".into());
    } else {
        out.println(format!("Image {} found.", image_tag));
        summary.dev_image = StepStatus::Ok("exists".into());
    }

    Ok(ReadyContext {
        image_tag,
        dockerfile_str,
        git_root_str,
        mount_path: mount_path_str,
        agent_name,
        env_vars,
    })
}

/// Phase 3 — Post-audit: Rebuild the Docker image after the agent has updated Dockerfile.dev.
pub async fn run_post_audit(
    out: &OutputSink,
    ctx: &ReadyContext,
    summary: &mut ReadySummary,
) -> Result<()> {
    out.println(format!(
        "Rebuilding image {} with updated Dockerfile.dev...",
        ctx.image_tag
    ));
    out.println(format!(
        "$ {}",
        docker::format_build_cmd(&ctx.image_tag, &ctx.dockerfile_str, &ctx.git_root_str)
    ));
    let out_clone = out.clone();
    docker::build_image_streaming(
        &ctx.image_tag,
        &ctx.dockerfile_str,
        &ctx.git_root_str,
        |line| { out_clone.println(line); },
    )
    .context("Failed to rebuild Docker image")?;

    summary.image_rebuild = StepStatus::Ok("rebuilt".into());
    Ok(())
}

/// Runs ready without audit, with captured output.
///
/// Used by TUI mode and integration tests where an interactive PTY is not available.
pub async fn run_with_sink(
    out: &OutputSink,
    mount_path: PathBuf,
    env_vars: Vec<(String, String)>,
    opts: &ReadyOptions,
) -> Result<ReadySummary> {
    let mut summary = ReadySummary::default();
    let ctx = run_pre_audit(out, mount_path, env_vars, &mut summary).await?;

    if opts.refresh {
        let entrypoint = if opts.non_interactive {
            audit_entrypoint_non_interactive(&ctx.agent_name)
        } else {
            audit_entrypoint(&ctx.agent_name)
        };
        let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

        let config_dir = docker::claude_config_dir();
        let (_run_cmd, audit_output) = docker::run_container_captured(
            &ctx.image_tag,
            &ctx.mount_path,
            &entrypoint_refs,
            &ctx.env_vars,
            config_dir.as_deref(),
        )
        .context("Dockerfile audit container failed")?;
        for line in audit_output.lines() {
            out.println(line);
        }
        summary.refresh = StepStatus::Ok("completed".into());

        run_post_audit(out, &ctx, &mut summary).await?;
    } else {
        out.println("Skipping Dockerfile audit (use --refresh to run it).");
        summary.refresh = StepStatus::Skipped("use --refresh to run".into());
        summary.image_rebuild = StepStatus::Skipped("no refresh".into());
    }

    print_summary(out, &summary);

    if !opts.refresh {
        out.println(String::new());
        out.println("Tip: use `aspec ready --refresh` to run the Dockerfile audit agent.");
    }

    out.println(String::new());
    out.println("aspec is ready.");
    Ok(summary)
}

/// Build the entrypoint command for the Dockerfile audit agent (interactive mode).
pub fn audit_entrypoint(agent: &str) -> Vec<String> {
    match agent {
        "claude" => vec![
            "claude".into(),
            "--allowedTools=Edit,Write".into(),
            AUDIT_PROMPT.into(),
        ],
        "codex" => vec!["codex".into(), AUDIT_PROMPT.into()],
        "opencode" => vec!["opencode".into(), "run".into(), AUDIT_PROMPT.into()],
        _ => vec![agent.into(), AUDIT_PROMPT.into()],
    }
}

/// Build the entrypoint command for the Dockerfile audit agent (non-interactive/print mode).
pub fn audit_entrypoint_non_interactive(agent: &str) -> Vec<String> {
    match agent {
        "claude" => vec![
            "claude".into(),
            "-p".into(),
            "--allowedTools=Edit,Write".into(),
            AUDIT_PROMPT.into(),
        ],
        "codex" => vec!["codex".into(), "--quiet".into(), AUDIT_PROMPT.into()],
        "opencode" => vec!["opencode".into(), "run".into(), AUDIT_PROMPT.into()],
        _ => vec![agent.into(), AUDIT_PROMPT.into()],
    }
}

fn agent_from_str(name: &str) -> Agent {
    match name {
        "codex" => Agent::Codex,
        "opencode" => Agent::Opencode,
        _ => Agent::Claude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn run_with_sink_fails_gracefully_without_docker() {
        if docker::is_daemon_running() {
            return;
        }
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let mount_path = PathBuf::from("/tmp");
        let opts = ReadyOptions::default();
        let result = run_with_sink(&sink, mount_path, vec![], &opts).await;
        assert!(result.is_err());
        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(messages.iter().any(|m| m.contains("FAILED") || m.contains("Checking")));
    }

    /// When Docker is available, `run_with_sink` must route status messages
    /// through the OutputSink (including image tag, etc.).
    #[tokio::test]
    async fn run_with_sink_routes_all_output_through_sink() {
        if !docker::is_daemon_running() {
            return;
        }
        let git_root = match find_git_root() {
            Some(r) => r,
            None => return,
        };
        if !git_root.join("Dockerfile.dev").exists() {
            return;
        }

        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let opts = ReadyOptions::default();
        let result = run_with_sink(&sink, git_root.clone(), vec![], &opts).await;
        let _ = result;

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

        // Must include Docker-related status messages.
        let has_checking = messages.iter().any(|m| m.contains("Checking Docker daemon"));
        let has_image_status = messages
            .iter()
            .any(|m| m.contains("found") || m.contains("Building") || m.contains("not found"));
        assert!(
            has_checking,
            "Expected Docker daemon check in output. Got: {:?}",
            messages
        );
        assert!(
            has_image_status,
            "Expected image status message in output. Got: {:?}",
            messages
        );
    }

    #[tokio::test]
    async fn run_with_sink_no_refresh_skips_audit() {
        if !docker::is_daemon_running() {
            return;
        }
        let git_root = match find_git_root() {
            Some(r) => r,
            None => return,
        };
        if !git_root.join("Dockerfile.dev").exists() {
            return;
        }

        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let opts = ReadyOptions { refresh: false, non_interactive: false };
        let result = run_with_sink(&sink, git_root.clone(), vec![], &opts).await;
        let _ = result;

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let has_skip = messages.iter().any(|m| m.contains("Skipping Dockerfile audit"));
        assert!(has_skip, "Expected skip message. Got: {:?}", messages);
    }

    #[test]
    fn audit_entrypoint_claude() {
        let args = audit_entrypoint("claude");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "--allowedTools=Edit,Write");
        assert!(args[2].contains("scan this project"));
    }

    #[test]
    fn audit_entrypoint_codex() {
        let args = audit_entrypoint("codex");
        assert_eq!(args[0], "codex");
        assert!(args[1].contains("scan this project"));
    }

    #[test]
    fn audit_entrypoint_opencode() {
        let args = audit_entrypoint("opencode");
        assert_eq!(args[0], "opencode");
        assert_eq!(args[1], "run");
        assert!(args[2].contains("scan this project"));
    }

    #[test]
    fn audit_entrypoint_non_interactive_claude() {
        let args = audit_entrypoint_non_interactive("claude");
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
        assert_eq!(args[2], "--allowedTools=Edit,Write");
        assert!(args[3].contains("scan this project"));
    }

    #[test]
    fn audit_entrypoint_non_interactive_codex() {
        let args = audit_entrypoint_non_interactive("codex");
        assert_eq!(args[0], "codex");
        assert_eq!(args[1], "--quiet");
        assert!(args[2].contains("scan this project"));
    }

    #[test]
    fn agent_from_str_defaults_to_claude() {
        assert!(matches!(agent_from_str("claude"), Agent::Claude));
        assert!(matches!(agent_from_str("codex"), Agent::Codex));
        assert!(matches!(agent_from_str("opencode"), Agent::Opencode));
        assert!(matches!(agent_from_str("unknown"), Agent::Claude));
    }

    #[test]
    fn summary_default_all_pending() {
        let summary = ReadySummary::default();
        assert_eq!(summary.docker_daemon, StepStatus::Pending);
        assert_eq!(summary.dockerfile, StepStatus::Pending);
        assert_eq!(summary.dev_image, StepStatus::Pending);
        assert_eq!(summary.refresh, StepStatus::Pending);
        assert_eq!(summary.image_rebuild, StepStatus::Pending);
    }

    #[test]
    fn print_summary_outputs_table() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let summary = ReadySummary {
            docker_daemon: StepStatus::Ok("running".into()),
            dockerfile: StepStatus::Ok("exists".into()),
            dev_image: StepStatus::Ok("exists".into()),
            refresh: StepStatus::Skipped("use --refresh to run".into()),
            image_rebuild: StepStatus::Skipped("no refresh".into()),
        };
        print_summary(&sink, &summary);

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("Ready Summary"), "Missing header");
        assert!(all.contains("Docker daemon"), "Missing docker row");
        assert!(all.contains("running"), "Missing running status");
        assert!(all.contains("Refresh"), "Missing refresh row");
        assert!(all.contains("Skipped") || all.contains("–"), "Missing skip indicator");
    }

    #[test]
    fn interactive_notice_includes_agent_name() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        print_interactive_notice(&sink, "claude");
        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("INTERACTIVE"), "Missing interactive label");
        assert!(all.contains("claude"), "Missing agent name");
        assert!(all.contains("Ctrl+C"), "Missing quit hint");
    }

    #[test]
    fn ready_options_default_no_refresh() {
        let opts = ReadyOptions::default();
        assert!(!opts.refresh);
        assert!(!opts.non_interactive);
    }
}
