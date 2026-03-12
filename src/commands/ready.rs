use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::docker;
use anyhow::{bail, Context, Result};

/// Command-mode entry point: runs ready and prints output to stdout.
pub async fn run() -> Result<()> {
    run_with_sink(&OutputSink::Stdout).await
}

/// Core logic shared between command mode and TUI mode.
pub async fn run_with_sink(out: &OutputSink) -> Result<()> {
    // 1. Docker daemon
    out.print("Checking Docker daemon... ");
    if docker::is_daemon_running() {
        out.println("OK");
    } else {
        out.println("FAILED");
        bail!("Docker daemon is not running or not accessible. Start Docker and try again.");
    }

    // 2. Git root + Dockerfile.dev
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let dockerfile = git_root.join("Dockerfile.dev");

    out.print("Checking Dockerfile.dev... ");
    if dockerfile.exists() {
        out.println(format!("OK ({})", dockerfile.display()));
    } else {
        out.println("MISSING");
        bail!(
            "Dockerfile.dev not found at {}. Run `aspec init` first.",
            dockerfile.display()
        );
    }

    // 3. Build image — capture Docker's stdout + stderr and route through the sink
    //    so the TUI can display build progress and scroll through it.
    let image_tag = "aspec-dev:latest";
    out.println(format!("Building Docker image ({})...", image_tag));
    let build_output =
        docker::build_image(image_tag, dockerfile.to_str().unwrap(), git_root.to_str().unwrap())
            .context("Failed to build Docker image")?;
    for line in build_output.lines() {
        out.println(line);
    }

    out.println(String::new());
    out.println("aspec is ready.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn run_with_sink_fails_gracefully_without_docker() {
        if docker::is_daemon_running() {
            // Docker is up; skip this specific test path.
            return;
        }
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let result = run_with_sink(&sink).await;
        assert!(result.is_err());
        // The FAILED message should have been sent before the error was returned.
        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(messages.iter().any(|m| m.contains("FAILED") || m.contains("Checking")));
    }

    /// When Docker is available, `run_with_sink` must capture Docker build
    /// stdout+stderr and route every line through the OutputSink. Without
    /// this, TUI mode would only see aspec's own messages (~7 lines) while
    /// the Docker build progress would bypass the TUI entirely.
    #[tokio::test]
    async fn run_with_sink_includes_docker_build_output() {
        if !docker::is_daemon_running() {
            return;
        }
        let git_root = find_git_root();
        if git_root.is_none() {
            return;
        }
        let git_root = git_root.unwrap();
        if !git_root.join("Dockerfile.dev").exists() {
            return;
        }

        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let result = run_with_sink(&sink).await;
        assert!(result.is_ok(), "ready should succeed with Docker running");

        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

        // Must include Docker build progress lines (not just aspec's own output).
        let docker_lines: Vec<&String> = messages
            .iter()
            .filter(|m| {
                m.contains("#") || m.contains("CACHED") || m.contains("DONE") || m.contains("FROM")
            })
            .collect();
        assert!(
            !docker_lines.is_empty(),
            "OutputSink must receive Docker build progress lines for TUI display. \
             Got {} total lines with 0 Docker lines. All messages: {:?}",
            messages.len(),
            &messages
        );
    }
}
