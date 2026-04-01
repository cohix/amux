pub mod input;
mod pty;
pub mod render;
pub mod state;

use crate::cli::Agent;
use crate::commands::auth::{agent_keychain_credentials, apply_auth_decision};
use dirs;
use crate::commands::chat::{chat_entrypoint, chat_entrypoint_non_interactive};
use crate::commands::implement::{
    agent_entrypoint, agent_entrypoint_non_interactive, find_work_item, parse_work_item,
    workflow_step_entrypoint,
};
use crate::commands::init::find_git_root_from;
use crate::commands::new::WorkItemKind;
use crate::commands::specs::{amend_agent_entrypoint, interview_agent_entrypoint};
use crate::commands::{claws, init, new, ready, status};
use crate::commands::ready::{ReadyOptions, print_interactive_notice, print_summary};
use crate::config::load_repo_config;
use crate::docker;
use crate::tui::input::Action;
use crate::tui::pty::{spawn_text_command, PtySession};
use crate::tui::render::{calculate_container_inner_size, workflow_strip_height};
use crate::tui::state::{App, ClawsPhase, ContainerWindowState, Dialog, PendingCommand, ReadyPhase};
use crate::workflow::{self, StepStatus};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
        KeyEventKind, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
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
    // Enable keyboard enhancement so that modifiers on special keys (e.g. Ctrl+Enter)
    // are reported as distinct events. This is a best-effort push: terminals that do
    // not support the Kitty keyboard protocol will silently ignore it.
    let keyboard_enhanced = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, startup_flags).await;

    // Always restore the terminal, even on error.
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
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
    let mut app = App::new(cwd.clone());

    // At startup: if we are inside a Git repo, run `ready` as usual.
    // If not, run `status --watch` so the user can see the global agent universe.
    let startup_cmd = if find_git_root_from(&cwd).is_some() {
        let mut cmd = "ready".to_string();
        if startup_flags.refresh {
            cmd.push_str(" --refresh");
        }
        if startup_flags.build {
            cmd.push_str(" --build");
        }
        if startup_flags.no_cache {
            cmd.push_str(" --no-cache");
        }
        cmd
    } else {
        "status --watch".to_string()
    };
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
                                // Resize the PTY and vt100 parser to match the container inner area,
                                // accounting for any active workflow strip that reduces exec height.
                                let wf_strip_h = tab.workflow.as_ref()
                                    .map(|wf| workflow_strip_height(wf))
                                    .unwrap_or(0);
                                let (inner_cols, inner_rows) = calculate_container_inner_size(cols, rows, wf_strip_h);
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
            check_workflow_step_completion(&mut app).await;
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

        Action::NewWorkItem { kind, title, interview } => {
            if interview {
                launch_new_interview(app, kind, title).await;
            } else {
                launch_new(app, kind, title).await;
            }
        }

        Action::NewInterviewSummarySubmitted { kind, title, work_item_number, summary } => {
            let tab = app.active_tab_mut();
            tab.pending_command = PendingCommand::SpecsNewInterview {
                work_item_number,
                kind,
                title,
                summary,
                allow_docker: false,
            };
            show_pre_command_dialogs(app).await;
        }

        Action::ClawsReadyProceed => {
            launch_claws_ready(app).await;
        }

        Action::ClawsReadyStartContainer => {
            launch_claws_start_container_status_only(app).await;
        }

        Action::ClawsReadyRestartStopped { container_id } => {
            launch_claws_restart_stopped_container(app, container_id).await;
        }

        Action::ClawsReadyDeleteAndStartFresh { container_id } => {
            launch_claws_delete_and_start_fresh(app, container_id).await;
        }

        Action::ClawsAuditConfirmAccept => {
            // Audit runs in the background — go straight to post-audit (dialogs + container launch).
            if app.active_tab().claws_audit_ctx.is_some() {
                launch_claws_init_post_audit(app).await;
            } else {
                app.active_tab_mut().push_output(
                    "Internal error: audit context missing when audit was accepted.".to_string(),
                );
                app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
            }
        }

        Action::ClawsAuditConfirmDecline => {
            app.active_tab_mut().push_output("Audit declined. Setup cancelled.".to_string());
            app.active_tab_mut().claws_audit_ctx = None;
            app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
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

        Action::WorkflowAdvance => {
            launch_next_workflow_step(app).await;
        }

        Action::WorkflowAbort => {
            abort_workflow(app);
        }

        Action::WorkflowRetry => {
            retry_workflow_step(app).await;
        }

        Action::WorkflowRestartStep => {
            // Same as retry: reset step to Pending and re-launch.
            retry_workflow_step(app).await;
        }

        Action::WorkflowCancelToPrevious => {
            cancel_to_previous_step(app).await;
        }

        Action::WorkflowNextInNewContainer => {
            advance_workflow_next_new_container(app).await;
        }

        Action::WorkflowNextInCurrentContainer => {
            advance_workflow_next_current_container(app).await;
        }

        Action::WorktreeMerge => {
            handle_worktree_merge(app).await;
        }

        Action::WorktreeDiscard => {
            handle_worktree_discard(app).await;
        }

        Action::WorktreeSkip => {
            handle_worktree_skip(app);
        }
    }
}

/// Merge the worktree branch into the current HEAD and clean up the worktree.
async fn handle_worktree_merge(app: &mut App) {
    let (branch, wt_path, git_root) = match (
        app.active_tab_mut().worktree_branch.take(),
        app.active_tab_mut().worktree_active_path.take(),
        app.active_tab_mut().worktree_git_root.take(),
    ) {
        (Some(b), Some(p), Some(r)) => (b, p, r),
        _ => return,
    };
    let tx = app.active_tab().output_tx.clone();
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    spawn_text_command(tx, exit_tx, move |sink| async move {
        match crate::git::merge_branch(&git_root, &branch) {
            Ok(()) => {
                sink.println(format!("Merged branch '{}' into current branch.", branch));
                let _ = crate::git::remove_worktree(&git_root, &wt_path);
                let _ = crate::git::delete_branch(&git_root, &branch);
                sink.println("Worktree removed and branch deleted.".to_string());
            }
            Err(e) => {
                sink.println(format!("Merge failed: {}. Worktree kept at {}.", e, wt_path.display()));
            }
        }
        Ok(())
    });
}

/// Discard the worktree branch and remove the worktree directory.
async fn handle_worktree_discard(app: &mut App) {
    let (branch, wt_path, git_root) = match (
        app.active_tab_mut().worktree_branch.take(),
        app.active_tab_mut().worktree_active_path.take(),
        app.active_tab_mut().worktree_git_root.take(),
    ) {
        (Some(b), Some(p), Some(r)) => (b, p, r),
        _ => return,
    };
    let tx = app.active_tab().output_tx.clone();
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    spawn_text_command(tx, exit_tx, move |sink| async move {
        match crate::git::remove_worktree(&git_root, &wt_path) {
            Ok(()) => {
                sink.println(format!("Worktree at {} removed.", wt_path.display()));
                let _ = crate::git::delete_branch(&git_root, &branch);
                sink.println(format!("Branch '{}' deleted.", branch));
            }
            Err(e) => {
                sink.println(format!("Failed to remove worktree: {}", e));
            }
        }
        Ok(())
    });
}

/// Keep the worktree branch as-is (no merge, no delete).
fn handle_worktree_skip(app: &mut App) {
    if let Some(path) = app.active_tab().worktree_active_path.clone() {
        app.active_tab_mut().push_output(format!(
            "Worktree kept at {}. Use 'git worktree list' to see active worktrees.",
            path.display()
        ));
    }
    app.active_tab_mut().worktree_branch = None;
    app.active_tab_mut().worktree_active_path = None;
    app.active_tab_mut().worktree_git_root = None;
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

/// Parse flags from implement command parts, returning (non_interactive, plan, allow_docker, workflow_path, worktree, mount_ssh).
fn parse_implement_flags(parts: &[&str]) -> (bool, bool, bool, Option<std::path::PathBuf>, bool, bool) {
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    let plan = parts.iter().any(|p| *p == "--plan");
    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
    let worktree = parts.iter().any(|p| *p == "--worktree");
    let mount_ssh = parts.iter().any(|p| *p == "--mount-ssh");
    // Accept --workflow=<path> or --workflow <path>
    let workflow = parts
        .iter()
        .find_map(|p| {
            if let Some(val) = p.strip_prefix("--workflow=") {
                Some(std::path::PathBuf::from(val))
            } else {
                None
            }
        })
        .or_else(|| {
            // --workflow <path> (separate token)
            parts
                .windows(2)
                .find(|w| w[0] == "--workflow")
                .map(|w| std::path::PathBuf::from(w[1]))
        });
    (non_interactive, plan, allow_docker, workflow, worktree, mount_ssh)
}

/// Parse flags from chat command parts, returning (non_interactive, plan, allow_docker, mount_ssh).
fn parse_chat_flags(parts: &[&str]) -> (bool, bool, bool, bool) {
    let non_interactive = parts.iter().any(|p| *p == "--non-interactive");
    let plan = parts.iter().any(|p| *p == "--plan");
    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
    let mount_ssh = parts.iter().any(|p| *p == "--mount-ssh");
    (non_interactive, plan, allow_docker, mount_ssh)
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
            let (non_interactive, plan, allow_docker, workflow, worktree, mount_ssh) = parse_implement_flags(&parts);
            // Filter out flags (and --workflow <path>) to find the work item number.
            let work_item: u32 = match parts.iter()
                .skip(1)
                .filter(|s| !s.starts_with("--"))
                .find(|s| parse_work_item(s).is_ok())
                .and_then(|s| parse_work_item(s).ok())
            {
                Some(n) => n,
                None => {
                    app.active_tab_mut().input_error =
                        Some("Usage: implement <work-item-number> [--non-interactive] [--plan] [--allow-docker] [--workflow=<path>] [--worktree] [--mount-ssh]".into());
                    return;
                }
            };
            app.active_tab_mut().pending_command = PendingCommand::Implement { work_item, non_interactive, plan, allow_docker, workflow, worktree, mount_ssh };
            show_pre_command_dialogs(app).await;
        }

        "chat" => {
            let (non_interactive, plan, allow_docker, mount_ssh) = parse_chat_flags(&parts);
            app.active_tab_mut().pending_command = PendingCommand::Chat { non_interactive, plan, allow_docker, mount_ssh };
            show_pre_command_dialogs(app).await;
        }


        "specs" => {
            match parts.get(1) {
                Some(&"new") => {
                    let interview = parts.iter().any(|p| *p == "--interview");
                    app.active_tab_mut().dialog = state::Dialog::NewKindSelect { interview };
                }
                Some(&"amend") => {
                    let allow_docker = parts.iter().any(|p| *p == "--allow-docker");
                    let work_item: u32 = match parts.iter()
                        .skip(2)
                        .find(|s| !s.starts_with("--"))
                        .and_then(|s| parse_work_item(s).ok())
                    {
                        Some(n) => n,
                        None => {
                            app.active_tab_mut().input_error =
                                Some("Usage: specs amend <NNNN>  e.g. specs amend 0025".into());
                            return;
                        }
                    };
                    app.active_tab_mut().pending_command = PendingCommand::SpecsAmend { work_item, allow_docker };
                    show_pre_command_dialogs(app).await;
                }
                _ => {
                    app.active_tab_mut().input_error =
                        Some("Usage: specs <new|amend>  e.g. specs new --interview, specs amend 0025".into());
                }
            }
        }

        "claws" => {
            match parts.get(1) {
                Some(&"init") => {
                    show_claws_init_start(app).await;
                }
                Some(&"ready") => {
                    show_claws_ready_status(app).await;
                }
                Some(&"chat") => {
                    launch_claws_chat_attach(app).await;
                }
                _ => {
                    app.active_tab_mut().input_error =
                        Some("Usage: claws <init|ready|chat>".into());
                }
            }
        }

        "status" => {
            let watch = parts.iter().any(|p| *p == "--watch");
            app.active_tab_mut().start_command(cmd.to_string());
            let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
            app.active_tab_mut().exit_rx = Some(exit_rx);
            let tx = app.active_tab().output_tx.clone();
            // Pass the shared Arc so the background task reads live state on every refresh.
            let tui_tabs = app.tui_tabs_shared.clone();
            if watch {
                // Create a cancel channel so that running a new command stops the loop.
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                app.active_tab_mut().status_watch_cancel_tx = Some(cancel_tx);
                spawn_text_command(tx, exit_tx, move |sink| async move {
                    status::run_with_sink(true, &sink, Some(cancel_rx), tui_tabs).await
                });
            } else {
                spawn_text_command(tx, exit_tx, move |sink| async move {
                    status::run_with_sink(false, &sink, None, tui_tabs).await
                });
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
        PendingCommand::Implement { work_item, non_interactive, plan, allow_docker, workflow, worktree, mount_ssh } => {
            launch_implement(app, work_item, non_interactive, plan, allow_docker, workflow, worktree, mount_ssh).await;
        }
        PendingCommand::Chat { non_interactive, plan, allow_docker, mount_ssh } => {
            launch_chat(app, non_interactive, plan, allow_docker, mount_ssh).await;
        }
        PendingCommand::ClawsReady => {
            // Claws ready is launched directly from dialog actions (ClawsReadyProceed /
            // ClawsReadyStartContainer), not through the mount-scope dialog flow.
        }
        PendingCommand::SpecsAmend { work_item, allow_docker } => {
            launch_specs_amend(app, work_item, allow_docker).await;
        }
        PendingCommand::SpecsNewInterview { work_item_number, kind, title, summary, allow_docker } => {
            launch_specs_interview_agent(app, work_item_number, kind, title, summary, allow_docker).await;
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
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));

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
        None,
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    // Use actual terminal dimensions for the PTY.
    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
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
            None,
            None,
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
async fn launch_implement(app: &mut App, work_item: u32, non_interactive: bool, plan: bool, allow_docker: bool, workflow_path: Option<std::path::PathBuf>, worktree: bool, mount_ssh: bool) {
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

    // Resolve SSH dir if requested.
    let ssh_dir: Option<std::path::PathBuf> = if mount_ssh {
        match dirs::home_dir() {
            Some(home) => {
                let ssh = home.join(".ssh");
                if ssh.exists() {
                    app.active_tab_mut().push_output(
                        "WARNING: --mount-ssh: mounting host ~/.ssh into container (read-only). Ensure you trust the agent image.".to_string(),
                    );
                    Some(ssh)
                } else {
                    app.active_tab_mut().push_output("Error: host ~/.ssh directory not found; cannot use --mount-ssh.".to_string());
                    app.active_tab_mut().finish_command(1);
                    return;
                }
            }
            None => {
                app.active_tab_mut().push_output("Error: cannot resolve home directory.".to_string());
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    } else {
        None
    };

    // Set up worktree if requested; otherwise use pending mount path.
    let mount_path = if worktree {
        // Validate git version.
        if let Err(e) = crate::git::git_version_check() {
            app.active_tab_mut().push_output(format!("Error: {}", e));
            app.active_tab_mut().finish_command(1);
            return;
        }
        // Warn if HEAD is detached — the worktree branch will be cut from a detached commit.
        if crate::git::is_detached_head(&git_root) {
            app.active_tab_mut().push_output(
                "WARNING: You are in detached HEAD state. The worktree branch will be created \
                 from the current commit. Consider checking out a branch first so the merge \
                 prompt has a target branch."
                    .to_string(),
            );
        }
        let wt_path = match crate::git::worktree_path(&git_root, work_item) {
            Ok(p) => p,
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error creating worktree path: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        };
        let branch = crate::git::worktree_branch_name(work_item);
        // If worktree already exists, reuse it; otherwise create it.
        if wt_path.exists() {
            app.active_tab_mut().push_output(format!("Resuming existing worktree at {}", wt_path.display()));
        } else {
            if let Err(e) = crate::git::create_worktree(&git_root, &wt_path, &branch) {
                app.active_tab_mut().push_output(format!("Error creating worktree: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
            app.active_tab_mut().push_output(format!("Created worktree at {} (branch: {})", wt_path.display(), branch));
        }
        // Store worktree info in tab for post-completion dialog.
        app.active_tab_mut().worktree_branch = Some(branch);
        app.active_tab_mut().worktree_active_path = Some(wt_path.clone());
        app.active_tab_mut().worktree_git_root = Some(git_root.clone());
        wt_path
    } else {
        // Clear any stale worktree state.
        app.active_tab_mut().worktree_branch = None;
        app.active_tab_mut().worktree_active_path = None;
        app.active_tab_mut().worktree_git_root = None;
        app.active_tab_mut().pending_mount_path.take().unwrap_or_else(|| git_root.clone())
    };

    // Auto-passthrough: always pass credentials from keychain if available.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare host settings (sanitized config files in a temp dir).
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));

    // Persist launch context so workflow step-advancement functions can reuse identical settings.
    app.active_tab_mut().workflow_ssh_dir = ssh_dir.clone();
    app.active_tab_mut().workflow_mount_path = Some(mount_path.clone());
    app.active_tab_mut().workflow_allow_docker = allow_docker;

    // If a workflow is specified, initialise/load its state and derive the step prompt.
    let effective_entrypoint: Vec<String>;
    let command_display: String;
    if let Some(ref wf_path) = workflow_path {
        // Resolve relative paths against the tab's working directory so that
        // paths like ./aspec/workflows/implement-feature.md work as expected.
        let resolved_wf_path: std::path::PathBuf = if wf_path.is_absolute() {
            wf_path.clone()
        } else {
            tab_cwd.join(wf_path)
        };
        // Load or resume workflow state.
        let wf_state = match init_workflow_tui(app, &resolved_wf_path, work_item, &git_root, non_interactive, plan) {
            Some(s) => s,
            None => return, // Error already pushed to output.
        };
        // Get the first ready step.
        let ready = wf_state.next_ready();
        if ready.is_empty() {
            if wf_state.all_done() {
                app.active_tab_mut().push_output("All workflow steps are already done.");
            } else {
                app.active_tab_mut().push_output("No workflow steps are ready to run.");
            }
            app.active_tab_mut().finish_command(0);
            return;
        }
        let step_name = ready[0].clone();
        let step_state = wf_state.get_step(&step_name).unwrap().clone();

        // Load work item content for prompt substitution.
        let work_item_content = match find_work_item(&git_root, work_item).and_then(|p| {
            std::fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("{}", e))
        }) {
            Ok(c) => c,
            Err(e) => {
                app.active_tab_mut().push_output(format!("Cannot read work item: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        };

        let prompt = workflow::substitute_prompt(&step_state.prompt_template, work_item, &work_item_content);
        effective_entrypoint = workflow_step_entrypoint(&agent_name, &prompt, non_interactive, plan);
        command_display = format!("implement {:04} [step: {}]", work_item, step_name);

        // Update state: mark step as Running, persist, store in tab.
        let mut wf_state_mut = wf_state;
        wf_state_mut.set_status(&step_name, StepStatus::Running);
        if let Some(ref git_root_path) = app.active_tab().workflow_git_root.clone() {
            let _ = workflow::save_workflow_state(git_root_path, &wf_state_mut);
        }
        app.active_tab_mut().workflow = Some(wf_state_mut);
        app.active_tab_mut().workflow_current_step = Some(step_name);
        app.active_tab_mut().workflow_git_root = Some(git_root.clone());
    } else {
        effective_entrypoint = if non_interactive {
            agent_entrypoint_non_interactive(&agent_name, work_item, plan)
        } else {
            agent_entrypoint(&agent_name, work_item, plan)
        };
        command_display = format!("implement {:04}", work_item);
    }

    let entrypoint = effective_entrypoint;
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);

    // Generate a container name for stats polling.
    let container_name = docker::generate_container_name();

    // Show the full Docker CLI command in the execution window (with masked env values).
    let display_args = if non_interactive {
        docker::build_run_args_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, app.active_tab().host_settings.as_ref(), allow_docker, None, ssh_dir.clone())
    } else {
        docker::build_run_args_pty_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker, ssh_dir.clone())
    };
    let cmd_display = docker::format_run_cmd(&display_args);

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
                None,
                ssh_dir.clone(),
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
            docker::build_run_args_pty(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker, ssh_dir.clone());
        let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

        // Use actual terminal dimensions for the PTY.
        let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
        let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
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
async fn launch_chat(app: &mut App, non_interactive: bool, plan: bool, allow_docker: bool, mount_ssh: bool) {
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

    // Resolve SSH dir if requested.
    let ssh_dir: Option<std::path::PathBuf> = if mount_ssh {
        match dirs::home_dir() {
            Some(home) => {
                let ssh = home.join(".ssh");
                if ssh.exists() {
                    app.active_tab_mut().push_output(
                        "WARNING: --mount-ssh: mounting host ~/.ssh into container (read-only). Ensure you trust the agent image.".to_string(),
                    );
                    Some(ssh)
                } else {
                    app.active_tab_mut().push_output("Error: host ~/.ssh directory not found; cannot use --mount-ssh.".to_string());
                    app.active_tab_mut().finish_command(1);
                    return;
                }
            }
            None => {
                app.active_tab_mut().push_output("Error: cannot resolve home directory.".to_string());
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    } else {
        None
    };

    // Auto-passthrough: always pass credentials from keychain if available.
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    // Prepare host settings (sanitized config files in a temp dir).
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));

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
        docker::build_run_args_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, app.active_tab().host_settings.as_ref(), allow_docker, None, ssh_dir.clone())
    } else {
        docker::build_run_args_pty_display(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker, ssh_dir.clone())
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
                None,
                ssh_dir.clone(),
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
            docker::build_run_args_pty(&image_tag, mount_path.to_str().unwrap(), &entrypoint_refs, &env_vars, Some(&container_name), app.active_tab().host_settings.as_ref(), allow_docker, ssh_dir.clone());
        let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

        // Use actual terminal dimensions for the PTY.
        let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
        let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
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

/// Determine what to show when `claws init` is entered.
///
/// Start the `claws init` workflow.
///
/// If `$HOME/.nanoclaw` already exists, skips the fork/clone wizard and
/// proceeds directly to the image build + audit flow. Otherwise, starts
/// the fork/clone dialog.
async fn show_claws_init_start(app: &mut App) {
    let nanoclaw_dir = claws::nanoclaw_path();
    if nanoclaw_dir.exists() {
        app.active_tab_mut().push_output(format!(
            "Existing nanoclaw installation found at {}. \
             Using existing installation, skipping fork/clone.",
            claws::nanoclaw_path_str()
        ));
        app.active_tab_mut().claws_wizard_username = None;
        launch_claws_ready(app).await;
    } else {
        app.active_tab_mut().dialog = Dialog::ClawsReadyHasForked;
    }
}

/// Determine what to show when `claws ready` is entered (status-only, no wizard).
///
/// - Nanoclaw not installed → show error suggesting `claws init`
/// - Nanoclaw installed, container running → show status table
/// - Nanoclaw installed, container stopped → OfferStart dialog
async fn show_claws_ready_status(app: &mut App) {
    let nanoclaw_dir = claws::nanoclaw_path();

    if !nanoclaw_dir.exists() {
        // Not installed — show error message.
        app.active_tab_mut().start_command("claws ready".to_string());
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
        app.active_tab_mut().exit_rx = Some(exit_rx);
        let tx = app.active_tab().output_tx.clone();
        spawn_text_command(tx, exit_tx, |sink| async move {
            sink.println(
                "nanoclaw is not installed. Run 'claws init' to set up nanoclaw.",
            );
            Ok(())
        });
        return;
    }

    // Nanoclaw is installed — check container state.
    match claws::load_nanoclaw_config() {
        Ok(config) => {
            if let Some(ref id) = config.nanoclaw_container_id {
                if docker::is_container_running(id) {
                    // Container is running — show status table.
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
            // Container not running or no saved ID — check for a stopped one first.
            if let Some(stopped) = docker::find_stopped_container(
                claws::NANOCLAW_CONTROLLER_NAME,
                claws::NANOCLAW_IMAGE_TAG,
            ) {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferRestartStopped {
                    container_id: stopped.id,
                    name: stopped.name,
                    created: stopped.created,
                };
            } else {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferStart;
            }
        }
        Err(_) => {
            // Config unreadable — still check for stopped container.
            if let Some(stopped) = docker::find_stopped_container(
                claws::NANOCLAW_CONTROLLER_NAME,
                claws::NANOCLAW_IMAGE_TAG,
            ) {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferRestartStopped {
                    container_id: stopped.id,
                    name: stopped.name,
                    created: stopped.created,
                };
            } else {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferStart;
            }
        }
    }
}

/// Attach to the running nanoclaw container for a freeform chat session (TUI mode).
///
/// If the container is not running, shows an error suggesting `claws ready`.
async fn launch_claws_chat_attach(app: &mut App) {
    let nanoclaw_dir = claws::nanoclaw_path();

    if !nanoclaw_dir.exists() {
        app.active_tab_mut().input_error = Some(
            "nanoclaw is not installed. Run 'claws init' to set up nanoclaw.".into(),
        );
        return;
    }

    let config = match claws::load_nanoclaw_config() {
        Ok(c) => c,
        Err(_) => {
            app.active_tab_mut().input_error = Some(
                "Failed to load nanoclaw config. Run 'claws ready' to check status.".into(),
            );
            return;
        }
    };

    let container_id = match config.nanoclaw_container_id {
        Some(ref id) if docker::is_container_running(id) => id.clone(),
        _ => {
            // Container not running — check for a stopped one and offer to start.
            app.active_tab_mut().claws_attach_after_start = true;
            if let Some(stopped) = docker::find_stopped_container(
                claws::NANOCLAW_CONTROLLER_NAME,
                claws::NANOCLAW_IMAGE_TAG,
            ) {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferRestartStopped {
                    container_id: stopped.id,
                    name: stopped.name,
                    created: stopped.created,
                };
            } else {
                app.active_tab_mut().dialog = Dialog::ClawsReadyOfferStart;
            }
            return;
        }
    };

    app.active_tab_mut().start_command("claws chat".to_string());
    launch_claws_exec(app, container_id).await;
}

/// Phase 1 of the claws init wizard (TUI mode): clone + initial image build.
///
/// Runs the clone and pre-audit image build as a background text command. When it
/// completes successfully, `check_claws_continuation` detects `ClawsPhase::PreAudit`
/// and launches the audit agent via PTY container window.
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
    // Stored in tab.host_settings so the temp dir outlives all phases of the wizard
    // and remains valid through the subsequent PTY exec session.
    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));
    // A path-only view is moved into the closure; the actual TempDir lives in the tab.
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    app.active_tab_mut().claws_phase = ClawsPhase::PreAudit;
    app.active_tab_mut().start_command("claws init".to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    // Channel: pre-audit task → TUI — delivers ClawsAuditCtx when initial build succeeds.
    let (audit_ctx_tx, audit_ctx_rx) =
        tokio::sync::oneshot::channel::<claws::ClawsAuditCtx>();
    app.active_tab_mut().claws_audit_ctx_rx = Some(audit_ctx_rx);

    // Channels for the background task to request sudo permission when the clone
    // destination ($HOME/.nanoclaw) is not writable by the current user.
    let (sudo_request_tx, sudo_request_rx) = tokio::sync::oneshot::channel::<()>();
    let (sudo_response_tx, sudo_response_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.active_tab_mut().claws_sudo_request_rx = Some(sudo_request_rx);
    app.active_tab_mut().claws_sudo_response_tx = Some(sudo_response_tx);

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

        // Pre-audit: Docker check + Dockerfile.dev + initial image build.
        let ctx = claws::build_nanoclaw_pre_audit(
            &sink,
            env_vars,
            &mut summary,
            closure_host_settings.as_ref(),
        ).await?;

        sink.println("Audit agent launching in container window...");
        let _ = audit_ctx_tx.send(ctx);
        Ok(())
    });
}

/// Phase 2 of the claws init wizard (TUI mode): /setup + docker socket dialogs,
/// background container launch, and detached audit agent exec.
///
/// Called by the `ClawsAuditConfirmAccept` action handler (user accepted the audit
/// explanation dialog) after the pre-audit text task completes.
async fn launch_claws_init_post_audit(app: &mut App) {
    let ctx = match app.active_tab_mut().claws_audit_ctx.take() {
        Some(ctx) => ctx,
        None => {
            app.active_tab_mut().push_output(
                "Internal error: missing audit context for post-audit phase.".to_string(),
            );
            app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
            return;
        }
    };

    // Retain a clone of ctx so the PTY exec phase (PostAudit continuation) can build
    // the audit entrypoint after the text task completes.
    app.active_tab_mut().claws_audit_ctx = Some(ctx.clone());

    // Path-only clone of host_settings for the background closure.
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    // Channel: container ID sent back to TUI so check_claws_continuation can open the PTY.
    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);

    // Channels for docker socket acceptance dialog.
    let (docker_accept_request_tx, docker_accept_request_rx) = tokio::sync::oneshot::channel::<()>();
    let (docker_accept_response_tx, docker_accept_response_rx) = tokio::sync::oneshot::channel::<bool>();
    app.active_tab_mut().claws_docker_accept_request_rx = Some(docker_accept_request_rx);
    app.active_tab_mut().claws_docker_accept_response_tx = Some(docker_accept_response_tx);

    app.active_tab_mut().claws_phase = ClawsPhase::PostAudit;
    app.active_tab_mut().continue_command("claws init".to_string());

    spawn_text_command(tx, exit_tx, move |sink| async move {
        let mut summary = claws::ClawsSummary::default();

        // Signal the TUI to show the docker socket warning dialog.
        if docker_accept_request_tx.send(()).is_err() {
            anyhow::bail!("Docker socket warning channel closed unexpectedly.");
        }
        if !docker_accept_response_rx.await.unwrap_or(false) {
            anyhow::bail!("Docker socket access declined. Cannot launch nanoclaw container.");
        }

        // Launch background nanoclaw container (sleep loop) with docker socket.
        let container_id = claws::launch_nanoclaw_container(
            &sink,
            &ctx.env_vars,
            &mut summary,
            closure_host_settings.as_ref(),
        ).await?;

        // Send container ID back — check_claws_continuation will open a foreground
        // PTY exec session with the audit prompt.
        let _ = container_tx.send(container_id);
        Ok(())
    });
}

/// Start a fresh nanoclaw container in the background (TUI mode).
///
/// Used by the `ClawsReadyOfferStart` dialog (both from `claws ready` and
/// `claws chat`). Delivers the container ID via `claws_container_id_rx` so that
/// `check_claws_continuation` can attach if `claws_attach_after_start` is set.
async fn launch_claws_start_container_status_only(app: &mut App) {
    let agent_name = {
        let config = load_repo_config(&claws::nanoclaw_path()).unwrap_or_default();
        config.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    let settings_dir = claws::nanoclaw_settings_dir();
    app.active_tab_mut().host_settings = docker::HostSettings::prepare_to_dir(&agent_name, &settings_dir);
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    app.active_tab_mut().claws_phase = ClawsPhase::Setup;
    let command_label = if app.active_tab().claws_attach_after_start {
        "claws chat"
    } else {
        "claws ready"
    };
    app.active_tab_mut().start_command(command_label.to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);

    spawn_text_command(tx, exit_tx, move |sink| async move {
        let nanoclaw_str = claws::nanoclaw_path_str();
        sink.println(format!("Starting nanoclaw controller container {}...", claws::NANOCLAW_CONTROLLER_NAME));

        let container_id = docker::run_container_detached(
            claws::NANOCLAW_IMAGE_TAG,
            &nanoclaw_str,
            &nanoclaw_str,
            &nanoclaw_str,
            Some(claws::NANOCLAW_CONTROLLER_NAME),
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

/// Restart a stopped nanoclaw container (TUI mode).
///
/// Calls `docker start` on the given container ID, waits for it to be running,
/// saves the ID to the nanoclaw config, and then attaches if
/// `claws_attach_after_start` is set.
async fn launch_claws_restart_stopped_container(app: &mut App, container_id: String) {
    app.active_tab_mut().claws_phase = ClawsPhase::Setup;
    app.active_tab_mut().claws_restarting_container_id = Some(container_id.clone());
    let command_label = if app.active_tab().claws_attach_after_start {
        "claws chat"
    } else {
        "claws ready"
    };
    app.active_tab_mut().start_command(command_label.to_string());

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);

    let cid = container_id.clone();
    spawn_text_command(tx, exit_tx, move |sink| async move {
        sink.println(format!(
            "Starting stopped container {}...",
            &cid[..cid.len().min(12)],
        ));
        if let Err(e) = docker::start_container(&cid) {
            sink.println(String::new());
            sink.println(format!("Docker error: {}", e));
            sink.println(String::new());
            sink.println("The bind-mount sources (e.g. claude.json) may have been cleaned up");
            sink.println("since the container was created.");
            anyhow::bail!("Failed to start container: {}", e);
        }

        sink.print("Waiting for container to start... ");
        if !claws::wait_for_container(&cid, 5) {
            sink.println("TIMEOUT");
            anyhow::bail!("Container did not start within 5 seconds.");
        }
        sink.println("OK");

        let mut config = claws::load_nanoclaw_config().unwrap_or_default();
        config.nanoclaw_container_id = Some(cid.clone());
        claws::save_nanoclaw_config(&config)?;

        let _ = container_tx.send(cid);
        Ok(())
    });
}

/// Delete a stopped container and start a fresh nanoclaw container (TUI mode).
async fn launch_claws_delete_and_start_fresh(app: &mut App, container_id: String) {
    app.active_tab_mut().claws_restarting_container_id = None;
    app.active_tab_mut().claws_phase = ClawsPhase::Setup;
    let command_label = if app.active_tab().claws_attach_after_start {
        "claws chat"
    } else {
        "claws ready"
    };
    app.active_tab_mut().start_command(command_label.to_string());

    let agent_name = {
        let config = load_repo_config(&claws::nanoclaw_path()).unwrap_or_default();
        config.agent.unwrap_or_else(|| "claude".to_string())
    };
    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    let settings_dir = claws::nanoclaw_settings_dir();
    app.active_tab_mut().host_settings = docker::HostSettings::prepare_to_dir(&agent_name, &settings_dir);
    let closure_host_settings = app.active_tab().host_settings.as_ref().map(|hs| {
        docker::HostSettings::from_paths(hs.config_path.clone(), hs.claude_dir_path.clone())
    });

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    app.active_tab_mut().exit_rx = Some(exit_rx);
    let tx = app.active_tab().output_tx.clone();

    let (container_tx, container_rx) = tokio::sync::oneshot::channel::<String>();
    app.active_tab_mut().claws_container_id_rx = Some(container_rx);

    spawn_text_command(tx, exit_tx, move |sink| async move {
        sink.println(format!(
            "Deleting stopped container {}...",
            &container_id[..container_id.len().min(12)],
        ));
        docker::remove_container(&container_id)?;
        sink.println("OK");

        let nanoclaw_str = claws::nanoclaw_path_str();
        sink.println(format!(
            "Starting fresh nanoclaw container {}...",
            claws::NANOCLAW_CONTROLLER_NAME,
        ));
        let new_container_id = docker::run_container_detached(
            claws::NANOCLAW_IMAGE_TAG,
            &nanoclaw_str,
            &nanoclaw_str,
            &nanoclaw_str,
            Some(claws::NANOCLAW_CONTROLLER_NAME),
            &env_vars,
            true,
            closure_host_settings.as_ref(),
        )?;

        sink.print("Waiting for container to start... ");
        if !claws::wait_for_container(&new_container_id, 5) {
            sink.println("TIMEOUT");
            anyhow::bail!("Container did not start within 5 seconds.");
        }
        sink.println("OK");

        let mut config = claws::load_nanoclaw_config().unwrap_or_default();
        config.nanoclaw_container_id = Some(new_container_id.clone());
        claws::save_nanoclaw_config(&config)?;

        let _ = container_tx.send(new_container_id);
        Ok(())
    });
}

/// Check if the claws workflow phase just completed and advance to the next phase.
async fn check_claws_continuation(app: &mut App) {
    let phase = app.active_tab().claws_phase.clone();
    match phase {
        ClawsPhase::Inactive => {}

        ClawsPhase::Setup => {
            if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
                // If this was a restart attempt, offer to delete and start fresh.
                let restarting_id = app.active_tab_mut().claws_restarting_container_id.take();
                let tab = app.active_tab_mut();
                tab.claws_phase = ClawsPhase::Inactive;
                tab.claws_container_id = None;
                tab.claws_container_id_rx = None;
                tab.claws_attach_after_start = false;
                if let Some(container_id) = restarting_id {
                    tab.dialog = Dialog::ClawsRestartFailedOfferFresh { container_id };
                }
                return;
            }
            // Container ID is delivered via tick() into claws_container_id.
            if let Some(container_id) = app.active_tab_mut().claws_container_id.take() {
                let attach = app.active_tab().claws_attach_after_start;
                app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
                app.active_tab_mut().claws_container_id_rx = None;
                app.active_tab_mut().claws_attach_after_start = false;
                if attach {
                    // Originated from `claws chat` — attach immediately.
                    launch_claws_exec(app, container_id).await;
                } else {
                    // Originated from `claws ready` — just report status.
                    app.active_tab_mut().push_output(
                        "nanoclaw container started. Run 'claws chat' to attach.".to_string(),
                    );
                }
            } else {
                // Task completed but no container ID yet — stay in Setup until tick delivers it.
            }
        }

        ClawsPhase::PreAudit => {
            // Pre-audit text task finished. If it failed, abort the wizard.
            if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
                let tab = app.active_tab_mut();
                tab.claws_phase = ClawsPhase::Inactive;
                tab.claws_audit_ctx = None;
                tab.claws_audit_ctx_rx = None;
                return;
            }
            // Audit context should have arrived via tick() by now.
            if let Some(ctx) = app.active_tab_mut().claws_audit_ctx.take() {
                // Show audit explanation dialog — user confirms before post-audit proceeds.
                // ctx is stored in claws_audit_ctx; the action handler will take it.
                app.active_tab_mut().claws_audit_ctx = Some(ctx);
                app.active_tab_mut().dialog = Dialog::ClawsAuditConfirm;
            } else {
                app.active_tab_mut().push_output(
                    "Internal error: pre-audit completed but no audit context received.".to_string(),
                );
                app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
            }
        }

        ClawsPhase::PostAudit => {
            // Post-audit text task finished. If it failed, abort.
            if matches!(app.active_tab().phase, state::ExecutionPhase::Error { .. }) {
                let tab = app.active_tab_mut();
                tab.claws_phase = ClawsPhase::Inactive;
                tab.claws_container_id = None;
                tab.claws_container_id_rx = None;
                return;
            }
            // Container ID is delivered via tick() into claws_container_id.
            if let Some(container_id) = app.active_tab_mut().claws_container_id.take() {
                let ctx = app.active_tab_mut().claws_audit_ctx.take();
                app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
                app.active_tab_mut().claws_container_id_rx = None;
                if let Some(ctx) = ctx {
                    // Open a foreground PTY exec with the audit prompt — user watches the
                    // audit, then runs /setup in the same session. Container stays running
                    // after the agent exits.
                    launch_claws_exec_audit(app, container_id, ctx).await;
                } else {
                    app.active_tab_mut().push_output(
                        "nanoclaw container started. Run 'claws chat' to attach.".to_string(),
                    );
                }
            } else {
                // Post-audit completed but no container ID.
                app.active_tab_mut().push_output(
                    "Internal error: post-audit completed but no container ID received.".to_string(),
                );
                app.active_tab_mut().claws_phase = ClawsPhase::Inactive;
                app.active_tab_mut().claws_container_id_rx = None;
            }
        }
    }
}

/// Open a foreground PTY exec session inside the nanoclaw controller container with
/// the audit prompt as the initial agent message.
///
/// The user watches the agent configure nanoclaw, then can run `/setup` in the same
/// session. The container keeps running after the agent exits.
async fn launch_claws_exec_audit(app: &mut App, container_id: String, ctx: claws::ClawsAuditCtx) {
    let entrypoint = claws::claws_init_audit_entrypoint(&ctx.agent_name);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let exec_args = docker::build_exec_args_pty(
        &container_id,
        &claws::nanoclaw_path_str(),
        &entrypoint_refs,
        &ctx.env_vars,
    );
    let exec_str_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let container_name = claws::NANOCLAW_CONTROLLER_NAME.to_string();
    let display_name = state::agent_display_name(&ctx.agent_name).to_string();

    app.active_tab_mut().continue_command("claws init (agent)".to_string());
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    match PtySession::spawn("docker", &exec_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to launch agent: {}", e));
            app.active_tab_mut().finish_command(1);
        }
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
    let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let container_name = claws::NANOCLAW_CONTROLLER_NAME.to_string();
    let display_name = state::agent_display_name(&agent_name).to_string();

    app.active_tab_mut().continue_command("claws chat".to_string());
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

/// Launch `specs new --interview`: create the work item file, then show the interview summary dialog.
async fn launch_new_interview(app: &mut App, kind: WorkItemKind, title: String) {
    use crate::commands::new::create_file_return_number;
    use crate::commands::output::OutputSink;
    let tab_cwd = app.active_tab().cwd.clone();
    let out = OutputSink::Channel(app.active_tab().output_tx.clone());
    app.active_tab_mut().start_command("specs new --interview".to_string());
    match create_file_return_number(&out, kind.clone(), title.clone(), &tab_cwd).await {
        Ok(number) => {
            drop(out);
            app.active_tab_mut().finish_command(0);
            app.active_tab_mut().dialog = state::Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number: number,
                summary: String::new(),
                cursor_pos: 0,
            };
        }
        Err(e) => {
            drop(out);
            app.active_tab_mut().finish_command(1);
            app.active_tab_mut().input_error = Some(format!("Failed to create work item: {}", e));
        }
    }
}

/// Launch the specs amend agent via PTY.
async fn launch_specs_amend(app: &mut App, work_item: u32, allow_docker: bool) {
    let tab_cwd = app.active_tab().cwd.clone();
    let git_root = match find_git_root_from(&tab_cwd) {
        Some(r) => r,
        None => {
            app.active_tab_mut().input_error = Some("Not inside a Git repository.".into());
            return;
        }
    };

    if let Err(e) = find_work_item(&git_root, work_item) {
        app.active_tab_mut().input_error = Some(format!("{}", e));
        return;
    }

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent_name = config.agent.as_deref().unwrap_or("claude").to_string();
    let mount_path = app.active_tab_mut().pending_mount_path.take().unwrap_or_else(|| git_root.clone());

    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));

    let entrypoint = amend_agent_entrypoint(&agent_name, work_item);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);
    let container_name = docker::generate_container_name();

    let display_args = docker::build_run_args_pty_display(
        &image_tag,
        mount_path.to_str().unwrap(),
        &entrypoint_refs,
        &env_vars,
        Some(&container_name),
        app.active_tab().host_settings.as_ref(),
        allow_docker,
        None,
    );
    let cmd_display = docker::format_run_cmd(&display_args);

    let command_display = format!("specs amend {:04}", work_item);
    app.active_tab_mut().start_command(command_display);

    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    app.active_tab_mut().push_output(format!("$ {}", cmd_display));

    let sink = crate::commands::output::OutputSink::Channel(app.active_tab().output_tx.clone());
    print_interactive_notice(&sink, &agent_name);

    let docker_args = docker::build_run_args_pty(
        &image_tag,
        mount_path.to_str().unwrap(),
        &entrypoint_refs,
        &env_vars,
        Some(&container_name),
        app.active_tab().host_settings.as_ref(),
        allow_docker,
        None,
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let display_name = state::agent_display_name(&agent_name).to_string();
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to launch container: {}", e));
            app.active_tab_mut().finish_command(1);
        }
    }
}

/// Launch the specs interview agent via PTY.
async fn launch_specs_interview_agent(
    app: &mut App,
    work_item_number: u32,
    kind: WorkItemKind,
    title: String,
    summary: String,
    allow_docker: bool,
) {
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

    let credentials = agent_keychain_credentials(&agent_name);
    let env_vars = credentials.env_vars;

    app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent_name)
        .or_else(|| docker::HostSettings::prepare_minimal(&agent_name));

    let entrypoint = interview_agent_entrypoint(&agent_name, work_item_number, &kind, &title, &summary);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);
    let container_name = docker::generate_container_name();

    let display_args = docker::build_run_args_pty_display(
        &image_tag,
        mount_path.to_str().unwrap(),
        &entrypoint_refs,
        &env_vars,
        Some(&container_name),
        app.active_tab().host_settings.as_ref(),
        allow_docker,
        None,
    );
    let cmd_display = docker::format_run_cmd(&display_args);

    let command_display = format!("specs new --interview {:04}", work_item_number);
    app.active_tab_mut().start_command(command_display);

    if allow_docker {
        match docker::check_docker_socket() {
            Ok(socket_path) => {
                app.active_tab_mut().push_output(format!("Docker socket: {} (found)", socket_path.display()));
            }
            Err(e) => {
                app.active_tab_mut().push_output(format!("Error: {}", e));
                app.active_tab_mut().finish_command(1);
                return;
            }
        }
    }

    app.active_tab_mut().push_output(format!("$ {}", cmd_display));

    let sink = crate::commands::output::OutputSink::Channel(app.active_tab().output_tx.clone());
    print_interactive_notice(&sink, &agent_name);

    let docker_args = docker::build_run_args_pty(
        &image_tag,
        mount_path.to_str().unwrap(),
        &entrypoint_refs,
        &env_vars,
        Some(&container_name),
        app.active_tab().host_settings.as_ref(),
        allow_docker,
        None,
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let wf_strip_h = app.active_tab().workflow.as_ref().map(|wf| workflow_strip_height(wf)).unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let display_name = state::agent_display_name(&agent_name).to_string();
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to launch container: {}", e));
            app.active_tab_mut().finish_command(1);
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

// ─── Multi-step workflow helpers ──────────────────────────────────────────────

/// Initialise or resume workflow state for TUI mode.
///
/// On error, pushes a message to the active tab's output and returns `None`.
fn init_workflow_tui(
    app: &mut App,
    wf_path: &std::path::Path,
    work_item: u32,
    git_root: &std::path::Path,
    _non_interactive: bool,
    _plan: bool,
) -> Option<crate::workflow::WorkflowState> {
    let (hash, title, steps) = match workflow::load_workflow_file(wf_path) {
        Ok(v) => v,
        Err(e) => {
            app.active_tab_mut().push_output(format!("Workflow error: {}", e));
            app.active_tab_mut().finish_command(1);
            return None;
        }
    };

    let workflow_name = wf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow")
        .to_string();

    let state_path = workflow::workflow_state_path(git_root, work_item, &workflow_name);

    let state = if state_path.exists() {
        match workflow::load_workflow_state(&state_path) {
            Ok(existing) => {
                // Hash mismatch or same hash — just try to resume.
                if existing.workflow_hash != hash {
                    app.active_tab_mut().push_output(
                        "Warning: workflow file changed since last run. Restarting from beginning.".to_string(),
                    );
                    let _ = std::fs::remove_file(&state_path);
                    crate::workflow::WorkflowState::new(title, steps, hash, work_item, workflow_name)
                } else {
                    app.active_tab_mut().push_output("Resuming previous workflow run.".to_string());
                    existing
                }
            }
            Err(_) => {
                crate::workflow::WorkflowState::new(title, steps, hash, work_item, workflow_name)
            }
        }
    } else {
        crate::workflow::WorkflowState::new(title, steps, hash, work_item, workflow_name)
    };

    // Persist state.
    if let Err(e) = workflow::save_workflow_state(git_root, &state) {
        app.active_tab_mut().push_output(format!("Cannot save workflow state: {}", e));
    }

    Some(state)
}

/// Called after a command completes: if a workflow step just finished, show the
/// confirm/error dialog for the next step.
async fn check_workflow_step_completion(app: &mut App) {
    let has_workflow = app.active_tab().workflow.is_some();
    let current_step = app.active_tab().workflow_current_step.clone();

    if !has_workflow || current_step.is_none() {
        return;
    }

    let step_name = current_step.unwrap();
    let phase = app.active_tab().phase.clone();

    match phase {
        state::ExecutionPhase::Done { .. } => {
            // Mark step as Done.
            if let Some(ref mut wf) = app.active_tab_mut().workflow {
                wf.set_status(&step_name, StepStatus::Done);
            }
            if let (Some(wf), Some(git_root)) = (
                app.active_tab().workflow.clone(),
                app.active_tab().workflow_git_root.clone(),
            ) {
                let _ = workflow::save_workflow_state(&git_root, &wf);
                let next_steps = wf.next_ready();
                if wf.all_done() {
                    app.active_tab_mut().push_output(format!(
                        "Workflow step '{}' complete. All steps done!", step_name
                    ));
                    app.active_tab_mut().workflow_current_step = None;
                    // Clean up state file.
                    let state_path = workflow::workflow_state_path(&git_root, wf.work_item, &wf.workflow_name);
                    let _ = std::fs::remove_file(state_path);
                } else if next_steps.is_empty() {
                    app.active_tab_mut().push_output(format!(
                        "Workflow step '{}' complete but no steps are ready.", step_name
                    ));
                    app.active_tab_mut().workflow_current_step = None;
                } else {
                    app.active_tab_mut().dialog = Dialog::WorkflowStepConfirm {
                        completed_step: step_name,
                        next_steps,
                    };
                }
            }
        }
        state::ExecutionPhase::Error { exit_code, .. } => {
            // Mark step as Error.
            let error_msg = format!("Container exited with code {}", exit_code);
            if let Some(ref mut wf) = app.active_tab_mut().workflow {
                wf.set_status(&step_name, StepStatus::Error(error_msg.clone()));
            }
            if let (Some(wf), Some(git_root)) = (
                app.active_tab().workflow.clone(),
                app.active_tab().workflow_git_root.clone(),
            ) {
                let _ = workflow::save_workflow_state(&git_root, &wf);
            }
            app.active_tab_mut().dialog = Dialog::WorkflowStepError {
                failed_step: step_name,
                error: error_msg,
            };
        }
        _ => {}
    }
}

/// Launch the next ready workflow step (called after user confirms advancing).
async fn launch_next_workflow_step(app: &mut App) {
    let (wf_state, git_root, work_item, agent_name, allow_docker, ssh_dir, mount_path) = {
        let tab = app.active_tab();
        let wf = match tab.workflow.clone() {
            Some(w) => w,
            None => return,
        };
        let git_root = match tab.workflow_git_root.clone() {
            Some(r) => r,
            None => return,
        };
        let config = load_repo_config(&git_root).unwrap_or_default();
        let agent = config.agent.as_deref().unwrap_or("claude").to_string();
        // Use the launch-time mount path (worktree or repo root) for all subsequent steps.
        let mount_path = tab.workflow_mount_path.clone().unwrap_or_else(|| git_root.clone());
        (
            wf,
            git_root,
            tab.workflow.as_ref().map(|w| w.work_item).unwrap_or(0),
            agent,
            tab.workflow_allow_docker,
            tab.workflow_ssh_dir.clone(),
            mount_path,
        )
    };

    let ready = wf_state.next_ready();
    if ready.is_empty() {
        return;
    }

    let step_name = ready[0].clone();
    let step_state = wf_state.get_step(&step_name).unwrap().clone();

    // Load work item content.
    let work_item_content = match find_work_item(&git_root, work_item).and_then(|p| {
        std::fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("{}", e))
    }) {
        Ok(c) => c,
        Err(e) => {
            app.active_tab_mut().push_output(format!("Cannot read work item: {}", e));
            return;
        }
    };

    let config = load_repo_config(&git_root).unwrap_or_default();
    let agent = config.agent.as_deref().unwrap_or("claude").to_string();
    let credentials = agent_keychain_credentials(&agent);
    let env_vars = credentials.env_vars;

    let prompt = workflow::substitute_prompt(&step_state.prompt_template, work_item, &work_item_content);
    let entrypoint = workflow_step_entrypoint(&agent_name, &prompt, false, false);
    let entrypoint_refs: Vec<&str> = entrypoint.iter().map(String::as_str).collect();

    let image_tag = docker::project_image_tag(&git_root);
    let container_name = docker::generate_container_name();

    if app.active_tab().host_settings.is_none() {
        app.active_tab_mut().host_settings = docker::HostSettings::prepare(&agent)
            .or_else(|| docker::HostSettings::prepare_minimal(&agent));
    }
    let host_settings_ref = app.active_tab().host_settings.as_ref();

    let docker_args = docker::build_run_args_pty(
        &image_tag,
        mount_path.to_str().unwrap_or("."),
        &entrypoint_refs,
        &env_vars,
        Some(&container_name),
        host_settings_ref,
        allow_docker,
        ssh_dir,
    );
    let docker_str_refs: Vec<&str> = docker_args.iter().map(String::as_str).collect();

    let command_display = format!("implement {:04} [step: {}]", work_item, step_name);
    app.active_tab_mut().continue_command(command_display);
    app.active_tab_mut().push_output(format!("--- Workflow step: {} ---", step_name));

    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let wf_strip_h = app.active_tab().workflow.as_ref()
        .map(|wf| workflow_strip_height(wf))
        .unwrap_or(0);
    let (inner_cols, inner_rows) = calculate_container_inner_size(term_cols, term_rows, wf_strip_h);
    let size = PtySize {
        rows: inner_rows,
        cols: inner_cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let display_name = state::agent_display_name(&agent).to_string();
    app.active_tab_mut().start_container(container_name.clone(), display_name, inner_cols, inner_rows);

    // Record container name in workflow state for persistence.
    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_container_id(&step_name, container_name.clone());
    }

    // Mark the step as Running and persist.
    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_status(&step_name, StepStatus::Running);
    }
    if let (Some(wf), Some(gr)) = (app.active_tab().workflow.clone(), app.active_tab().workflow_git_root.clone()) {
        let _ = workflow::save_workflow_state(&gr, &wf);
    }
    app.active_tab_mut().workflow_current_step = Some(step_name);

    match PtySession::spawn("docker", &docker_str_refs, size) {
        Ok((session, pty_rx)) => {
            app.active_tab_mut().pty = Some(session);
            app.active_tab_mut().pty_rx = Some(pty_rx);
            app.active_tab_mut().stats_rx = Some(spawn_stats_poller(container_name));
        }
        Err(e) => {
            app.active_tab_mut().push_output(format!("Failed to launch container: {}", e));
            app.active_tab_mut().finish_command(1);
        }
    }
}

/// Abort the current workflow: clear workflow state from tab.
fn abort_workflow(app: &mut App) {
    app.active_tab_mut().push_output("Workflow paused. Run again to resume.".to_string());
    app.active_tab_mut().workflow_current_step = None;
    // Keep `workflow` state so the user can resume later.
}

/// Retry the failed workflow step.
async fn retry_workflow_step(app: &mut App) {
    let step_name = app.active_tab().workflow_current_step.clone();
    if let Some(ref step_name) = step_name {
        if let Some(ref mut wf) = app.active_tab_mut().workflow {
            wf.set_status(step_name, StepStatus::Pending);
        }
    }
    if let (Some(wf), Some(git_root)) = (app.active_tab().workflow.clone(), app.active_tab().workflow_git_root.clone()) {
        let _ = workflow::save_workflow_state(&git_root, &wf);
    }
    // Re-launch via advance.
    launch_next_workflow_step(app).await;
}

/// Handle the all-done / no-next-ready case after marking a step Done.
///
/// Returns `true` if the workflow is complete or stalled (caller should not launch next step),
/// `false` if there are ready steps to launch.
fn mark_workflow_complete_if_needed(app: &mut App, current_step: &str) -> bool {
    if let (Some(wf), Some(git_root)) = (app.active_tab().workflow.clone(), app.active_tab().workflow_git_root.clone()) {
        let _ = workflow::save_workflow_state(&git_root, &wf);
        if wf.all_done() {
            app.active_tab_mut().push_output(format!(
                "Workflow step '{}' complete. All steps done!", current_step
            ));
            app.active_tab_mut().workflow_current_step = None;
            let state_path = workflow::workflow_state_path(&git_root, wf.work_item, &wf.workflow_name);
            let _ = std::fs::remove_file(state_path);
            return true;
        }
        if wf.next_ready().is_empty() {
            app.active_tab_mut().push_output(format!(
                "Workflow step '{}' complete but no steps are ready.", current_step
            ));
            app.active_tab_mut().workflow_current_step = None;
            return true;
        }
    }
    false
}

/// Cancel the current step and return to the previous (most recently Done) step.
async fn cancel_to_previous_step(app: &mut App) {
    let current_step = match app.active_tab().workflow_current_step.clone() {
        Some(s) => s,
        None => return,
    };

    // Mark current step Pending (undo Running status).
    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_status(&current_step, StepStatus::Pending);
    }

    // Find predecessor: scan steps in reverse, find last Done step.
    let predecessor = app.active_tab().workflow.as_ref().and_then(|wf| {
        wf.steps.iter().rev().find(|s| s.status == StepStatus::Done).map(|s| s.name.clone())
    });

    if let Some(pred_name) = predecessor {
        // Mark predecessor Pending so it can be re-run.
        if let Some(ref mut wf) = app.active_tab_mut().workflow {
            wf.set_status(&pred_name, StepStatus::Pending);
        }
        if let (Some(wf), Some(git_root)) = (app.active_tab().workflow.clone(), app.active_tab().workflow_git_root.clone()) {
            let _ = workflow::save_workflow_state(&git_root, &wf);
        }
        launch_next_workflow_step(app).await;
    } else {
        // No predecessor: revert current step to Running and reopen dialog with error.
        if let Some(ref mut wf) = app.active_tab_mut().workflow {
            wf.set_status(&current_step, StepStatus::Running);
        }
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step,
            error: Some("No previous step to return to".into()),
        };
    }
}

/// Mark the current workflow step Done and advance to the next step in a new container.
async fn advance_workflow_next_new_container(app: &mut App) {
    let current_step = match app.active_tab().workflow_current_step.clone() {
        Some(s) => s,
        None => return,
    };

    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_status(&current_step, StepStatus::Done);
    }

    if mark_workflow_complete_if_needed(app, &current_step) {
        return;
    }

    launch_next_workflow_step(app).await;
}

/// Mark the current workflow step Done and send the next step's prompt to the existing PTY.
async fn advance_workflow_next_current_container(app: &mut App) {
    // If PTY is not available, fall back to new container.
    if app.active_tab().pty.is_none() {
        app.active_tab_mut().push_output("PTY session ended — starting new container".to_string());
        advance_workflow_next_new_container(app).await;
        return;
    }

    let current_step = match app.active_tab().workflow_current_step.clone() {
        Some(s) => s,
        None => return,
    };

    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_status(&current_step, StepStatus::Done);
    }

    if mark_workflow_complete_if_needed(app, &current_step) {
        return;
    }

    launch_next_workflow_step_in_current_container(app).await;
}

/// Send the next workflow step's prompt to the existing PTY session (no new container).
async fn launch_next_workflow_step_in_current_container(app: &mut App) {
    debug_assert!(app.active_tab().pty.is_some());
    debug_assert!(app.active_tab().container_info.is_some());

    let (wf_state, git_root, work_item) = {
        let tab = app.active_tab();
        let wf = match tab.workflow.clone() {
            Some(w) => w,
            None => return,
        };
        let git_root = match tab.workflow_git_root.clone() {
            Some(r) => r,
            None => return,
        };
        let work_item = wf.work_item;
        (wf, git_root, work_item)
    };

    let ready = wf_state.next_ready();
    if ready.is_empty() {
        return;
    }

    let step_name = ready[0].clone();
    let step_state = match wf_state.get_step(&step_name) {
        Some(s) => s.clone(),
        None => return,
    };

    // Load work item content for prompt substitution.
    let work_item_content = match find_work_item(&git_root, work_item).and_then(|p| {
        std::fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("{}", e))
    }) {
        Ok(c) => c,
        Err(e) => {
            app.active_tab_mut().push_output(format!("Cannot read work item: {}", e));
            return;
        }
    };

    let prompt = workflow::substitute_prompt(&step_state.prompt_template, work_item, &work_item_content);

    // Send prompt to the existing PTY, followed by a newline (acts as Enter).
    let bytes = format!("{}\n", prompt).into_bytes();
    if let Some(ref pty) = app.active_tab().pty {
        pty.write_bytes(&bytes);
    }

    // Update step status and current step tracking.
    if let Some(ref mut wf) = app.active_tab_mut().workflow {
        wf.set_status(&step_name, StepStatus::Running);
    }
    app.active_tab_mut().workflow_current_step = Some(step_name.clone());

    // Persist state.
    if let (Some(wf), Some(gr)) = (app.active_tab().workflow.clone(), app.active_tab().workflow_git_root.clone()) {
        let _ = workflow::save_workflow_state(&gr, &wf);
    }

    // Maximize the container window so the user sees the PTY output.
    app.active_tab_mut().container_window = ContainerWindowState::Maximized;

    app.active_tab_mut().push_output(format!("--- Workflow step: {} (reusing container) ---", step_name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::{App, Dialog, ExecutionPhase};
    use crate::workflow::{StepStatus, WorkflowState, WorkflowStepState};

    fn new_app() -> App {
        App::new(std::path::PathBuf::new())
    }

    fn make_step_state(name: &str, deps: &[&str], status: StepStatus) -> WorkflowStepState {
        WorkflowStepState {
            name: name.to_string(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            prompt_template: format!("do {}", name),
            status,
            container_id: None,
        }
    }

    fn make_workflow(steps: Vec<WorkflowStepState>) -> WorkflowState {
        WorkflowState {
            title: None,
            steps,
            workflow_hash: "hash".to_string(),
            work_item: 1,
            workflow_name: "test-wf".to_string(),
        }
    }

    // ─── cancel_to_previous_step ────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_to_previous_step_on_first_step_sets_error_dialog() {
        let mut app = new_app();
        // Single step — no predecessor exists.
        let wf = make_workflow(vec![make_step_state("plan", &[], StepStatus::Running)]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("plan".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };

        cancel_to_previous_step(&mut app).await;

        // Step should revert to Running (no predecessor to go back to).
        assert_eq!(
            app.active_tab().workflow.as_ref().unwrap().get_step("plan").unwrap().status,
            StepStatus::Running,
            "First step should revert to Running when no predecessor exists"
        );
        // Dialog should open with an error message.
        match &app.active_tab().dialog {
            Dialog::WorkflowControlBoard { current_step, error } => {
                assert_eq!(current_step, "plan");
                assert!(error.is_some(), "Error message should be set");
                assert!(
                    error.as_ref().unwrap().contains("No previous step"),
                    "Error should mention no previous step: {:?}", error
                );
            }
            other => panic!("Expected WorkflowControlBoard with error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn cancel_to_previous_step_linear_marks_predecessor_pending() {
        let mut app = new_app();
        // Linear: plan (Done) → impl (Running)
        let wf = make_workflow(vec![
            make_step_state("plan", &[], StepStatus::Done),
            make_step_state("impl", &["plan"], StepStatus::Running),
        ]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("impl".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        // No git root → launch_next_workflow_step returns early without spawning Docker.
        app.active_tab_mut().workflow_git_root = None;

        cancel_to_previous_step(&mut app).await;

        let wf = app.active_tab().workflow.as_ref().unwrap();
        assert_eq!(
            wf.get_step("impl").unwrap().status,
            StepStatus::Pending,
            "Current step (impl) should be Pending after cancel"
        );
        assert_eq!(
            wf.get_step("plan").unwrap().status,
            StepStatus::Pending,
            "Predecessor (plan) should revert to Pending"
        );
    }

    #[tokio::test]
    async fn cancel_to_previous_step_parallel_picks_last_done_step() {
        let mut app = new_app();
        // plan (Done) → branch-a (Done), branch-b (Done) → merge (Running)
        let wf = make_workflow(vec![
            make_step_state("plan", &[], StepStatus::Done),
            make_step_state("branch-a", &["plan"], StepStatus::Done),
            make_step_state("branch-b", &["plan"], StepStatus::Done),
            make_step_state("merge", &["branch-a", "branch-b"], StepStatus::Running),
        ]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("merge".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().workflow_git_root = None;

        cancel_to_previous_step(&mut app).await;

        let wf = app.active_tab().workflow.as_ref().unwrap();
        assert_eq!(
            wf.get_step("merge").unwrap().status,
            StepStatus::Pending,
            "merge should be Pending after cancel"
        );
        // The most recent Done step in Vec order (branch-b) should be reverted.
        assert_eq!(
            wf.get_step("branch-b").unwrap().status,
            StepStatus::Pending,
            "branch-b (last Done step) should revert to Pending"
        );
        // Earlier Done steps should remain Done.
        assert_eq!(
            wf.get_step("plan").unwrap().status,
            StepStatus::Done,
            "plan should remain Done"
        );
        assert_eq!(
            wf.get_step("branch-a").unwrap().status,
            StepStatus::Done,
            "branch-a should remain Done"
        );
    }

    // ─── advance_workflow_next_current_container ────────────────────────────────

    #[tokio::test]
    async fn advance_next_current_container_falls_back_when_pty_is_none() {
        let mut app = new_app();
        let wf = make_workflow(vec![
            make_step_state("plan", &[], StepStatus::Running),
            make_step_state("impl", &["plan"], StepStatus::Pending),
        ]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("plan".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        // pty = None (default) — triggers the PTY-unavailable fallback path.
        // No git root → launch_next_workflow_step returns early.

        advance_workflow_next_current_container(&mut app).await;

        assert!(
            app.active_tab().output_lines.iter().any(|l| l.contains("PTY session ended")),
            "Expected PTY fallback message in output. Got: {:?}",
            app.active_tab().output_lines
        );
        // The fallback calls advance_workflow_next_new_container, which marks current step Done.
        assert_eq!(
            app.active_tab().workflow.as_ref().unwrap().get_step("plan").unwrap().status,
            StepStatus::Done,
            "Current step should be marked Done even when falling back"
        );
    }

    // ─── advance_workflow_next_new_container boundary ───────────────────────────

    #[tokio::test]
    async fn advance_next_new_container_final_step_transitions_to_complete() {
        let mut app = new_app();
        // Single-step workflow — completing it makes all_done() true.
        let wf = make_workflow(vec![make_step_state("plan", &[], StepStatus::Running)]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("plan".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        // Use a real temp dir so save_workflow_state succeeds and all_done() is evaluated.
        let tmp = tempfile::tempdir().unwrap();
        app.active_tab_mut().workflow_git_root = Some(tmp.path().to_path_buf());

        advance_workflow_next_new_container(&mut app).await;

        assert!(
            app.active_tab().workflow_current_step.is_none(),
            "workflow_current_step should be cleared after the final step completes"
        );
        assert!(
            app.active_tab().output_lines.iter().any(|l| l.contains("All steps done")),
            "Expected completion message in output. Got: {:?}",
            app.active_tab().output_lines
        );
    }

    // ─── advance_workflow_next_new_container: state file persisted ──────────────

    #[tokio::test]
    async fn advance_next_new_container_persists_state_before_launch() {
        let mut app = new_app();
        let wf = make_workflow(vec![
            make_step_state("plan", &[], StepStatus::Running),
            make_step_state("impl", &["plan"], StepStatus::Pending),
        ]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("plan".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        let tmp = tempfile::tempdir().unwrap();
        app.active_tab_mut().workflow_git_root = Some(tmp.path().to_path_buf());

        advance_workflow_next_new_container(&mut app).await;

        // plan is Done and state file exists (impl is Pending, so not all_done).
        let state_path = crate::workflow::workflow_state_path(tmp.path(), 1, "test-wf");
        assert!(state_path.exists(), "State file should be written before any launch attempt");
        let saved = std::fs::read_to_string(&state_path).unwrap();
        assert!(saved.contains("Done") || saved.contains("done"), "State file should record plan as Done");
    }

    // ─── WorkflowRestartStep action dispatch ───────────────────────────────────

    #[tokio::test]
    async fn workflow_restart_step_resets_step_to_pending() {
        let mut app = new_app();
        let wf = make_workflow(vec![make_step_state("plan", &[], StepStatus::Running)]);
        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("plan".to_string());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        // No git root — launch returns early without Docker.

        // WorkflowRestartStep calls retry_workflow_step which resets to Pending.
        retry_workflow_step(&mut app).await;

        assert_eq!(
            app.active_tab().workflow.as_ref().unwrap().get_step("plan").unwrap().status,
            StepStatus::Pending,
            "Restart should reset step to Pending"
        );
    }
}
