pub mod input;
mod pty;
mod render;
pub mod state;

use crate::cli::Agent;
use crate::commands::auth::{agent_keychain_vars, apply_auth_decision};
use crate::commands::implement::{
    agent_entrypoint, agent_entrypoint_non_interactive, find_work_item, parse_work_item,
};
use crate::commands::init::find_git_root;
use crate::commands::new::WorkItemKind;
use crate::commands::{init, new, ready};
use crate::commands::ready::{ReadyOptions, print_interactive_notice, print_summary};
use crate::config::load_repo_config;
use crate::docker;
use crate::tui::input::Action;
use crate::tui::pty::{spawn_text_command, PtySession};
use crate::tui::state::{App, Dialog, PendingCommand, ReadyPhase};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind},
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal).await;

    // Always restore the terminal, even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
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
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            let max = app.output_lines.len();
                            if app.scroll_offset < max {
                                app.scroll_offset = app.scroll_offset.saturating_add(3);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll_offset = app.scroll_offset.saturating_sub(3);
                        }
                        _ => {}
                    }
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
        let was_running = matches!(app.phase, state::ExecutionPhase::Running { .. });
        app.tick();
        let now_done = !matches!(app.phase, state::ExecutionPhase::Running { .. });

        // Check if a ready workflow phase just completed and continue to the next phase.
        if was_running && now_done {
            check_ready_continuation(&mut app).await;
        }

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
            launch_pending_command(app).await;
        }

        Action::AuthAccepted => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, true);
            }
            launch_pending_command(app).await;
        }

        Action::AuthDeclined => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, false);
            }
            launch_pending_command(app).await;
        }

        Action::ForwardToPty(bytes) => {
            if let Some(ref pty) = app.pty {
                pty.write_bytes(&bytes);
            }
        }

        Action::NewWorkItem { kind, title } => {
            launch_new(app, kind, title).await;
        }
    }
}

/// Parse flags from the command parts, returning (refresh, non_interactive).
fn parse_ready_flags(parts: &[&str]) -> (bool, bool) {
    let refresh = parts.iter().any(|p| *p == "--refresh");
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    (refresh, non_interactive)
}

/// Parse flags from implement command parts, returning non_interactive.
fn parse_implement_flags(parts: &[&str]) -> bool {
    parts.iter().any(|p| *p == "--non-interactive")
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
            let (refresh, non_interactive) = parse_ready_flags(&parts);
            app.pending_command = PendingCommand::Ready { refresh, non_interactive };
            app.ready_opts = ReadyOptions { refresh, non_interactive };
            show_pre_command_dialogs(app).await;
        }

        "implement" => {
            let non_interactive = parse_implement_flags(&parts);
            // Filter out flags to find the work item number.
            let work_item: u32 = match parts.iter()
                .skip(1)
                .find(|s| !s.starts_with("--"))
                .and_then(|s| parse_work_item(s).ok())
            {
                Some(n) => n,
                None => {
                    app.input_error =
                        Some("Usage: implement <work-item-number> [--non-interactive]".into());
                    return;
                }
            };
            app.pending_command = PendingCommand::Implement { work_item, non_interactive };
            show_pre_command_dialogs(app).await;
        }

        "new" => {
            app.dialog = state::Dialog::NewKindSelect;
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

/// Show any needed dialogs (mount scope, agent auth) before launching a command.
/// Used by both `ready` and `implement` in TUI mode.
async fn show_pre_command_dialogs(app: &mut App) {
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
    launch_pending_command(app).await;
}

/// Resume the pending command after all dialogs have been answered.
async fn launch_pending_command(app: &mut App) {
    match app.pending_command.clone() {
        PendingCommand::Ready { refresh, non_interactive } => {
            app.ready_opts = ReadyOptions { refresh, non_interactive };
            launch_ready(app).await;
        }
        PendingCommand::Implement { work_item, non_interactive } => {
            launch_implement(app, work_item, non_interactive).await;
        }
        PendingCommand::None => {}
    }
}

/// Launch the ready command — phase 1 (pre-audit) as a text command.
/// The audit and post-audit phases are triggered automatically via `check_ready_continuation`.
async fn launch_ready(app: &mut App) {
    let git_root = match find_git_root() {
        Some(r) => r,
        None => {
            app.input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    let env_vars = if config.auto_agent_auth_accepted == Some(true) {
        agent_keychain_vars(&agent_name)
    } else {
        vec![]
    };

    let opts = app.ready_opts.clone();

    app.ready_phase = ReadyPhase::PreAudit;
    app.start_command("ready".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.exit_rx = Some(exit_rx);
    let (ctx_tx, ctx_rx) = tokio::sync::oneshot::channel();
    app.ready_ctx_rx = Some(ctx_rx);
    let tx = app.output_tx.clone();

    // If not refreshing, run the full sink-based workflow (no audit/post-audit).
    if !opts.refresh {
        app.ready_phase = ReadyPhase::Inactive; // No multi-phase needed.
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let _ = ready::run_with_sink(&sink, mount_path, env_vars, &opts).await?;
            Ok(())
        });
    } else {
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let mut summary = ready::ReadySummary::default();
            let ctx = ready::run_pre_audit(&sink, mount_path, env_vars, &mut summary).await?;
            let _ = ctx_tx.send((ctx, summary));
            Ok(())
        });
    }
}

/// Check if a ready workflow phase just completed and automatically launch the next phase.
async fn check_ready_continuation(app: &mut App) {
    match app.ready_phase {
        ReadyPhase::PreAudit => {
            // Pre-audit just finished. If it failed, abort the workflow.
            if matches!(app.phase, state::ExecutionPhase::Error { .. }) {
                app.ready_phase = ReadyPhase::Inactive;
                app.ready_ctx = None;
                app.ready_ctx_rx = None;
                return;
            }
            // The context should have arrived via the channel by now.
            if app.ready_ctx.is_none() {
                app.push_output("Internal error: pre-audit completed but no context received.");
                app.ready_phase = ReadyPhase::Inactive;
                return;
            }

            let opts = app.ready_opts.clone();
            if opts.refresh {
                if !opts.non_interactive {
                    // Print the interactive notice via output.
                    let agent_name = app.ready_ctx.as_ref()
                        .map(|c| c.agent_name.clone())
                        .unwrap_or_else(|| "agent".into());
                    let sink = crate::commands::output::OutputSink::Channel(app.output_tx.clone());
                    print_interactive_notice(&sink, &agent_name);
                }
                // Launch the audit via PTY (or captured if non-interactive).
                if opts.non_interactive {
                    launch_ready_audit_captured(app);
                } else {
                    launch_ready_audit(app);
                }
            } else {
                // No refresh — skip audit & post-audit, print summary.
                app.ready_phase = ReadyPhase::Inactive;
                app.ready_ctx = None;
            }
        }
        ReadyPhase::Audit => {
            // Audit PTY just finished. If it failed, abort.
            if matches!(app.phase, state::ExecutionPhase::Error { .. }) {
                app.ready_phase = ReadyPhase::Inactive;
                app.ready_ctx = None;
                return;
            }
            // Launch post-audit.
            launch_ready_post_audit(app);
        }
        ReadyPhase::PostAudit => {
            // Post-audit done; workflow complete.
            app.ready_phase = ReadyPhase::Inactive;
            app.ready_ctx = None;
        }
        ReadyPhase::Inactive => {}
    }
}

/// Phase 2: Launch the interactive audit agent via PTY.
fn launch_ready_audit(app: &mut App) {
    let ctx = match &app.ready_ctx {
        Some(ctx) => ctx.clone(),
        None => {
            app.push_output("Internal error: missing ready context for audit phase.");
            app.ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    let entrypoint = ready::audit_entrypoint(&ctx.agent_name);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let config_dir = docker::claude_config_dir();
    let docker_args = docker::build_run_args_pty(
        &ctx.image_tag,
        &ctx.mount_path,
        &entrypoint_refs,
        &ctx.env_vars,
        config_dir.as_deref(),
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    let terminal_area = (80u16, 40u16);
    let size = PtySize {
        rows: terminal_area.1,
        cols: terminal_area.0,
        pixel_width: 0,
        pixel_height: 0,
    };

    app.ready_phase = ReadyPhase::Audit;
    app.continue_command("ready (audit)".to_string());

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.pty = Some(session);
            app.pty_rx = Some(pty_rx);
        }
        Err(e) => {
            app.push_output(format!("Failed to launch audit container: {}", e));
            app.finish_command(1);
        }
    }
}

/// Phase 2 (non-interactive): Launch audit agent in captured mode.
fn launch_ready_audit_captured(app: &mut App) {
    let ctx = match &app.ready_ctx {
        Some(ctx) => ctx.clone(),
        None => {
            app.push_output("Internal error: missing ready context for audit phase.");
            app.ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    app.ready_phase = ReadyPhase::Audit;
    app.continue_command("ready (audit - non-interactive)".to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.exit_rx = Some(exit_rx);
    let tx = app.output_tx.clone();

    spawn_text_command(tx, exit_tx, move |sink| async move {
        let entrypoint = ready::audit_entrypoint_non_interactive(&ctx.agent_name);
        let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
        let config_dir = docker::claude_config_dir();
        let (_cmd, output) = docker::run_container_captured(
            &ctx.image_tag,
            &ctx.mount_path,
            &entrypoint_refs,
            &ctx.env_vars,
            config_dir.as_deref(),
        )?;
        for line in output.lines() {
            sink.println(line);
        }
        Ok(())
    });
}

/// Phase 3: Rebuild the Docker image after the audit agent has updated Dockerfile.dev.
fn launch_ready_post_audit(app: &mut App) {
    let ctx = match &app.ready_ctx {
        Some(ctx) => ctx.clone(),
        None => {
            app.push_output("Internal error: missing ready context for post-audit phase.");
            app.ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    app.ready_phase = ReadyPhase::PostAudit;
    app.continue_command("ready (rebuild)".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.exit_rx = Some(exit_rx);
    let tx = app.output_tx.clone();
    spawn_text_command(tx, exit_tx, move |sink| async move {
        let mut summary = ready::ReadySummary::default();
        // Populate summary fields for the steps that already completed.
        summary.docker_daemon = ready::StepStatus::Ok("running".into());
        summary.dockerfile = ready::StepStatus::Ok("checked".into());
        summary.dev_image = ready::StepStatus::Ok("checked".into());
        summary.refresh = ready::StepStatus::Ok("completed".into());
        ready::run_post_audit(&sink, &ctx, &mut summary).await?;
        print_summary(&sink, &summary);
        sink.println(String::new());
        sink.println("aspec is ready.");
        Ok(())
    });
}

/// Actually spawn the docker container for `implement` via PTY.
async fn launch_implement(app: &mut App, work_item: u32, non_interactive: bool) {
    let git_root = match find_git_root() {
        Some(r) => r,
        None => {
            app.input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    // Validate work item exists before proceeding.
    if let Err(e) = find_work_item(&git_root, work_item) {
        app.input_error = Some(format!("{}", e));
        return;
    }

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    // Resolve credential env vars based on saved preference.
    let env_vars = if config.auto_agent_auth_accepted == Some(true) {
        agent_keychain_vars(&agent_name)
    } else {
        vec![]
    };

    let entrypoint = if non_interactive {
        agent_entrypoint_non_interactive(&agent_name, work_item)
    } else {
        agent_entrypoint(&agent_name, work_item)
    };
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);
    let config_dir = docker::claude_config_dir();

    // Show the full Docker CLI command in the execution window (with masked env values).
    let display_args = if non_interactive {
        docker::build_run_args_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, config_dir.as_deref())
    } else {
        docker::build_run_args_pty_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, config_dir.as_deref())
    };
    let cmd_display = docker::format_run_cmd(&display_args);

    let command_display = format!("implement {:04}", work_item);
    app.start_command(command_display);
    app.push_output(format!("$ {}", cmd_display));

    if non_interactive {
        app.push_output("Tip: remove --non-interactive to interact with the agent directly.");
        // Run captured in a text command.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
        app.exit_rx = Some(exit_rx);
        let tx = app.output_tx.clone();
        let mount_str = mount_path.to_str().unwrap().to_string();
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let entrypoint = agent_entrypoint_non_interactive(&agent_name, work_item);
            let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
            let config_dir = docker::claude_config_dir();
            let (_cmd, output) = docker::run_container_captured(
                &image_tag,
                &mount_str,
                &entrypoint_refs,
                &env_vars,
                config_dir.as_deref(),
            )?;
            for line in output.lines() {
                sink.println(line);
            }
            Ok(())
        });
    } else {
        // Print interactive notice.
        let sink = crate::commands::output::OutputSink::Channel(app.output_tx.clone());
        print_interactive_notice(&sink, &agent_name);

        let docker_args =
            docker::build_run_args_pty(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, config_dir.as_deref());
        let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

        let terminal_area = (80u16, 40u16); // fallback; real size set on first resize event
        let size = PtySize {
            rows: terminal_area.1,
            cols: terminal_area.0,
            pixel_width: 0,
            pixel_height: 0,
        };

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
}

/// Launch the `new` command after collecting kind and title from the dialog.
async fn launch_new(app: &mut App, kind: WorkItemKind, title: String) {
    app.start_command("new".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.exit_rx = Some(exit_rx);
    let tx = app.output_tx.clone();
    spawn_text_command(tx, exit_tx, move |sink| async move {
        new::run_with_sink(&sink, Some(kind), Some(title)).await
    });
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
