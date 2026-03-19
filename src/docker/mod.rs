use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Docker container stats returned by `docker stats`.
#[derive(Debug, Clone)]
pub struct ContainerStats {
    pub name: String,
    pub cpu_percent: String,
    pub memory: String,
}

/// Host-machine agent settings prepared for injection into a Docker container.
///
/// Stores sanitized config files in a temporary directory. The directory is
/// bind-mounted into the container as `/root/.claude.json` and `/root/.claude`.
/// The temp directory is automatically cleaned up when this struct is dropped
/// (RAII via `tempfile::TempDir`).
pub struct HostSettings {
    /// Kept alive so the temp dir survives as long as the container runs.
    _temp_dir: tempfile::TempDir,
    /// Path to the sanitized `.claude.json` inside the temp dir.
    pub config_path: PathBuf,
    /// Path to the copied `.claude/` directory inside the temp dir.
    pub claude_dir_path: PathBuf,
}

/// Top-level entries in `~/.claude/` that are large, host-specific, or
/// irrelevant inside a container. Everything else is copied.
const CLAUDE_DIR_DENYLIST: &[&str] = &[
    "projects",
    "sessions",
    "session-env",
    "debug",
    "file-history",
    "history.jsonl",
    "telemetry",
    "downloads",
    "ide",
    "shell-snapshots",
    "paste-cache",
];

impl HostSettings {
    /// Reads and sanitizes host agent settings for container injection.
    ///
    /// - Reads `~/.claude.json`, strips `oauthAccount`, adds `/workspace` project trust
    /// - Copies `~/.claude/` (filtered) into a temp directory
    ///
    /// Returns `None` if the agent is not `claude` or the host has no config.
    pub fn prepare(agent: &str) -> Option<Self> {
        if agent != "claude" {
            return None;
        }

        let home = dirs::home_dir()?;
        let host_config_file = home.join(".claude.json");
        if !host_config_file.exists() {
            return None;
        }

        let temp_dir = tempfile::TempDir::new().ok()?;

        // Sanitize .claude.json — strip oauthAccount to prevent broken
        // OAuth state (tokens are in macOS keychain, inaccessible from container).
        let raw = std::fs::read_to_string(&host_config_file).ok()?;
        let mut parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
        if let Some(obj) = parsed.as_object_mut() {
            obj.remove("oauthAccount");

            // Ensure /workspace project trust for the container environment.
            // Without this, Claude Code shows the trust dialog inside the container.
            let projects = obj
                .entry("projects")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(projects_obj) = projects.as_object_mut() {
                projects_obj.insert(
                    "/workspace".to_string(),
                    serde_json::json!({"hasTrustDialogAccepted": true}),
                );
            }
        }
        let config_json = serde_json::to_string(&parsed).ok()?;
        let config_path = temp_dir.path().join("claude.json");
        std::fs::write(&config_path, &config_json).ok()?;

        // Copy ~/.claude/ directory with denylist filter.
        let claude_dir_path = temp_dir.path().join("dot-claude");
        let host_claude_dir = home.join(".claude");
        if host_claude_dir.is_dir() {
            copy_dir_filtered(&host_claude_dir, &claude_dir_path, CLAUDE_DIR_DENYLIST).ok()?;
        } else {
            // Create an empty directory so the mount target exists.
            std::fs::create_dir_all(&claude_dir_path).ok()?;
        }

        Some(HostSettings {
            _temp_dir: temp_dir,
            config_path,
            claude_dir_path,
        })
    }
}

/// Recursively copy `src` to `dst`, skipping top-level entries in `denylist`.
fn copy_dir_filtered(src: &Path, dst: &Path, denylist: &[&str]) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if denylist.iter().any(|d| *d == name_str.as_ref()) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            // No denylist for nested directories — only filter at the top level.
            copy_dir_filtered(&src_path, &dst_path, &[])?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Returns the host Docker daemon socket path for the current platform.
///
/// - Linux/macOS: `/var/run/docker.sock`
/// - Windows: `\\.\pipe\docker_engine`
pub fn docker_socket_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"\\.\pipe\docker_engine")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/var/run/docker.sock")
    }
}

/// Checks that the host Docker daemon socket file exists and is accessible.
///
/// Returns the socket path on success, or an error if the socket is not found.
pub fn check_docker_socket() -> Result<PathBuf> {
    let path = docker_socket_path();
    if !path.exists() {
        bail!(
            "Docker socket not found at {}. Ensure the Docker daemon is running and accessible.",
            path.display()
        );
    }
    Ok(path)
}

/// Appends the Docker daemon socket mount args to the list.
///
/// On Linux/macOS: `-v /var/run/docker.sock:/var/run/docker.sock`
/// On Windows: `--mount type=npipe,source=\\.\pipe\docker_engine,target=\\.\pipe\docker_engine`
fn append_docker_socket_mount_args(args: &mut Vec<String>) {
    let path = docker_socket_path();
    let path_str = path.to_string_lossy().to_string();
    #[cfg(target_os = "windows")]
    {
        args.push("--mount".into());
        args.push(format!("type=npipe,source={},target={}", path_str, path_str));
    }
    #[cfg(not(target_os = "windows"))]
    {
        args.push("-v".into());
        args.push(format!("{}:{}", path_str, path_str));
    }
}

/// Appends `-v` bind-mount args for host settings into the container.
fn append_settings_mounts(args: &mut Vec<String>, settings: &HostSettings) {
    args.push("-v".into());
    args.push(format!(
        "{}:/root/.claude.json",
        settings.config_path.display()
    ));
    args.push("-v".into());
    args.push(format!(
        "{}:/root/.claude",
        settings.claude_dir_path.display()
    ));
}

/// Appends display-safe bind-mount args for host settings (paths shortened).
fn append_settings_mounts_display(args: &mut Vec<String>) {
    args.push("-v".into());
    args.push("<settings>:/root/.claude.json".into());
    args.push("-v".into());
    args.push("<settings>:/root/.claude".into());
}

/// Appends the container image and entrypoint to the args list.
fn append_entrypoint(args: &mut Vec<String>, image: &str, entrypoint: &[&str]) {
    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
}

/// Generate a unique container name for aspec-managed containers.
pub fn generate_container_name() -> String {
    use std::time::SystemTime;
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    format!("aspec-{}-{}", pid, ts.subsec_nanos())
}

/// Query Docker stats for a named container. Returns None if the container
/// is not running or the stats command fails.
pub fn query_container_stats(name: &str) -> Option<ContainerStats> {
    let output = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.Name}}|{{.CPUPerc}}|{{.MemUsage}}",
            name,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(ContainerStats {
        name: parts[0].to_string(),
        cpu_percent: parts[1].trim().to_string(),
        memory: parts[2]
            .split('/')
            .next()
            .unwrap_or("?")
            .trim()
            .to_string(),
    })
}

/// Parse a CPU percentage string like "5.23%" into a f64 (5.23).
pub fn parse_cpu_percent(s: &str) -> f64 {
    s.trim_end_matches('%').trim().parse::<f64>().unwrap_or(0.0)
}

/// Parse a memory string like "200MiB" or "1.5GiB" into megabytes as f64.
pub fn parse_memory_mb(s: &str) -> f64 {
    let s = s.trim();
    if let Some(val) = s.strip_suffix("GiB") {
        val.trim().parse::<f64>().unwrap_or(0.0) * 1024.0
    } else if let Some(val) = s.strip_suffix("MiB") {
        val.trim().parse::<f64>().unwrap_or(0.0)
    } else if let Some(val) = s.strip_suffix("KiB") {
        val.trim().parse::<f64>().unwrap_or(0.0) / 1024.0
    } else if let Some(val) = s.strip_suffix("B") {
        val.trim().parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

/// Appends `-e KEY=VALUE` args for each environment variable.
fn append_env_args(args: &mut Vec<String>, env_vars: &[(String, String)]) {
    for (key, value) in env_vars {
        args.push("-e".into());
        args.push(format!("{}={}", key, value));
    }
}

/// Formats `-e KEY=…` args for display, masking the values for security.
fn append_env_args_display(args: &mut Vec<String>, env_vars: &[(String, String)]) {
    for (key, _) in env_vars {
        args.push("-e".into());
        args.push(format!("{}=***", key));
    }
}


/// Formats a `docker build` invocation as a single-line CLI string for display.
pub fn format_build_cmd(tag: &str, dockerfile: &str, context: &str) -> String {
    format!("docker build -t {} -f {} {}", tag, dockerfile, context)
}

/// Formats a `docker build --no-cache` invocation as a single-line CLI string for display.
pub fn format_build_cmd_no_cache(tag: &str, dockerfile: &str, context: &str) -> String {
    format!("docker build --no-cache -t {} -f {} {}", tag, dockerfile, context)
}

/// Formats a `docker run` invocation (from pre-built args) as a CLI string for display.
///
/// **Note**: callers should use `build_run_args_display` to build the args for display,
/// which masks environment variable values.
pub fn format_run_cmd(args: &[String]) -> String {
    format!("docker {}", args.join(" "))
}

/// Returns true if the Docker daemon is running and accessible.
pub fn is_daemon_running() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Builds a Docker image from the given Dockerfile and build context directory.
///
/// When `no_cache` is true, `--no-cache` is passed to `docker build`.
///
/// Returns the combined stdout + stderr output so callers (especially the TUI)
/// can display progress. Docker emits most build progress on stderr.
#[allow(dead_code)]
pub fn build_image(tag: &str, dockerfile: &str, context: &str, no_cache: bool) -> Result<String> {
    let mut args = vec!["build"];
    if no_cache {
        args.push("--no-cache");
    }
    args.extend_from_slice(&["-t", tag, "-f", dockerfile, context]);
    let output = Command::new("docker")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to invoke `docker build`")?;

    let mut combined = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.is_empty() {
        combined.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    if !output.status.success() {
        bail!("`docker build` failed:\n{}", combined);
    }
    Ok(combined)
}

/// Builds a Docker image with streaming output, calling `on_line` for each line
/// of stdout/stderr as it is produced. This avoids the "frozen" appearance of
/// buffered builds.
///
/// Both stdout and stderr are read concurrently in background threads and
/// forwarded through a channel so that `on_line` receives lines in real time.
/// Docker emits most build progress on stderr, so streaming stderr is critical.
///
/// When `no_cache` is true, `--no-cache` is passed to `docker build`.
///
/// Returns the full combined output for callers that also need the text.
pub fn build_image_streaming<F>(
    tag: &str,
    dockerfile: &str,
    context: &str,
    no_cache: bool,
    mut on_line: F,
) -> Result<String>
where
    F: FnMut(&str),
{
    use std::io::BufRead;
    use std::sync::mpsc;

    let mut build_args = vec!["build"];
    if no_cache {
        build_args.push("--no-cache");
    }
    build_args.extend_from_slice(&["-t", tag, "-f", dockerfile, context]);
    let mut child = Command::new("docker")
        .args(&build_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `docker build`")?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut combined = String::new();

    // Use a channel so both reader threads can send lines as they arrive.
    let (tx, rx) = mpsc::channel::<String>();

    // Read stderr in a background thread (Docker emits most build output here).
    let tx_stderr = tx.clone();
    let stderr_handle = std::thread::spawn(move || {
        if let Some(stderr) = stderr {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let _ = tx_stderr.send(line);
                }
            }
        }
    });

    // Read stdout in a background thread.
    let tx_stdout = tx;
    let stdout_handle = std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let _ = tx_stdout.send(line);
                }
            }
        }
    });

    // Receive lines from both streams and forward to the callback in real time.
    // The channel closes when both sender clones are dropped (threads finish).
    for line in rx {
        on_line(&line);
        combined.push_str(&line);
        combined.push('\n');
    }

    // Wait for reader threads to finish.
    let _ = stderr_handle.join();
    let _ = stdout_handle.join();

    let status = child.wait().context("Failed to wait for `docker build`")?;
    if !status.success() {
        bail!("`docker build` failed:\n{}", combined);
    }
    Ok(combined)
}

/// Returns true if the given Docker image exists locally.
pub fn image_exists(tag: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Derives the project-specific image tag from the Git root folder name.
///
/// E.g. `/home/user/myproject` → `aspec-myproject:latest`.
pub fn project_image_tag(git_root: &Path) -> String {
    let project_name = git_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    format!("aspec-{}:latest", project_name)
}

/// Runs a container and captures stdout+stderr output.
///
/// Used for non-interactive agent runs (e.g. the Dockerfile audit step) where
/// output needs to be routed through the OutputSink for TUI display.
///
/// When `allow_docker` is true, the host Docker daemon socket is bind-mounted
/// into the container so the agent can build and run Docker containers.
///
/// Returns `(command_line, output)` — the formatted CLI string and combined output.
pub fn run_container_captured(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Result<(String, String)> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ];

    if let Some(settings) = host_settings {
        append_settings_mounts(&mut args, settings);
    }
    if allow_docker {
        append_docker_socket_mount_args(&mut args);
    }
    append_env_args(&mut args, env_vars);
    append_entrypoint(&mut args, image, entrypoint);

    let cmd_line = format_run_cmd(&build_run_args_display(
        image, host_path, entrypoint, env_vars, host_settings, allow_docker,
    ));

    let output = Command::new("docker")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to invoke `docker run`")?;

    let mut combined = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.is_empty() {
        combined.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    if !output.status.success() {
        bail!("Container exited with an error:\n{}", combined);
    }
    Ok((cmd_line, combined))
}

/// Runs a container from the given image, mounting `host_path` to `/workspace`.
///
/// stdin, stdout, and stderr are inherited so the user can interact with the
/// container directly (required by aspec/uxui/cli.md I/O guidance).
///
/// Security: only `host_path` is mounted — never any parent directory beyond
/// what the user has confirmed (aspec/architecture/security.md). Agent credentials
/// are passed as environment variables, not file mounts.
///
/// When `allow_docker` is true, the host Docker daemon socket is bind-mounted
/// into the container so the agent can build and run Docker containers.
///
/// Returns the formatted CLI command line that was executed.
pub fn run_container(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Result<String> {
    let args = build_run_args(image, host_path, entrypoint, env_vars, host_settings, allow_docker);
    let cmd_line = format_run_cmd(&build_run_args_display(
        image, host_path, entrypoint, env_vars, host_settings, allow_docker,
    ));

    let status = Command::new("docker")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to invoke `docker run`")?;

    if !status.success() {
        bail!("Container exited with status: {}", status);
    }
    Ok(cmd_line)
}

/// Builds the `docker run` argument list.
///
/// Uses `-it` so the container has a TTY — suitable for inheriting the host terminal.
/// For TUI/PTY mode, use `build_run_args_pty` which omits `-it`.
///
/// When `allow_docker` is true, the host Docker daemon socket is bind-mounted
/// into the container.
pub fn build_run_args(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-it".into(),
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ];

    if let Some(settings) = host_settings {
        append_settings_mounts(&mut args, settings);
    }
    if allow_docker {
        append_docker_socket_mount_args(&mut args);
    }
    append_env_args(&mut args, env_vars);
    append_entrypoint(&mut args, image, entrypoint);
    args
}

/// Builds a display-safe version of `docker run` args with env var values masked.
///
/// When `allow_docker` is true, the Docker socket mount is included in the display.
pub fn build_run_args_display(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-it".into(),
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ];

    if host_settings.is_some() {
        append_settings_mounts_display(&mut args);
    }
    if allow_docker {
        append_docker_socket_mount_args(&mut args);
    }
    append_env_args_display(&mut args, env_vars);
    append_entrypoint(&mut args, image, entrypoint);
    args
}

/// Builds `docker run` args for use inside a PTY-managed session.
///
/// Includes `-it` so Docker allocates a pseudo-TTY inside the container and keeps
/// stdin open. This is required for interactive tools like Claude Code — without
/// a container-side TTY, they fall back to non-interactive output mode. The `-t`
/// here creates a TTY *inside* the container, which is independent of the host-side
/// PTY that `portable-pty` provides.
///
/// When `container_name` is Some, `--name <name>` is added so the container can be
/// queried for stats while running.
///
/// When `allow_docker` is true, the host Docker daemon socket is bind-mounted
/// into the container.
pub fn build_run_args_pty(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    container_name: Option<&str>,
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-it".into(),
    ];

    if let Some(name) = container_name {
        args.push("--name".into());
        args.push(name.into());
    }

    args.extend_from_slice(&[
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ]);

    if let Some(settings) = host_settings {
        append_settings_mounts(&mut args, settings);
    }
    if allow_docker {
        append_docker_socket_mount_args(&mut args);
    }
    append_env_args(&mut args, env_vars);
    append_entrypoint(&mut args, image, entrypoint);
    args
}

/// Builds a display-safe version of PTY `docker run` args with env var values masked.
///
/// When `allow_docker` is true, the Docker socket mount is included in the display.
pub fn build_run_args_pty_display(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    container_name: Option<&str>,
    host_settings: Option<&HostSettings>,
    allow_docker: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-it".into(),
    ];

    if let Some(name) = container_name {
        args.push("--name".into());
        args.push(name.into());
    }

    args.extend_from_slice(&[
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ]);

    if host_settings.is_some() {
        append_settings_mounts_display(&mut args);
    }
    if allow_docker {
        append_docker_socket_mount_args(&mut args);
    }
    append_env_args_display(&mut args, env_vars);
    append_entrypoint(&mut args, image, entrypoint);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_image_tag_from_git_root() {
        let tag = project_image_tag(Path::new("/home/user/myproject"));
        assert_eq!(tag, "aspec-myproject:latest");
    }

    #[test]
    fn project_image_tag_handles_root_path() {
        let tag = project_image_tag(Path::new("/"));
        assert_eq!(tag, "aspec-project:latest");
    }

    #[test]
    fn image_exists_returns_false_for_nonexistent() {
        assert!(!image_exists("aspec-nonexistent-test-image-xyz:latest"));
    }

    #[test]
    fn run_args_include_mount_and_workdir() {
        let args =
            build_run_args("aspec-dev:latest", "/repo", &["claude", "--print", "go"], &[], None, false);
        assert!(args.contains(&"-v".to_string()));
        assert!(args.contains(&"/repo:/workspace".to_string()));
        assert!(args.contains(&"-w".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        assert!(args.contains(&"aspec-dev:latest".to_string()));
        assert!(args.contains(&"claude".to_string()));
    }

    #[test]
    fn run_args_use_rm_and_interactive() {
        let args = build_run_args("img", "/repo", &[], &[], None, false);
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"-it".to_string()));
    }

    #[test]
    fn pty_args_include_interactive_flag() {
        let args = build_run_args_pty("img", "/repo", &[], &[], None, None, false);
        assert!(args.contains(&"-it".to_string()));
        assert!(args.contains(&"--rm".to_string()));
    }

    #[test]
    fn pty_args_include_container_name_when_provided() {
        let args = build_run_args_pty("img", "/repo", &[], &[], Some("aspec-test-123"), None, false);
        assert!(args.contains(&"--name".to_string()));
        assert!(args.contains(&"aspec-test-123".to_string()));
    }

    #[test]
    fn pty_args_omit_name_when_none() {
        let args = build_run_args_pty("img", "/repo", &[], &[], None, None, false);
        assert!(!args.contains(&"--name".to_string()));
    }

    #[test]
    fn env_vars_passed_to_run_args() {
        let env = vec![("ANTHROPIC_API_KEY".into(), "sk-test".into())];
        let args = build_run_args("img", "/repo", &[], &env, None, false);
        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&"ANTHROPIC_API_KEY=sk-test".to_string()));
    }

    #[test]
    fn multiple_env_vars_all_passed() {
        let env = vec![
            ("ANTHROPIC_API_KEY".into(), "sk-ant".into()),
            ("OPENAI_API_KEY".into(), "sk-oai".into()),
        ];
        let args = build_run_args("img", "/repo", &[], &env, None, false);
        let env_args: Vec<&String> = args
            .iter()
            .filter(|a| a.contains("_API_KEY="))
            .collect();
        assert_eq!(env_args.len(), 2);
        assert_eq!(env_args[0], "ANTHROPIC_API_KEY=sk-ant");
        assert_eq!(env_args[1], "OPENAI_API_KEY=sk-oai");
    }

    #[test]
    fn pty_env_vars_all_passed() {
        let env = vec![
            ("ANTHROPIC_API_KEY".into(), "sk-ant".into()),
            ("OPENAI_API_KEY".into(), "sk-oai".into()),
        ];
        let args = build_run_args_pty("img", "/repo", &[], &env, None, None, false);
        let env_args: Vec<&String> = args
            .iter()
            .filter(|a| a.contains("_API_KEY="))
            .collect();
        assert_eq!(env_args.len(), 2);
    }

    #[test]
    fn display_args_mask_env_values() {
        let env = vec![("ANTHROPIC_API_KEY".into(), "sk-secret-key".into())];
        let args = build_run_args_display("img", "/repo", &[], &env, None, false);
        assert!(args.contains(&"ANTHROPIC_API_KEY=***".to_string()));
        assert!(!args.iter().any(|a| a.contains("sk-secret-key")));
    }

    #[test]
    fn pty_display_args_mask_env_values() {
        let env = vec![("OPENAI_API_KEY".into(), "sk-secret".into())];
        let args = build_run_args_pty_display("img", "/repo", &[], &env, None, None, false);
        assert!(args.contains(&"OPENAI_API_KEY=***".to_string()));
        assert!(!args.iter().any(|a| a.contains("sk-secret")));
    }

    #[test]
    fn generate_container_name_is_unique() {
        let name1 = generate_container_name();
        // Small sleep to ensure different nanos
        std::thread::sleep(std::time::Duration::from_millis(1));
        let name2 = generate_container_name();
        assert!(name1.starts_with("aspec-"));
        assert!(name2.starts_with("aspec-"));
        assert_ne!(name1, name2);
    }

    #[test]
    fn parse_cpu_percent_valid() {
        assert!((parse_cpu_percent("5.23%") - 5.23).abs() < 0.001);
        assert!((parse_cpu_percent("0.00%") - 0.0).abs() < 0.001);
        assert!((parse_cpu_percent("100%") - 100.0).abs() < 0.001);
    }

    #[test]
    fn parse_cpu_percent_invalid() {
        assert!((parse_cpu_percent("not-a-number") - 0.0).abs() < 0.001);
    }

    #[test]
    fn parse_memory_mb_various_units() {
        assert!((parse_memory_mb("200MiB") - 200.0).abs() < 0.1);
        assert!((parse_memory_mb("1.5GiB") - 1536.0).abs() < 0.1);
        assert!((parse_memory_mb("512KiB") - 0.5).abs() < 0.1);
    }

    #[test]
    fn no_settings_mounts_when_none() {
        let env = vec![("ANTHROPIC_API_KEY".into(), "sk-ant-oat01-test".into())];
        let args = build_run_args("img", "/repo", &[], &env, None, false);
        // Without host_settings or allow_docker, only the workspace mount should be present.
        let volume_mounts: Vec<&String> = args.iter()
            .zip(args.iter().skip(1))
            .filter(|(flag, _)| *flag == "-v")
            .map(|(_, val)| val)
            .collect();
        assert_eq!(volume_mounts.len(), 1, "Expected exactly one volume mount (workspace). Got: {:?}", volume_mounts);
        assert!(volume_mounts[0].contains(":/workspace"), "Expected workspace mount");
    }

    #[test]
    fn host_settings_adds_bind_mounts() {
        // Create a temporary HostSettings manually for testing.
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("claude.json");
        std::fs::write(&config_path, r#"{"hasCompletedOnboarding":true}"#).unwrap();
        let claude_dir_path = temp_dir.path().join("dot-claude");
        std::fs::create_dir_all(&claude_dir_path).unwrap();

        let hs = HostSettings {
            _temp_dir: temp_dir,
            config_path: config_path.clone(),
            claude_dir_path: claude_dir_path.clone(),
        };

        let args = build_run_args("img", "/repo", &["claude", "--print", "hi"], &[], Some(&hs), false);

        // Should have bind mounts for .claude.json and .claude/
        let volume_mounts: Vec<&String> = args.windows(2)
            .filter(|w| w[0] == "-v")
            .map(|w| &w[1])
            .collect();
        assert_eq!(volume_mounts.len(), 3, "Expected 3 mounts (workspace + config + claude dir). Got: {:?}", volume_mounts);
        assert!(volume_mounts[0].contains(":/workspace"), "First mount should be workspace");
        assert!(volume_mounts[1].contains(":/root/.claude.json"), "Second mount should be .claude.json");
        assert!(volume_mounts[2].contains(":/root/.claude"), "Third mount should be .claude/");

        // No shell wrapper — entrypoint is just image + command directly.
        let img_pos = args.iter().position(|a| a == "img").unwrap();
        assert_eq!(args[img_pos + 1], "claude");
        assert_eq!(args[img_pos + 2], "--print");
        assert_eq!(args[img_pos + 3], "hi");
        // Should NOT have "sh" "-c" wrapper
        assert!(!args[img_pos..].contains(&"sh".to_string()));
    }

    #[test]
    fn host_settings_display_shows_mount_placeholders() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("claude.json");
        std::fs::write(&config_path, r#"{"secret":"data"}"#).unwrap();
        let claude_dir_path = temp_dir.path().join("dot-claude");
        std::fs::create_dir_all(&claude_dir_path).unwrap();

        let hs = HostSettings {
            _temp_dir: temp_dir,
            config_path,
            claude_dir_path,
        };

        let args = build_run_args_display("img", "/repo", &["claude"], &[], Some(&hs), false);
        assert!(args.iter().any(|a| a == "<settings>:/root/.claude.json"));
        assert!(args.iter().any(|a| a == "<settings>:/root/.claude"));
        assert!(!args.iter().any(|a| a.contains("secret")));
        // No shell wrapper in display
        assert!(!args.iter().any(|a| a == "<write-settings-then-exec>"));
    }

    #[test]
    fn host_settings_prepare_sanitizes_oauth() {
        // This test only works on a dev machine with ~/.claude.json present.
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        if !home.join(".claude.json").exists() {
            return;
        }

        let hs = HostSettings::prepare("claude");
        let hs = match hs {
            Some(h) => h,
            None => return,
        };

        // Read the sanitized config from the temp file.
        let config_json = std::fs::read_to_string(&hs.config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&config_json).unwrap();
        assert!(parsed.get("oauthAccount").is_none(), "oauthAccount should be stripped from sanitized config");
        assert!(parsed.as_object().unwrap().len() > 0, "Sanitized config should not be empty");
        // Verify /workspace trust is added.
        assert!(parsed["projects"]["/workspace"]["hasTrustDialogAccepted"].as_bool() == Some(true));

        // Verify the .claude/ directory was copied.
        assert!(hs.claude_dir_path.is_dir(), "claude dir should exist in temp");

        // Verify denylist entries are not copied.
        assert!(!hs.claude_dir_path.join("projects").exists(), "projects/ should be denied");
        assert!(!hs.claude_dir_path.join("sessions").exists(), "sessions/ should be denied");
    }

    #[test]
    fn copy_dir_filtered_respects_denylist() {
        let src = tempfile::TempDir::new().unwrap();
        let dst = tempfile::TempDir::new().unwrap();

        // Create source structure.
        std::fs::write(src.path().join("settings.json"), "{}").unwrap();
        std::fs::create_dir(src.path().join("plugins")).unwrap();
        std::fs::write(src.path().join("plugins/test.json"), "{}").unwrap();
        std::fs::create_dir(src.path().join("sessions")).unwrap();
        std::fs::write(src.path().join("sessions/data.json"), "{}").unwrap();
        std::fs::write(src.path().join("history.jsonl"), "line1").unwrap();

        let dst_path = dst.path().join("output");
        copy_dir_filtered(src.path(), &dst_path, &["sessions", "history.jsonl"]).unwrap();

        assert!(dst_path.join("settings.json").exists());
        assert!(dst_path.join("plugins/test.json").exists());
        assert!(!dst_path.join("sessions").exists(), "sessions should be filtered");
        assert!(!dst_path.join("history.jsonl").exists(), "history.jsonl should be filtered");
    }

    #[test]
    fn format_build_cmd_produces_valid_string() {
        let cmd = format_build_cmd("aspec-test:latest", "Dockerfile.dev", "/repo");
        assert_eq!(
            cmd,
            "docker build -t aspec-test:latest -f Dockerfile.dev /repo"
        );
    }

    #[test]
    fn format_build_cmd_no_cache_produces_valid_string() {
        let cmd = format_build_cmd_no_cache("aspec-test:latest", "Dockerfile.dev", "/repo");
        assert_eq!(
            cmd,
            "docker build --no-cache -t aspec-test:latest -f Dockerfile.dev /repo"
        );
    }

    #[test]
    fn format_run_cmd_produces_valid_string() {
        let args = build_run_args("img", "/repo", &["echo", "hello"], &[], None, false);
        let cmd = format_run_cmd(&args);
        assert!(cmd.starts_with("docker run"));
        assert!(cmd.contains("/repo:/workspace"));
        assert!(cmd.contains("echo"));
    }

    #[test]
    fn build_image_streaming_fails_on_bad_dockerfile() {
        if !super::is_daemon_running() {
            return;
        }

        // Use a non-existent Dockerfile to trigger a failure.
        let result = build_image_streaming(
            "aspec-test-fail:latest",
            "/nonexistent/Dockerfile",
            "/tmp",
            false,
            |_line| {},
        );
        assert!(result.is_err(), "Should fail with a bad Dockerfile path");
    }

    // --- docker_socket_path and check_docker_socket tests ---

    #[test]
    fn docker_socket_path_is_nonempty() {
        let path = docker_socket_path();
        assert!(!path.as_os_str().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn docker_socket_path_linux() {
        let path = docker_socket_path();
        assert_eq!(path.to_str().unwrap(), "/var/run/docker.sock");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn docker_socket_path_macos() {
        let path = docker_socket_path();
        assert_eq!(path.to_str().unwrap(), "/var/run/docker.sock");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn docker_socket_path_windows() {
        let path = docker_socket_path();
        let s = path.to_string_lossy();
        assert!(s.contains("docker_engine"), "Windows path should reference docker_engine pipe");
    }

    #[test]
    fn check_docker_socket_fails_on_missing_path() {
        // On a system where Docker is not installed at the default path, this
        // test verifies the error message. If the socket exists (Docker is running),
        // we skip the negative assertion.
        let path = docker_socket_path();
        if path.exists() {
            // Socket exists — check_docker_socket should succeed.
            let result = check_docker_socket();
            assert!(result.is_ok(), "Socket exists but check_docker_socket failed");
        } else {
            // Socket missing — check_docker_socket should return an error.
            let result = check_docker_socket();
            assert!(result.is_err(), "Expected error when socket is missing");
            let msg = format!("{}", result.unwrap_err());
            assert!(msg.contains("Docker socket not found"), "Error message should mention socket: {}", msg);
        }
    }

    #[test]
    fn allow_docker_adds_socket_mount_to_run_args() {
        let args = build_run_args("img", "/repo", &[], &[], None, true);
        // The docker socket should appear as a volume mount.
        let socket_path = docker_socket_path().to_string_lossy().to_string();
        let has_socket_mount = args.windows(2)
            .any(|w| w[0] == "-v" && w[1].contains(&socket_path));
        #[cfg(not(target_os = "windows"))]
        assert!(has_socket_mount, "allow_docker should add docker socket volume mount: {:?}", args);
    }

    #[test]
    fn no_allow_docker_does_not_add_socket_mount() {
        let args = build_run_args("img", "/repo", &[], &[], None, false);
        let socket_path = docker_socket_path().to_string_lossy().to_string();
        let has_socket_mount = args.iter().any(|a| a.contains(&socket_path));
        assert!(!has_socket_mount, "Without allow_docker, socket should not be mounted: {:?}", args);
    }

    #[test]
    fn allow_docker_adds_socket_mount_to_pty_args() {
        let args = build_run_args_pty("img", "/repo", &[], &[], None, None, true);
        let socket_path = docker_socket_path().to_string_lossy().to_string();
        let has_socket = args.iter().any(|a| a.contains(&socket_path));
        #[cfg(not(target_os = "windows"))]
        assert!(has_socket, "allow_docker should add socket to pty args: {:?}", args);
    }

    #[test]
    fn allow_docker_adds_socket_mount_to_display_args() {
        let args = build_run_args_display("img", "/repo", &[], &[], None, true);
        let socket_path = docker_socket_path().to_string_lossy().to_string();
        let has_socket = args.iter().any(|a| a.contains(&socket_path));
        #[cfg(not(target_os = "windows"))]
        assert!(has_socket, "allow_docker should add socket to display args: {:?}", args);
    }

    #[test]
    fn allow_docker_adds_socket_mount_to_pty_display_args() {
        let args = build_run_args_pty_display("img", "/repo", &[], &[], None, None, true);
        let socket_path = docker_socket_path().to_string_lossy().to_string();
        let has_socket = args.iter().any(|a| a.contains(&socket_path));
        #[cfg(not(target_os = "windows"))]
        assert!(has_socket, "allow_docker should add socket to pty display args: {:?}", args);
    }

    #[test]
    fn allow_docker_socket_mount_appears_after_workspace_mount() {
        let args = build_run_args("img", "/repo", &[], &[], None, true);
        let socket_path = docker_socket_path().to_string_lossy().to_string();

        #[cfg(not(target_os = "windows"))]
        {
            let workspace_pos = args.windows(2)
                .position(|w| w[0] == "-v" && w[1].contains(":/workspace"));
            let socket_pos = args.windows(2)
                .position(|w| w[0] == "-v" && w[1].contains(&socket_path));
            assert!(workspace_pos.is_some(), "Workspace mount should be present");
            assert!(socket_pos.is_some(), "Socket mount should be present");
            assert!(socket_pos.unwrap() > workspace_pos.unwrap(),
                "Socket mount should appear after workspace mount");
        }
    }
}
