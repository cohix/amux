use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use crate::config::load_repo_config;
use crate::docker;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Shared logic for launching a containerized agent session.
///
/// Used by both `implement` (with a pre-configured prompt) and `chat` (no prompt).
///
/// `entrypoint`: the Docker entrypoint command (agent + optional prompt).
/// `status_message`: displayed to the user before launching.
/// `mount_override`: when `Some`, skip the interactive stdin prompt and use this path.
/// `env_vars`: agent credential env vars to pass into the container.
/// `non_interactive`: when true, launch agent in print/non-interactive mode.
/// `allow_docker`: when true, mount the host Docker daemon socket into the container.
pub async fn run_agent_with_sink(
    entrypoint: Vec<String>,
    status_message: &str,
    out: &OutputSink,
    mount_override: Option<PathBuf>,
    env_vars: Vec<(String, String)>,
    non_interactive: bool,
    host_settings: Option<&docker::HostSettings>,
    allow_docker: bool,
    container_name_override: Option<String>,
) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;
    let config = load_repo_config(&git_root)?;
    let agent = config.agent.as_deref().unwrap_or("claude").to_string();

    out.println(status_message);

    let mount_path = match mount_override {
        Some(p) => p,
        None => crate::commands::implement::confirm_mount_scope_stdin(&git_root)?,
    };

    // If --allow-docker, check the socket and print a warning before launching.
    if allow_docker {
        let socket_path = docker::check_docker_socket()
            .context("Cannot mount Docker socket")?;
        out.println(format!("Docker socket: {} (found)", socket_path.display()));
        out.println(format!(
            "WARNING: --allow-docker: mounting host Docker socket into container ({}:{}). \
             This grants the agent elevated host access.",
            socket_path.display(),
            socket_path.display()
        ));
    }

    let image_tag = docker::project_image_tag(&git_root);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    // Show the full Docker CLI command being run (with masked env values).
    let display_args = docker::build_run_args_display(
        &image_tag,
        mount_path.to_str().unwrap(),
        &entrypoint_refs,
        &env_vars,
        host_settings,
        allow_docker,
        container_name_override.as_deref(),
    );
    out.println(format!("$ {}", docker::format_run_cmd(&display_args)));

    if !non_interactive {
        crate::commands::ready::print_interactive_notice(out, &agent);
    } else {
        out.println("Tip: remove --non-interactive to interact with the agent directly.");
    }

    if non_interactive {
        let (_cmd, output) = docker::run_container_captured(
            &image_tag,
            mount_path.to_str().unwrap(),
            &entrypoint_refs,
            &env_vars,
            host_settings,
            allow_docker,
            container_name_override.as_deref(),
        )
        .context("Container exited with an error")?;
        for line in output.lines() {
            out.println(line);
        }
    } else {
        docker::run_container(
            &image_tag,
            mount_path.to_str().unwrap(),
            &entrypoint_refs,
            &env_vars,
            host_settings,
            allow_docker,
            container_name_override.as_deref(),
        )
        .context("Container exited with an error")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn run_agent_with_sink_fails_without_git_root() {
        let (tx, _rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        let entrypoint = vec!["claude".to_string()];
        // Run from a temp dir with no git repo.
        let tmp = tempfile::TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = run_agent_with_sink(
            entrypoint,
            "test",
            &sink,
            Some(tmp.path().to_path_buf()),
            vec![],
            false,
            None,
            false,
            None,
        )
        .await;

        std::env::set_current_dir(original_dir).unwrap();
        assert!(result.is_err());
    }
}
