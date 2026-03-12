pub mod input;
mod pty;
mod render;
pub mod state;

use crate::cli::Agent;
use crate::commands::auth::apply_auth_decision;
use crate::commands::implement::{agent_entrypoint, find_work_item};
use crate::commands::init::find_git_root;
use crate::commands::{init, ready};
use crate::config::load_repo_config;
use crate::docker;
use crate::tui::input::Action;
use crate::tui::pty::{spawn_text_command, PtySession};
use crate::tui::state::{App, Dialog};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::PtySize;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

/// Launches the interactive TUI. Blocks until the user quits.
pub async fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal).await;

    // Always restore the terminal, even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_app<B>(terminal: &mut Terminal<B>) -> Result<()>
where
    B: ratatui::backend::Backend + io::Write,
    <B as ratatui::backend::Backend>::Error: Send + Sync + 'static,
{
    let mut app = App::new();

    // Auto-run `ready` at startup (edge case from work item spec).
    execute_command(&mut app, "ready").await;

    loop {
        terminal.draw(|f| render::draw(f, &app))?;

        // Poll for crossterm events with a short timeout to keep the UI responsive.
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let action = input::handle_key(&mut app, key);
                    handle_action(&mut app, action).await;
                }
                Event::Resize(cols, rows) => {
                    if let Some(ref pty) = app.pty {
                        pty.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }
                _ => {}
            }
        }

        // Drain all pending channel messages (PTY output, command output, exit codes).
        app.tick();

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Dispatch an `Action` returned by the key handler to the appropriate async logic.
async fn handle_action(app: &mut App, action: Action) {
    match action {
        Action::None => {}

        Action::QuitConfirmed => {
            app.should_quit = true;
        }

        Action::Submit(cmd) => {
            if cmd.is_empty() {
                return;
            }
            execute_command(app, &cmd).await;
        }

        Action::MountScopeChosen(path) => {
            app.pending_mount_path = Some(path);
            // Resume the pending implement command with the resolved mount.
            if let Some(work_item) = app.pending_work_item.take() {
                launch_implement(app, work_item).await;
            }
        }

        Action::AuthAccepted => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, true);
            }
            if let Some(work_item) = app.pending_work_item.take() {
                launch_implement(app, work_item).await;
            }
        }

        Action::AuthDeclined => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, false);
            }
            if let Some(work_item) = app.pending_work_item.take() {
                launch_implement(app, work_item).await;
            }
        }

        Action::ForwardToPty(bytes) => {
            if let Some(ref pty) = app.pty {
                pty.write_bytes(&bytes);
            }
        }
    }
}

/// Parse and dispatch a command string entered by the user.
async fn execute_command(app: &mut App, cmd: &str) {
    let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
    if parts.is_empty() {
        return;
    }

    match parts[0] {
        "init" => {
            let agent = parse_agent_flag(&parts).unwrap_or(Agent::Claude);
            app.start_command(cmd.to_string());
            let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
            app.exit_rx = Some(exit_rx);
            let tx = app.output_tx.clone();
            spawn_text_command(tx, exit_tx, move |sink| async move {
                init::run_with_sink(agent, &sink).await
            });
        }

        "ready" => {
            app.start_command(cmd.to_string());
            let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
            app.exit_rx = Some(exit_rx);
            let tx = app.output_tx.clone();
            spawn_text_command(tx, exit_tx, move |sink| async move {
                ready::run_with_sink(&sink).await
            });
        }

        "implement" => {
            let work_item: u32 = match parts.get(1).and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => {
                    app.input_error =
                        Some("Usage: implement <work-item-number>  e.g. implement 1".into());
                    return;
                }
            };
            app.pending_work_item = Some(work_item);
            show_pre_implement_dialogs(app, work_item).await;
        }

        unknown => {
            let suggestion = input::closest_subcommand(unknown)
                .map(|s| format!("  Did you mean: {}", s))
                .unwrap_or_default();
            app.input_error = Some(format!(
                "'{}' is not an aspec command.{}",
                unknown, suggestion
            ));
        }
    }
}

/// Show any needed dialogs (mount scope, agent auth) before launching `implement`.
/// If no dialogs are required, launches immediately.
async fn show_pre_implement_dialogs(app: &mut App, work_item: u32) {
    let git_root = match find_git_root() {
        Some(r) => r,
        None => {
            app.input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    // Check mount scope.
    let cwd = std::env::current_dir().unwrap_or_else(|_| git_root.clone());
    if cwd != git_root {
        app.dialog = Dialog::MountScope {
            git_root: git_root.clone(),
            cwd,
        };
        return; // Wait for user choice; handle_action resumes after dialog.
    }
    app.pending_mount_path = Some(git_root.clone());

    // Check agent auth.
    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent = config.agent.unwrap_or_else(|| "claude".into());
    if config.auto_agent_auth_accepted.is_none() {
        app.dialog = Dialog::AgentAuth {
            agent,
            git_root,
        };
        return; // Wait for user choice.
    }

    // No dialogs needed; launch directly.
    launch_implement(app, work_item).await;
}

/// Actually spawn the docker container for `implement` via PTY.
async fn launch_implement(app: &mut App, work_item: u32) {
    let git_root = match find_git_root() {
        Some(r) => r,
        None => {
            app.input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    let work_item_path = match find_work_item(&git_root, work_item) {
        Ok(p) => p,
        Err(e) => {
            app.input_error = Some(format!("{}", e));
            return;
        }
    };

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    // Resolve credential path based on saved preference.
    let cred_path = if config.auto_agent_auth_accepted == Some(true) {
        crate::commands::auth::credential_path_for_agent(&agent_name)
    } else {
        None
    };

    let entrypoint = agent_entrypoint(&agent_name, &work_item_path, &git_root);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = "aspec-dev:latest";
    let docker_args =
        docker::build_run_args_pty(image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, cred_path.as_deref());
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    let terminal_area = (80u16, 40u16); // fallback; real size set on first resize event
    let size = PtySize {
        rows: terminal_area.1,
        cols: terminal_area.0,
        pixel_width: 0,
        pixel_height: 0,
    };

    let command_display = format!("implement {}", work_item);
    app.start_command(command_display);

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.pty = Some(session);
            app.pty_rx = Some(pty_rx);
        }
        Err(e) => {
            app.push_output(format!("Failed to launch container: {}", e));
            app.finish_command(1);
        }
    }
}

fn parse_agent_flag(parts: &[&str]) -> Option<Agent> {
    parts.iter().find_map(|part| {
        let value = if let Some(v) = part.strip_prefix("--agent=") {
            v
        } else {
            return None;
        };
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "opencode" => Some(Agent::Opencode),
            _ => None,
        }
    })
}
