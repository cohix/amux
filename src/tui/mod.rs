pub mod input;
mod pty;
pub mod render;
pub mod state;

use crate::cli::Agent;
use crate::commands::auth::{agent_keychain_credentials, apply_auth_decision};
use crate::commands::chat::{chat_entrypoint, chat_entrypoint_non_interactive};
use crate::commands::implement::{
    agent_entrypoint, agent_entrypoint_non_interactive, find_work_item, parse_work_item,
};
use crate::commands::init::find_git_root_from;
use crate::commands::new::WorkItemKind;
use crate::commands::{claws, init, new, ready};
use crate::commands::ready::{ReadyOptions, print_interactive_notice, print_summary};
use crate::config::load_repo_config;
use crate::docker;
use crate::tui::input::Action;
use crate::tui::pty::{spawn_text_command, PtySession};
use crate::tui::render::calculate_container_inner_size;
use crate::tui::state::{App, ClawsPhase, ContainerWindowState, Dialog, PendingCommand, ReadyPhase};
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

/// Flags passed from the root `amux` CLI to the `ready` command run at TUI startup.
#[derive(Clone, Debug, Default)]
pub struct StartupReadyFlags {
    pub build: bool,
    pub no_cache: bool,
    pub refresh: bool,
}

/// Launches the interactive TUI. Blocks until the user quits.
pub async fn run(startup_flags: StartupReadyFlags) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, startup_flags).await;

    // Always restore the terminal, even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}

async fn run_app<B>(terminal: &mut Terminal<B>, startup_flags: StartupReadyFlags) -> Result<()>
where
    B: ratatui::backend::Backend + io::Write,
    <B as ratatui::backend::Backend>::Error: Send + Sync + 'static,
{
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut app = App::new(cwd);

    // Auto-run `ready` at startup, forwarding any flags passed to the root `amux` command.
    let mut startup_cmd = "ready".to_string();
    if startup_flags.refresh {
        startup_cmd.push_str(" --refresh");
    }
    if startup_flags.build {
        startup_cmd.push_str(" --build");
    }
    if startup_flags.no_cache {
        startup_cmd.push_str(" --no-cache");
    }
    execute_command(&mut app, &startup_cmd).await;

    loop {
        terminal.draw(|f| render::draw(f, &mut app))?;

        // Poll for crossterm events with a short timeout to keep the UI responsive.
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let action = input::handle_key(&mut app, key);
                    handle_action(&mut app, action).await;
                }
                Event::Mouse(mouse) => {
                    // Any mouse interaction counts as "checking on" the tab.
                    app.active_tab_mut().acknowledge_stuck();
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            let tab = app.active_tab_mut();
                            if tab.container_window == ContainerWindowState::Maximized {
                                // Scroll up through the container's vt100 scrollback.
                                // Cap at the screen row count due to a vt100 crate
                                // limitation in set_scrollback's internal row arithmetic.
                                let max_scroll = tab.vt100_parser.as_ref()
                                    .map(|p| p.screen().size().0 as usize)
                                    .unwrap_or(0);
                                tab.container_scroll_offset =
                                    (tab.container_scroll_offset + 3).min(max_scroll);
                            } else {
                                let max = tab.output_lines.len();
                                if tab.scroll_offset < max {
                                    tab.scroll_offset = tab.scroll_offset.saturating_add(3);
                                }
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            let tab = app.active_tab_mut();
                            if tab.container_window == ContainerWindowState::Maximized {
                                // Scroll down towards the live view.
                                tab.container_scroll_offset =
                                    tab.container_scroll_offset.saturating_sub(3);
                            } else {
                                tab.scroll_offset = tab.scroll_offset.saturating_sub(3);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Resize(cols, rows) => {
                    for tab in app.tabs.iter_mut() {
                        if let Some(ref pty) = tab.pty {
                            if tab.container_window != ContainerWindowState::Hidden {
                                // Resize the PTY and vt100 parser to match the container inner area.
                                let (inner_cols, inner_rows) = calculate_container_inner_size(cols, rows);
                                pty.resize(PtySize {
                                    rows: inner_rows,
                                    cols: inner_cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                                if let Some(ref mut parser) = tab.vt100_parser {
                                    parser.set_size(inner_rows, inner_cols);
                                }
                            } else {
                                pty.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Drain all pending channel messages (PTY output, command output, exit codes).
        let was_running = matches!(app.active_tab().phase, state::ExecutionPhase::Running { .. });
        app.tick_all();
        let now_done = !matches!(app.active_tab().phase, state::ExecutionPhase::Running { .. });

        // Check if a ready workflow phase just completed and continue to the next phase.
        if was_running && now_done {
            check_ready_continuation(&mut app).await;
            check_claws_continuation(&mut app).await;
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
            app.active_tab_mut().pending_mount_path = Some(path);
            launch_pending_command(app).await;
        }

        Action::AuthAccepted => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.active_tab().dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, true);
            }
            launch_pending_command(app).await;
        }

        Action::AuthDeclined => {
            if let Dialog::AgentAuth { ref agent, ref git_root } = app.active_tab().dialog.clone() {
                let _ = apply_auth_decision(git_root, agent, false);
            }
            launch_pending_command(app).await;
        }

        Action::ForwardToPty(bytes) => {
            if let Some(ref pty) = app.active_tab().pty {
                pty.write_bytes(&bytes);
            }
        }

        Action::NewWorkItem { kind, title } => {
            launch_new(app, kind, title).await;
        }

        Action::ClawsReadyProceed => {
            launch_claws_ready(app).await;
        }

        Action::ClawsReadyStartContainer => {
            launch_claws_start_container(app).await;
        }

        Action::CreateTab => {
            let cwd = app.active_tab().cwd.clone();
            app.active_tab_mut().dialog = Dialog::NewTabDirectory { input: cwd.to_string_lossy().to_string() };
        }

        Action::SwitchTabLeft => {
            let len = app.tabs.len();
            if len > 0 {
                app.active_tab_idx = (app.active_tab_idx + len - 1) % len;
            }
            // Switching to a tab counts as "checking on it" — clear any stuck warning.
            app.active_tab_mut().acknowledge_stuck();
        }

        Action::SwitchTabRight => {
            let len = app.tabs.len();
            if len > 0 {
                app.active_tab_idx = (app.active_tab_idx + 1) % len;
            }
            // Switching to a tab counts as "checking on it" — clear any stuck warning.
            app.active_tab_mut().acknowledge_stuck();
        }

        Action::CloseCurrentTab => {
            let idx = app.active_tab_idx;
            app.close_tab(idx);
        }

        Action::NewTabDirectoryChosen(path) => {
            let new_idx = app.create_tab(path.clone());
            app.active_tab_idx = new_idx;
            execute_tab_command(app, new_idx, "ready").await;
        }
    }
}

/// Execute a command on a specific tab by index.
async fn execute_tab_command(app: &mut App, _tab_idx: usize, cmd: &str) {
    execute_command(app, cmd).await;
}

/// Parse flags from the command parts, returning (refresh, build, no_cache, non_interactive, allow_docker).
fn parse_ready_flags(parts: &[&str]) -> (bool, bool, bool, bool, bool) {
    let refresh = parts.iter().any(|p| *p == "--refresh");
    let build = parts.iter().any(|p| *p == "--build");
    let no_cache = parts.iter().any(|p| *p == "--no-cache");
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
    (refresh, build, no_cache, non_interactive, allow_docker)
}

/// Parse flags from implement command parts, returning (non_interactive, plan, allow_docker).
fn parse_implement_flags(parts: &[&str]) -> (bool, bool, bool) {
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    let plan = parts.iter().any(|p| *p == "--plan");
    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
    (non_interactive, plan, allow_docker)
}

/// Parse flags from chat command parts, returning (non_interactive, plan, allow_docker).
fn parse_chat_flags(parts: &[&str]) -> (bool, bool, bool) {
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    let plan = parts.iter().any(|p| *p == "--plan");
    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
    (non_interactive, plan, allow_docker)
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
            app.active_tab_mut().start_command(cmd.to_string());
            let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
            app.active_tab_mut().exit_rx = Some(exit_rx);
            let tx = app.active_tab().output_tx.clone();
            let aspec = parts.iter().any(|p| *p == "--aspec");
            let tab_cwd = app.active_tab().cwd.clone();
            spawn_text_command(tx, exit_tx, move |sink| async move {
                init::run_with_sink(agent, aspec, false, false, &sink, &tab_cwd).await
            });
        }

        "ready" => {
            let (refresh, build, no_cache, non_interactive, allow_docker) = parse_ready_flags(&parts);
            // If --refresh is set, ignore --build (refresh always rebuilds after audit).
            let effective_build = if refresh { false } else { build };
            app.active_tab_mut().pending_command = PendingCommand::Ready { refresh, build: effective_build, no_cache, non_interactive, allow_docker };
            app.active_tab_mut().ready_opts = ReadyOptions { refresh, build: effective_build, no_cache, non_interactive, allow_docker, auto_create_dockerfile: true };
            show_pre_command_dialogs(app).await;
        }

        "implement" => {
            let (non_interactive, plan, allow_docker) = parse_implement_flags(&parts);
            // Filter out flags to find the work item number.
            let work_item: u32 = match parts.iter()
                .skip(1)
                .find(|s| !s.starts_with("--"))
                .and_then(|s| parse_work_item(s).ok())
            {
                Some(n) => n,
                None => {
                    app.active_tab_mut().input_error =
                        Some("Usage: implement <work-item-number> [--non-interactive] [--plan] [--allow-docker]".into());
                    return;
                }
            };
            app.active_tab_mut().pending_command = PendingCommand::Implement { work_item, non_interactive, plan, allow_docker };
            show_pre_command_dialogs(app).await;
        }

        "chat" => {
            let (non_interactive, plan, allow_docker) = parse_chat_flags(&parts);
            app.active_tab_mut().pending_command = PendingCommand::Chat { non_interactive, plan, allow_docker };
            show_pre_command_dialogs(app).await;
        }

        "new" => {
            app.active_tab_mut().dialog = state::Dialog::NewKindSelect;
        }

        "claws" => {
            match parts.get(1) {
                Some(&"ready") => {
                    app.active_tab_mut().pending_command = PendingCommand::ClawsReady;
                    show_claws_ready_start(app).await;
                }
                _ => {
                    app.active_tab_mut().input_error = Some("Usage: claws ready".into());
                }
            }
        }

        unknown => {
            let suggestion = input::closest_subcommand(unknown)
                .map(|s| format!("  Did you mean: {}", s))
                .unwrap_or_default();
            app.active_tab_mut().input_error = Some(format!(
                "'{}' is not an amux command.{}",
                unknown, suggestion
            ));
        }
    }
}

/// Show any needed dialogs (mount scope, agent auth) before launching a command.
/// Used by both `ready` and `implement` in TUI mode.
async fn show_pre_command_dialogs(app: &mut App) {
    let tab_cwd = app.active_tab().cwd.clone();
    let git_root = match find_git_root_from(&tab_cwd) {
        Some(r) => r,
        None => {
            app.active_tab_mut().input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    // Check mount scope.
    let cwd = tab_cwd;
    if cwd != git_root {
        app.active_tab_mut().dialog = Dialog::MountScope {
            git_root: git_root.clone(),
            cwd,
        };
        return; // Wait for user choice; handle_action resumes after dialog.
    }
    app.active_tab_mut().pending_mount_path = Some(git_root.clone());

    // Auto-passthrough: no agent auth dialog needed. Credentials are always
    // read from the keychain automatically.
    launch_pending_command(app).await;
}

/// Resume the pending command after all dialogs have been answered.
async fn launch_pending_command(app: &mut App) {
    match app.active_tab().pending_command.clone() {
        PendingCommand::Ready { refresh, build, no_cache, non_interactive, allow_docker } => {
            app.active_tab_mut().ready_opts = ReadyOptions { refresh, build, no_cache, non_interactive, allow_docker, auto_create_dockerfile: true };
            launch_ready(app).await;
        }
        PendingCommand::Implement { work_item, non_interactive, plan, allow_docker } => {
            launch_implement(app, work_item, non_interactive, plan, allow_docker).await;
        }
        PendingCommand::Chat { non_interactive, plan, allow_docker } => {
            launch_chat(app, non_interactive, plan, allow_docker).await;
        }
        PendingCommand::ClawsReady => {
            // Claws ready is launched directly from dialog actions (ClawsReadyProceed /
            // ClawsReadyStartContainer), not through the mount-scope dialog flow.
        }
        PendingCommand::None => {}
    }
}

/// Launch the ready command — phase 1 (pre-audit) as a text command.
/// The audit and post-audit phases are triggered automatically via `check_ready_continuation`.
async fn launch_ready(app: &mut App) {
    let tab_cwd = app.active_tab().cwd.clone();
    let git_root = match find_git_root_from(&tab_cwd) {
        Some(r) => r,
        None => {
            app.active_tab_mut().input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.active_tab_mut().pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    // Auto-passthrough: always pass credentials from keychain if available.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare host settings (sanitized config files in a temp dir).
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name);

    let opts = app.active_tab().ready_opts.clone();

    app.active_tab_mut().ready_phase = ReadyPhase::PreAudit;
    app.active_tab_mut().start_command("ready".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let (ctx_tx, ctx_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().ready_ctx_rx = Some(ctx_rx);
    let tx = app.active_tab().output_tx.clone();

    // If not refreshing, run the full sink-based workflow (no audit/post-audit).
    if !opts.refresh {
        app.active_tab_mut().ready_phase = ReadyPhase::Inactive; // No multi-phase needed.
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let _ = ready::run_with_sink(&sink, mount_path, env_vars, &opts, None).await?;
            Ok(())
        });
    } else {
        let opts_clone = opts.clone();
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let mut summary = ready::ReadySummary::default();
            let ctx = ready::run_pre_audit(&sink, mount_path, env_vars, &opts_clone, &mut summary).await?;
            let _ = ctx_tx.send((ctx, summary));
            Ok(())
        });
    }
}

/// Check if a ready workflow phase just completed and automatically launch the next phase.
async fn check_ready_continuation(app: &mut App) {
    match app.active_tab().ready_phase {
        ReadyPhase::PreAudit => {
            // Pre-audit just finished. If it failed, abort the workflow.
            if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
                let tab = app.active_tab_mut();
                tab.ready_phase = ReadyPhase::Inactive;
                tab.ready_ctx = None;
                tab.ready_ctx_rx = None;
                tab.host_settings = None;
                return;
            }
            // The context should have arrived via the channel by now.
            if app.active_tab().ready_ctx.is_none() {
                app.active_tab_mut().push_output("Internal error: pre-audit completed but no context received.");
                app.active_tab_mut().ready_phase = ReadyPhase::Inactive;
                return;
            }

            let opts = app.active_tab().ready_opts.clone();
            if opts.refresh {
                if !opts.non_interactive {
                    // Print the interactive notice via output.
                    let agent_name = app.active_tab().ready_ctx.as_ref()
                        .map(|c| c.agent_name.clone())
                        .unwrap_or_else(|| "agent".into());
                    let sink = crate::commands::output::OutputSink::Channel(app.active_tab().output_tx.clone());
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
                let tab = app.active_tab_mut();
                tab.ready_phase = ReadyPhase::Inactive;
                tab.ready_ctx = None;
            }
        }
        ReadyPhase::Audit => {
            // Audit PTY just finished. If it failed, abort.
            if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
                let tab = app.active_tab_mut();
                tab.ready_phase = ReadyPhase::Inactive;
                tab.ready_ctx = None;
                tab.host_settings = None;
                return;
            }
            // Launch post-audit (image rebuild — no container, no settings needed).
            app.active_tab_mut().host_settings = None;
            launch_ready_post_audit(app);
        }
        ReadyPhase::PostAudit => {
            // Post-audit done; workflow complete.
            let tab = app.active_tab_mut();
            tab.ready_phase = ReadyPhase::Inactive;
            tab.ready_ctx = None;
        }
        ReadyPhase::Inactive => {}
    }
}

/// Phase 2: Launch the interactive audit agent via PTY.
fn launch_ready_audit(app: &mut App) {
    let ctx = match app.active_tab().ready_ctx.clone() {
        Some(ctx) => ctx,
        None => {
            app.active_tab_mut().push_output("Internal error: missing ready context for audit phase.");
            app.active_tab_mut().ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    let allow_docker = app.active_tab().ready_opts.allow_docker;

    // If --allow-docker, check the socket and print a warning before launching.
    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
                app.active_tab_mut().push_output(format!(
                    "WARNING: --allow-docker: mounting host Docker socket into audit container ({}:{}). \
                     This grants the agent elevated host access.",
                    socket_path.display(),
                    socket_path.display()
                ));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    let container_name = docker::generate_container_name();
    let entrypoint = ready::audit_entrypoint(&ctx.agent_name);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let docker_args = docker::build_run_args_pty(
        &ctx.image_tag,
        &ctx.mount_path,
        &entrypoint_refs,
        &ctx.env_vars,
        Some(&container_name),
        app.active_tab().host_settings.as_ref(),
        allow_docker,
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    // Use actual terminal dimensions for the PTY.
    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    app.active_tab_mut().ready_phase = ReadyPhase::Audit;
    app.active_tab_mut().continue_command("ready (audit)".to_string());

    // Activate the container window.
    let display_name = state::agent_display_name(&ctx.agent_name).to_string();
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to launch audit container: {}", e));
            app.active_tab_mut().finish_command(1);
        }
    }
}

/// Phase 2 (non-interactive): Launch audit agent in captured mode.
fn launch_ready_audit_captured(app: &mut App) {
    let ctx = match app.active_tab().ready_ctx.clone() {
        Some(ctx) => ctx,
        None => {
            app.active_tab_mut().push_output("Internal error: missing ready context for audit phase.");
            app.active_tab_mut().ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    let allow_docker = app.active_tab().ready_opts.allow_docker;

    // If --allow-docker, check the socket and print a warning before launching.
    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
                app.active_tab_mut().push_output(format!(
                    "WARNING: --allow-docker: mounting host Docker socket into audit container ({}:{}). \
                     This grants the agent elevated host access.",
                    socket_path.display(),
                    socket_path.display()
                ));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    // Move host_settings into the task so the temp dir lives until the container exits.
    let host_settings = app.active_tab_mut().host_settings.take();

    app.active_tab_mut().ready_phase = ReadyPhase::Audit;
    app.active_tab_mut().continue_command("ready (audit - non-interactive)".to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    spawn_text_command(tx, exit_tx, move |sink| async move {
        let entrypoint = ready::audit_entrypoint_non_interactive(&ctx.agent_name);
        let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
        let (_cmd, output) = docker::run_container_captured(
            &ctx.image_tag,
            &ctx.mount_path,
            &entrypoint_refs,
            &ctx.env_vars,
            host_settings.as_ref(),
            allow_docker,
        )?;
        for line in output.lines() {
            sink.println(line);
        }
        Ok(())
    });
}

/// Phase 3: Rebuild the Docker image after the audit agent has updated Dockerfile.dev.
fn launch_ready_post_audit(app: &mut App) {
    let ctx = match app.active_tab().ready_ctx.clone() {
        Some(ctx) => ctx,
        None => {
            app.active_tab_mut().push_output("Internal error: missing ready context for post-audit phase.");
            app.active_tab_mut().ready_phase = ReadyPhase::Inactive;
            return;
        }
    };

    let opts = app.active_tab().ready_opts.clone();
    app.active_tab_mut().ready_phase = ReadyPhase::PostAudit;
    app.active_tab_mut().continue_command("ready (rebuild)".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();
    spawn_text_command(tx, exit_tx, move |sink| async move {
        let mut summary = ready::ReadySummary::default();
        // Populate summary fields for the steps that already completed.
        summary.docker_daemon = ready::StepStatus::Ok("running".into());
        summary.dockerfile = ready::StepStatus::Ok("checked".into());
        summary.dev_image = ready::StepStatus::Ok("checked".into());
        summary.refresh = ready::StepStatus::Ok("completed".into());
        ready::run_post_audit(&sink, &ctx, &opts, &mut summary).await?;
        print_summary(&sink, &summary);
        sink.println(String::new());
        sink.println("amux is ready.");
        Ok(())
    });
}

/// Actually spawn the docker container for `implement` via PTY.
async fn launch_implement(app: &mut App, work_item: u32, non_interactive: bool, plan: bool, allow_docker: bool) {
    let tab_cwd = app.active_tab().cwd.clone();
    let git_root = match find_git_root_from(&tab_cwd) {
        Some(r) => r,
        None => {
            app.active_tab_mut().input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    // Validate work item exists before proceeding.
    if let Err(e) = find_work_item(&git_root, work_item) {
        app.active_tab_mut().input_error = Some(format!("{}", e));
        return;
    }

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.active_tab_mut().pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    // Auto-passthrough: always pass credentials from keychain if available.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare host settings (sanitized config files in a temp dir).
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name);

    let entrypoint = if non_interactive {
        agent_entrypoint_non_interactive(&agent_name, work_item, plan)
    } else {
        agent_entrypoint(&agent_name, work_item, plan)
    };
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);

    // Generate a container name for stats polling.
    let container_name = docker::generate_container_name();

    // Show the full Docker CLI command in the execution window (with masked env values).
    let display_args = if non_interactive {
        docker::build_run_args_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, app.active_tab().host_settings.as_ref(), allow_docker)
    } else {
        docker::build_run_args_pty_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker)
    };
    let cmd_display = docker::format_run_cmd(&display_args);

    let command_display = format!("implement {:04}", work_item);
    app.active_tab_mut().start_command(command_display);

    // If --allow-docker, check the socket and print a warning before launching.
    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
                app.active_tab_mut().push_output(format!(
                    "WARNING: --allow-docker: mounting host Docker socket into container ({}:{}). \
                     This grants the agent elevated host access.",
                    socket_path.display(),
                    socket_path.display()
                ));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    app.active_tab_mut().push_output(format!("$ {}", cmd_display));

    if non_interactive {
        app.active_tab_mut().push_output("Tip: remove --non-interactive to interact with the agent directly.");
        // Move host_settings into the task so the temp dir lives until the container exits.
        let host_settings = app.active_tab_mut().host_settings.take();
        // Run captured in a text command.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
        app.active_tab_mut().exit_rx = Some(exit_rx);
        let tx = app.active_tab().output_tx.clone();
        let mount_str = mount_path.to_str().unwrap().to_string();
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let entrypoint = agent_entrypoint_non_interactive(&agent_name, work_item, plan);
            let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
            let (_cmd, output) = docker::run_container_captured(
                &image_tag,
                &mount_str,
                &entrypoint_refs,
                &env_vars,
                host_settings.as_ref(),
                allow_docker,
            )?;
            for line in output.lines() {
                sink.println(line);
            }
            Ok(())
        });
    } else {
        // Print interactive notice to the outer window.
        let sink = crate::commands::output::OutputSink::Channel(app.active_tab().output_tx.clone());
        print_interactive_notice(&sink, &agent_name);

        let docker_args =
            docker::build_run_args_pty(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker);
        let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

        // Use actual terminal dimensions for the PTY.
        let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows);
        let size = PtySize {
            rows: inner_rows,
            cols: inner_cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        // Activate the container window.
        let display_name = state::agent_display_name(&agent_name).to_string();
        app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

        match PtySession::spawn("docker", &docker_str_refs, size) {
            Ok((session, pty_rx)) => {
                app.active_tab_mut().pty = Some(session);
                app.active_tab_mut().pty_rx = Some(pty_rx);
                // Start Docker stats polling.
                app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Failed to launch container: {}", e));
                app.active_tab_mut().finish_command(1);
            }
        }
    }
}

/// Actually spawn the docker container for `chat` via PTY.
async fn launch_chat(app: &mut App, non_interactive: bool, plan: bool, allow_docker: bool) {
    let tab_cwd = app.active_tab().cwd.clone();
    let git_root = match find_git_root_from(&tab_cwd) {
        Some(r) => r,
        None => {
            app.active_tab_mut().input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.active_tab_mut().pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    // Auto-passthrough: always pass credentials from keychain if available.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare host settings (sanitized config files in a temp dir).
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name);

    let entrypoint = if non_interactive {
        chat_entrypoint_non_interactive(&agent_name, plan)
    } else {
        chat_entrypoint(&agent_name, plan)
    };
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);

    // Generate a container name for stats polling.
    let container_name = docker::generate_container_name();

    // Show the full Docker CLI command in the execution window (with masked env values).
    let display_args = if non_interactive {
        docker::build_run_args_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, app.active_tab().host_settings.as_ref(), allow_docker)
    } else {
        docker::build_run_args_pty_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker)
    };
    let cmd_display = docker::format_run_cmd(&display_args);

    let command_display = "chat".to_string();
    app.active_tab_mut().start_command(command_display);

    // If --allow-docker, check the socket and print a warning before launching.
    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
                app.active_tab_mut().push_output(format!(
                    "WARNING: --allow-docker: mounting host Docker socket into container ({}:{}). \
                     This grants the agent elevated host access.",
                    socket_path.display(),
                    socket_path.display()
                ));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    app.active_tab_mut().push_output(format!("$ {}", cmd_display));

    if non_interactive {
        app.active_tab_mut().push_output("Tip: remove --non-interactive to interact with the agent directly.");
        // Move host_settings into the task so the temp dir lives until the container exits.
        let host_settings = app.active_tab_mut().host_settings.take();
        // Run captured in a text command.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
        app.active_tab_mut().exit_rx = Some(exit_rx);
        let tx = app.active_tab().output_tx.clone();
        let mount_str = mount_path.to_str().unwrap().to_string();
        spawn_text_command(tx, exit_tx, move |sink| async move {
            let entrypoint = chat_entrypoint_non_interactive(&agent_name, plan);
            let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();
            let (_cmd, output) = docker::run_container_captured(
                &image_tag,
                &mount_str,
                &entrypoint_refs,
                &env_vars,
                host_settings.as_ref(),
                allow_docker,
            )?;
            for line in output.lines() {
                sink.println(line);
            }
            Ok(())
        });
    } else {
        // Print interactive notice to the outer window.
        let sink = crate::commands::output::OutputSink::Channel(app.active_tab().output_tx.clone());
        print_interactive_notice(&sink, &agent_name);

        let docker_args =
            docker::build_run_args_pty(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker);
        let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

        // Use actual terminal dimensions for the PTY.
        let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows);
        let size = PtySize {
            rows: inner_rows,
            cols: inner_cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        // Activate the container window.
        let display_name = state::agent_display_name(&agent_name).to_string();
        app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

        match PtySession::spawn("docker", &docker_str_refs, size) {
            Ok((session, pty_rx)) => {
                app.active_tab_mut().pty = Some(session);
                app.active_tab_mut().pty_rx = Some(pty_rx);
                // Start Docker stats polling.
                app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Failed to launch container: {}", e));
                app.active_tab_mut().finish_command(1);
            }
        }
    }
}

/// Spawn a background task that polls Docker stats every 5 seconds.
fn spawn_stats_poller(
    container_name: String,
) -> tokio::sync::mpsc::UnboundedReceiver<docker::ContainerStats> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let name = container_name.clone();
            let stats = tokio::task::spawn_blocking(move || docker::query_container_stats(&name))
                .await;
            match stats {
                Ok(Some(s)) => {
                    if tx.send(s).is_err() {
                        break;
                    }
                }
                _ => {
                    // Container may not be running yet or has exited.
                    // If the receiver is dropped, the send will fail and we'll break.
                }
            }
        }
    });
    rx
}

/// Determine what to show when `claws ready` is entered.
///
/// - Nanoclaw not installed → first-run wizard (HasForked dialog)
/// - Nanoclaw installed, container running → show status in output
/// - Nanoclaw installed, container stopped → OfferStart dialog
async fn show_claws_ready_start(app: &mut App) {
    let nanoclaw_dir = claws::nanoclaw_path();

    if !nanoclaw_dir.exists() {
        // First run: start the wizard.
        app.active_tab_mut().dialog = Dialog::ClawsReadyHasForked;
        return;
    }

    // Nanoclaw is installed — check container state.
    match claws::load_nanoclaw_config() {
        Ok(config) => {
            if let Some(ref id) = config.nanoclaw_container_id {
                if docker::is_container_running(id) {
                    // Container is running — show status.
                    app.active_tab_mut().start_command("claws ready".to_string());
                    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
                    app.active_tab_mut().exit_rx = Some(exit_rx);
                    let tx = app.active_tab().output_tx.clone();
                    let container_id = id.clone();
                    spawn_text_command(tx, exit_tx, move |sink| async move {
                        let mut summary = claws::ClawsSummary {
                            nanoclaw_cloned: crate::commands::ready::StepStatus::Ok("exists".into()),
                            docker_daemon: crate::commands::ready::StepStatus::Ok("running".into()),
                            nanoclaw_image: crate::commands::ready::StepStatus::Ok("exists".into()),
                            nanoclaw_container: crate::commands::ready::StepStatus::Ok(
                                format!("running ({})", &container_id[..container_id.len().min(12)])
                            ),
                        };
                        claws::print_claws_summary(&sink, &mut summary);
                        sink.println("nanoclaw container is running.");
                        Ok(())
                    });
                    return;
                }
            }
            // Container not running or no saved ID.
            app.active_tab_mut().dialog = Dialog::ClawsReadyOfferStart;
        }
        Err(_) => {
            app.active_tab_mut().dialog = Dialog::ClawsReadyOfferStart;
        }
    }
}

/// Launch the claws first-run setup as a text command (TUI mode).
///
/// Clones the repo and sets up the container. After the text phase completes,
/// `check_claws_continuation` attaches the agent via PTY.
async fn launch_claws_ready(app: &mut App) {
    let username = app.active_tab().claws_wizard_username.clone();

    // Resolve credentials using the same auto-passthrough as other containers.
    let agent_name = {
        let config = load_repo_config(&claws::nanoclaw_path()).unwrap_or_default();
        config.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare sanitized host config (same as `chat`/`implement` auto-configuration).
    // Stored in tab.host_settings so the temp dir outlives the background setup task
    // and remains valid through the subsequent PTY exec session.
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name);
    // A path-only view is moved into the closure; the actual TempDir lives in the tab.
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    app.active_tab_mut().claws_phase = ClawsPhase::Setup;
    app.active_tab_mut().start_command("claws ready".to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);
    let tx = app.active_tab().output_tx.clone();

    // Channels for the background task to request sudo permission when the clone
    // destination ($HOME/.nanoclaw) is not writable by the current user.
    // The response carries Option<String>: Some(password) = user accepted with their
    // sudo password, None = user declined.
    let (sudo_request_tx, sudo_request_rx) = tokio::sync::oneshot::channel::<()>();
    let (sudo_response_tx, sudo_response_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.active_tab_mut().claws_sudo_request_rx = Some(sudo_request_rx);
    app.active_tab_mut().claws_sudo_response_tx = Some(sudo_response_tx);

    // Channels for the background task to request docker socket acceptance after the
    // image rebuild completes. The response carries bool: true = accepted, false = declined.
    let (docker_accept_request_tx, docker_accept_request_rx) = tokio::sync::oneshot::channel::<()>();
    let (docker_accept_response_tx, docker_accept_response_rx) = tokio::sync::oneshot::channel::<bool>();
    app.active_tab_mut().claws_docker_accept_request_rx = Some(docker_accept_request_rx);
    app.active_tab_mut().claws_docker_accept_response_tx = Some(docker_accept_response_tx);

    spawn_text_command(tx, exit_tx, move |sink| async move {
        if let Some(ref username) = username {
            match claws::clone_nanoclaw(username.trim(), &sink)? {
                claws::CloneOutcome::Success => {
                    claws::chmod_nanoclaw_permissive(&sink);
                }
                claws::CloneOutcome::PermissionDenied => {
                    sink.println(format!(
                        "Clone failed: permission denied writing to {}.",
                        claws::nanoclaw_path_str()
                    ));
                    // Signal the TUI to show the sudo password dialog.
                    if sudo_request_tx.send(()).is_err() {
                        anyhow::bail!("Clone cancelled: permission denied.");
                    }
                    // Block until the user enters their password (or cancels) in the dialog.
                    match sudo_response_rx.await.unwrap_or(None) {
                        None => anyhow::bail!("Clone cancelled: sudo not accepted."),
                        Some(password) => {
                            claws::clone_nanoclaw_sudo(username.trim(), &sink, Some(&password))?;
                            claws::chmod_nanoclaw_permissive(&sink);
                        }
                    }
                }
            }
        }
        let mut summary = claws::ClawsSummary {
            nanoclaw_cloned: crate::commands::ready::StepStatus::Ok("cloned".into()),
            ..Default::default()
        };

        // Phase 1: build image + run audit agent (no docker socket).
        claws::build_nanoclaw_image(&sink, &env_vars, &mut summary, closure_host_settings.as_ref()).await?;

        // Signal the TUI to show the docker socket warning dialog (after image rebuild).
        if docker_accept_request_tx.send(()).is_err() {
            anyhow::bail!("Docker socket warning channel closed unexpectedly.");
        }
        // Block until the user accepts or declines in the dialog.
        if !docker_accept_response_rx.await.unwrap_or(false) {
            anyhow::bail!("Docker socket access declined. Cannot launch nanoclaw container.");
        }

        // Phase 2: launch background container with docker socket.
        let container_id =
            claws::launch_nanoclaw_container(&sink, &env_vars, &mut summary, closure_host_settings.as_ref()).await?;
        let _ = container_tx.send(container_id);
        Ok(())
    });
}

/// Launch the claws container start for a subsequent run (TUI mode).
///
/// Starts the background container with a direct host bind mount for
/// `$HOME/.nanoclaw` and sends the container ID to
/// `check_claws_continuation` for the PTY attach.
async fn launch_claws_start_container(app: &mut App) {
    // Resolve credentials using the same auto-passthrough as other containers.
    let agent_name = {
        let config = load_repo_config(&claws::nanoclaw_path()).unwrap_or_default();
        config.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare sanitized host config. Stored in tab so TempDir outlives the
    // background setup task and remains valid through the PTY exec session.
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name);
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    app.active_tab_mut().claws_phase = ClawsPhase::Setup;
    app.active_tab_mut().start_command("claws ready".to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);
    let tx = app.active_tab().output_tx.clone();

    spawn_text_command(tx, exit_tx, move |sink| async move {
        let nanoclaw_str = claws::nanoclaw_path_str();
        let container_name = docker::generate_container_name();
        sink.println(format!("Starting nanoclaw container {}...", container_name));

        let container_id = docker::run_container_detached(
            claws::NANOCLAW_IMAGE_TAG,
            &nanoclaw_str,
            &nanoclaw_str,
            &nanoclaw_str,
            Some(&container_name),
            &env_vars,
            true,
            closure_host_settings.as_ref(),
        )?;

        sink.print("Waiting for container to start... ");
        if !claws::wait_for_container(&container_id, 5) {
            sink.println("TIMEOUT");
            anyhow::bail!("Container did not start within 5 seconds.");
        }
        sink.println("OK");

        let mut config = claws::load_nanoclaw_config().unwrap_or_default();
        config.nanoclaw_container_id = Some(container_id.clone());
        claws::save_nanoclaw_config(&config)?;

        let _ = container_tx.send(container_id);
        Ok(())
    });
}

/// Check if the claws setup phase just completed and attach to the container.
async fn check_claws_continuation(app: &mut App) {
    if app.active_tab().claws_phase != ClawsPhase::Setup {
        return;
    }

    if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
        let tab = app.active_tab_mut();
        tab.claws_phase = ClawsPhase::Inactive;
        tab.claws_container_id = None;
        tab.claws_container_id_rx = None;
        return;
    }

    // Container ID is delivered via tick() into claws_container_id.
    if let Some(container_id) = app.active_tab_mut().claws_container_id.take() {
        app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
        app.active_tab_mut().claws_container_id_rx = None;
        launch_claws_exec(app, container_id).await;
    } else {
        // Setup completed but no container ID — error path.
        let tab = app.active_tab_mut();
        tab.claws_phase = ClawsPhase::Inactive;
        tab.claws_container_id_rx = None;
    }
}

/// Attach to a running nanoclaw container via PTY (TUI mode).
async fn launch_claws_exec(app: &mut App, container_id: String) {
    let agent_name = {
        let config = load_repo_config(&claws::nanoclaw_path()).unwrap_or_default();
        config.agent.unwrap_or_else(|| "claude".to_string())
    };

    // Resolve credentials using the same auto-passthrough as other containers.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // The setup container receives no premade prompt — the user interacts directly
    // with their agent (e.g. to run /setup on first launch).
    let entrypoint = chat_entrypoint(&agent_name, false);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let exec_args = docker::build_exec_args_pty(
        &container_id,
        &claws::nanoclaw_path_str(),
        &entrypoint_refs,
        &env_vars,
    );
    let exec_str_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let container_name = format!("nanoclaw-{}", &container_id[..container_id.len().min(12)]);
    let display_name = state::agent_display_name(&agent_name).to_string();

    app.active_tab_mut().continue_command("claws ready (attached)".to_string());
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    match PtySession::spawn("docker", &exec_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to attach to nanoclaw container: {}", e));
            app.active_tab_mut().finish_command(1);
        }
    }
}

/// Launch the `new` command after collecting kind and title from the dialog.
async fn launch_new(app: &mut App, kind: WorkItemKind, title: String) {
    app.active_tab_mut().start_command("new".to_string());
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();
    let tab_cwd = app.active_tab().cwd.clone();
    spawn_text_command(tx, exit_tx, move |sink| async move {
        new::run_with_sink(&sink, Some(kind), Some(title), &tab_cwd).await
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
