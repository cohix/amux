use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// Runs a container from the given image, mounting `host_path` to `/workspace`.
///
/// stdin, stdout, and stderr are inherited so the user can interact with the
/// container directly (required by aspec/uxui/cli.md I/O guidance).
///
/// `cred_path`: when `Some`, an additional read-only bind-mount is added for agent credentials.
///
/// Security: only `host_path` (and optionally the credential dir) are mounted — never
/// any parent directory beyond what the user has confirmed (aspec/architecture/security.md).
pub fn run_container(
    image: &str,
    host_path: &str,
    entrypoint: &[&str],
    cred_path: Option<PathBuf>,
) -> Result<()> {
    let args = build_run_args(image, host_path, entrypoint, cred_path.as_deref());

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
    Ok(())
}

/// Builds the `docker run` argument list.
///
/// Uses `-it` so the container has a TTY — suitable for inheriting the host terminal.
/// For TUI/PTY mode, use `build_run_args_pty` which omits `-it`.
pub fn build_run_args<'a>(
    image: &'a str,
    host_path: &'a str,
    entrypoint: &'a [&'a str],
    cred_path: Option<&'a std::path::Path>,
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

    if let Some(cred) = cred_path {
        // Mount credentials read-only at the same path inside the container.
        args.push("-v".into());
        args.push(format!("{}:{}:ro", cred.display(), cred.display()));
    }

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
    args
}

/// Builds `docker run` args without the `-it` flag, for use inside a PTY-managed session.
///
/// When the container is launched via `portable-pty`, the slave PTY provides the TTY;
/// passing `-it` again would conflict.
pub fn build_run_args_pty<'a>(
    image: &'a str,
    host_path: &'a str,
    entrypoint: &'a [&'a str],
    cred_path: Option<&'a std::path::Path>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/workspace", host_path),
        "-w".into(),
        "/workspace".into(),
    ];

    if let Some(cred) = cred_path {
        args.push("-v".into());
        args.push(format!("{}:{}:ro", cred.display(), cred.display()));
    }

    args.push(image.into());
    args.extend(entrypoint.iter().map(|s| s.to_string()));
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn run_args_include_mount_and_workdir() {
        let args = build_run_args("aspec-dev:latest", "/repo", &["claude", "--print", "go"], None);
        assert!(args.contains(&"-v".to_string()));
        assert!(args.contains(&"/repo:/workspace".to_string()));
        assert!(args.contains(&"-w".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        assert!(args.contains(&"aspec-dev:latest".to_string()));
        assert!(args.contains(&"claude".to_string()));
    }

    #[test]
    fn run_args_use_rm_and_interactive() {
        let args = build_run_args("img", "/repo", &[], None);
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"-it".to_string()));
    }

    #[test]
    fn pty_args_omit_interactive_flag() {
        let args = build_run_args_pty("img", "/repo", &[], None);
        assert!(!args.contains(&"-it".to_string()));
        assert!(args.contains(&"--rm".to_string()));
    }

    #[test]
    fn cred_path_added_as_readonly_mount() {
        let cred = Path::new("/home/user/.claude");
        let args = build_run_args("img", "/repo", &[], Some(cred));
        let mounts: Vec<&String> = args.iter().filter(|a| a.contains(".claude")).collect();
        assert!(!mounts.is_empty());
        assert!(mounts[0].ends_with(":ro"));
    }

    /// Verifies that `build_image` captures Docker's stdout+stderr output
    /// instead of inheriting it (which would bypass the TUI's OutputSink).
    #[test]
    fn build_image_captures_output() {
        if !super::is_daemon_running() {
            return; // skip when Docker is not available
        }
        // Use the project's Dockerfile.dev which should already exist.
        let git_root = std::env::current_dir().unwrap();
        let dockerfile = git_root.join("Dockerfile.dev");
        if !dockerfile.exists() {
            return; // skip if Dockerfile.dev is missing
        }
        let output = build_image(
            "aspec-dev:latest",
            dockerfile.to_str().unwrap(),
            git_root.to_str().unwrap(),
        )
        .expect("docker build should succeed");

        // Docker build always emits progress — even cached builds emit step info.
        assert!(
            !output.is_empty(),
            "build_image must capture Docker output (stdout+stderr), not inherit it"
        );
        // Should contain typical Docker build markers.
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
