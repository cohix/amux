use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Appends `-v host:container` volume mounts for the agent's config directory
/// and config file.
///
/// Claude Code uses two locations:
/// - `~/.claude/`     — settings directory (preferences, plugins, session state)
/// - `~/.claude.json` — primary config file (onboarding state, model prefs, etc.)
///
/// Both are mounted so the containerized agent inherits the host's settings
/// and skips first-run setup.
fn append_config_mount(args: &mut Vec<String>, agent_config_dir: Option<&Path>) {
    if let Some(dir) = agent_config_dir {
        args.push("-v".into());
        args.push(format!("{}:/root/.claude", dir.display()));

        // Also mount ~/.claude.json if it exists alongside the config directory.
        if let Some(parent) = dir.parent() {
            let config_file = parent.join(".claude.json");
            if config_file.exists() {
                args.push("-v".into());
                args.push(format!("{}:/root/.claude.json", config_file.display()));
            }
        }
    }
}

/// Returns the host path to the Claude Code config directory (`~/.claude`),
/// if it exists. Used to mount settings into the container.
pub fn claude_config_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".claude");
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// Formats a `docker build` invocation as a single-line CLI string for display.
pub fn format_build_cmd(tag: &str, dockerfile: &str, context: &str) -> String {
    format!("docker build -t {} -f {} {}", tag, dockerfile, context)
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
/// Returns the combined stdout + stderr output so callers (especially the TUI)
/// can display progress. Docker emits most build progress on stderr.
pub fn build_image(tag: &str, dockerfile: &str, context: &str) -> Result<String> {
    let output = Command::new("docker")
        .args(["build", "-t", tag, "-f", dockerfile, context])
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
/// Returns the full combined output for callers that also need the text.
pub fn build_image_streaming<F>(
    tag: &str,
    dockerfile: &str,
    context: &str,
    mut on_line: F,
) -> Result<String>
where
    F: FnMut(&str),
{
    use std::io::BufRead;

    let mut child = Command::new("docker")
        .args(["build", "-t", tag, "-f", dockerfile, context])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to invoke `docker build`")?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut combined = String::new();

    // Read stderr in a background thread (Docker emits most build output there).
    let stderr_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(stderr) = stderr {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    lines.push(line);
                }
            }
        }
        lines
    });

    // Read stdout on the current thread.
    if let Some(stdout) = stdout {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(line) = line {
                on_line(&line);
                combined.push_str(&line);
                combined.push('\n');
            }
        }
    }

    let stderr_lines = stderr_handle.join().unwrap_or_default();
    for line in &stderr_lines {
        on_line(line);
        combined.push_str(line);
        combined.push('\n');
    }

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
/// Returns `(command_line, output)` — the formatted CLI string and combined output.
pub fn run_container_captured(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
) -> Result<(String, String)> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ];

    append_config_mount(&mut args, agent_config_dir);
    append_env_args(&mut args, env_vars);

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));

    let cmd_line = format_run_cmd(&build_run_args_display(
        image, host_path, entrypoint, env_vars, agent_config_dir,
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
/// Returns the formatted CLI command line that was executed.
pub fn run_container(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
) -> Result<String> {
    let args = build_run_args(image, host_path, entrypoint, env_vars, agent_config_dir);
    let cmd_line = format_run_cmd(&build_run_args_display(
        image, host_path, entrypoint, env_vars, agent_config_dir,
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
pub fn build_run_args(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
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

    append_config_mount(&mut args, agent_config_dir);
    append_env_args(&mut args, env_vars);

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
    args
}

/// Builds a display-safe version of `docker run` args with env var values masked.
pub fn build_run_args_display(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
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

    append_config_mount(&mut args, agent_config_dir);
    append_env_args_display(&mut args, env_vars);

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
    args
}

/// Builds `docker run` args for use inside a PTY-managed session.
///
/// Includes `-it` so Docker allocates a pseudo-TTY inside the container and keeps
/// stdin open. This is required for interactive tools like Claude Code — without
/// a container-side TTY, they fall back to non-interactive output mode. The `-t`
/// here creates a TTY *inside* the container, which is independent of the host-side
/// PTY that `portable-pty` provides.
pub fn build_run_args_pty(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
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

    append_config_mount(&mut args, agent_config_dir);
    append_env_args(&mut args, env_vars);

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
    args
}

/// Builds a display-safe version of PTY `docker run` args with env var values masked.
pub fn build_run_args_pty_display(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    env_vars: &[(String, String)],
    agent_config_dir: Option<&Path>,
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

    append_config_mount(&mut args, agent_config_dir);
    append_env_args_display(&mut args, env_vars);

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
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
            build_run_args("aspec-dev:latest", "/repo", &["claude", "--print", "go"], &[], None);
        assert!(args.contains(&"-v".to_string()));
        assert!(args.contains(&"/repo:/workspace".to_string()));
        assert!(args.contains(&"-w".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        assert!(args.contains(&"aspec-dev:latest".to_string()));
        assert!(args.contains(&"claude".to_string()));
    }

    #[test]
    fn run_args_use_rm_and_interactive() {
        let args = build_run_args("img", "/repo", &[], &[], None);
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"-it".to_string()));
    }

    #[test]
    fn pty_args_include_interactive_flag() {
        let args = build_run_args_pty("img", "/repo", &[], &[], None);
        assert!(args.contains(&"-it".to_string()));
        assert!(args.contains(&"--rm".to_string()));
    }

    #[test]
    fn env_vars_passed_to_run_args() {
        let env = vec![("ANTHROPIC_API_KEY".into(), "sk-test".into())];
        let args = build_run_args("img", "/repo", &[], &env, None);
        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&"ANTHROPIC_API_KEY=sk-test".to_string()));
    }

    #[test]
    fn multiple_env_vars_all_passed() {
        let env = vec![
            ("ANTHROPIC_API_KEY".into(), "sk-ant".into()),
            ("OPENAI_API_KEY".into(), "sk-oai".into()),
        ];
        let args = build_run_args("img", "/repo", &[], &env, None);
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
        let args = build_run_args_pty("img", "/repo", &[], &env, None);
        let env_args: Vec<&String> = args
            .iter()
            .filter(|a| a.contains("_API_KEY="))
            .collect();
        assert_eq!(env_args.len(), 2);
    }

    #[test]
    fn display_args_mask_env_values() {
        let env = vec![("ANTHROPIC_API_KEY".into(), "sk-secret-key".into())];
        let args = build_run_args_display("img", "/repo", &[], &env, None);
        assert!(args.contains(&"ANTHROPIC_API_KEY=***".to_string()));
        assert!(!args.iter().any(|a| a.contains("sk-secret-key")));
    }

    #[test]
    fn pty_display_args_mask_env_values() {
        let env = vec![("OPENAI_API_KEY".into(), "sk-secret".into())];
        let args = build_run_args_pty_display("img", "/repo", &[], &env, None);
        assert!(args.contains(&"OPENAI_API_KEY=***".to_string()));
        assert!(!args.iter().any(|a| a.contains("sk-secret")));
    }

    #[test]
    fn config_dir_mounted_when_provided() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Create a .claude.json sibling file to test both mounts.
        let parent = tmp.path().parent().unwrap();
        let config_file = parent.join(".claude.json");
        let created_file = !config_file.exists();
        if created_file {
            std::fs::write(&config_file, "{}").unwrap();
        }

        let config_path = tmp.path();
        let args = build_run_args("img", "/repo", &[], &[], Some(config_path));

        // Directory mount
        let dir_mount = args.iter().find(|a| a.ends_with(":/root/.claude"));
        assert!(dir_mount.is_some(), "Expected ~/.claude dir mount in args: {:?}", args);

        // File mount (only if .claude.json existed or we created it)
        let file_mount = args.iter().find(|a| a.ends_with(":/root/.claude.json"));
        assert!(file_mount.is_some(), "Expected ~/.claude.json file mount in args: {:?}", args);

        if created_file {
            let _ = std::fs::remove_file(&config_file);
        }
    }

    #[test]
    fn config_dir_omitted_when_none() {
        let args = build_run_args("img", "/repo", &[], &[], None);
        assert!(!args.iter().any(|a| a.contains("/root/.claude")));
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
    fn format_run_cmd_produces_valid_string() {
        let args = build_run_args("img", "/repo", &["echo", "hello"], &[], None);
        let cmd = format_run_cmd(&args);
        assert!(cmd.starts_with("docker run"));
        assert!(cmd.contains("/repo:/workspace"));
        assert!(cmd.contains("echo"));
    }

    #[test]
    fn build_image_captures_output() {
        if !super::is_daemon_running() {
            return;
        }
        let git_root = std::env::current_dir().unwrap();
        let dockerfile = git_root.join("Dockerfile.dev");
        if !dockerfile.exists() {
            return;
        }
        let output = build_image(
            "aspec-dev:latest",
            dockerfile.to_str().unwrap(),
            git_root.to_str().unwrap(),
        )
        .expect("docker build should succeed");

        assert!(
            !output.is_empty(),
            "build_image must capture Docker output (stdout+stderr), not inherit it"
        );
        let has_build_markers = output.contains("DONE")
            || output.contains("CACHED")
            || output.contains("FROM")
            || output.contains("building")
            || output.contains("#");
        assert!(
            has_build_markers,
            "Captured output should contain Docker build progress. Got:\n{}",
            &output[..output.len().min(500)]
        );
    }
}
