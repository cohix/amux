use crate::commands::new::WorkItemKind;
use crate::tui::state::{App, TabState, ContainerWindowState, Dialog, ExecutionPhase, Focus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use strsim::levenshtein;

/// Describes what the event loop should do after processing a key press.
pub enum Action {
    None,
    /// User submitted a valid command string.
    Submit(String),
    /// Quit has been confirmed.
    QuitConfirmed,
    /// Mount scope dialog: user chose this path.
    MountScopeChosen(PathBuf),
    /// Agent auth dialog: user accepted.
    AuthAccepted,
    /// Agent auth dialog: user declined.
    AuthDeclined,
    /// Forward these raw bytes to the PTY.
    ForwardToPty(Vec<u8>),
    /// New work item: kind and title have been collected.
    NewWorkItem {
        kind: WorkItemKind,
        title: String,
        interview: bool,
    },
    /// New work item interview summary submitted.
    NewInterviewSummarySubmitted {
        kind: WorkItemKind,
        title: String,
        work_item_number: u32,
        summary: String,
    },
    /// Claws first-run wizard completed: proceed with launch.
    ClawsReadyProceed,
    /// Claws subsequent run: start the stopped container.
    ClawsReadyStartContainer,
    /// Claws subsequent run: restart the specific stopped container by ID.
    ClawsReadyRestartStopped { container_id: String },
    /// Claws subsequent run: restart failed — delete the stopped container and start fresh.
    ClawsReadyDeleteAndStartFresh { container_id: String },
    /// Claws audit confirmation accepted: launch the audit agent.
    ClawsAuditConfirmAccept,
    /// Claws audit confirmation declined: cancel the audit (and setup).
    ClawsAuditConfirmDecline,
    // Tab management actions:
    CreateTab,
    SwitchTabLeft,
    SwitchTabRight,
    CloseCurrentTab,
    NewTabDirectoryChosen(PathBuf),
    /// Workflow: advance to the next step.
    WorkflowAdvance,
    /// Workflow: abort the current workflow run.
    WorkflowAbort,
    /// Workflow: retry the failed step.
    WorkflowRetry,
    /// Workflow control board: restart the current step.
    WorkflowRestartStep,
    /// Workflow control board: cancel current step and return to previous step.
    WorkflowCancelToPrevious,
    /// Workflow control board: mark current step done, start next step in a new container.
    WorkflowNextInNewContainer,
    /// Workflow control board: mark current step done, send next step prompt to the existing PTY.
    WorkflowNextInCurrentContainer,
    /// Worktree merge prompt: merge the worktree branch into the current branch.
    WorktreeMerge,
    /// Worktree merge prompt: discard the worktree branch and remove the worktree.
    WorktreeDiscard,
    /// Worktree merge prompt: keep the worktree branch as-is without merging.
    WorktreeSkip,
    /// Worktree commit prompt: commit uncommitted files with the given message.
    WorktreeCommitFiles {
        message: String,
        branch: String,
        worktree_path: PathBuf,
        git_root: PathBuf,
    },
    /// Worktree merge confirm: proceed with squash-merge into current HEAD.
    WorktreeMergeConfirmed {
        branch: String,
        worktree_path: PathBuf,
        git_root: PathBuf,
    },
    /// Worktree delete confirm: remove the worktree directory and branch.
    WorktreeDeleteConfirmed {
        branch: String,
        worktree_path: PathBuf,
        git_root: PathBuf,
    },
    /// Worktree delete confirm: keep the worktree and branch as-is after merging.
    WorktreeKeepAfterMerge,
}

/// Dispatch a key press to the correct handler based on application state.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    // Any key press on the active tab counts as interaction — clear stuck warning.
    // (Tab-switch keys also call acknowledge_stuck on the newly active tab in mod.rs.)
    app.active_tab_mut().acknowledge_stuck();

    // Modal dialogs intercept all input.
    let dialog = app.active_tab().dialog.clone();
    match dialog {
        Dialog::QuitConfirm => return handle_quit_confirm(app.active_tab_mut(), key),
        Dialog::CloseTabConfirm => return handle_close_tab_confirm(app.active_tab_mut(), key),
        Dialog::MountScope { git_root, cwd } => {
            return handle_mount_scope(app.active_tab_mut(), key, git_root, cwd)
        }
        Dialog::AgentAuth { .. } => return handle_agent_auth(app.active_tab_mut(), key),
        Dialog::NewKindSelect { interview } => {
            return handle_new_kind_select(app.active_tab_mut(), key, interview)
        }
        Dialog::NewTitleInput { kind, title, interview } => {
            return handle_new_title_input(app.active_tab_mut(), key, kind, title, interview)
        }
        Dialog::NewInterviewSummary { kind, title, work_item_number, summary, cursor_pos } => {
            return handle_new_interview_summary(
                app.active_tab_mut(),
                key,
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            )
        }
        Dialog::NewTabDirectory { input } => {
            return handle_new_tab_directory(app.active_tab_mut(), key, input)
        }
        Dialog::ClawsAuditConfirm => return handle_claws_audit_confirm(app.active_tab_mut(), key),
        Dialog::ClawsReadyHasForked => return handle_claws_has_forked(app.active_tab_mut(), key),
        Dialog::ClawsReadyUsernameInput { username } => {
            return handle_claws_username_input(app.active_tab_mut(), key, username)
        }
        Dialog::ClawsReadyDockerSocketWarning => {
            return handle_claws_docker_socket_warning(app.active_tab_mut(), key)
        }
        Dialog::ClawsReadyOfferRestartStopped { container_id, .. } => {
            return handle_claws_offer_restart_stopped(app.active_tab_mut(), key, container_id)
        }
        Dialog::ClawsReadyOfferStart => return handle_claws_offer_start(app.active_tab_mut(), key),
        Dialog::ClawsRestartFailedOfferFresh { container_id } => {
            return handle_claws_restart_failed_offer_fresh(app.active_tab_mut(), key, container_id)
        }
        Dialog::ClawsReadySudoConfirm { password } => {
            return handle_claws_sudo_confirm(app.active_tab_mut(), key, password)
        }
        Dialog::WorkflowStepConfirm { completed_step, next_steps } => {
            return handle_workflow_step_confirm(app.active_tab_mut(), key, completed_step, next_steps)
        }
        Dialog::WorkflowStepError { failed_step, error } => {
            return handle_workflow_step_error(app.active_tab_mut(), key, failed_step, error)
        }
        Dialog::WorkflowControlBoard { .. } => {
            return handle_workflow_control_board(app.active_tab_mut(), key)
        }
        Dialog::WorktreeMergePrompt { .. } => {
            return handle_worktree_merge_prompt(app.active_tab_mut(), key)
        }
        Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos } => {
            return handle_worktree_commit_prompt(
                app.active_tab_mut(), key, branch, worktree_path, git_root,
                uncommitted_files, message, cursor_pos,
            )
        }
        Dialog::WorktreeMergeConfirm { branch, worktree_path, git_root } => {
            return handle_worktree_merge_confirm(app.active_tab_mut(), key, branch, worktree_path, git_root)
        }
        Dialog::WorktreeDeleteConfirm { branch, worktree_path, git_root } => {
            return handle_worktree_delete_confirm(app.active_tab_mut(), key, branch, worktree_path, git_root)
        }
        Dialog::None => {}
    }

    // Tab management keys (only when no dialog active).
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('t') => return Action::CreateTab,
            KeyCode::Char('a') => return Action::SwitchTabLeft,
            KeyCode::Char('d') => return Action::SwitchTabRight,
            KeyCode::Char('w') => {
                let tab = app.active_tab();
                // Guard: only open when workflow is running, not maximized, no other dialog.
                if tab.dialog == Dialog::None
                    && tab.workflow.is_some()
                    && tab.workflow_current_step.is_some()
                    && matches!(tab.phase, ExecutionPhase::Running { .. })
                    && tab.container_window != ContainerWindowState::Maximized
                {
                    let step = tab.workflow_current_step.clone().unwrap();
                    app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
                        current_step: step,
                        error: None,
                    };
                }
                return Action::None;
            }
            _ => {}
        }
    }

    let num_tabs = app.tabs.len();
    let tab = app.active_tab_mut();
    match tab.focus {
        Focus::ExecutionWindow => handle_window_key(tab, key),
        Focus::CommandBox => handle_input_key(tab, key, num_tabs),
    }
}

// --- Execution window key handling ---

fn handle_window_key(tab: &mut TabState, key: KeyEvent) -> Action {
    match &tab.phase {
        ExecutionPhase::Running { .. } => {
            // Container window maximized: Esc minimizes instead of going to command box.
            if tab.container_window == ContainerWindowState::Maximized {
                if key.code == KeyCode::Esc {
                    tab.container_window = ContainerWindowState::Minimized;
                    return Action::None;
                }
                // All other keys forwarded to the PTY for full interactivity.
                if let Some(bytes) = key_to_bytes(&key) {
                    return Action::ForwardToPty(bytes);
                }
                return Action::None;
            }

            // Container window minimized: outer window is in focus for scrolling.
            if tab.container_window == ContainerWindowState::Minimized {
                match key.code {
                    KeyCode::Char('c') => {
                        tab.container_window = ContainerWindowState::Maximized;
                        return Action::None;
                    }
                    KeyCode::Up => {
                        let max = tab.output_lines.len();
                        if tab.scroll_offset < max {
                            tab.scroll_offset = tab.scroll_offset.saturating_add(1);
                        }
                    }
                    KeyCode::Down => {
                        tab.scroll_offset = tab.scroll_offset.saturating_sub(1);
                    }
                    KeyCode::Char('b') => {
                        tab.scroll_offset = tab.output_lines.len();
                    }
                    KeyCode::Char('e') => {
                        tab.scroll_offset = 0;
                    }
                    KeyCode::Esc => {
                        tab.focus = Focus::CommandBox;
                    }
                    _ => {}
                }
                return Action::None;
            }

            // No container window: original behavior.
            if key.code == KeyCode::Esc {
                tab.focus = Focus::CommandBox;
                return Action::None;
            }
            // Ctrl-C cancels a running `status --watch` loop.
            // Only intercept if status_watch_cancel_tx is set; otherwise fall through
            // so the byte is forwarded to any active PTY (e.g. an agent container).
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                if tab.status_watch_cancel_tx.take().is_some() {
                    return Action::None;
                }
            }
            // Forward all other keys to the PTY.
            if let Some(bytes) = key_to_bytes(&key) {
                return Action::ForwardToPty(bytes);
            }
        }
        ExecutionPhase::Done { .. } | ExecutionPhase::Error { .. } => {
            match key.code {
                KeyCode::Up => {
                    // Cap at total lines so we don't scroll past the beginning.
                    let max = tab.output_lines.len();
                    if tab.scroll_offset < max {
                        tab.scroll_offset = tab.scroll_offset.saturating_add(1);
                    }
                }
                KeyCode::Down => {
                    tab.scroll_offset = tab.scroll_offset.saturating_sub(1);
                }
                KeyCode::Char('b') => {
                    // Jump to the beginning (oldest output).
                    tab.scroll_offset = tab.output_lines.len();
                }
                KeyCode::Char('e') => {
                    // Jump to the end (newest output).
                    tab.scroll_offset = 0;
                }
                KeyCode::Esc => {
                    tab.focus = Focus::CommandBox;
                }
                _ => {
                    // Any other key refocuses the command box.
                    tab.focus = Focus::CommandBox;
                }
            }
        }
        ExecutionPhase::Idle => {
            tab.focus = Focus::CommandBox;
        }
    }
    Action::None
}

// --- Command input box key handling ---

fn handle_input_key(tab: &mut TabState, key: KeyEvent, num_tabs: usize) -> Action {
    // Ctrl+C → close tab (if multiple tabs open) or quit confirm (single tab).
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        if num_tabs > 1 {
            tab.dialog = Dialog::CloseTabConfirm;
        } else {
            tab.dialog = Dialog::QuitConfirm;
        }
        return Action::None;
    }

    // Up arrow navigates to the execution window regardless of phase.
    if key.code == KeyCode::Up {
        if !tab.output_lines.is_empty() {
            tab.focus = Focus::ExecutionWindow;
        }
        return Action::None;
    }

    // When a command is running, the command box is view-only (block editing input).
    if matches!(tab.phase, ExecutionPhase::Running { .. }) {
        return Action::None;
    }

    if key.code == KeyCode::Char('q') && tab.input.is_empty() {
        tab.dialog = Dialog::QuitConfirm;
        return Action::None;
    }

    // Shift+Enter → insert newline.
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        tab.input.insert(tab.cursor_col, '\n');
        tab.cursor_col += 1;
        tab.suggestions = autocomplete_suggestions(&tab.input);
        return Action::None;
    }

    // Enter → submit command.
    if key.code == KeyCode::Enter {
        let cmd = tab.input.trim().to_string();
        tab.input.clear();
        tab.cursor_col = 0;
        tab.suggestions.clear();
        tab.input_error = None;
        return Action::Submit(cmd);
    }

    // Arrow keys: move cursor.
    match key.code {
        KeyCode::Left => {
            tab.cursor_col = tab.cursor_col.saturating_sub(1);
            return Action::None;
        }
        KeyCode::Right => {
            if tab.cursor_col < tab.input.len() {
                tab.cursor_col += 1;
            }
            return Action::None;
        }
        _ => {}
    }

    // Backspace.
    if key.code == KeyCode::Backspace && tab.cursor_col > 0 {
        tab.cursor_col -= 1;
        tab.input.remove(tab.cursor_col);
        tab.suggestions = autocomplete_suggestions(&tab.input);
        tab.input_error = None;
        return Action::None;
    }

    // Delete.
    if key.code == KeyCode::Delete && tab.cursor_col < tab.input.len() {
        tab.input.remove(tab.cursor_col);
        tab.suggestions = autocomplete_suggestions(&tab.input);
        return Action::None;
    }

    // Regular character.
    if let KeyCode::Char(c) = key.code {
        tab.input.insert(tab.cursor_col, c);
        tab.cursor_col += 1;
        tab.suggestions = autocomplete_suggestions(&tab.input);
        tab.input_error = None;
    }

    Action::None
}

// --- Dialog handlers ---

fn handle_quit_confirm(tab: &mut TabState, key: KeyEvent) -> Action {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        tab.dialog = Dialog::None;
        return Action::QuitConfirmed;
    }
    if key.code == KeyCode::Esc {
        tab.dialog = Dialog::None;
    }
    Action::None
}

fn handle_close_tab_confirm(tab: &mut TabState, key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => {
                tab.dialog = Dialog::None;
                return Action::QuitConfirmed;
            }
            KeyCode::Char('t') => {
                tab.dialog = Dialog::None;
                return Action::CloseCurrentTab;
            }
            _ => {}
        }
    }
    if key.code == KeyCode::Esc {
        tab.dialog = Dialog::None;
    }
    Action::None
}

fn handle_new_tab_directory(tab: &mut TabState, key: KeyEvent, mut input: String) -> Action {
    match key.code {
        KeyCode::Enter => {
            tab.dialog = Dialog::None;
            let path = if input.trim().is_empty() {
                tab.cwd.clone()
            } else {
                PathBuf::from(input.trim())
            };
            Action::NewTabDirectoryChosen(path)
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::None
        }
        KeyCode::Backspace => {
            input.pop();
            tab.dialog = Dialog::NewTabDirectory { input };
            Action::None
        }
        KeyCode::Char(c) => {
            input.push(c);
            tab.dialog = Dialog::NewTabDirectory { input };
            Action::None
        }
        _ => Action::None,
    }
}

fn handle_mount_scope(
    tab: &mut TabState,
    key: KeyEvent,
    git_root: PathBuf,
    cwd: PathBuf,
) -> Action {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            tab.dialog = Dialog::None;
            return Action::MountScopeChosen(git_root);
        }
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Enter => {
            tab.dialog = Dialog::None;
            return Action::MountScopeChosen(cwd);
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some("Command cancelled.".into());
        }
        _ => {}
    }
    Action::None
}

fn handle_agent_auth(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            tab.dialog = Dialog::None;
            return Action::AuthAccepted;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            return Action::AuthDeclined;
        }
        _ => {}
    }
    Action::None
}

fn handle_new_kind_select(tab: &mut TabState, key: KeyEvent, interview: bool) -> Action {
    match key.code {
        KeyCode::Char('1') | KeyCode::Char('f') | KeyCode::Char('F') => {
            tab.dialog = Dialog::NewTitleInput {
                kind: WorkItemKind::Feature,
                title: String::new(),
                interview,
            };
        }
        KeyCode::Char('2') | KeyCode::Char('b') | KeyCode::Char('B') => {
            tab.dialog = Dialog::NewTitleInput {
                kind: WorkItemKind::Bug,
                title: String::new(),
                interview,
            };
        }
        KeyCode::Char('3') | KeyCode::Char('t') | KeyCode::Char('T') => {
            tab.dialog = Dialog::NewTitleInput {
                kind: WorkItemKind::Task,
                title: String::new(),
                interview,
            };
        }
        KeyCode::Char('4') | KeyCode::Char('e') | KeyCode::Char('E') => {
            tab.dialog = Dialog::NewTitleInput {
                kind: WorkItemKind::Enhancement,
                title: String::new(),
                interview,
            };
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some("Command cancelled.".into());
        }
        _ => {}
    }
    Action::None
}

fn handle_new_title_input(
    tab: &mut TabState,
    key: KeyEvent,
    kind: WorkItemKind,
    mut title: String,
    interview: bool,
) -> Action {
    match key.code {
        KeyCode::Enter => {
            let trimmed = title.trim().to_string();
            if trimmed.is_empty() {
                return Action::None;
            }
            tab.dialog = Dialog::None;
            return Action::NewWorkItem {
                kind,
                title: trimmed,
                interview,
            };
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some("Command cancelled.".into());
        }
        KeyCode::Backspace => {
            title.pop();
            tab.dialog = Dialog::NewTitleInput { kind, title, interview };
        }
        KeyCode::Char(c) => {
            title.push(c);
            tab.dialog = Dialog::NewTitleInput { kind, title, interview };
        }
        _ => {}
    }
    Action::None
}

fn handle_new_interview_summary(
    tab: &mut TabState,
    key: KeyEvent,
    kind: WorkItemKind,
    title: String,
    work_item_number: u32,
    mut summary: String,
    mut cursor_pos: usize,
) -> Action {
    // Ctrl+Enter or Ctrl+S → submit.
    let is_submit = (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        || (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL));
    if is_submit {
        let trimmed = summary.trim().to_string();
        if !trimmed.is_empty() {
            tab.dialog = Dialog::None;
            return Action::NewInterviewSummarySubmitted {
                kind,
                title,
                work_item_number,
                summary: trimmed,
            };
        }
        return Action::None;
    }

    match key.code {
        KeyCode::Enter => {
            // Insert newline at cursor position.
            summary.insert(cursor_pos, '\n');
            cursor_pos += 1;
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some("Command cancelled.".into());
        }
        KeyCode::Backspace => {
            if cursor_pos > 0 {
                // Find the char boundary before cursor_pos.
                let mut char_start = cursor_pos - 1;
                while char_start > 0 && !summary.is_char_boundary(char_start) {
                    char_start -= 1;
                }
                summary.remove(char_start);
                cursor_pos = char_start;
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Delete => {
            if cursor_pos < summary.len() {
                // Find the next char boundary.
                let mut char_end = cursor_pos + 1;
                while char_end < summary.len() && !summary.is_char_boundary(char_end) {
                    char_end += 1;
                }
                summary.remove(cursor_pos);
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Left => {
            if cursor_pos > 0 {
                cursor_pos -= 1;
                while cursor_pos > 0 && !summary.is_char_boundary(cursor_pos) {
                    cursor_pos -= 1;
                }
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Right => {
            if cursor_pos < summary.len() {
                cursor_pos += 1;
                while cursor_pos < summary.len() && !summary.is_char_boundary(cursor_pos) {
                    cursor_pos += 1;
                }
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Up => {
            // Navigate to the same column in the previous line.
            let before = &summary[..cursor_pos];
            if let Some(prev_newline) = before.rfind('\n') {
                let col = cursor_pos - prev_newline - 1;
                let line_start = before[..prev_newline].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let line_len = prev_newline - line_start;
                cursor_pos = line_start + col.min(line_len);
            } else {
                cursor_pos = 0;
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Down => {
            // Navigate to the same column in the next line.
            let before = &summary[..cursor_pos];
            let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
            let col = cursor_pos - line_start;
            if let Some(next_newline) = summary[cursor_pos..].find('\n') {
                let next_line_start = cursor_pos + next_newline + 1;
                let next_line_end = summary[next_line_start..]
                    .find('\n')
                    .map(|i| next_line_start + i)
                    .unwrap_or(summary.len());
                let next_line_len = next_line_end - next_line_start;
                cursor_pos = next_line_start + col.min(next_line_len);
            } else {
                cursor_pos = summary.len();
            }
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Home => {
            let before = &summary[..cursor_pos];
            let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
            cursor_pos = line_start;
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::End => {
            let after = &summary[cursor_pos..];
            let line_end = after.find('\n').map(|i| cursor_pos + i).unwrap_or(summary.len());
            cursor_pos = line_end;
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            summary.insert(cursor_pos, c);
            cursor_pos += c.len_utf8();
            tab.dialog = Dialog::NewInterviewSummary {
                kind,
                title,
                work_item_number,
                summary,
                cursor_pos,
            };
        }
        _ => {}
    }
    Action::None
}

// --- Claws dialog handlers ---

fn handle_claws_has_forked(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.claws_wizard_already_forked = true;
            tab.dialog = Dialog::ClawsReadyUsernameInput { username: String::new() };
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some(
                "Please fork nanoclaw at github.com/qwibitai/nanoclaw, \
                 then run 'claws init' again."
                    .into(),
            );
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_username_input(tab: &mut TabState, key: KeyEvent, mut username: String) -> Action {
    match key.code {
        KeyCode::Enter => {
            let trimmed = username.trim().to_string();
            if trimmed.is_empty() {
                return Action::None;
            }
            tab.claws_wizard_username = Some(trimmed);
            tab.dialog = Dialog::None;
            return Action::ClawsReadyProceed;
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.input_error = Some("Command cancelled.".into());
        }
        KeyCode::Backspace => {
            username.pop();
            tab.dialog = Dialog::ClawsReadyUsernameInput { username };
        }
        KeyCode::Char(c) => {
            username.push(c);
            tab.dialog = Dialog::ClawsReadyUsernameInput { username };
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_docker_socket_warning(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            if let Some(tx) = tab.claws_docker_accept_response_tx.take() {
                let _ = tx.send(true);
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            if let Some(tx) = tab.claws_docker_accept_response_tx.take() {
                let _ = tx.send(false);
            }
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_audit_confirm(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            return Action::ClawsAuditConfirmAccept;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            return Action::ClawsAuditConfirmDecline;
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_offer_start(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            return Action::ClawsReadyStartContainer;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            tab.claws_attach_after_start = false;
            tab.input_error = Some("Container not started.".into());
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_offer_restart_stopped(
    tab: &mut TabState,
    key: KeyEvent,
    container_id: String,
) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            return Action::ClawsReadyRestartStopped { container_id };
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            // User declined to restart stopped container — offer fresh start instead.
            tab.dialog = Dialog::ClawsReadyOfferStart;
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_restart_failed_offer_fresh(
    tab: &mut TabState,
    key: KeyEvent,
    container_id: String,
) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            return Action::ClawsReadyDeleteAndStartFresh { container_id };
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
        }
        _ => {}
    }
    Action::None
}

fn handle_claws_sudo_confirm(tab: &mut TabState, key: KeyEvent, mut password: String) -> Action {
    match key.code {
        KeyCode::Enter => {
            tab.dialog = Dialog::None;
            if let Some(tx) = tab.claws_sudo_response_tx.take() {
                let _ = tx.send(Some(password));
            }
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            if let Some(tx) = tab.claws_sudo_response_tx.take() {
                let _ = tx.send(None);
            }
        }
        KeyCode::Backspace => {
            password.pop();
            tab.dialog = Dialog::ClawsReadySudoConfirm { password };
        }
        KeyCode::Char(c) => {
            password.push(c);
            tab.dialog = Dialog::ClawsReadySudoConfirm { password };
        }
        _ => {}
    }
    Action::None
}

// --- Workflow dialog handlers ---

fn handle_workflow_step_confirm(
    tab: &mut TabState,
    key: KeyEvent,
    _completed_step: String,
    _next_steps: Vec<String>,
) -> Action {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            Action::WorkflowAdvance
        }
        KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2')
        | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::WorkflowAbort
        }
        _ => Action::None,
    }
}

fn handle_workflow_step_error(
    tab: &mut TabState,
    key: KeyEvent,
    _failed_step: String,
    _error: String,
) -> Action {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Char('1') => {
            tab.dialog = Dialog::None;
            Action::WorkflowRetry
        }
        KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2')
        | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::WorkflowAbort
        }
        _ => Action::None,
    }
}

fn handle_workflow_control_board(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => {
            tab.dialog = Dialog::None;
            Action::WorkflowRestartStep
        }
        KeyCode::Left => {
            tab.dialog = Dialog::None;
            Action::WorkflowCancelToPrevious
        }
        KeyCode::Right => {
            tab.dialog = Dialog::None;
            Action::WorkflowNextInNewContainer
        }
        KeyCode::Down => {
            tab.dialog = Dialog::None;
            Action::WorkflowNextInCurrentContainer
        }
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::None
        }
        _ => Action::None, // dialog stays open
    }
}

fn handle_worktree_merge_prompt(tab: &mut TabState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('m') | KeyCode::Char('M') => {
            tab.dialog = Dialog::None;
            Action::WorktreeMerge
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            tab.dialog = Dialog::None;
            Action::WorktreeDiscard
        }
        KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::WorktreeSkip
        }
        _ => Action::None,
    }
}

fn handle_worktree_commit_prompt(
    tab: &mut TabState,
    key: KeyEvent,
    branch: String,
    worktree_path: PathBuf,
    git_root: PathBuf,
    uncommitted_files: Vec<String>,
    mut message: String,
    mut cursor_pos: usize,
) -> Action {
    // Ctrl+Enter or Ctrl+S → submit commit
    let is_submit = (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        || (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL));
    if is_submit {
        let trimmed = message.trim().to_string();
        if !trimmed.is_empty() {
            tab.dialog = Dialog::None;
            return Action::WorktreeCommitFiles { message: trimmed, branch, worktree_path, git_root };
        }
        return Action::None;
    }

    match key.code {
        KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::None
        }
        KeyCode::Backspace => {
            if cursor_pos > 0 {
                let mut char_start = cursor_pos - 1;
                while char_start > 0 && !message.is_char_boundary(char_start) {
                    char_start -= 1;
                }
                message.remove(char_start);
                cursor_pos = char_start;
            }
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::Delete => {
            if cursor_pos < message.len() {
                let mut char_end = cursor_pos + 1;
                while char_end < message.len() && !message.is_char_boundary(char_end) {
                    char_end += 1;
                }
                message.remove(cursor_pos);
            }
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::Left => {
            if cursor_pos > 0 {
                cursor_pos -= 1;
                while cursor_pos > 0 && !message.is_char_boundary(cursor_pos) {
                    cursor_pos -= 1;
                }
            }
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::Right => {
            if cursor_pos < message.len() {
                cursor_pos += 1;
                while cursor_pos < message.len() && !message.is_char_boundary(cursor_pos) {
                    cursor_pos += 1;
                }
            }
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::Home => {
            cursor_pos = 0;
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::End => {
            cursor_pos = message.len();
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            message.insert(cursor_pos, c);
            cursor_pos += c.len_utf8();
            tab.dialog = Dialog::WorktreeCommitPrompt { branch, worktree_path, git_root, uncommitted_files, message, cursor_pos };
            Action::None
        }
        _ => Action::None,
    }
}

fn handle_worktree_merge_confirm(
    tab: &mut TabState,
    key: KeyEvent,
    branch: String,
    worktree_path: PathBuf,
    git_root: PathBuf,
) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            tab.dialog = Dialog::None;
            Action::WorktreeMergeConfirmed { branch, worktree_path, git_root }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::None
        }
        _ => Action::None,
    }
}

fn handle_worktree_delete_confirm(
    tab: &mut TabState,
    key: KeyEvent,
    branch: String,
    worktree_path: PathBuf,
    git_root: PathBuf,
) -> Action {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            tab.dialog = Dialog::None;
            Action::WorktreeDeleteConfirmed { branch, worktree_path, git_root }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            tab.dialog = Dialog::None;
            Action::WorktreeKeepAfterMerge
        }
        _ => Action::None,
    }
}

// --- Autocomplete ---

const SUBCOMMANDS: &[&str] = &["init", "ready", "implement", "chat", "specs", "claws", "status"];

/// Return suggestions for the current input string.
pub fn autocomplete_suggestions(input: &str) -> Vec<String> {
    if input.trim().is_empty() {
        return SUBCOMMANDS.iter().map(|s| s.to_string()).collect();
    }

    // Split on the FIRST space to separate command from arguments.
    // Use the raw input (not trimmed) so a trailing space signals "show flags".
    let tokens: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = tokens[0].trim();

    // If there is content after the first space (even empty), the user has
    // committed to a subcommand — show its flag suggestions.
    if tokens.len() == 2 {
        return flag_suggestions_for(cmd, tokens[1]);
    }

    // Otherwise, suggest subcommands that start with the typed prefix.
    SUBCOMMANDS
        .iter()
        .filter(|s| s.starts_with(cmd))
        .map(|s| s.to_string())
        .collect()
}

fn flag_suggestions_for(cmd: &str, _typed: &str) -> Vec<String> {
    match cmd {
        "init" => vec![
            "init --agent=claude".into(),
            "init --agent=codex".into(),
            "init --agent=opencode".into(),
        ],
        "ready" => vec![
            "ready --refresh".into(),
            "ready --build".into(),
            "ready --no-cache".into(),
            "ready --build --no-cache".into(),
            "ready --non-interactive".into(),
            "ready --refresh --non-interactive".into(),
            "ready --refresh --allow-docker".into(),
        ],
        "implement" => vec![
            "implement <NNNN>  e.g. implement 0001".into(),
            "implement <NNNN> --non-interactive".into(),
            "implement <NNNN> --plan".into(),
            "implement <NNNN> --allow-docker".into(),
            "implement <NNNN> --workflow aspec/workflows/implement-feature.md".into(),
        ],
        "chat" => vec![
            "chat  (start a freeform agent session)".into(),
            "chat --non-interactive".into(),
            "chat --plan".into(),
            "chat --allow-docker".into(),
        ],
        "specs" => vec![
            "specs new".into(),
            "specs new --interview".into(),
            "specs amend <NNNN>  e.g. specs amend 0025".into(),
        ],
        "claws" => vec![
            "claws init   (first-time setup: clone, build image, launch container)".into(),
            "claws ready  (check status; start container if stopped)".into(),
            "claws chat   (attach to running nanoclaw container)".into(),
        ],
        "status" => vec![
            "status         (show all running agents and nanoclaw containers)".into(),
            "status --watch (refresh every 3 seconds)".into(),
        ],
        _ => vec![],
    }
}

/// Return the subcommand name most similar to `input` (for typo correction).
pub fn closest_subcommand(input: &str) -> Option<String> {
    let word = input.trim().split_whitespace().next()?;
    // Already an exact match.
    if SUBCOMMANDS.contains(&word) {
        return None;
    }
    SUBCOMMANDS
        .iter()
        .map(|&s| (s, levenshtein(word, s)))
        .filter(|(_, d)| *d <= 4) // only suggest if "close enough"
        .min_by_key(|(_, d)| *d)
        .map(|(s, _)| s.to_string())
}

/// Convert a crossterm key event to the raw bytes that a terminal would send.
pub fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+letter → ASCII control code.
                let n = (c as u8).to_ascii_lowercase();
                if n >= b'a' && n <= b'z' {
                    return Some(vec![n - b'a' + 1]);
                }
            }
            let mut buf = [0u8; 4];
            Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(b"\x7f".to_vec()),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Esc => Some(b"\x1b".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::F(n) => Some(format!("\x1b[{}~", n).into_bytes()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_empty_input_returns_all() {
        let suggestions = autocomplete_suggestions("");
        assert!(suggestions.contains(&"init".to_string()));
        assert!(suggestions.contains(&"ready".to_string()));
        assert!(suggestions.contains(&"implement".to_string()));
        assert!(suggestions.contains(&"claws".to_string()));
    }

    #[test]
    fn suggestions_prefix_filters_correctly() {
        let suggestions = autocomplete_suggestions("im");
        assert_eq!(suggestions, vec!["implement"]);
    }

    #[test]
    fn suggestions_prefix_init() {
        let suggestions = autocomplete_suggestions("in");
        assert_eq!(suggestions, vec!["init"]);
    }

    #[test]
    fn suggestions_full_command_with_space_shows_flags() {
        let suggestions = autocomplete_suggestions("init ");
        assert!(suggestions.iter().any(|s| s.contains("--agent")));
    }

    #[test]
    fn closest_subcommand_corrects_typo() {
        assert_eq!(closest_subcommand("implemnt"), Some("implement".into()));
        assert_eq!(closest_subcommand("redy"), Some("ready".into()));
        assert_eq!(closest_subcommand("int"), Some("init".into()));
    }

    #[test]
    fn closest_subcommand_exact_returns_none() {
        assert_eq!(closest_subcommand("ready"), None);
    }

    #[test]
    fn closest_subcommand_gibberish_returns_none() {
        // "xyzxyzxyz" is too far from any subcommand.
        assert_eq!(closest_subcommand("xyzxyzxyz"), None);
    }

    #[test]
    fn key_to_bytes_regular_char() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(key_to_bytes(&key), Some(b"a".to_vec()));
    }

    #[test]
    fn key_to_bytes_enter_is_cr() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(key_to_bytes(&key), Some(b"\r".to_vec()));
    }

    #[test]
    fn key_to_bytes_arrow_up() {
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(key_to_bytes(&key), Some(b"\x1b[A".to_vec()));
    }

    #[test]
    fn key_to_bytes_ctrl_c() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_to_bytes(&key), Some(vec![3]));
    }

    fn new_app() -> App {
        App::new(std::path::PathBuf::new())
    }

    #[test]
    fn arrow_up_scrolls_in_done_state_with_window_focused() {
        let mut app = new_app();
        for i in 0..50 {
            app.active_tab_mut().output_lines.push(format!("line {}", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Done { command: "ready".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().scroll_offset = 0;

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().scroll_offset, 1, "Up should increment scroll_offset");
        assert_eq!(app.active_tab().focus, Focus::ExecutionWindow, "Focus should stay on window");

        // Press Down to go back.
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().scroll_offset, 0, "Down should decrement scroll_offset");
    }

    // --- Container window input tests ---

    #[test]
    fn esc_minimizes_container_window_when_maximized() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Maximized;

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().container_window, ContainerWindowState::Minimized);
        // Focus stays on ExecutionWindow (outer window), not CommandBox
        assert_eq!(app.active_tab().focus, Focus::ExecutionWindow);
    }

    #[test]
    fn c_key_restores_container_window_when_minimized() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().container_window, ContainerWindowState::Maximized);
    }

    #[test]
    fn esc_from_minimized_outer_window_goes_to_command_box() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().focus, Focus::CommandBox);
    }

    #[test]
    fn keys_forwarded_to_pty_when_container_maximized() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Maximized;

        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::ForwardToPty(_)));
    }

    #[test]
    fn arrow_keys_scroll_outer_when_container_minimized() {
        let mut app = new_app();
        for i in 0..50 {
            app.active_tab_mut().output_lines.push(format!("line {}", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;
        app.active_tab_mut().scroll_offset = 0;

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().scroll_offset, 1, "Up should scroll outer window when container minimized");
    }

    #[test]
    fn up_arrow_from_command_box_focuses_outer_regardless_of_container_state() {
        let mut app = new_app();
        app.active_tab_mut().output_lines.push("some output".into());
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::CommandBox;
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().focus, Focus::ExecutionWindow);
    }

    #[test]
    fn suggestions_claws_prefix() {
        let suggestions = autocomplete_suggestions("cl");
        assert!(suggestions.contains(&"claws".to_string()), "cl should match claws: {:?}", suggestions);
    }

    #[test]
    fn suggestions_claws_space_shows_ready() {
        let suggestions = autocomplete_suggestions("claws ");
        assert!(
            suggestions.iter().any(|s| s.contains("ready")),
            "claws  should show ready suggestion: {:?}",
            suggestions
        );
    }

    #[test]
    fn arrow_up_from_command_box_focuses_window_then_scrolls() {
        let mut app = new_app();
        for i in 0..50 {
            app.active_tab_mut().output_lines.push(format!("line {}", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Done { command: "ready".into() };
        app.active_tab_mut().focus = Focus::CommandBox;
        app.active_tab_mut().scroll_offset = 0;

        // First Up: should move focus to ExecutionWindow but NOT scroll.
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().focus, Focus::ExecutionWindow);
        assert_eq!(app.active_tab().scroll_offset, 0, "First Up only focuses, doesn't scroll");

        // Second Up: now that we're in ExecutionWindow, should scroll.
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().focus, Focus::ExecutionWindow);
        assert_eq!(app.active_tab().scroll_offset, 1, "Second Up should scroll");
    }

    #[test]
    fn sudo_confirm_dialog_enter_sends_password_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::ClawsReadySudoConfirm { password: "s3cr3t".to_string() };
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Option<String>>();
        app.active_tab_mut().claws_sudo_response_tx = Some(tx);

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        handle_key(&mut app, key);

        assert_eq!(app.active_tab().dialog, Dialog::None);
        assert!(app.active_tab().claws_sudo_response_tx.is_none());
        assert_eq!(rx.try_recv().unwrap(), Some("s3cr3t".to_string()));
    }

    #[test]
    fn sudo_confirm_dialog_esc_sends_none_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::ClawsReadySudoConfirm { password: "abc".to_string() };
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Option<String>>();
        app.active_tab_mut().claws_sudo_response_tx = Some(tx);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        handle_key(&mut app, key);

        assert_eq!(app.active_tab().dialog, Dialog::None);
        assert_eq!(rx.try_recv().unwrap(), None);
    }

    #[test]
    fn sudo_confirm_dialog_char_appends_to_password() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::ClawsReadySudoConfirm { password: String::new() };
        let (tx, _rx) = tokio::sync::oneshot::channel::<Option<String>>();
        app.active_tab_mut().claws_sudo_response_tx = Some(tx);

        for c in "pass".chars() {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty());
            handle_key(&mut app, key);
        }
        assert_eq!(app.active_tab().dialog, Dialog::ClawsReadySudoConfirm { password: "pass".to_string() });
    }

    #[test]
    fn sudo_confirm_dialog_backspace_removes_last_char() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::ClawsReadySudoConfirm { password: "abc".to_string() };
        let (tx, _rx) = tokio::sync::oneshot::channel::<Option<String>>();
        app.active_tab_mut().claws_sudo_response_tx = Some(tx);

        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty());
        handle_key(&mut app, key);

        assert_eq!(app.active_tab().dialog, Dialog::ClawsReadySudoConfirm { password: "ab".to_string() });
    }

    #[test]
    fn sudo_confirm_dialog_enter_with_empty_password_sends_some_empty() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::ClawsReadySudoConfirm { password: String::new() };
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Option<String>>();
        app.active_tab_mut().claws_sudo_response_tx = Some(tx);

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        handle_key(&mut app, key);

        assert_eq!(app.active_tab().dialog, Dialog::None);
        // Empty password is allowed (e.g. NOPASSWD sudo configs).
        assert_eq!(rx.try_recv().unwrap(), Some(String::new()));
    }

    #[test]
    fn suggestions_empty_input_includes_specs() {
        let suggestions = autocomplete_suggestions("");
        assert!(suggestions.contains(&"specs".to_string()), "Empty input should include specs: {:?}", suggestions);
    }

    #[test]
    fn suggestions_specs_space_shows_subcommands() {
        let suggestions = autocomplete_suggestions("specs ");
        assert!(
            suggestions.iter().any(|s| s.contains("new")),
            "specs  should show new suggestion: {:?}",
            suggestions
        );
        assert!(
            suggestions.iter().any(|s| s.contains("amend")),
            "specs  should show amend suggestion: {:?}",
            suggestions
        );
    }

    // ─── Workflow control board: dialog state transitions ────────────────────────

    fn make_test_workflow_state() -> crate::workflow::WorkflowState {
        crate::workflow::WorkflowState::new(
            None,
            vec![crate::workflow::parser::WorkflowStep {
                name: "step-one".to_string(),
                depends_on: vec![],
                prompt_template: "do step one".to_string(),
            }],
            "hash".to_string(),
            1,
            "test-wf".to_string(),
        )
    }

    fn setup_running_workflow_app() -> App {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;
        app.active_tab_mut().workflow = Some(make_test_workflow_state());
        app.active_tab_mut().workflow_current_step = Some("step-one".to_string());
        app
    }

    #[test]
    fn workflow_control_board_up_returns_restart_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::WorkflowRestartStep));
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn workflow_control_board_left_returns_cancel_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::WorkflowCancelToPrevious));
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn workflow_control_board_right_returns_next_new_container_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::WorkflowNextInNewContainer));
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn workflow_control_board_down_returns_next_current_container_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::WorkflowNextInCurrentContainer));
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn workflow_control_board_esc_returns_none_and_clears_dialog() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn workflow_control_board_non_arrow_key_leaves_dialog_open() {
        let mut app = new_app();
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        handle_key(&mut app, key);
        assert!(
            matches!(app.active_tab().dialog, Dialog::WorkflowControlBoard { .. }),
            "Dialog should remain open for non-arrow keys"
        );
    }

    // ─── Ctrl+W guard conditions ─────────────────────────────────────────────────

    #[test]
    fn ctrl_w_opens_workflow_control_board_when_all_guards_pass() {
        let mut app = setup_running_workflow_app();
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        handle_key(&mut app, key);
        match &app.active_tab().dialog {
            Dialog::WorkflowControlBoard { current_step, error } => {
                assert_eq!(current_step, "step-one");
                assert_eq!(*error, None);
            }
            other => panic!("Expected WorkflowControlBoard dialog, got {:?}", other),
        }
    }

    #[test]
    fn ctrl_w_does_not_open_dialog_when_workflow_is_none() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        // workflow is None (default)
        app.active_tab_mut().workflow_current_step = Some("step-one".to_string());
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn ctrl_w_does_not_open_dialog_when_current_step_is_none() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().workflow = Some(make_test_workflow_state());
        // workflow_current_step is None (default)
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn ctrl_w_does_not_open_dialog_when_container_is_maximized() {
        let mut app = setup_running_workflow_app();
        app.active_tab_mut().container_window = ContainerWindowState::Maximized;
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        handle_key(&mut app, key);
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    #[test]
    fn ctrl_w_does_not_open_dialog_when_another_dialog_is_active() {
        let mut app = setup_running_workflow_app();
        app.active_tab_mut().dialog = Dialog::QuitConfirm;
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        handle_key(&mut app, key);
        // Dialog should remain QuitConfirm, not become WorkflowControlBoard.
        assert_eq!(app.active_tab().dialog, Dialog::QuitConfirm);
    }

    // ─── Auto-advance: input routing over maximized container ────────────────────

    /// When a WorkflowControlBoard dialog is open over a maximized container window,
    /// keystrokes must be dispatched to the dialog handler, not forwarded to the PTY.
    /// handle_key dispatches dialogs before reaching handle_window_key, so this
    /// property holds regardless of container_window state.
    #[test]
    fn keys_route_to_dialog_not_pty_when_dialog_open_over_maximized_container() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Maximized;
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        let action = handle_key(&mut app, key);

        // Esc must not leak through to the PTY.
        assert!(
            !matches!(action, Action::ForwardToPty(_)),
            "Esc should not be forwarded to the PTY when a dialog is open"
        );
        // The dialog handler consumed Esc and cleared the dialog.
        assert_eq!(app.active_tab().dialog, Dialog::None);
    }

    /// Confirm the same holds for an arrow key (↑ = WorkflowRestartStep).
    #[test]
    fn arrow_key_routes_to_dialog_not_pty_when_dialog_open_over_maximized_container() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().container_window = ContainerWindowState::Maximized;
        app.active_tab_mut().dialog = Dialog::WorkflowControlBoard {
            current_step: "step-one".to_string(),
            error: None,
        };

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        let action = handle_key(&mut app, key);

        assert!(
            !matches!(action, Action::ForwardToPty(_)),
            "Up arrow should not be forwarded to the PTY when a dialog is open"
        );
        assert!(matches!(action, Action::WorkflowRestartStep));
    }
}
