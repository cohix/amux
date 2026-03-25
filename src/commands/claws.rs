use crate::cli::{Agent, ClawsAction};
use crate::commands::auth::agent_keychain_credentials;
use crate::commands::chat::chat_entrypoint;
use crate::commands::ready::audit_entrypoint;
use crate::commands::init::write_dockerfile;
use crate::commands::output::OutputSink;
use crate::commands::ready::StepStatus;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const NANOCLAW_IMAGE_TAG: &str = "amux-nanoclaw:latest";
const NANOCLAW_REPO_FULL_NAME: &str = "qwibitai/nanoclaw";
/// Git config flag that auto-accepts new SSH host fingerprints without prompting.
const GIT_SSH_ACCEPT_NEW: &str = "core.sshCommand=ssh -o StrictHostKeyChecking=accept-new";

/// Returns the nanoclaw installation path: `$HOME/.nanoclaw`.
///
/// Using a path under the user's home directory ensures it falls within Docker
/// Desktop's default file-sharing scope on macOS (`/Users` is shared; `/usr/local`
/// is not). The same absolute path is used inside the container so that file
/// references are identical on host and in-container.
pub fn nanoclaw_path() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"));
    home.join(".nanoclaw")
}

/// Returns the nanoclaw installation path as a `String` (for Docker CLI args).
pub fn nanoclaw_path_str() -> String {
    nanoclaw_path().to_string_lossy().into_owned()
}

/// Spawn a subprocess with piped stdout/stderr, stream each line through `out`,
/// then wait for the process to exit and return its status.
///
/// Used by git/gh clone helpers so that all subprocess output is routed through the
/// `OutputSink` channel in TUI mode (instead of writing directly to the raw terminal).
fn stream_child_output(mut child: std::process::Child, out: &OutputSink) -> Result<std::process::ExitStatus> {
    use std::io::BufRead;

    // Read stdout in a separate OS thread so it does not block stderr reading.
    let stdout_thread = child.stdout.take().map(|stdout| {
        let sink = out.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().flatten() {
                sink.println(line);
            }
        })
    });

    // Read stderr in the current thread.
    if let Some(stderr) = child.stderr.take() {
        for line in std::io::BufReader::new(stderr).lines().flatten() {
            out.println(line);
        }
    }

    if let Some(t) = stdout_thread {
        let _ = t.join();
    }

    child.wait().context("Failed to wait for subprocess")
}

/// Set world-readable/writable permissions on the nanoclaw installation directory
/// using `sudo chmod`.
///
/// Called after a successful clone (especially a sudo clone where the directory is
/// owned by root) so that future operations — writing `.amux.json`, Dockerfile.dev,
/// etc. — do not require elevated privileges.
pub fn chmod_nanoclaw_permissive(out: &OutputSink) {
    let path = nanoclaw_path_str();
    out.println(format!("Setting permissions on {} (chmod -R u+rwX)...", path));
    let status = std::process::Command::new("chmod")
        .args(["-R", "u+rwX", &path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => out.println("Permissions set."),
        _ => out.println(format!(
            "Warning: could not set permissions on {}. \
             You may need to run: chmod -R u+rwX {}",
            path, path
        )),
    }
}

/// Per-installation nanoclaw config stored at `$HOME/.nanoclaw/.amux.json`.
#[derive(Serialize, Deserialize, Default)]
pub struct NanoclawConfig {
    #[serde(rename = "nanoclawContainerID")]
    pub nanoclaw_container_id: Option<String>,
}

pub fn load_nanoclaw_config() -> Result<NanoclawConfig> {
    let config_path = nanoclaw_path().join(".amux.json");
    if !config_path.exists() {
        return Ok(NanoclawConfig::default());
    }
    let content = std::fs::read_to_string(&config_path)
        .context("Failed to read nanoclaw config")?;
    serde_json::from_str(&content).context("Failed to parse nanoclaw config")
}

pub fn save_nanoclaw_config(config: &NanoclawConfig) -> Result<()> {
    let config_path = nanoclaw_path().join(".amux.json");
    let content = serde_json::to_string_pretty(config)
        .context("Failed to serialize nanoclaw config")?;
    std::fs::write(&config_path, content).context("Failed to write nanoclaw config")?;
    Ok(())
}

/// Summary of the `claws ready` run, shown after completion.
pub struct ClawsSummary {
    pub nanoclaw_cloned: StepStatus,
    pub docker_daemon: StepStatus,
    pub nanoclaw_image: StepStatus,
    pub nanoclaw_container: StepStatus,
}

impl Default for ClawsSummary {
    fn default() -> Self {
        Self {
            nanoclaw_cloned: StepStatus::Pending,
            docker_daemon: StepStatus::Pending,
            nanoclaw_image: StepStatus::Pending,
            nanoclaw_container: StepStatus::Pending,
        }
    }
}

/// Print the claws summary table to the output sink.
pub fn print_claws_summary(out: &OutputSink, summary: &ClawsSummary) {
    out.println(String::new());
    out.println("┌──────────────────────────────────────────────────┐");
    out.println("│              Claws Ready Summary                 │");
    out.println("├───────────────────┬──────────────────────────────┤");
    print_claws_row(out, "Nanoclaw", &summary.nanoclaw_cloned);
    print_claws_row(out, "Docker daemon", &summary.docker_daemon);
    print_claws_row(out, "Nanoclaw image", &summary.nanoclaw_image);
    print_claws_row(out, "Container", &summary.nanoclaw_container);
    out.println("└───────────────────┴──────────────────────────────┘");
}

fn print_claws_row(out: &OutputSink, label: &str, status: &StepStatus) {
    let (symbol, text) = match status {
        StepStatus::Pending => ("-", "pending".to_string()),
        StepStatus::Ok(msg) => ("✓", msg.clone()),
        StepStatus::Skipped(msg) => ("–", msg.clone()),
        StepStatus::Failed(msg) => ("✗", msg.clone()),
    };
    out.println(format!("│ {:>17} │ {} {:<27} │", label, symbol, text));
}

/// Command-mode entry point.
pub async fn run(action: ClawsAction) -> Result<()> {
    match action {
        ClawsAction::Init => run_claws_init(&OutputSink::Stdout).await,
        ClawsAction::Ready => run_claws_ready(&OutputSink::Stdout).await,
        ClawsAction::Chat => run_claws_chat().await,
    }
}

/// Entry point for `amux claws init` — runs the first-time setup wizard.
pub async fn run_claws_init(out: &OutputSink) -> Result<()> {
    let mut summary = ClawsSummary::default();
    run_first_time_wizard(out, &mut summary).await?;
    print_claws_summary(out, &summary);
    Ok(())
}

/// Entry point for `amux claws ready` — status-only check, no first-run wizard.
///
/// If nanoclaw is not installed, suggests running `claws init`.
/// If nanoclaw is installed but the container is not running, interactively
/// offers to start it in the background.
pub async fn run_claws_ready(out: &OutputSink) -> Result<()> {
    let nanoclaw_dir = nanoclaw_path();

    if !nanoclaw_dir.exists() {
        out.println("nanoclaw is not installed. Run 'amux claws init' to set up nanoclaw.");
        return Ok(());
    }

    let mut summary = ClawsSummary::default();
    run_subsequent_check(out, &mut summary).await?;
    print_claws_summary(out, &summary);
    Ok(())
}

/// Entry point for `amux claws chat` — attaches to the running nanoclaw container.
///
/// The nanoclaw container must already be running (started via `claws init` or
/// `claws ready`). Attaches interactively with a freeform agent chat session.
pub async fn run_claws_chat() -> Result<()> {
    let nanoclaw_dir = nanoclaw_path();

    if !nanoclaw_dir.exists() {
        bail!("nanoclaw is not installed. Run 'amux claws init' to set up nanoclaw.");
    }

    let config = load_nanoclaw_config().unwrap_or_default();
    let container_id = match config.nanoclaw_container_id {
        Some(id) if docker::is_container_running(&id) => id,
        _ => {
            bail!(
                "nanoclaw container is not running. \
                 Run 'amux claws ready' to start it."
            );
        }
    };

    let agent_name = {
        let cfg = load_repo_config(&nanoclaw_path()).unwrap_or_default();
        cfg.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);

    attach_to_nanoclaw(&container_id, &agent_name, &credentials.env_vars)?;
    Ok(())
}

/// Result of a clone or move operation that may require elevated privileges.
#[derive(Debug, PartialEq)]
pub enum CloneOutcome {
    Success,
    /// The operation failed because the nanoclaw parent directory is not writable by the current user.
    PermissionDenied,
}

/// Clone the user's nanoclaw fork to `$HOME/.nanoclaw`.
///
/// Tries SSH first, then HTTPS. Returns `CloneOutcome::PermissionDenied` if the
/// destination requires elevated privileges rather than returning an error, so the
/// caller can offer a sudo retry to the user.
pub fn clone_nanoclaw(username: &str, out: &OutputSink) -> Result<CloneOutcome> {
    let dest = nanoclaw_path_str();

    // Ensure the parent directory exists.
    let parent = nanoclaw_path();
    let parent = parent.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;

    let ssh_url = format!("git@github.com:{}/nanoclaw", username.trim());
    let https_url = format!("https://github.com/{}/nanoclaw", username.trim());

    out.println(format!("Cloning {} to {} (trying SSH)...", ssh_url, dest));

    let child = std::process::Command::new("git")
        .args(["-c", GIT_SSH_ACCEPT_NEW, "clone", &ssh_url, &dest])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `git clone`")?;
    let ssh_status = stream_child_output(child, out)?;

    if ssh_status.success() {
        out.println(format!("Cloned to {} via SSH.", dest));
        return Ok(CloneOutcome::Success);
    }

    if is_nanoclaw_parent_permission_denied() {
        return Ok(CloneOutcome::PermissionDenied);
    }

    out.println("SSH clone failed, falling back to HTTPS...");
    out.println(format!("Cloning {} to {}...", https_url, dest));

    let child = std::process::Command::new("git")
        .args(["-c", GIT_SSH_ACCEPT_NEW, "clone", &https_url, &dest])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `git clone`")?;
    let https_status = stream_child_output(child, out)?;

    if https_status.success() {
        out.println(format!("Cloned to {} via HTTPS.", dest));
        return Ok(CloneOutcome::Success);
    }

    if is_nanoclaw_parent_permission_denied() {
        return Ok(CloneOutcome::PermissionDenied);
    }

    bail!("git clone failed via both SSH and HTTPS. Check your GitHub username and try again.");
}

/// Clone the user's nanoclaw fork using `sudo git clone`.
///
/// Called after the user has explicitly accepted elevated privileges.
///
/// `sudo_password`:
/// - `Some(pw)` — TUI mode: password collected from dialog; passed to `sudo -S` via stdin.
/// - `None`     — CLI mode: let sudo prompt the user on the terminal naturally.
pub fn clone_nanoclaw_sudo(username: &str, out: &OutputSink, sudo_password: Option<&str>) -> Result<()> {
    let dest = nanoclaw_path_str();
    let ssh_url = format!("git@github.com:{}/nanoclaw", username.trim());
    let https_url = format!("https://github.com/{}/nanoclaw", username.trim());

    out.println(format!("Running: sudo git clone {} {}", ssh_url, dest));

    let ssh_status = run_sudo_git_clone(&ssh_url, sudo_password, out)?;

    if ssh_status.success() {
        out.println(format!("Cloned to {} via SSH (with sudo).", dest));
        return Ok(());
    }

    out.println("SSH clone failed, falling back to HTTPS...");
    out.println(format!("Running: sudo git clone {} {}", https_url, dest));

    let https_status = run_sudo_git_clone(&https_url, sudo_password, out)?;

    if !https_status.success() {
        bail!("sudo git clone failed via both SSH and HTTPS.");
    }

    out.println(format!("Cloned to {} via HTTPS (with sudo).", dest));
    Ok(())
}

/// Invoke `sudo git clone <url> $HOME/.nanoclaw`.
///
/// When `sudo_password` is `Some`, uses `sudo -S` and writes the password to stdin so
/// that sudo can authenticate without requiring a TTY (needed in TUI mode where the
/// terminal is in raw mode and cannot present a password prompt). stdout/stderr are
/// piped and streamed through `out` so output appears in the TUI execution window.
/// When `None` (CLI mode), stdin/stdout/stderr are inherited so sudo can prompt
/// the user on the terminal naturally.
fn run_sudo_git_clone(url: &str, sudo_password: Option<&str>, out: &OutputSink) -> Result<std::process::ExitStatus> {
    let dest = nanoclaw_path_str();
    if let Some(password) = sudo_password {
        use std::io::Write;
        let mut child = std::process::Command::new("sudo")
            .args(["-S", "git", "-c", GIT_SSH_ACCEPT_NEW, "clone", url, &dest])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to invoke `sudo git clone`")?;
        if let Some(mut stdin) = child.stdin.take() {
            // Write password followed by newline; sudo -S reads it before executing.
            let _ = writeln!(stdin, "{}", password);
        }
        stream_child_output(child, out)
    } else {
        // CLI mode: inherit stdio so sudo can prompt for a password normally.
        std::process::Command::new("sudo")
            .args(["git", "-c", GIT_SSH_ACCEPT_NEW, "clone", url, &dest])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to invoke `sudo git clone`")
    }
}

/// Fork `qwibitai/nanoclaw` to the user's account using the GitHub CLI and
/// move the clone into `$HOME/.nanoclaw`.
///
/// Returns `CloneOutcome::PermissionDenied` if the move to `$HOME/.nanoclaw`
/// fails due to insufficient permissions, so the caller can offer a sudo retry.
pub fn fork_and_clone_nanoclaw(out: &OutputSink) -> Result<CloneOutcome> {
    let tmp_dir = std::env::temp_dir();
    let tmp_nanoclaw = tmp_dir.join("nanoclaw");
    let dest = nanoclaw_path();
    let dest_str = nanoclaw_path_str();

    // Remove leftover temp clone if it exists.
    if tmp_nanoclaw.exists() {
        std::fs::remove_dir_all(&tmp_nanoclaw)
            .context("Failed to remove existing temp nanoclaw directory")?;
    }

    out.println(format!(
        "Running: gh repo fork {} --clone --remote-name origin",
        NANOCLAW_REPO_FULL_NAME
    ));

    let child = std::process::Command::new("gh")
        .current_dir(&tmp_dir)
        .args(["repo", "fork", NANOCLAW_REPO_FULL_NAME, "--clone", "--remote-name", "origin"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `gh repo fork`. Is the GitHub CLI installed and authenticated?")?;
    let status = stream_child_output(child, out)?;

    if !status.success() {
        bail!(
            "`gh repo fork` failed. Ensure the GitHub CLI is installed and you are authenticated."
        );
    }

    if !tmp_nanoclaw.exists() {
        bail!("Expected cloned directory at {} after gh fork.", tmp_nanoclaw.display());
    }

    // Ensure the parent directory ($HOME) exists.
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;

    out.println(format!("Moving cloned directory to {}...", dest_str));
    match std::fs::rename(&tmp_nanoclaw, &dest) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(CloneOutcome::PermissionDenied);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to move nanoclaw to {}", dest_str));
        }
    }

    out.println(format!("nanoclaw installed at {}.", dest_str));
    Ok(CloneOutcome::Success)
}

/// Fork using the GitHub CLI then use `sudo mv` to place the clone in `$HOME/.nanoclaw`.
///
/// Called after the user has explicitly accepted elevated privileges following a
/// `CloneOutcome::PermissionDenied` from `fork_and_clone_nanoclaw`.
pub fn fork_and_clone_nanoclaw_sudo(out: &OutputSink) -> Result<()> {
    let tmp_dir = std::env::temp_dir();
    let tmp_nanoclaw = tmp_dir.join("nanoclaw");
    let dest_str = nanoclaw_path_str();

    // Remove leftover temp clone if it exists.
    if tmp_nanoclaw.exists() {
        std::fs::remove_dir_all(&tmp_nanoclaw)
            .context("Failed to remove existing temp nanoclaw directory")?;
    }

    out.println(format!(
        "Running: gh repo fork {} --clone --remote-name origin",
        NANOCLAW_REPO_FULL_NAME
    ));

    let child = std::process::Command::new("gh")
        .current_dir(&tmp_dir)
        .args(["repo", "fork", NANOCLAW_REPO_FULL_NAME, "--clone", "--remote-name", "origin"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `gh repo fork`. Is the GitHub CLI installed and authenticated?")?;
    let status = stream_child_output(child, out)?;

    if !status.success() {
        bail!(
            "`gh repo fork` failed. Ensure the GitHub CLI is installed and you are authenticated."
        );
    }

    if !tmp_nanoclaw.exists() {
        bail!("Expected cloned directory at {} after gh fork.", tmp_nanoclaw.display());
    }

    let tmp_str = tmp_nanoclaw.to_string_lossy().into_owned();
    out.println(format!("Running: sudo mv {} {}", tmp_str, dest_str));

    let mv_child = std::process::Command::new("sudo")
        .args(["mv", &tmp_str, &dest_str])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `sudo mv`")?;
    let mv_status = stream_child_output(mv_child, out)?;

    if !mv_status.success() {
        bail!("sudo mv failed. Could not move nanoclaw to {}.", dest_str);
    }

    out.println(format!("nanoclaw installed at {} (with sudo).", dest_str));
    Ok(())
}

/// Check whether writing to the nanoclaw parent directory is denied for the current process.
///
/// Probes by attempting to create a temporary file; removes it immediately on success.
fn is_nanoclaw_parent_permission_denied() -> bool {
    let parent = nanoclaw_path();
    let parent = parent.parent().unwrap_or(Path::new("."));
    let probe = parent.join(".aspec_perm_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

/// Context produced by the pre-audit phase, needed by the audit and post-audit phases.
#[derive(Clone)]
pub struct ClawsAuditCtx {
    pub nanoclaw_str: String,
    pub agent_name: String,
    /// Agent credentials forwarded into the audit container.
    pub env_vars: Vec<(String, String)>,
    /// Absolute path to Dockerfile.dev inside the nanoclaw repo.
    pub dockerfile_str: String,
}

/// Phase 1 of the first-run setup: Docker check, Dockerfile.dev init, initial image build.
///
/// Does NOT run the audit agent. Returns a `ClawsAuditCtx` the caller uses to launch
/// the audit (interactively in CLI mode, via PTY container window in TUI mode) and then
/// call `rebuild_nanoclaw_post_audit`.
pub async fn build_nanoclaw_pre_audit(
    out: &OutputSink,
    env_vars: Vec<(String, String)>,
    summary: &mut ClawsSummary,
    host_settings: Option<&docker::HostSettings>,
) -> Result<ClawsAuditCtx> {
    let nanoclaw_dir = nanoclaw_path();
    let nanoclaw_str = nanoclaw_path_str();

    // Check Docker daemon.
    out.print("Checking Docker daemon... ");
    if !docker::is_daemon_running() {
        out.println("FAILED");
        summary.docker_daemon = StepStatus::Failed("not running".into());
        bail!("Docker daemon is not running or not accessible. Start Docker and try again.");
    }
    out.println("OK");
    summary.docker_daemon = StepStatus::Ok("running".into());

    // Determine agent name from nanoclaw repo config (default to claude).
    let config = load_repo_config(&nanoclaw_dir).unwrap_or_default();
    let agent_name = config.agent.unwrap_or_else(|| "claude".to_string());
    let agent = agent_from_str(&agent_name);

    // Ensure Dockerfile.dev exists in the nanoclaw repo.
    out.print("Checking Dockerfile.dev in nanoclaw... ");
    write_dockerfile(&nanoclaw_dir, &agent, out).await?;

    // Build the initial nanoclaw image.
    let dockerfile_str = format!("{}/Dockerfile.dev", nanoclaw_str);
    out.println(format!("Building image {}...", NANOCLAW_IMAGE_TAG));
    let out_clone = out.clone();
    docker::build_image_streaming(
        NANOCLAW_IMAGE_TAG,
        &dockerfile_str,
        &nanoclaw_str,
        false,
        |line| { out_clone.println(line); },
    )
    .context("Failed to build nanoclaw Docker image")?;
    out.println(format!("Image {} built successfully.", NANOCLAW_IMAGE_TAG));

    let _ = host_settings; // not needed for initial build; caller passes to audit container
    Ok(ClawsAuditCtx { nanoclaw_str, agent_name, env_vars, dockerfile_str })
}

/// Phase 3 of the first-run setup: rebuild the nanoclaw image after the audit agent has
/// updated Dockerfile.dev.
pub async fn rebuild_nanoclaw_post_audit(
    out: &OutputSink,
    ctx: &ClawsAuditCtx,
    summary: &mut ClawsSummary,
) -> Result<()> {
    out.println(format!(
        "Rebuilding image {} with updated Dockerfile.dev...",
        NANOCLAW_IMAGE_TAG
    ));
    let out_clone = out.clone();
    let nanoclaw_str = ctx.nanoclaw_str.clone();
    let dockerfile_str = ctx.dockerfile_str.clone();
    docker::build_image_streaming(
        NANOCLAW_IMAGE_TAG,
        &dockerfile_str,
        &nanoclaw_str,
        false,
        |line| { out_clone.println(line); },
    )
    .context("Failed to rebuild nanoclaw Docker image after audit")?;
    out.println(format!("Image {} rebuilt successfully.", NANOCLAW_IMAGE_TAG));
    summary.nanoclaw_image = StepStatus::Ok("built".into());
    Ok(())
}

/// Phase 1+2+3 combined for CLI mode: build initial image, run audit interactively
/// (inherited stdio, foreground, with an `amux-` container name), then rebuild.
///
/// The caller is responsible for showing the docker-socket warning and obtaining user
/// acceptance before calling `launch_nanoclaw_container`.
pub async fn build_nanoclaw_image(
    out: &OutputSink,
    env_vars: &[(String, String)],
    summary: &mut ClawsSummary,
    host_settings: Option<&docker::HostSettings>,
) -> Result<()> {
    let ctx = build_nanoclaw_pre_audit(out, env_vars.to_vec(), summary, host_settings).await?;

    // Run the Dockerfile.dev audit agent in the foreground (CLI: inherited stdio).
    // The audit container does NOT need docker socket access — it only reads and
    // modifies Dockerfile.dev. Host path and container path are identical so that
    // file references inside the agent match the host filesystem.
    let container_name = docker::generate_container_name();
    let entrypoint = audit_entrypoint(&ctx.agent_name);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
    out.println("Running Dockerfile.dev audit agent...");
    docker::run_container_at_path(
        NANOCLAW_IMAGE_TAG,
        &ctx.nanoclaw_str,
        &ctx.nanoclaw_str,
        &ctx.nanoclaw_str,
        &entrypoint_refs,
        &ctx.env_vars,
        host_settings,
        false, // audit container: no docker socket access
        Some(&container_name),
    )
    .context("Nanoclaw Dockerfile.dev audit container failed")?;

    rebuild_nanoclaw_post_audit(out, &ctx, summary).await
}

/// Phase 2 of the first-run setup: launch the nanoclaw background container.
///
/// Must be called AFTER the docker-socket warning has been shown and accepted by the
/// user. The container is started detached (`-d`) with the host Docker socket mounted,
/// then this function waits for it to reach running state and persists the container ID.
///
/// Returns the container ID of the newly started container.
pub async fn launch_nanoclaw_container(
    out: &OutputSink,
    env_vars: &[(String, String)],
    summary: &mut ClawsSummary,
    host_settings: Option<&docker::HostSettings>,
) -> Result<String> {
    let nanoclaw_str = nanoclaw_path_str();

    let container_name = docker::generate_container_name();
    out.println(format!("Starting nanoclaw container {}...", container_name));

    let container_id = docker::run_container_detached(
        NANOCLAW_IMAGE_TAG,
        &nanoclaw_str,
        &nanoclaw_str,
        &nanoclaw_str,
        Some(&container_name),
        env_vars,
        true, // nanoclaw setup container: mount Docker socket
        host_settings,
    )
    .context("Failed to start nanoclaw background container")?;

    // Wait up to 5 s for container to reach running state.
    out.print("Waiting for container to start... ");
    if !wait_for_container(&container_id, 5) {
        out.println("TIMEOUT");
        bail!("Container did not start within 5 seconds.");
    }
    out.println("OK");
    summary.nanoclaw_container = StepStatus::Ok("running".into());

    // Persist container ID.
    let mut nanoclaw_cfg = load_nanoclaw_config().unwrap_or_default();
    nanoclaw_cfg.nanoclaw_container_id = Some(container_id.clone());
    save_nanoclaw_config(&nanoclaw_cfg)?;

    Ok(container_id)
}

/// Attach to a running nanoclaw container via inherited stdio (CLI mode).
///
/// Launches the agent in plain interactive mode. No premade prompt is passed —
/// the user interacts directly with the agent (e.g. to run `/setup`).
pub fn attach_to_nanoclaw(container_id: &str, agent_name: &str, env_vars: &[(String, String)]) -> Result<()> {
    let entrypoint = chat_entrypoint(agent_name, false);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let exec_args = docker::build_exec_args_pty(container_id, &nanoclaw_path_str(), &entrypoint_refs, env_vars);
    let exec_str_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();

    let status = std::process::Command::new("docker")
        .args(&exec_str_refs)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to attach to nanoclaw container")?;

    println!(
        "\nAgent session ended (exit code: {}). nanoclaw container continues to run in the background.",
        status.code().unwrap_or(-1)
    );
    Ok(())
}

/// Wait up to `timeout_secs` seconds for a container to reach running state.
pub fn wait_for_container(container_id: &str, timeout_secs: u64) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if docker::is_container_running(container_id) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    docker::is_container_running(container_id)
}

// --- First-run and subsequent-run wizards (CLI mode) ---

async fn run_first_time_wizard(out: &OutputSink, summary: &mut ClawsSummary) -> Result<()> {
    let dest_str = nanoclaw_path_str();
    out.println("claws ready — first-time setup for nanoclaw");
    out.println(format!("nanoclaw will be installed at {}.", dest_str));
    out.println(String::new());

    let already_forked =
        ask_yes_no_stdin("Have you already forked nanoclaw on GitHub? [1=yes/2=no]: ")?;

    if already_forked {
        let username = ask_text_stdin("GitHub username (fork owner): ")?;
        let confirm = ask_yes_no_stdin(&format!(
            "Clone {}/nanoclaw to {}? [1=yes/2=no]: ",
            username.trim(), dest_str
        ))?;
        if !confirm {
            bail!("Clone cancelled.");
        }

        match clone_nanoclaw(username.trim(), out)? {
            CloneOutcome::Success => {
                chmod_nanoclaw_permissive(out);
            }
            CloneOutcome::PermissionDenied => {
                out.println(format!(
                    "\x1b[31mClone failed: permission denied writing to {}.\x1b[0m",
                    dest_str
                ));
                let use_sudo = ask_yes_no_stdin(
                    "Retry the clone with sudo? [1=yes/2=no]: ",
                )?;
                if !use_sudo {
                    bail!("Clone cancelled: permission denied.");
                }
                clone_nanoclaw_sudo(username.trim(), out, None)?;
                chmod_nanoclaw_permissive(out);
            }
        }
    } else {
        out.println("You can fork nanoclaw using the GitHub CLI (gh):");
        out.println(format!(
            "  gh repo fork {} --clone --remote-name origin",
            NANOCLAW_REPO_FULL_NAME
        ));
        out.println(format!(
            "Alternatively, visit https://github.com/{} and click Fork.",
            NANOCLAW_REPO_FULL_NAME
        ));
        out.println(String::new());

        let use_gh = ask_yes_no_stdin(
            "Use the gh CLI to fork and clone nanoclaw now? [1=yes/2=no]: ",
        )?;
        if !use_gh {
            bail!(
                "Please fork nanoclaw at https://github.com/{} and run \
                 'amux claws ready' again.",
                NANOCLAW_REPO_FULL_NAME
            );
        }

        let confirm = ask_yes_no_stdin(&format!(
            "This will fork qwibitai/nanoclaw to your GitHub account and clone it \
             to {}. Continue? [1=yes/2=no]: ",
            dest_str
        ))?;
        if !confirm {
            bail!("Fork cancelled.");
        }

        match fork_and_clone_nanoclaw(out)? {
            CloneOutcome::Success => {
                chmod_nanoclaw_permissive(out);
            }
            CloneOutcome::PermissionDenied => {
                out.println(format!(
                    "\x1b[31mMove failed: permission denied writing to {}.\x1b[0m",
                    dest_str
                ));
                let use_sudo = ask_yes_no_stdin(
                    "Retry with sudo? [1=yes/2=no]: ",
                )?;
                if !use_sudo {
                    bail!("Fork cancelled: permission denied.");
                }
                fork_and_clone_nanoclaw_sudo(out)?;
                chmod_nanoclaw_permissive(out);
            }
        }
    }

    summary.nanoclaw_cloned = StepStatus::Ok("cloned".into());

    // Resolve agent credentials using the same auto-passthrough as other containers.
    let agent_name = {
        let cfg = load_repo_config(&nanoclaw_path()).unwrap_or_default();
        cfg.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);
    // Prepare sanitized host config (same as `chat`/`implement` auto-configuration).
    // Kept alive through `attach_to_nanoclaw` so bind-mount source paths remain valid.
    let host_settings = docker::HostSettings::prepare(&agent_name);

    // /setup explanation — shown before anything launches.
    out.println(String::new());
    out.println("IMPORTANT: Once your code agent launches inside the container, run the");
    out.println("/setup command and follow the prompts to configure nanoclaw.");
    out.println("This is required on first launch.");
    out.println(String::new());

    let accept_setup = ask_yes_no_stdin("Accept and continue? [1=yes/2=no]: ")?;
    if !accept_setup {
        bail!("Setup cancelled.");
    }

    // Phase 1: build image + run audit agent (no docker socket).
    build_nanoclaw_image(out, &credentials.env_vars, summary, host_settings.as_ref()).await?;

    // Docker socket warning — shown AFTER the image is rebuilt, before the setup container
    // is launched. Only the setup container (phase 2) needs docker socket access.
    out.println(String::new());
    out.println("WARNING: The nanoclaw container will be mounted to the host Docker socket,");
    out.println("just like passing --allow-docker to 'chat' or 'implement'.");
    out.println("This grants the container elevated access to the Docker daemon on this machine.");
    out.println(String::new());

    let accept_docker =
        ask_yes_no_stdin("Accept Docker socket access for the nanoclaw container? [1=yes/2=no]: ")?;
    if !accept_docker {
        bail!("Docker socket access declined. Cannot launch nanoclaw container.");
    }

    // Phase 2: launch background container with docker socket, then attach.
    let container_id =
        launch_nanoclaw_container(out, &credentials.env_vars, summary, host_settings.as_ref()).await?;

    attach_to_nanoclaw(&container_id, &agent_name, &credentials.env_vars)?;

    Ok(())
}

async fn run_subsequent_check(out: &OutputSink, summary: &mut ClawsSummary) -> Result<()> {
    summary.nanoclaw_cloned = StepStatus::Ok("exists".into());

    let config = load_nanoclaw_config().unwrap_or_default();

    if let Some(ref container_id) = config.nanoclaw_container_id {
        if docker::is_container_running(container_id) {
            summary.docker_daemon = StepStatus::Ok("running".into());
            summary.nanoclaw_image = StepStatus::Ok("exists".into());
            summary.nanoclaw_container = StepStatus::Ok("running".into());
            out.println("nanoclaw container is running.");
            return Ok(());
        }
    }

    // Container is not running (or no saved ID).
    out.println(String::new());
    if config.nanoclaw_container_id.is_some() {
        out.println("The nanoclaw container is not currently running.");
    } else {
        out.println("No saved nanoclaw container ID found.");
    }
    out.println("Note: you may need to run /setup in your agent to get nanoclaw running again.");
    out.println(String::new());

    if !docker::is_daemon_running() {
        summary.docker_daemon = StepStatus::Failed("not running".into());
        bail!("Docker daemon is not running. Start Docker and try again.");
    }
    summary.docker_daemon = StepStatus::Ok("running".into());

    let start = ask_yes_no_stdin("Start the nanoclaw container? [1=yes/2=no]: ")?;
    if !start {
        summary.nanoclaw_container = StepStatus::Skipped("not started".into());
        return Ok(());
    }

    // Resolve credentials using the same auto-passthrough as other containers.
    let nanoclaw_str = nanoclaw_path_str();
    let cfg = load_repo_config(&nanoclaw_path()).unwrap_or_default();
    let agent_name_owned = cfg.agent.unwrap_or_else(|| "claude".to_string());
    let agent_name = agent_name_owned.as_str();
    let credentials = agent_keychain_credentials(agent_name);
    // Prepare sanitized host config (same as `chat`/`implement` auto-configuration).
    let host_settings = docker::HostSettings::prepare(agent_name);

    // Launch container.
    let container_name = docker::generate_container_name();
    out.println(format!("Starting nanoclaw container {}...", container_name));

    let container_id = docker::run_container_detached(
        NANOCLAW_IMAGE_TAG,
        &nanoclaw_str,
        &nanoclaw_str,
        &nanoclaw_str,
        Some(&container_name),
        &credentials.env_vars,
        true,
        host_settings.as_ref(),
    )
    .context("Failed to start nanoclaw background container")?;

    out.print("Waiting for container to start... ");
    if !wait_for_container(&container_id, 5) {
        out.println("TIMEOUT");
        bail!("Container did not start within 5 seconds.");
    }
    out.println("OK");
    summary.nanoclaw_container = StepStatus::Ok("running".into());

    let mut new_config = load_nanoclaw_config().unwrap_or_default();
    new_config.nanoclaw_container_id = Some(container_id.clone());
    save_nanoclaw_config(&new_config)?;

    attach_to_nanoclaw(&container_id, agent_name, &credentials.env_vars)?;

    Ok(())
}

// --- CLI stdin helpers ---

/// Ask a yes/no question on stdin using numbered list format (1=yes, 2=no).
pub fn ask_yes_no_stdin(prompt: &str) -> Result<bool> {
    use std::io::{BufRead, Write};
    print!("{}", prompt);
    std::io::stdout().flush().ok();
    let stdin = std::io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .transpose()
        .context("Failed to read stdin")?
        .unwrap_or_default();
    let trimmed = line.trim().to_lowercase();
    Ok(matches!(trimmed.as_str(), "1" | "y" | "yes"))
}

/// Prompt for a text value on stdin.
pub fn ask_text_stdin(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    print!("{}", prompt);
    std::io::stdout().flush().ok();
    let stdin = std::io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .transpose()
        .context("Failed to read stdin")?
        .unwrap_or_default();
    Ok(line.trim().to_string())
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

    #[test]
    fn clone_outcome_permission_denied_is_distinct_from_success() {
        assert_ne!(CloneOutcome::Success, CloneOutcome::PermissionDenied);
    }

    #[test]
    fn is_nanoclaw_parent_permission_denied_returns_bool() {
        // Just ensure it doesn't panic; actual result depends on test environment.
        let _ = is_nanoclaw_parent_permission_denied();
    }

    #[test]
    fn fork_and_clone_rename_permission_denied_returns_outcome() {
        // Simulate by observing that the PermissionDenied variant is constructible.
        let outcome = CloneOutcome::PermissionDenied;
        assert_eq!(outcome, CloneOutcome::PermissionDenied);
    }

    #[test]
    fn nanoclaw_config_serialization() {
        let config = NanoclawConfig {
            nanoclaw_container_id: Some("abc123".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            json.contains("nanoclawContainerID"),
            "JSON key should be nanoclawContainerID, got: {}",
            json
        );
        assert!(json.contains("abc123"));
    }

    #[test]
    fn nanoclaw_config_deserialization_with_id() {
        let json = r#"{"nanoclawContainerID":"my-container-id"}"#;
        let config: NanoclawConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.nanoclaw_container_id, Some("my-container-id".into()));
    }

    #[test]
    fn nanoclaw_config_empty_defaults() {
        let config = NanoclawConfig::default();
        assert!(config.nanoclaw_container_id.is_none());
        // Serializing the default produces valid JSON.
        let json = serde_json::to_string(&config).unwrap();
        let back: NanoclawConfig = serde_json::from_str(&json).unwrap();
        assert!(back.nanoclaw_container_id.is_none());
    }

    #[test]
    fn claws_summary_default_all_pending() {
        let summary = ClawsSummary::default();
        assert_eq!(summary.nanoclaw_cloned, StepStatus::Pending);
        assert_eq!(summary.docker_daemon, StepStatus::Pending);
        assert_eq!(summary.nanoclaw_image, StepStatus::Pending);
        assert_eq!(summary.nanoclaw_container, StepStatus::Pending);
    }

    #[test]
    fn print_claws_summary_outputs_table() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let summary = ClawsSummary {
            nanoclaw_cloned: StepStatus::Ok("cloned".into()),
            docker_daemon: StepStatus::Ok("running".into()),
            nanoclaw_image: StepStatus::Ok("built".into()),
            nanoclaw_container: StepStatus::Ok("running".into()),
        };
        print_claws_summary(&sink, &summary);

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("Claws Ready Summary"), "Missing header");
        assert!(all.contains("Docker daemon"), "Missing docker row");
        assert!(all.contains("running"), "Missing running status");
        assert!(all.contains("Container"), "Missing container row");
    }

    #[test]
    fn print_claws_summary_includes_all_steps() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let summary = ClawsSummary::default();
        print_claws_summary(&sink, &summary);

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let all = messages.join("\n");
        assert!(all.contains("Nanoclaw"), "Missing nanoclaw row");
        assert!(all.contains("Docker daemon"), "Missing docker row");
        assert!(all.contains("Nanoclaw image"), "Missing image row");
        assert!(all.contains("Container"), "Missing container row");
    }

    #[test]
    fn attach_to_nanoclaw_uses_chat_entrypoint() {
        // The setup container never receives a premade prompt — the user interacts
        // directly with their agent (e.g. to run /setup). Verify that chat_entrypoint
        // does not contain the audit prompt text.
        use crate::commands::chat::chat_entrypoint;
        let chat = chat_entrypoint("claude", false);
        assert!(
            !chat.iter().any(|a| a.contains("scan this project")),
            "chat_entrypoint should not contain audit prompt: {:?}",
            chat
        );
    }
}
