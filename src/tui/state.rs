use crate::commands::claws::ClawsAuditCtx;
use crate::commands::ready::{ReadyContext, ReadyOptions, ReadySummary};
use crate::commands::status::TuiTabInfo;
use crate::docker;
use crate::tui::pty::PtySession;
use crate::workflow::WorkflowState;
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Duration of container output inactivity before a tab is considered "stuck".
pub const STUCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Which widget currently receives keyboard input.
#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    CommandBox,
    ExecutionWindow,
}

/// Lifecycle of the currently running (or last run) command.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionPhase {
    /// No command has run yet (or previous output has been cleared).
    Idle,
    /// A command is running; output is live.
    Running { command: String },
    /// Command completed successfully; window is read-only.
    Done { command: String },
    /// Command exited with a non-zero status.
    Error { command: String, exit_code: i32 },
}

/// An overlay modal dialog, if any.
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    None,
    QuitConfirm,
    /// Prompts user for new tab's working directory.
    NewTabDirectory { input: String },
    /// Close current tab (when idle + multiple tabs).
    CloseTabConfirm,
    /// Ask whether to mount the Git root or just CWD.
    MountScope { git_root: PathBuf, cwd: PathBuf },
    /// Ask whether to mount agent credentials (and save the decision).
    /// Retained for completeness but currently unused (auto-passthrough).
    #[allow(dead_code)]
    AgentAuth { agent: String, git_root: PathBuf },
    /// Step 1 of `new`: select work item kind (Feature/Bug/Task/Enhancement).
    NewKindSelect { interview: bool },
    /// Step 2 of `new`: enter title. The kind has already been chosen.
    NewTitleInput {
        kind: crate::commands::new::WorkItemKind,
        /// Current title text being typed.
        title: String,
        interview: bool,
    },
    /// Interview summary input: large freeform text box.
    NewInterviewSummary {
        kind: crate::commands::new::WorkItemKind,
        title: String,
        work_item_number: u32,
        /// The text being typed.
        summary: String,
        /// Byte offset of the cursor in `summary`.
        cursor_pos: usize,
    },
    /// Claws wizard: ask if user has already forked nanoclaw.
    ClawsReadyHasForked,
    /// Claws wizard: enter GitHub username (if already forked).
    ClawsReadyUsernameInput { username: String },
    /// Claws wizard: confirm Docker socket access warning.
    ClawsReadyDockerSocketWarning,
    /// Claws wizard: confirm launching the audit agent (shown when Dockerfile.dev matches template).
    ClawsAuditConfirm,
    /// Claws subsequent run: offer to restart a found stopped container.
    /// Shows container details and a y/n prompt.
    ClawsReadyOfferRestartStopped {
        container_id: String,
        name: String,
        created: String,
    },
    /// Claws subsequent run: offer to start the stopped container.
    ClawsReadyOfferStart,
    /// Claws subsequent run: container restart failed — offer to delete and start fresh.
    ClawsRestartFailedOfferFresh { container_id: String },
    /// Claws wizard: clone failed with permission denied; collect sudo password for retry.
    ClawsReadySudoConfirm {
        /// The sudo password being entered (displayed as '*').
        password: String,
    },
    /// Workflow step completed: ask user to advance to the next step or abort.
    WorkflowStepConfirm {
        /// Name of the step that just completed.
        completed_step: String,
        /// Names of the next ready step(s).
        next_steps: Vec<String>,
    },
    /// Workflow step failed: ask user to retry or abort.
    WorkflowStepError {
        /// Name of the step that failed.
        failed_step: String,
        /// Error message.
        error: String,
    },
    /// Workflow control board: opened with Ctrl+W during a running workflow.
    WorkflowControlBoard {
        /// Name of the currently running step.
        current_step: String,
        /// Optional error message (e.g. "No previous step to return to").
        error: Option<String>,
    },
}

/// Tracks which command is waiting for dialog answers (mount scope, auth).
#[derive(Debug, Clone, PartialEq)]
pub enum PendingCommand {
    None,
    Ready {
        refresh: bool,
        build: bool,
        no_cache: bool,
        non_interactive: bool,
        allow_docker: bool,
    },
    Implement {
        work_item: u32,
        non_interactive: bool,
        plan: bool,
        allow_docker: bool,
        /// Optional workflow file path for multi-step execution.
        workflow: Option<PathBuf>,
    },
    Chat {
        non_interactive: bool,
        plan: bool,
        allow_docker: bool,
    },
    ClawsReady,
    /// specs amend: run amend agent for a work item.
    SpecsAmend {
        work_item: u32,
        allow_docker: bool,
    },
    /// specs new --interview: run interview agent after file creation.
    SpecsNewInterview {
        work_item_number: u32,
        kind: crate::commands::new::WorkItemKind,
        title: String,
        summary: String,
        allow_docker: bool,
    },
}

/// Which phase of the multi-step claws workflow is active in the TUI.
#[derive(Debug, Clone, PartialEq)]
pub enum ClawsPhase {
    /// Not running a claws workflow.
    Inactive,
    /// Container start-only task is running (used by `claws ready` when container stopped).
    Setup,
    /// Initial image build text task is running (downloads Dockerfile.nanoclaw + builds image).
    PreAudit,
    /// Post-build: /setup dialog + docker socket dialog + container launch + detached audit exec.
    PostAudit,
}

/// State of the container overlay window.
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerWindowState {
    /// No container window is visible.
    Hidden,
    /// Container window is open and capturing all keyboard input.
    Maximized,
    /// Container window is collapsed to a 1-line bar below the outer window.
    Minimized,
}

/// Metadata about the currently running (or most recently run) container.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub container_name: String,
    pub agent_display_name: String,
    pub start_time: Instant,
    pub latest_stats: Option<docker::ContainerStats>,
    /// History of (cpu%, memory_mb) samples for averaging.
    pub stats_history: Vec<(f64, f64)>,
}

/// Summary of a completed container session, displayed after the container exits.
#[derive(Debug, Clone)]
pub struct LastContainerSummary {
    pub agent_display_name: String,
    pub container_name: String,
    pub avg_cpu: String,
    pub avg_memory: String,
    pub total_time: String,
    pub exit_code: i32,
}

/// Human-readable display name for an agent.
pub fn agent_display_name(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "Opencode",
        _ => agent,
    }
}

/// Format a duration in seconds into a human-readable string (e.g. "5s", "12m", "1h 23m").
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h {}m", h, m)
        }
    }
}

/// Which phase of the multi-step `ready` workflow is active.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadyPhase {
    /// Not running a multi-phase ready workflow.
    Inactive,
    /// Pre-audit text command is running; audit PTY should launch next.
    PreAudit,
    /// Interactive audit PTY is running; post-audit should launch next.
    Audit,
    /// Post-audit text command is running; workflow is done when it finishes.
    PostAudit,
}

/// Per-tab application state for the TUI event loop.
pub struct TabState {
    /// Working directory for this tab.
    pub cwd: PathBuf,
    pub focus: Focus,
    pub phase: ExecutionPhase,
    pub dialog: Dialog,

    // --- Command input box ---
    /// Current text in the command input box.
    pub input: String,
    /// Cursor position (byte offset).
    pub cursor_col: usize,
    /// Autocomplete suggestions for the current input.
    pub suggestions: Vec<String>,
    /// Error message to display below the command box (cleared on next keypress).
    pub input_error: Option<String>,

    // --- Execution window ---
    /// Output lines received from the running command (ANSI stripped).
    pub output_lines: Vec<String>,
    /// How many lines from the bottom to skip (for post-run scrolling).
    pub scroll_offset: usize,

    // --- Live PTY session (Some only while Running with a PTY process) ---
    pub pty: Option<PtySession>,
    pub pty_rx: Option<Receiver<crate::tui::pty::PtyEvent>>,
    /// Accumulates the current incomplete line from PTY output.
    /// Handles `\r` (carriage return) by clearing the buffer so subsequent
    /// characters overwrite from the start — this is how terminal spinners
    /// and progress indicators work.
    pub pty_line_buffer: String,
    /// When true, the last entry in `output_lines` is a "live" (unfinalised)
    /// line that should be updated in-place rather than appended to.
    pub pty_live_line: bool,
    /// When true, the previous chunk ended with `\r` and we haven't yet seen
    /// the next byte to decide if it's `\r\n` (newline) or bare `\r` (overwrite).
    pub pty_pending_cr: bool,

    // --- Channels for text-based command output (init/ready) ---
    pub output_rx: UnboundedReceiver<String>,
    /// Cloned into OutputSink::Channel when launching non-PTY commands.
    pub output_tx: UnboundedSender<String>,
    /// Fires once when the current non-PTY command exits.
    pub exit_rx: Option<tokio::sync::oneshot::Receiver<i32>>,

    // --- Pending TUI state before launching a command (used by dialogs) ---
    pub pending_command: PendingCommand,
    pub pending_mount_path: Option<PathBuf>,

    // --- Multi-phase ready command state ---
    /// When Some, the ready command is mid-workflow; the audit or post-audit phase
    /// should be launched when the current phase finishes.
    pub ready_ctx: Option<ReadyContext>,
    /// Receives the ReadyContext and summary from the pre-audit task when it completes.
    pub ready_ctx_rx: Option<tokio::sync::oneshot::Receiver<(ReadyContext, ReadySummary)>>,
    /// Which phase of the ready workflow just completed.
    pub ready_phase: ReadyPhase,
    /// Options for the current ready workflow.
    pub ready_opts: ReadyOptions,

    // --- Container window state ---
    /// Whether the container overlay window is visible (and in what state).
    pub container_window: ContainerWindowState,
    /// How many lines from the bottom to skip when rendering the container's
    /// vt100 scrollback (mouse-wheel scrolling). 0 = live view (auto-follow).
    pub container_scroll_offset: usize,
    /// Metadata about the currently running container.
    pub container_info: Option<ContainerInfo>,
    /// VT100 terminal emulator for rendering container output with full ANSI support
    /// (colors, bold, cursor positioning, tabs, etc.). Replaces plain-text line buffer.
    pub vt100_parser: Option<vt100::Parser>,
    /// Summary of the last container session (shown after container exits).
    pub last_container_summary: Option<LastContainerSummary>,
    /// Receives Docker stats from the background polling task.
    pub stats_rx: Option<UnboundedReceiver<docker::ContainerStats>>,

    /// Host settings mounted into the container (sanitized config files in a temp dir).
    /// Held here so the temp dir lives as long as the container runs; dropped on finish.
    pub host_settings: Option<docker::HostSettings>,

    /// Timestamp of the most recent PTY byte received from the running container.
    /// `None` when no container is active. Used to detect stuck (idle) containers:
    /// if the elapsed time exceeds `STUCK_TIMEOUT`, `is_stuck()` returns `true`
    /// and the tab turns yellow.
    pub last_output_time: Option<Instant>,

    // --- Claws wizard state ---
    /// Which phase of the claws workflow is active.
    pub claws_phase: ClawsPhase,
    /// Container ID received from the claws setup task; consumed when attaching.
    pub claws_container_id: Option<String>,
    /// Receives the container ID from the claws setup task when it completes.
    pub claws_container_id_rx: Option<tokio::sync::oneshot::Receiver<String>>,
    /// GitHub username entered during the claws first-run wizard.
    pub claws_wizard_username: Option<String>,
    /// Whether the user indicated they have already forked nanoclaw.
    pub claws_wizard_already_forked: bool,
    /// Receives a unit signal from the background clone task when it encounters
    /// a permission-denied error and needs the user's permission to use sudo.
    pub claws_sudo_request_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Sends the user's sudo password (Some = accepted with password, None = declined) to the clone task.
    pub claws_sudo_response_tx: Option<tokio::sync::oneshot::Sender<Option<String>>>,
    /// Receives a unit signal from the background build task when it needs the user to
    /// accept docker socket access (shown after the image rebuild completes).
    pub claws_docker_accept_request_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Sends the user's docker-socket acceptance (true = accepted, false = declined) to the build task.
    pub claws_docker_accept_response_tx: Option<tokio::sync::oneshot::Sender<bool>>,
    /// Context produced by the pre-audit background task, consumed to launch the audit PTY.
    /// Also stored across the Audit phase so the post-audit task can access env_vars etc.
    pub claws_audit_ctx: Option<ClawsAuditCtx>,
    /// Receives the audit context from the pre-audit background task.
    pub claws_audit_ctx_rx: Option<tokio::sync::oneshot::Receiver<ClawsAuditCtx>>,
    /// When true, attach to the nanoclaw container after it is started.
    /// Set by `claws chat` when the container is not running.
    pub claws_attach_after_start: bool,
    /// Container ID of a stopped container that we tried (and failed) to restart.
    /// Stored so the error-recovery dialog can offer to delete it and start fresh.
    pub claws_restarting_container_id: Option<String>,

    /// Cancels a running `status --watch` loop in the TUI.
    ///
    /// `start_command` sends on this channel (if present) before starting any new
    /// command, stopping the background watch task so stale status output does not
    /// overwrite the new command's output.
    pub status_watch_cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // --- Multi-step workflow state ---
    /// Active workflow state for the current `implement --workflow` run.
    /// `None` when no workflow is active.
    pub workflow: Option<WorkflowState>,
    /// Name of the workflow step currently executing (while `workflow` is `Some`).
    pub workflow_current_step: Option<String>,
    /// Git root path captured when the workflow was launched (needed for state persistence).
    pub workflow_git_root: Option<PathBuf>,
}

impl TabState {
    pub fn new(cwd: PathBuf) -> Self {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        Self {
            cwd,
            focus: Focus::CommandBox,
            phase: ExecutionPhase::Idle,
            dialog: Dialog::None,
            input: String::new(),
            cursor_col: 0,
            suggestions: Vec::new(),
            input_error: None,
            output_lines: Vec::new(),
            scroll_offset: 0,
            pty: None,
            pty_rx: None,
            pty_line_buffer: String::new(),
            pty_live_line: false,
            pty_pending_cr: false,
            output_rx,
            output_tx,
            exit_rx: None,
            pending_command: PendingCommand::None,
            pending_mount_path: None,
            ready_ctx: None,
            ready_ctx_rx: None,
            ready_phase: ReadyPhase::Inactive,
            ready_opts: ReadyOptions::default(),
            container_window: ContainerWindowState::Hidden,
            container_scroll_offset: 0,
            container_info: None,
            vt100_parser: None,
            last_container_summary: None,
            stats_rx: None,
            host_settings: None,
            claws_phase: ClawsPhase::Inactive,
            claws_container_id: None,
            claws_container_id_rx: None,
            claws_wizard_username: None,
            claws_wizard_already_forked: false,
            claws_sudo_request_rx: None,
            claws_sudo_response_tx: None,
            claws_docker_accept_request_rx: None,
            claws_docker_accept_response_tx: None,
            claws_audit_ctx: None,
            claws_audit_ctx_rx: None,
            claws_attach_after_start: false,
            claws_restarting_container_id: None,
            status_watch_cancel_tx: None,
            last_output_time: None,
            workflow: None,
            workflow_current_step: None,
            workflow_git_root: None,
        }
    }

    /// Append a line to the execution window output.
    pub fn push_output(&mut self, line: impl Into<String>) {
        self.output_lines.push(line.into());
        // Auto-scroll to bottom while running.
        if matches!(self.phase, ExecutionPhase::Running { .. }) {
            self.scroll_offset = 0;
        }
    }

    /// Clear output and reset state for a fresh command execution.
    pub fn start_command(&mut self, command: String) {
        // Cancel any running status --watch loop so it doesn't overwrite the
        // new command's output.
        if let Some(tx) = self.status_watch_cancel_tx.take() {
            let _ = tx.send(());
        }
        self.output_lines.clear();
        self.scroll_offset = 0;
        self.pty_line_buffer.clear();
        self.pty_live_line = false;
        self.pty_pending_cr = false;
        self.phase = ExecutionPhase::Running { command };
        self.focus = Focus::ExecutionWindow;
        self.input_error = None;
    }

    /// Activate the container window for a new PTY container session.
    ///
    /// `cols` and `rows` specify the inner dimensions of the container window
    /// (used to initialise the VT100 terminal emulator).
    pub fn start_container(
        &mut self,
        container_name: String,
        agent_display_name: String,
        cols: u16,
        rows: u16,
    ) {
        self.container_window = ContainerWindowState::Maximized;
        self.container_scroll_offset = 0;
        self.vt100_parser = Some(vt100::Parser::new(rows, cols, 1000));
        self.last_container_summary = None;
        self.last_output_time = Some(Instant::now());
        self.container_info = Some(ContainerInfo {
            container_name,
            agent_display_name,
            start_time: Instant::now(),
            latest_stats: None,
            stats_history: Vec::new(),
        });
    }

    /// Transition to the next phase of a multi-step workflow (e.g. ready).
    /// Like `start_command` but preserves existing output instead of clearing it.
    pub fn continue_command(&mut self, command: String) {
        self.scroll_offset = 0;
        self.pty_line_buffer.clear();
        self.pty_live_line = false;
        self.pty_pending_cr = false;
        self.phase = ExecutionPhase::Running { command };
        self.focus = Focus::ExecutionWindow;
        self.input_error = None;
    }

    /// Transition to Done or Error based on exit code; re-enable input.
    pub fn finish_command(&mut self, exit_code: i32) {
        let command = match &self.phase {
            ExecutionPhase::Running { command } => command.clone(),
            _ => String::new(),
        };
        self.phase = if exit_code == 0 {
            ExecutionPhase::Done { command }
        } else {
            ExecutionPhase::Error { command, exit_code }
        };
        self.focus = Focus::CommandBox;
        self.pty = None;
        self.pty_rx = None;
        self.pty_line_buffer.clear();
        self.pty_live_line = false;
        self.pty_pending_cr = false;
        self.exit_rx = None;

        // Drop host settings only if no multi-phase workflow is in progress.
        // During ready --refresh, the pre-audit phase completes before the audit container
        // launches — host_settings must survive across phases.
        // During claws setup, the text task completes before the PTY exec session starts —
        // host_settings must survive until the exec session ends.
        // Also preserve host_settings while a workflow step sequence is in progress.
        if self.ready_phase == ReadyPhase::Inactive
            && self.claws_phase == ClawsPhase::Inactive
            && self.workflow.is_none()
        {
            self.host_settings = None;
        }

        // Clear the stuck-detection timer; the container is no longer running.
        self.last_output_time = None;

        // Close the container window and generate a summary if applicable.
        if self.container_window != ContainerWindowState::Hidden {
            if let Some(info) = self.container_info.take() {
                let elapsed = info.start_time.elapsed().as_secs();
                let (avg_cpu, avg_memory) = if info.stats_history.is_empty() {
                    ("n/a".to_string(), "n/a".to_string())
                } else {
                    let count = info.stats_history.len() as f64;
                    let cpu_avg: f64 = info.stats_history.iter().map(|(c, _)| c).sum::<f64>() / count;
                    let mem_avg: f64 = info.stats_history.iter().map(|(_, m)| m).sum::<f64>() / count;
                    (format!("{:.1}%", cpu_avg), format!("{:.0}MiB", mem_avg))
                };
                self.last_container_summary = Some(LastContainerSummary {
                    agent_display_name: info.agent_display_name,
                    container_name: info.container_name,
                    avg_cpu,
                    avg_memory,
                    total_time: format_duration(elapsed),
                    exit_code,
                });
            }
            self.container_window = ContainerWindowState::Hidden;
            self.vt100_parser = None;
            self.stats_rx = None;
        }
    }

    /// Whether PTY output should be routed to the vt100 terminal emulator.
    pub fn pty_uses_container(&self) -> bool {
        self.container_window != ContainerWindowState::Hidden
    }

    /// Process raw PTY output bytes, handling carriage returns (`\r`) correctly.
    ///
    /// This method is used for the *outer* execution window (non-container output).
    /// Container output is routed through the vt100 parser instead.
    ///
    /// Terminal applications use `\r` (without `\n`) to move the cursor back to
    /// column 0 so the next output overwrites the current line — this is how
    /// spinners and progress indicators work. `\r\n` is treated as a newline.
    ///
    /// The method maintains `pty_line_buffer` (the current incomplete line) and
    /// a "live line" at the end of `output_lines` that is updated in-place until
    /// a `\n` finalises it.
    pub fn process_pty_data(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        // Process \r and \n from the raw bytes BEFORE stripping ANSI escapes,
        // because strip_ansi_escapes::strip removes \r characters.
        let mut i = 0;

        // Resolve a pending \r from the previous chunk.
        if self.pty_pending_cr {
            self.pty_pending_cr = false;
            if bytes[0] == b'\n' {
                // Previous \r + this \n → newline.
                self.finalise_pty_line();
                i = 1;
            } else {
                // Previous \r was a bare carriage return → move cursor to column 0.
                // Clear the buffer so subsequent content overwrites the current line.
                self.pty_line_buffer.clear();
            }
        }

        while i < bytes.len() {
            match bytes[i] {
                b'\r' => {
                    if i + 1 < bytes.len() {
                        if bytes[i + 1] == b'\n' {
                            // \r\n → newline
                            self.finalise_pty_line();
                            i += 2;
                        } else {
                            // Bare \r → move cursor to column 0. Clear the buffer
                            // so subsequent content overwrites the current line
                            // (this is how terminal spinners/progress bars work).
                            self.pty_line_buffer.clear();
                            i += 1;
                        }
                    } else {
                        // \r at the very end of the chunk — defer until next chunk
                        // so we can distinguish \r\n (newline) from bare \r (overwrite).
                        self.pty_pending_cr = true;
                        i += 1;
                    }
                }
                b'\n' => {
                    self.finalise_pty_line();
                    i += 1;
                }
                _ => {
                    // Collect a content segment (up to next \r or \n).
                    let start = i;
                    while i < bytes.len() && bytes[i] != b'\r' && bytes[i] != b'\n' {
                        i += 1;
                    }
                    // Strip ANSI escape sequences from the content segment only.
                    let segment = &bytes[start..i];
                    let stripped = strip_ansi_escapes::strip(segment);
                    let text = String::from_utf8_lossy(&stripped);
                    // Filter out remaining C0 control characters (BEL, BS, ESC
                    // fragments, etc.) that have zero display width but non-zero
                    // byte length — they cause scroll calculation mismatches.
                    for ch in text.chars() {
                        if ch >= ' ' {
                            self.pty_line_buffer.push(ch);
                        }
                    }
                }
            }
        }

        // Sync the live-line display with the current buffer contents.
        if !self.pty_line_buffer.is_empty() {
            if self.pty_live_line {
                if let Some(last) = self.output_lines.last_mut() {
                    *last = self.pty_line_buffer.clone();
                }
            } else {
                self.output_lines.push(self.pty_line_buffer.clone());
                self.pty_live_line = true;
            }
            // Auto-scroll to bottom while running.
            if matches!(self.phase, ExecutionPhase::Running { .. }) {
                self.scroll_offset = 0;
            }
        }
    }

    /// Finalise the current PTY line buffer: push it to `output_lines`
    /// (or update the existing live line) and reset the buffer.
    fn finalise_pty_line(&mut self) {
        let line = std::mem::take(&mut self.pty_line_buffer);
        if self.pty_live_line {
            if let Some(last) = self.output_lines.last_mut() {
                *last = line;
            }
        } else {
            self.output_lines.push(line);
        }
        self.pty_live_line = false;
    }

    /// Border color for the execution window based on current state and focus.
    ///
    /// Selected:   blue (running) | green (done/success) | red (done/error)
    /// Unselected: grey (idle/running/done) | red (error, persists when unselected)
    pub fn window_border_color(&self) -> Color {
        match (&self.phase, &self.focus) {
            (ExecutionPhase::Running { .. }, Focus::ExecutionWindow) => Color::Blue,
            (ExecutionPhase::Running { .. }, Focus::CommandBox) => Color::Gray,
            (ExecutionPhase::Done { .. }, Focus::ExecutionWindow) => Color::Green,
            (ExecutionPhase::Done { .. }, Focus::CommandBox) => Color::Gray,
            (ExecutionPhase::Error { .. }, _) => Color::Red,
            (ExecutionPhase::Idle, _) => Color::DarkGray,
        }
    }

    /// Returns `true` if the running container has produced no output for
    /// longer than [`STUCK_TIMEOUT`]. Only meaningful when a container is
    /// active; always `false` otherwise.
    pub fn is_stuck(&self) -> bool {
        matches!(&self.phase, ExecutionPhase::Running { .. })
            && self.container_window != ContainerWindowState::Hidden
            && self
                .last_output_time
                .map(|t| t.elapsed() > STUCK_TIMEOUT)
                .unwrap_or(false)
    }

    /// Reset the stuck timer to now.  Call this whenever the user interacts
    /// with this tab (switching to it, typing, mouse scroll, etc.) so the
    /// yellow warning colour is immediately cleared.
    pub fn acknowledge_stuck(&mut self) {
        if self.last_output_time.is_some() {
            self.last_output_time = Some(Instant::now());
        }
    }

    /// Color for the tab indicator based on current phase and container state.
    pub fn tab_color(&self) -> Color {
        if self.is_stuck() {
            return Color::Yellow;
        }
        match &self.phase {
            ExecutionPhase::Error { .. } => Color::Red,
            ExecutionPhase::Running { command } => {
                if self.claws_phase != ClawsPhase::Inactive || command.starts_with("claws") {
                    Color::Magenta
                } else if self.container_window != ContainerWindowState::Hidden {
                    Color::Green
                } else {
                    Color::Blue
                }
            }
            ExecutionPhase::Idle | ExecutionPhase::Done { .. } => Color::DarkGray,
        }
    }

    /// Project folder name for the tab border title (≤14 chars).
    pub fn tab_project_name(&self) -> String {
        let name = self.cwd.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        if name.chars().count() > 14 {
            let t: String = name.chars().take(13).collect();
            format!("{}…", t)
        } else {
            name
        }
    }

    /// Full subcommand shown inside the tab box, truncated if wider than the tab.
    /// `tab_width` is the total width of the tab widget (including borders).
    /// Empty string when idle. Prepends "⚠️ " when the tab is stuck.
    pub fn tab_subcommand_label(&self, tab_width: u16) -> String {
        let cmd = match &self.phase {
            ExecutionPhase::Idle => return String::new(),
            ExecutionPhase::Running { command }
            | ExecutionPhase::Done { command }
            | ExecutionPhase::Error { command, .. } => command.as_str(),
        };
        // Prepend warning prefix for stuck tabs.
        let prefix = if self.is_stuck() { "⚠️ " } else { "" };
        let prefix_chars = prefix.chars().count();
        // Inner width: tab_width minus 2 borders minus 2 padding spaces.
        let max_chars = tab_width.saturating_sub(4) as usize;
        let cmd_max = max_chars.saturating_sub(prefix_chars);
        let cmd_str = if cmd.chars().count() > cmd_max && cmd_max > 1 {
            let truncated: String = cmd.chars().take(cmd_max - 1).collect();
            format!("{}…", truncated)
        } else {
            cmd.to_string()
        };
        format!("{}{}", prefix, cmd_str)
    }

    /// Combined display name for the tab: "projname" or "projname | cmd".
    pub fn tab_display_name(&self) -> String {
        let proj = self.tab_project_name();
        let cmd = self.tab_subcommand_label(20);
        if cmd.is_empty() { proj } else { format!("{} | {}", proj, cmd) }
    }

    /// Poll all channels for new data; called once per event loop tick.
    pub fn tick(&mut self) {
        // Drain text command output.
        while let Ok(line) = self.output_rx.try_recv() {
            // Special marker sent by `status --watch` to clear the window before
            // rendering an updated snapshot. This makes the tables appear to update
            // in place even though the outer execution window does not support ANSI
            // cursor movement.
            if line == crate::commands::status::CLEAR_MARKER {
                self.output_lines.clear();
                self.scroll_offset = 0;
                continue;
            }
            // Split on newlines in case a single send contains multiple lines.
            for part in line.split('\n') {
                self.push_output(part.to_string());
            }
        }

        // Drain PTY output — collect events first to avoid a split borrow.
        let pty_events: Vec<crate::tui::pty::PtyEvent> = if let Some(ref rx) = self.pty_rx {
            let mut events = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(ev) => events.push(ev),
                    Err(_) => break,
                }
            }
            events
        } else {
            vec![]
        };
        for event in pty_events {
            match event {
                crate::tui::pty::PtyEvent::Data(bytes) => {
                    // Route container PTY data through the vt100 terminal emulator
                    // for full ANSI rendering. Non-container data goes through the
                    // plain-text line processor for the outer window.
                    if self.pty_uses_container() {
                        if let Some(ref mut parser) = self.vt100_parser {
                            parser.process(&bytes);
                        }
                        // Any output from the container resets the stuck timer.
                        self.last_output_time = Some(Instant::now());
                    } else {
                        self.process_pty_data(&bytes);
                    }
                }
                crate::tui::pty::PtyEvent::Exit(code) => {
                    self.finish_command(code);
                    break;
                }
            }
        }

        // Check non-PTY exit code.
        if let Some(ref mut rx) = self.exit_rx {
            if let Ok(code) = rx.try_recv() {
                self.finish_command(code);
            }
        }

        // Check for ready context from pre-audit phase.
        if let Some(ref mut rx) = self.ready_ctx_rx {
            if let Ok((ctx, _summary)) = rx.try_recv() {
                self.ready_ctx = Some(ctx);
            }
        }

        // Check for container ID from the claws setup task.
        if let Some(ref mut rx) = self.claws_container_id_rx {
            if let Ok(id) = rx.try_recv() {
                self.claws_container_id = Some(id);
            }
        }

        // Check for audit context from the pre-audit background task.
        if let Some(ref mut rx) = self.claws_audit_ctx_rx {
            if let Ok(ctx) = rx.try_recv() {
                self.claws_audit_ctx = Some(ctx);
                self.claws_audit_ctx_rx = None;
            }
        }

        // Check if the background clone task needs sudo permission.
        if let Some(ref mut rx) = self.claws_sudo_request_rx {
            if rx.try_recv().is_ok() {
                self.claws_sudo_request_rx = None;
                self.dialog = Dialog::ClawsReadySudoConfirm { password: String::new() };
            }
        }

        // Check if the background build task needs docker socket acceptance.
        if let Some(ref mut rx) = self.claws_docker_accept_request_rx {
            if rx.try_recv().is_ok() {
                self.claws_docker_accept_request_rx = None;
                self.dialog = Dialog::ClawsReadyDockerSocketWarning;
            }
        }

        // Drain Docker stats from the polling task.
        if let Some(ref mut rx) = self.stats_rx {
            while let Ok(stats) = rx.try_recv() {
                if let Some(ref mut info) = self.container_info {
                    let cpu = docker::parse_cpu_percent(&stats.cpu_percent);
                    let mem = docker::parse_memory_mb(&stats.memory);
                    info.stats_history.push((cpu, mem));
                    info.latest_stats = Some(stats);
                }
            }
        }
    }
}

/// Top-level application state: manages multiple tabs.
pub struct App {
    pub tabs: Vec<TabState>,
    pub active_tab_idx: usize,
    pub should_quit: bool,
    /// Live snapshot of tab→container associations, kept up-to-date by `tick_all()`.
    /// Shared with any running `status --watch` background task so the table reflects
    /// current state on every refresh rather than the state at command-start time.
    pub tui_tabs_shared: Arc<Mutex<Vec<TuiTabInfo>>>,
}

impl App {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self {
            tabs: vec![TabState::new(cwd)],
            active_tab_idx: 0,
            should_quit: false,
            tui_tabs_shared: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn active_tab(&self) -> &TabState {
        &self.tabs[self.active_tab_idx]
    }

    pub fn active_tab_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab_idx]
    }

    /// Create a new tab immediately after the active tab. Returns the new tab index.
    pub fn create_tab(&mut self, cwd: std::path::PathBuf) -> usize {
        let new_idx = self.active_tab_idx + 1;
        self.tabs.insert(new_idx, TabState::new(cwd));
        new_idx
    }

    /// Close the tab at `idx`. Adjusts `active_tab_idx`.
    /// If only one tab remains, sets `should_quit`.
    pub fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 {
            self.should_quit = true;
            return;
        }
        self.tabs.remove(idx);
        if self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.tabs.len() - 1;
        }
    }

    /// Call `tick()` on every tab so background PTY sessions stay live.
    /// Also refreshes the shared `tui_tabs_shared` snapshot so any running
    /// `status --watch` task sees up-to-date container associations and stuck state.
    pub fn tick_all(&mut self) {
        for tab in &mut self.tabs {
            tab.tick();
        }
        let snapshot: Vec<TuiTabInfo> = self.tabs.iter().enumerate()
            .map(|(i, tab)| TuiTabInfo {
                tab_number: i + 1,
                container_name: tab.container_info.as_ref()
                    .map(|ci| ci.container_name.clone())
                    .unwrap_or_default(),
                is_stuck: tab.is_stuck(),
            })
            .collect();
        if let Ok(mut guard) = self.tui_tabs_shared.lock() {
            *guard = snapshot;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_tab() -> TabState {
        TabState::new(std::path::PathBuf::new())
    }

    #[test]
    fn window_border_color_blue_when_selected_and_running() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "ready".into() };
        tab.focus = Focus::ExecutionWindow;
        assert_eq!(tab.window_border_color(), Color::Blue);
    }

    #[test]
    fn window_border_color_grey_when_unselected_running() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "ready".into() };
        tab.focus = Focus::CommandBox;
        assert_eq!(tab.window_border_color(), Color::Gray);
    }

    #[test]
    fn window_border_color_green_when_selected_and_done() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Done { command: "ready".into() };
        tab.focus = Focus::ExecutionWindow;
        assert_eq!(tab.window_border_color(), Color::Green);
    }

    #[test]
    fn window_border_color_grey_when_unselected_done() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Done { command: "ready".into() };
        tab.focus = Focus::CommandBox;
        assert_eq!(tab.window_border_color(), Color::Gray);
    }

    #[test]
    fn window_border_color_red_on_error_selected() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Error { command: "ready".into(), exit_code: 1 };
        tab.focus = Focus::ExecutionWindow;
        assert_eq!(tab.window_border_color(), Color::Red);
    }

    #[test]
    fn window_border_color_red_on_error_unselected() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Error { command: "ready".into(), exit_code: 1 };
        tab.focus = Focus::CommandBox;
        assert_eq!(tab.window_border_color(), Color::Red);
    }

    #[test]
    fn start_command_clears_output_and_focuses_window() {
        let mut tab = new_tab();
        tab.output_lines.push("old line".into());
        tab.start_command("ready".into());
        assert!(tab.output_lines.is_empty());
        assert_eq!(tab.focus, Focus::ExecutionWindow);
        assert!(matches!(tab.phase, ExecutionPhase::Running { .. }));
    }

    #[test]
    fn continue_command_preserves_output() {
        let mut tab = new_tab();
        tab.output_lines.push("phase 1 output".into());
        tab.output_lines.push("more output".into());
        tab.continue_command("phase 2".into());
        // Output from previous phase must be preserved.
        assert_eq!(tab.output_lines.len(), 2);
        assert_eq!(tab.output_lines[0], "phase 1 output");
        assert!(matches!(tab.phase, ExecutionPhase::Running { .. }));
        assert_eq!(tab.focus, Focus::ExecutionWindow);
    }

    #[test]
    fn finish_command_zero_transitions_to_done() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "init".into() };
        tab.finish_command(0);
        assert!(matches!(tab.phase, ExecutionPhase::Done { .. }));
        assert_eq!(tab.focus, Focus::CommandBox);
    }

    #[test]
    fn finish_command_nonzero_transitions_to_error() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "ready".into() };
        tab.finish_command(1);
        assert!(matches!(tab.phase, ExecutionPhase::Error { exit_code: 1, .. }));
    }

    #[test]
    fn pty_data_newlines_create_separate_lines() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        tab.process_pty_data(b"Hello\nWorld\n");
        assert_eq!(tab.output_lines, vec!["Hello", "World"]);
        assert!(!tab.pty_live_line);
    }

    #[test]
    fn pty_data_cr_overwrites_current_line() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // First chunk: spinner frame 1
        tab.process_pty_data(b"Thinking...");
        assert_eq!(tab.output_lines, vec!["Thinking..."]);
        assert!(tab.pty_live_line);

        // Second chunk: \r clears the buffer, "Done!" overwrites the live line
        tab.process_pty_data(b"\rDone!      ");
        assert_eq!(tab.output_lines, vec!["Done!      "]);
        assert!(tab.pty_live_line);

        // Newline finalises the line
        tab.process_pty_data(b"\n");
        assert_eq!(tab.output_lines, vec!["Done!      "]);
        assert!(!tab.pty_live_line);
    }

    #[test]
    fn pty_data_cr_lf_treated_as_newline() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        tab.process_pty_data(b"Hello\r\nWorld\r\n");
        assert_eq!(tab.output_lines, vec!["Hello", "World"]);
        assert!(!tab.pty_live_line);
    }

    #[test]
    fn pty_data_multiple_cr_in_one_chunk() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // Multiple carriage returns in one chunk — each \r clears the buffer
        // so only the final frame survives (overwrite behavior).
        tab.process_pty_data(b"frame1\rframe2\rframe3\n");
        assert_eq!(tab.output_lines, vec!["frame3"]);
    }

    #[test]
    fn pty_data_cr_lf_split_across_chunks() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // \r\n split: \r at end of chunk 1, \n at start of chunk 2.
        // Must be treated as a newline, NOT as bare \r (which would lose text).
        tab.process_pty_data(b"Hello\r");
        assert!(tab.pty_pending_cr, "should defer \\r at end of chunk");
        // The text should still be visible as a live line while pending.
        assert_eq!(tab.output_lines, vec!["Hello"]);

        tab.process_pty_data(b"\nWorld\r\n");
        assert!(!tab.pty_pending_cr);
        assert_eq!(tab.output_lines, vec!["Hello", "World"]);
        assert!(!tab.pty_live_line);
    }

    #[test]
    fn pty_data_cr_split_then_bare_cr() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // \r at end of chunk, but next chunk does NOT start with \n → bare \r.
        tab.process_pty_data(b"old text\r");
        assert!(tab.pty_pending_cr);

        tab.process_pty_data(b"new text\n");
        assert!(!tab.pty_pending_cr);
        // bare \r clears the buffer, so "new text" overwrites "old text".
        assert_eq!(tab.output_lines, vec!["new text"]);
    }

    #[test]
    fn pty_data_empty_chunk_preserves_pending_cr() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        tab.process_pty_data(b"text\r");
        assert!(tab.pty_pending_cr);
        // Empty chunk should not resolve the pending \r.
        tab.process_pty_data(b"");
        assert!(tab.pty_pending_cr);
    }

    #[test]
    fn pty_data_control_chars_filtered() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // BEL (0x07) and BS (0x08) should be filtered out of the line buffer.
        tab.process_pty_data(b"Hello\x07World\x08!\n");
        assert_eq!(tab.output_lines, vec!["HelloWorld!"]);
    }

    #[test]
    fn pty_data_tabs_stripped_by_ansi_strip() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };
        // strip_ansi_escapes also removes tabs; verify they don't cause issues.
        tab.process_pty_data(b"col1\tcol2\n");
        assert_eq!(tab.output_lines, vec!["col1col2"]);
    }

    // --- Container window tests ---

    #[test]
    fn container_window_starts_hidden() {
        let tab = new_tab();
        assert_eq!(tab.container_window, ContainerWindowState::Hidden);
        assert!(tab.container_info.is_none());
        assert!(tab.vt100_parser.is_none());
        assert!(tab.last_container_summary.is_none());
    }

    #[test]
    fn start_container_activates_window() {
        let mut tab = new_tab();
        tab.start_container("amux-test".into(), "Claude Code".into(), 78, 18);
        assert_eq!(tab.container_window, ContainerWindowState::Maximized);
        assert!(tab.container_info.is_some());
        assert!(tab.vt100_parser.is_some());
        let info = tab.container_info.as_ref().unwrap();
        assert_eq!(info.container_name, "amux-test");
        assert_eq!(info.agent_display_name, "Claude Code");
    }

    #[test]
    fn pty_data_routes_to_vt100_when_container_active() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);

        // Feed data through the vt100 parser (simulating what tick() does).
        if let Some(ref mut parser) = tab.vt100_parser {
            parser.process(b"Hello from container\r\n");
        }

        // Output goes to vt100 screen, not outer window lines.
        let screen_text = tab.vt100_parser.as_ref().unwrap().screen().contents();
        assert!(
            screen_text.contains("Hello from container"),
            "vt100 screen should contain container output"
        );
        assert!(
            tab.output_lines.is_empty()
                || !tab.output_lines.iter().any(|l| l.contains("Hello from container")),
            "Outer window should not contain container output"
        );
    }

    #[test]
    fn pty_data_routes_to_outer_when_no_container() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "test".into() };

        tab.process_pty_data(b"Hello outer\n");
        assert_eq!(tab.output_lines, vec!["Hello outer"]);
        assert!(tab.vt100_parser.is_none());
    }

    #[test]
    fn finish_command_closes_container_and_creates_summary() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 78, 18);

        tab.finish_command(0);

        assert_eq!(tab.container_window, ContainerWindowState::Hidden);
        assert!(tab.container_info.is_none());
        assert!(tab.vt100_parser.is_none());
        assert!(tab.last_container_summary.is_some());
        let summary = tab.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.container_name, "amux-test");
        assert_eq!(summary.agent_display_name, "Claude Code");
        assert_eq!(summary.exit_code, 0);
    }

    #[test]
    fn finish_command_with_error_records_exit_code() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 78, 18);

        tab.finish_command(1);

        let summary = tab.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.exit_code, 1);
    }

    #[test]
    fn start_container_clears_previous_summary() {
        let mut tab = new_tab();
        tab.last_container_summary = Some(LastContainerSummary {
            agent_display_name: "old".into(),
            container_name: "old".into(),
            avg_cpu: "0%".into(),
            avg_memory: "0MiB".into(),
            total_time: "0s".into(),
            exit_code: 0,
        });

        tab.start_container("amux-new".into(), "Claude Code".into(), 78, 18);
        assert!(tab.last_container_summary.is_none());
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(120), "2m");
        assert_eq!(format_duration(3599), "59m");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(5400), "1h 30m");
        assert_eq!(format_duration(7200), "2h");
    }

    #[test]
    fn agent_display_name_known_agents() {
        assert_eq!(agent_display_name("claude"), "Claude Code");
        assert_eq!(agent_display_name("codex"), "Codex");
        assert_eq!(agent_display_name("opencode"), "Opencode");
    }

    #[test]
    fn agent_display_name_unknown_returns_input() {
        assert_eq!(agent_display_name("custom-agent"), "custom-agent");
    }

    #[test]
    fn container_stats_history_used_for_averages() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 78, 18);

        // Simulate stats arriving
        if let Some(ref mut info) = tab.container_info {
            info.stats_history.push((5.0, 200.0));
            info.stats_history.push((10.0, 300.0));
        }

        tab.finish_command(0);

        let summary = tab.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.avg_cpu, "7.5%");
        assert_eq!(summary.avg_memory, "250MiB");
    }

    #[test]
    fn container_scroll_offset_starts_at_zero() {
        let tab = new_tab();
        assert_eq!(tab.container_scroll_offset, 0);
    }

    #[test]
    fn start_container_resets_scroll_offset() {
        let mut tab = new_tab();
        tab.container_scroll_offset = 10;
        tab.start_container("test".into(), "Agent".into(), 80, 24);
        assert_eq!(tab.container_scroll_offset, 0);
    }

    #[test]
    fn claws_wizard_defaults_correct() {
        let tab = TabState::new(std::path::PathBuf::from("/tmp"));
        assert!(tab.claws_wizard_username.is_none());
        assert!(!tab.claws_wizard_already_forked);
        assert_eq!(tab.claws_phase, ClawsPhase::Inactive);
        assert!(tab.claws_container_id.is_none());
        assert!(tab.claws_sudo_request_rx.is_none());
        assert!(tab.claws_sudo_response_tx.is_none()); // channel for Option<String> (password or None)
    }

    #[test]
    fn tick_shows_sudo_confirm_dialog_when_request_received() {
        let mut tab = new_tab();
        let (sudo_tx, sudo_rx) = tokio::sync::oneshot::channel::<()>();
        tab.claws_sudo_request_rx = Some(sudo_rx);
        // Send the signal.
        sudo_tx.send(()).unwrap();
        tab.tick();
        assert_eq!(tab.dialog, Dialog::ClawsReadySudoConfirm { password: String::new() });
        assert!(tab.claws_sudo_request_rx.is_none(), "rx should be consumed after signal");
    }

    #[test]
    fn tick_does_not_show_sudo_dialog_when_no_signal() {
        let mut tab = new_tab();
        let (_sudo_tx, sudo_rx) = tokio::sync::oneshot::channel::<()>();
        tab.claws_sudo_request_rx = Some(sudo_rx);
        // Do NOT send the signal.
        tab.tick();
        assert_eq!(tab.dialog, Dialog::None);
    }

    #[test]
    fn pending_command_claws_ready() {
        let mut tab = new_tab();
        tab.pending_command = PendingCommand::ClawsReady;
        assert_eq!(tab.pending_command, PendingCommand::ClawsReady);
    }

    #[test]
    fn finish_command_does_not_leave_stale_scroll_offset() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("test".into(), "Agent".into(), 80, 24);
        tab.container_scroll_offset = 15;
        tab.finish_command(0);
        // After finishing, container is hidden and scroll offset is irrelevant,
        // but it should be left at 0 for the next container session.
        // start_container resets it, so this just verifies no panic.
        assert_eq!(tab.container_window, ContainerWindowState::Hidden);
    }

    // --- tab_color tests ---

    #[test]
    fn tab_color_idle_is_dark_gray() {
        let tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        assert_eq!(tab.tab_color(), Color::DarkGray);
    }

    #[test]
    fn tab_color_running_no_container_is_blue() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Running { command: "chat".into() };
        assert_eq!(tab.tab_color(), Color::Blue);
    }

    #[test]
    fn tab_color_running_with_container_is_green() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.container_window = ContainerWindowState::Maximized;
        assert_eq!(tab.tab_color(), Color::Green);
    }

    #[test]
    fn tab_color_error_is_red() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Error { command: "ready".into(), exit_code: 1 };
        assert_eq!(tab.tab_color(), Color::Red);
    }

    #[test]
    fn tab_color_claws_command_no_container_is_magenta() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Running { command: "claws ready".into() };
        assert_eq!(tab.tab_color(), Color::Magenta);
    }

    #[test]
    fn tab_color_claws_command_with_container_is_magenta() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Running { command: "claws ready (attached)".into() };
        tab.container_window = ContainerWindowState::Maximized;
        assert_eq!(tab.tab_color(), Color::Magenta);
    }

    #[test]
    fn tab_color_claws_phase_active_is_magenta() {
        let mut tab = TabState::new(std::path::PathBuf::from("/tmp/proj"));
        tab.phase = ExecutionPhase::Running { command: "claws ready".into() };
        tab.claws_phase = ClawsPhase::Setup;
        assert_eq!(tab.tab_color(), Color::Magenta);
    }

    #[test]
    fn tab_display_name_idle_shows_project() {
        let tab = TabState::new(std::path::PathBuf::from("/home/user/myproject"));
        assert_eq!(tab.tab_display_name(), "myproject");
    }

    #[test]
    fn tab_display_name_running_shows_command() {
        let mut tab = TabState::new(std::path::PathBuf::from("/home/user/proj"));
        tab.phase = ExecutionPhase::Running { command: "chat --plan".into() };
        // Full command shown: "proj | chat --plan"
        assert_eq!(tab.tab_display_name(), "proj | chat --plan");
    }

    #[test]
    fn tab_display_name_truncates_long_names() {
        let tab = TabState::new(std::path::PathBuf::from("/home/user/a-very-long-project-name"));
        // "a-very-long-pr…" should be 15 chars with ellipsis
        let name = tab.tab_display_name();
        assert!(name.chars().count() <= 14, "Name too long: {}", name);
    }

    #[test]
    fn app_new_creates_one_tab() {
        let app = App::new(std::path::PathBuf::from("/tmp"));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);
        assert!(!app.should_quit);
    }

    #[test]
    fn app_create_tab_inserts_after_active() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/a"));
        let new_idx = app.create_tab(std::path::PathBuf::from("/tmp/b"));
        assert_eq!(new_idx, 1);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[1].cwd, std::path::PathBuf::from("/tmp/b"));
    }

    #[test]
    fn app_close_tab_removes_and_adjusts_idx() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/a"));
        app.create_tab(std::path::PathBuf::from("/tmp/b"));
        app.active_tab_idx = 1;
        app.close_tab(1);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);
    }

    #[test]
    fn app_close_tab_single_tab_is_noop() {
        let mut app = App::new(std::path::PathBuf::from("/tmp"));
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn app_active_tab_returns_correct_tab() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/a"));
        app.create_tab(std::path::PathBuf::from("/tmp/b"));
        app.active_tab_idx = 1;
        assert_eq!(app.active_tab().cwd, std::path::PathBuf::from("/tmp/b"));
    }

    // --- Stuck tab detection tests ---

    #[test]
    fn is_stuck_false_when_idle() {
        let tab = new_tab();
        assert!(!tab.is_stuck());
    }

    #[test]
    fn is_stuck_false_when_running_without_container() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "init".into() };
        // No container → never stuck.
        assert!(!tab.is_stuck());
    }

    #[test]
    fn is_stuck_false_when_container_just_started() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        // last_output_time was just set → not yet stuck.
        assert!(!tab.is_stuck());
    }

    #[test]
    fn is_stuck_true_when_container_silent_over_30s() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        // Wind the clock back past the timeout.
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(1)));
        assert!(tab.is_stuck());
    }

    #[test]
    fn is_stuck_false_exactly_at_timeout_boundary() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        // 29 seconds elapsed — just under the threshold.
        tab.last_output_time = Some(Instant::now() - Duration::from_secs(29));
        assert!(!tab.is_stuck());
    }

    #[test]
    fn tab_color_is_yellow_when_stuck() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(1)));
        assert_eq!(tab.tab_color(), Color::Yellow);
    }

    #[test]
    fn tab_color_not_yellow_after_acknowledge() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(1)));
        assert_eq!(tab.tab_color(), Color::Yellow);

        tab.acknowledge_stuck();
        // After acknowledging, last_output_time is reset to now → no longer stuck.
        assert_ne!(tab.tab_color(), Color::Yellow);
        assert_eq!(tab.tab_color(), Color::Green); // running + container = green
    }

    #[test]
    fn tab_subcommand_label_has_warning_prefix_when_stuck() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(1)));

        let label = tab.tab_subcommand_label(30);
        assert!(
            label.contains("⚠️"),
            "expected warning emoji in stuck label, got: {:?}",
            label
        );
        assert!(label.contains("implement 0001"), "expected command in label, got: {:?}", label);
    }

    #[test]
    fn tab_subcommand_label_no_warning_prefix_when_not_stuck() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        let label = tab.tab_subcommand_label(30);
        assert!(!label.contains('⚠'), "expected no warning in non-stuck label, got: {:?}", label);
        assert_eq!(label, "implement 0001");
    }

    #[test]
    fn tab_subcommand_label_warning_prefix_after_acknowledge_is_gone() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(1)));

        // Stuck → warning present.
        assert!(tab.tab_subcommand_label(30).contains('⚠'));

        // After acknowledgment → warning gone.
        tab.acknowledge_stuck();
        assert!(!tab.tab_subcommand_label(30).contains('⚠'));
    }

    #[test]
    fn acknowledge_stuck_is_noop_when_no_container() {
        let mut tab = new_tab();
        // last_output_time is None — acknowledge_stuck should not crash or change anything.
        tab.acknowledge_stuck();
        assert!(tab.last_output_time.is_none());
    }

    #[test]
    fn start_container_initialises_last_output_time() {
        let mut tab = new_tab();
        assert!(tab.last_output_time.is_none());
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        assert!(tab.last_output_time.is_some());
    }

    #[test]
    fn finish_command_clears_last_output_time() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        assert!(tab.last_output_time.is_some());

        tab.finish_command(0);
        assert!(tab.last_output_time.is_none());
    }

    #[test]
    fn is_stuck_false_after_finish_command() {
        let mut tab = new_tab();
        tab.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        tab.start_container("amux-test".into(), "Claude Code".into(), 80, 24);
        tab.last_output_time = Some(Instant::now() - (STUCK_TIMEOUT + Duration::from_secs(5)));
        assert!(tab.is_stuck());

        tab.finish_command(0);
        assert!(!tab.is_stuck());
    }
}
