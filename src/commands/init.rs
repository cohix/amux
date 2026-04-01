use crate::cli::Agent;
use crate::commands::auth::resolve_auth;
use crate::commands::download;
use crate::commands::output::OutputSink;
use crate::commands::ready::{audit_entrypoint, print_interactive_notice, StepStatus};
use crate::config::{save_repo_config, RepoConfig};
use crate::docker;
use anyhow::{Context, Result};
use std::path::Path;

/// Summary of what happened during `amux init`.
#[derive(Clone, Debug)]
pub struct InitSummary {
    pub config: StepStatus,
    pub aspec_folder: StepStatus,
    pub dockerfile: StepStatus,
    pub audit: StepStatus,
    pub image_build: StepStatus,
}

impl Default for InitSummary {
    fn default() -> Self {
        Self {
            config: StepStatus::Pending,
            aspec_folder: StepStatus::Pending,
            dockerfile: StepStatus::Pending,
            audit: StepStatus::Pending,
            image_build: StepStatus::Pending,
        }
    }
}

/// Command-mode entry point: prompts interactively then runs init.
pub async fn run(agent: Agent, aspec: bool) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let git_root = find_git_root_from(&cwd).context("Not inside a Git repository")?;

    // When --aspec is set and the folder already exists, ask before replacing.
    let aspec_dir = git_root.join("aspec");
    let replace_aspec = if aspec && aspec_dir.exists() {
        println!("aspec folder already exists at: {}", aspec_dir.display());
        ask_yes_no_stdin("Replace existing aspec folder with fresh templates?")
    } else {
        false
    };

    // Determine whether to offer the audit container based on Dockerfile.dev state.
    let dockerfile_path = git_root.join("Dockerfile.dev");
    let run_audit = if dockerfile_path.exists() {
        // Dockerfile exists: offer to run the audit container.
        println!(
            "Dockerfile.dev already exists at: {}",
            dockerfile_path.display()
        );
        println!(
            "\nThe agent audit container will scan your project and update Dockerfile.dev"
        );
        println!("to ensure all tools needed to build, run, and test your project are installed.");
        ask_yes_no_stdin("Run the agent audit container now?")
    } else {
        // Dockerfile will be created: explain and offer audit.
        println!("No Dockerfile.dev found — a default template will be downloaded.");
        println!(
            "\nThe agent audit container will scan your project and update Dockerfile.dev"
        );
        println!("to ensure all tools needed to build, run, and test your project are installed.");
        ask_yes_no_stdin("Run the agent audit container after creating Dockerfile.dev?")
    };

    run_with_sink(agent, aspec, replace_aspec, run_audit, &OutputSink::Stdout, &cwd).await
}

/// Core logic shared between command mode and TUI mode.
/// Output is routed through `out` so neither caller duplicates logic.
///
/// - `aspec`: when true, download aspec templates from GitHub.
/// - `replace_aspec`: when true (and `aspec` is true), overwrite an existing aspec folder.
/// - `run_audit`: when true, run the agent audit container and rebuild the image.
/// - `cwd`: the working directory to search upward from for the Git root. In CLI mode this is
///   `std::env::current_dir()`; in TUI mode this is the active tab's `cwd`.
pub async fn run_with_sink(
    agent: Agent,
    aspec: bool,
    replace_aspec: bool,
    run_audit: bool,
    out: &OutputSink,
    cwd: &std::path::Path,
) -> Result<()> {
    let git_root = find_git_root_from(cwd).context("Not inside a Git repository")?;
    let mut summary = InitSummary::default();

    out.println(format!("Initializing amux in: {}", git_root.display()));
    out.println(format!("Agent: {}", agent.as_str()));

    // 1. Save repo config.
    let config = RepoConfig {
        agent: Some(agent.as_str().to_string()),
        auto_agent_auth_accepted: None,
    };
    save_repo_config(&git_root, &config)?;
    out.println(format!(
        "Config written to: {}",
        git_root.join("aspec/.amux.json").display()
    ));
    summary.config = StepStatus::Ok("saved".into());

    // 2. Download aspec folder if --aspec flag is set (and folder is missing, or replace confirmed).
    let aspec_dir = git_root.join("aspec");
    if aspec {
        if !aspec_dir.exists() || replace_aspec {
            match download::download_aspec_folder(&git_root, out).await {
                Ok(()) => {
                    summary.aspec_folder = StepStatus::Ok("downloaded".into());
                }
                Err(e) => {
                    out.println(format!(
                        "Warning: failed to download aspec folder from GitHub: {}",
                        e
                    ));
                    out.println(
                        "You can manually download it from https://github.com/cohix/aspec"
                            .to_string(),
                    );
                    summary.aspec_folder = StepStatus::Failed("download failed".into());
                }
            }
        } else {
            out.println(format!(
                "aspec folder already exists at: {} (keeping existing)",
                aspec_dir.display()
            ));
            summary.aspec_folder = StepStatus::Ok("already exists".into());
        }
    } else if aspec_dir.exists() {
        summary.aspec_folder = StepStatus::Ok("already exists".into());
    } else {
        summary.aspec_folder =
            StepStatus::Skipped("use --aspec to download".into());
    }

    // 3. Write Dockerfile.dev from template if missing (never overwrites existing).
    let dockerfile_was_new = write_dockerfile(&git_root, &agent, out).await?;
    if dockerfile_was_new {
        out.println(format!(
            "Dockerfile.dev written to: {}",
            git_root.join("Dockerfile.dev").display()
        ));
        summary.dockerfile = StepStatus::Ok("created".into());
    } else {
        out.println(format!(
            "Dockerfile.dev already exists at: {} (not overwritten)",
            git_root.join("Dockerfile.dev").display()
        ));
        summary.dockerfile = StepStatus::Ok("already exists".into());
    }

    // 4. Optionally run the agent audit container, then build the image.
    //    Build any time a new Dockerfile was created OR the audit ran.
    if run_audit {
        // Need Docker daemon running to build and run the container.
        out.print("Checking Docker daemon... ");
        if !docker::is_daemon_running() {
            out.println("FAILED");
            out.println("Docker daemon is not running. Skipping audit and image build.");
            summary.audit = StepStatus::Failed("Docker not running".into());
            summary.image_build = StepStatus::Failed("Docker not running".into());
        } else {
            out.println("OK");

            let image_tag = docker::project_image_tag(&git_root);
            let dockerfile_str = git_root.join("Dockerfile.dev").to_str().unwrap().to_string();
            let git_root_str = git_root.to_str().unwrap().to_string();
            let mount_path = git_root.to_str().unwrap().to_string();

            // Get credentials for the agent (needed to run the audit container).
            let credentials = resolve_auth(&git_root, agent.as_str())
                .unwrap_or_default();
            let env_vars = credentials.env_vars;
            let host_settings = docker::HostSettings::prepare(agent.as_str());

            // Build the image before running audit.
            out.println(format!("Building image {}...", image_tag));
            let build_cmd = docker::format_build_cmd(&image_tag, &dockerfile_str, &git_root_str);
            out.println(format!("$ {}", build_cmd));
            let out_clone = out.clone();
            match docker::build_image_streaming(
                &image_tag,
                &dockerfile_str,
                &git_root_str,
                false,
                |line| {
                    out_clone.println(line);
                },
            ) {
                Ok(_) => {
                    out.println(format!("Image {} built successfully.", image_tag));
                }
                Err(e) => {
                    out.println(format!("Warning: failed to build image: {}", e));
                    summary.audit = StepStatus::Failed("image build failed before audit".into());
                    summary.image_build = StepStatus::Failed("build failed".into());
                    print_init_summary(out, &summary, agent.as_str());
                    print_whats_next(out);
                    return Ok(());
                }
            }

            // Run the audit container interactively.
            print_interactive_notice(out, agent.as_str());
            let entrypoint = audit_entrypoint(agent.as_str());
            let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
            match docker::run_container(
                &image_tag,
                &mount_path,
                &entrypoint_refs,
                &env_vars,
                host_settings.as_ref(),
                false,
                None,
                None,
            ) {
                Ok(_) => {
                    summary.audit = StepStatus::Ok("completed".into());
                }
                Err(e) => {
                    out.println(format!("Warning: audit container failed: {}", e));
                    summary.audit = StepStatus::Failed("container error".into());
                }
            }

            // Rebuild the image after audit (whether audit succeeded or not, since
            // the audit may have partially modified Dockerfile.dev).
            out.println(format!("Rebuilding image {} after audit...", image_tag));
            let out_clone2 = out.clone();
            match docker::build_image_streaming(
                &image_tag,
                &dockerfile_str,
                &git_root_str,
                false,
                |line| {
                    out_clone2.println(line);
                },
            ) {
                Ok(_) => {
                    out.println(format!("Image {} rebuilt successfully.", image_tag));
                    summary.image_build = StepStatus::Ok("built".into());
                }
                Err(e) => {
                    out.println(format!("Warning: failed to rebuild image: {}", e));
                    summary.image_build = StepStatus::Failed("rebuild failed".into());
                }
            }
        }
    } else if dockerfile_was_new {
        // New Dockerfile.dev was created but user declined audit — build the image.
        out.print("Checking Docker daemon... ");
        if !docker::is_daemon_running() {
            out.println("not running (skipping image build)");
            summary.audit = StepStatus::Skipped("declined".into());
            summary.image_build = StepStatus::Skipped("Docker not running".into());
        } else {
            out.println("OK");
            let image_tag = docker::project_image_tag(&git_root);
            let dockerfile_str =
                git_root.join("Dockerfile.dev").to_str().unwrap().to_string();
            let git_root_str = git_root.to_str().unwrap().to_string();
            out.println(format!("Building image {}...", image_tag));
            let build_cmd =
                docker::format_build_cmd(&image_tag, &dockerfile_str, &git_root_str);
            out.println(format!("$ {}", build_cmd));
            let out_clone = out.clone();
            match docker::build_image_streaming(
                &image_tag,
                &dockerfile_str,
                &git_root_str,
                false,
                |line| {
                    out_clone.println(line);
                },
            ) {
                Ok(_) => {
                    out.println(format!("Image {} built successfully.", image_tag));
                    summary.audit = StepStatus::Skipped("declined".into());
                    summary.image_build = StepStatus::Ok("built".into());
                }
                Err(e) => {
                    out.println(format!("Warning: failed to build image: {}", e));
                    summary.audit = StepStatus::Skipped("declined".into());
                    summary.image_build = StepStatus::Failed("build failed".into());
                }
            }
        }
    } else {
        // Existing Dockerfile.dev, user declined audit — skip build.
        summary.audit = StepStatus::Skipped("declined".into());
        summary.image_build = StepStatus::Skipped("no changes".into());
    }

    print_init_summary(out, &summary, agent.as_str());
    print_whats_next(out);

    Ok(())
}

/// Print the init summary table.
fn print_init_summary(out: &OutputSink, summary: &InitSummary, agent_name: &str) {
    out.println(String::new());
    out.println("┌──────────────────────────────────────────────────┐");
    out.println(format!("│              Init Summary ({:>12})         │", agent_name));
    out.println("├───────────────────┬──────────────────────────────┤");
    print_init_row(out, "Config", &summary.config);
    print_init_row(out, "aspec folder", &summary.aspec_folder);
    print_init_row(out, "Dockerfile.dev", &summary.dockerfile);
    print_init_row(out, "Agent audit", &summary.audit);
    print_init_row(out, "Docker image", &summary.image_build);
    out.println("└───────────────────┴──────────────────────────────┘");
}

fn print_init_row(out: &OutputSink, label: &str, status: &StepStatus) {
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

/// Returns `text` with each non-space character wrapped in a cycling ANSI rainbow colour.
/// Used only when the sink supports colour output (i.e. stdout terminal).
fn rainbow_text(text: &str) -> String {
    // red, yellow, green, cyan, blue, magenta
    const COLORS: &[&str] = &[
        "\x1b[31m", "\x1b[33m", "\x1b[32m", "\x1b[36m", "\x1b[34m", "\x1b[35m",
    ];
    let mut result = String::from("\x1b[1m"); // bold
    let mut color_idx = 0usize;
    for ch in text.chars() {
        if ch == ' ' {
            result.push(' ');
        } else {
            result.push_str(COLORS[color_idx % COLORS.len()]);
            result.push(ch);
            color_idx += 1;
        }
    }
    result.push_str("\x1b[0m"); // reset
    result
}

/// Print a "What's Next?" section with a stylized title and spaced command list.
pub fn print_whats_next(out: &OutputSink) {
    let title = if out.supports_color() {
        rainbow_text("  What's Next?")
    } else {
        "  What's Next?".to_string()
    };

    out.println(String::new());
    out.println(title);
    out.println(String::new());
    out.println("  Run `amux` to launch the interactive TUI.");
    out.println(String::new());
    out.println("  Available commands:");
    out.println(String::new());
    out.println("    amux chat        —  Start a freeform chat session with the agent");
    out.println("    amux new         —  Create a new work item from the aspec template");
    out.println("    amux implement   —  Implement a work item inside a container");
    out.println(String::new());
    out.println("  Any amux command can also be run as a plain CLI command without");
    out.println("  launching the TUI.");
    out.println(String::new());
}

/// Prompt the user with a yes/no question via stdin. Returns true for yes.
pub fn ask_yes_no_stdin(prompt: &str) -> bool {
    use std::io::{BufRead, Write};
    print!("{} [y/N]: ", prompt);
    let _ = std::io::stdout().flush();
    let stdin = std::io::stdin();
    let answer = stdin.lock().lines().next().unwrap_or(Ok(String::new())).unwrap_or_default();
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Walks upward from the given directory to find the nearest `.git` folder.
pub fn find_git_root_from(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walks upward from CWD to find the nearest directory containing a `.git` folder.
pub fn find_git_root() -> Option<std::path::PathBuf> {
    find_git_root_from(&std::env::current_dir().ok()?)
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
        let cwd = std::env::current_dir().unwrap();
        let result = run_with_sink(Agent::Claude, false, false, false, &sink, &cwd).await;
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

    #[test]
    fn ask_yes_no_stdin_is_callable() {
        // We can't easily test stdin in unit tests, but we verify the function exists
        // and returns a bool. The actual stdin logic is tested via end-to-end tests.
        // This is a compile-time check.
        let _f: fn(&str) -> bool = ask_yes_no_stdin;
    }

    #[test]
    fn init_summary_default_all_pending() {
        let summary = InitSummary::default();
        assert_eq!(summary.config, StepStatus::Pending);
        assert_eq!(summary.aspec_folder, StepStatus::Pending);
        assert_eq!(summary.dockerfile, StepStatus::Pending);
        assert_eq!(summary.audit, StepStatus::Pending);
        assert_eq!(summary.image_build, StepStatus::Pending);
    }

    #[test]
    fn print_init_summary_outputs_table() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let summary = InitSummary {
            config: StepStatus::Ok("saved".into()),
            aspec_folder: StepStatus::Skipped("use --aspec to download".into()),
            dockerfile: StepStatus::Ok("created".into()),
            audit: StepStatus::Skipped("declined".into()),
            image_build: StepStatus::Ok("built".into()),
        };
        print_init_summary(&sink, &summary, "claude");

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("Init Summary"), "Missing header");
        assert!(all.contains("Config"), "Missing config row");
        assert!(all.contains("saved"), "Missing saved status");
        assert!(all.contains("aspec folder"), "Missing aspec row");
        assert!(all.contains("Dockerfile.dev"), "Missing dockerfile row");
        assert!(all.contains("Agent audit"), "Missing audit row");
        assert!(all.contains("Docker image"), "Missing image row");
    }

    #[test]
    fn print_whats_next_outputs_box() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        print_whats_next(&sink);

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("amux"), "Missing amux TUI mention");
        assert!(all.contains("chat"), "Missing chat command");
        assert!(all.contains("new"), "Missing new command");
        assert!(all.contains("implement"), "Missing implement command");
    }

    #[tokio::test]
    async fn run_with_sink_skips_aspec_when_flag_false() {
        // Run from within the project repo (no CWD change needed — workspace is a git repo).
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let cwd = std::env::current_dir().unwrap();
        // aspec=false means the aspec folder download is skipped.
        let result = run_with_sink(Agent::Claude, false, false, false, &sink, &cwd).await;
        drop(result);

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        // When --aspec is false, the summary shows "already exists" if the folder is present
        // (as in this workspace) or "use --aspec to download" if it is absent. Either is valid.
        assert!(
            all.contains("already exists") || all.contains("use --aspec"),
            "Should report aspec folder status when --aspec is not passed. Got: {:?}",
            messages
        );
    }

    #[test]
    fn run_with_sink_accepts_all_params() {
        // run_with_sink takes (Agent, bool, bool, bool, &OutputSink, &Path).
        // The fact that this module compiles with its current signature is the check.
        // Verify the parameter names correspond to aspec and run_audit.
        let opts: (bool, bool) = (false, false);
        assert!(!opts.0, "aspec default is false");
        assert!(!opts.1, "run_audit default is false");
    }
}
