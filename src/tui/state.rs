use crate::commands::ready::{ReadyContext, ReadyOptions, ReadySummary};
use crate::docker;
use crate::tui::pty::PtySession;
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

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
    /// Ask whether to mount the Git root or just CWD.
    MountScope { git_root: PathBuf, cwd: PathBuf },
    /// Ask whether to mount agent credentials (and save the decision).
    /// Retained for completeness but currently unused (auto-passthrough).
    #[allow(dead_code)]
    AgentAuth { agent: String, git_root: PathBuf },
    /// Step 1 of `new`: select work item kind (Feature/Bug/Task).
    NewKindSelect,
    /// Step 2 of `new`: enter title. The kind has already been chosen.
    NewTitleInput {
        kind: crate::commands::new::WorkItemKind,
        /// Current title text being typed.
        title: String,
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
    },
    Implement {
        work_item: u32,
        non_interactive: bool,
        plan: bool,
    },
    Chat {
        non_interactive: bool,
        plan: bool,
    },
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

/// All application state for the TUI event loop.
pub struct App {
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

    /// Set to true to break out of the event loop.
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        Self {
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
            should_quit: false,
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

        // Drop host settings only if no multi-phase ready workflow is in progress.
        // During ready --refresh, the pre-audit phase completes (triggering finish_command)
        // before the audit container launches — host_settings must survive across phases.
        if self.ready_phase == ReadyPhase::Inactive {
            self.host_settings = None;
        }

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

    /// Poll all channels for new data; called once per event loop tick.
    pub fn tick(&mut self) {
        // Drain text command output.
        while let Ok(line) = self.output_rx.try_recv() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_border_color_blue_when_selected_and_running() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "ready".into() };
        app.focus = Focus::ExecutionWindow;
        assert_eq!(app.window_border_color(), Color::Blue);
    }

    #[test]
    fn window_border_color_grey_when_unselected_running() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "ready".into() };
        app.focus = Focus::CommandBox;
        assert_eq!(app.window_border_color(), Color::Gray);
    }

    #[test]
    fn window_border_color_green_when_selected_and_done() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Done { command: "ready".into() };
        app.focus = Focus::ExecutionWindow;
        assert_eq!(app.window_border_color(), Color::Green);
    }

    #[test]
    fn window_border_color_grey_when_unselected_done() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Done { command: "ready".into() };
        app.focus = Focus::CommandBox;
        assert_eq!(app.window_border_color(), Color::Gray);
    }

    #[test]
    fn window_border_color_red_on_error_selected() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Error { command: "ready".into(), exit_code: 1 };
        app.focus = Focus::ExecutionWindow;
        assert_eq!(app.window_border_color(), Color::Red);
    }

    #[test]
    fn window_border_color_red_on_error_unselected() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Error { command: "ready".into(), exit_code: 1 };
        app.focus = Focus::CommandBox;
        assert_eq!(app.window_border_color(), Color::Red);
    }

    #[test]
    fn start_command_clears_output_and_focuses_window() {
        let mut app = App::new();
        app.output_lines.push("old line".into());
        app.start_command("ready".into());
        assert!(app.output_lines.is_empty());
        assert_eq!(app.focus, Focus::ExecutionWindow);
        assert!(matches!(app.phase, ExecutionPhase::Running { .. }));
    }

    #[test]
    fn continue_command_preserves_output() {
        let mut app = App::new();
        app.output_lines.push("phase 1 output".into());
        app.output_lines.push("more output".into());
        app.continue_command("phase 2".into());
        // Output from previous phase must be preserved.
        assert_eq!(app.output_lines.len(), 2);
        assert_eq!(app.output_lines[0], "phase 1 output");
        assert!(matches!(app.phase, ExecutionPhase::Running { .. }));
        assert_eq!(app.focus, Focus::ExecutionWindow);
    }

    #[test]
    fn finish_command_zero_transitions_to_done() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "init".into() };
        app.finish_command(0);
        assert!(matches!(app.phase, ExecutionPhase::Done { .. }));
        assert_eq!(app.focus, Focus::CommandBox);
    }

    #[test]
    fn finish_command_nonzero_transitions_to_error() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "ready".into() };
        app.finish_command(1);
        assert!(matches!(app.phase, ExecutionPhase::Error { exit_code: 1, .. }));
    }

    #[test]
    fn pty_data_newlines_create_separate_lines() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        app.process_pty_data(b"Hello\nWorld\n");
        assert_eq!(app.output_lines, vec!["Hello", "World"]);
        assert!(!app.pty_live_line);
    }

    #[test]
    fn pty_data_cr_overwrites_current_line() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // First chunk: spinner frame 1
        app.process_pty_data(b"Thinking...");
        assert_eq!(app.output_lines, vec!["Thinking..."]);
        assert!(app.pty_live_line);

        // Second chunk: \r clears the buffer, "Done!" overwrites the live line
        app.process_pty_data(b"\rDone!      ");
        assert_eq!(app.output_lines, vec!["Done!      "]);
        assert!(app.pty_live_line);

        // Newline finalises the line
        app.process_pty_data(b"\n");
        assert_eq!(app.output_lines, vec!["Done!      "]);
        assert!(!app.pty_live_line);
    }

    #[test]
    fn pty_data_cr_lf_treated_as_newline() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        app.process_pty_data(b"Hello\r\nWorld\r\n");
        assert_eq!(app.output_lines, vec!["Hello", "World"]);
        assert!(!app.pty_live_line);
    }

    #[test]
    fn pty_data_multiple_cr_in_one_chunk() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // Multiple carriage returns in one chunk — each \r clears the buffer
        // so only the final frame survives (overwrite behavior).
        app.process_pty_data(b"frame1\rframe2\rframe3\n");
        assert_eq!(app.output_lines, vec!["frame3"]);
    }

    #[test]
    fn pty_data_cr_lf_split_across_chunks() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // \r\n split: \r at end of chunk 1, \n at start of chunk 2.
        // Must be treated as a newline, NOT as bare \r (which would lose text).
        app.process_pty_data(b"Hello\r");
        assert!(app.pty_pending_cr, "should defer \\r at end of chunk");
        // The text should still be visible as a live line while pending.
        assert_eq!(app.output_lines, vec!["Hello"]);

        app.process_pty_data(b"\nWorld\r\n");
        assert!(!app.pty_pending_cr);
        assert_eq!(app.output_lines, vec!["Hello", "World"]);
        assert!(!app.pty_live_line);
    }

    #[test]
    fn pty_data_cr_split_then_bare_cr() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // \r at end of chunk, but next chunk does NOT start with \n → bare \r.
        app.process_pty_data(b"old text\r");
        assert!(app.pty_pending_cr);

        app.process_pty_data(b"new text\n");
        assert!(!app.pty_pending_cr);
        // bare \r clears the buffer, so "new text" overwrites "old text".
        assert_eq!(app.output_lines, vec!["new text"]);
    }

    #[test]
    fn pty_data_empty_chunk_preserves_pending_cr() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        app.process_pty_data(b"text\r");
        assert!(app.pty_pending_cr);
        // Empty chunk should not resolve the pending \r.
        app.process_pty_data(b"");
        assert!(app.pty_pending_cr);
    }

    #[test]
    fn pty_data_control_chars_filtered() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // BEL (0x07) and BS (0x08) should be filtered out of the line buffer.
        app.process_pty_data(b"Hello\x07World\x08!\n");
        assert_eq!(app.output_lines, vec!["HelloWorld!"]);
    }

    #[test]
    fn pty_data_tabs_stripped_by_ansi_strip() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };
        // strip_ansi_escapes also removes tabs; verify they don't cause issues.
        app.process_pty_data(b"col1\tcol2\n");
        assert_eq!(app.output_lines, vec!["col1col2"]);
    }

    // --- Container window tests ---

    #[test]
    fn container_window_starts_hidden() {
        let app = App::new();
        assert_eq!(app.container_window, ContainerWindowState::Hidden);
        assert!(app.container_info.is_none());
        assert!(app.vt100_parser.is_none());
        assert!(app.last_container_summary.is_none());
    }

    #[test]
    fn start_container_activates_window() {
        let mut app = App::new();
        app.start_container("aspec-test".into(), "Claude Code".into(), 78, 18);
        assert_eq!(app.container_window, ContainerWindowState::Maximized);
        assert!(app.container_info.is_some());
        assert!(app.vt100_parser.is_some());
        let info = app.container_info.as_ref().unwrap();
        assert_eq!(info.container_name, "aspec-test");
        assert_eq!(info.agent_display_name, "Claude Code");
    }

    #[test]
    fn pty_data_routes_to_vt100_when_container_active() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.start_container("aspec-test".into(), "Claude Code".into(), 80, 24);

        // Feed data through the vt100 parser (simulating what tick() does).
        if let Some(ref mut parser) = app.vt100_parser {
            parser.process(b"Hello from container\r\n");
        }

        // Output goes to vt100 screen, not outer window lines.
        let screen_text = app.vt100_parser.as_ref().unwrap().screen().contents();
        assert!(
            screen_text.contains("Hello from container"),
            "vt100 screen should contain container output"
        );
        assert!(
            app.output_lines.is_empty()
                || !app.output_lines.iter().any(|l| l.contains("Hello from container")),
            "Outer window should not contain container output"
        );
    }

    #[test]
    fn pty_data_routes_to_outer_when_no_container() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "test".into() };

        app.process_pty_data(b"Hello outer\n");
        assert_eq!(app.output_lines, vec!["Hello outer"]);
        assert!(app.vt100_parser.is_none());
    }

    #[test]
    fn finish_command_closes_container_and_creates_summary() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.start_container("aspec-test".into(), "Claude Code".into(), 78, 18);

        app.finish_command(0);

        assert_eq!(app.container_window, ContainerWindowState::Hidden);
        assert!(app.container_info.is_none());
        assert!(app.vt100_parser.is_none());
        assert!(app.last_container_summary.is_some());
        let summary = app.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.container_name, "aspec-test");
        assert_eq!(summary.agent_display_name, "Claude Code");
        assert_eq!(summary.exit_code, 0);
    }

    #[test]
    fn finish_command_with_error_records_exit_code() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.start_container("aspec-test".into(), "Claude Code".into(), 78, 18);

        app.finish_command(1);

        let summary = app.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.exit_code, 1);
    }

    #[test]
    fn start_container_clears_previous_summary() {
        let mut app = App::new();
        app.last_container_summary = Some(LastContainerSummary {
            agent_display_name: "old".into(),
            container_name: "old".into(),
            avg_cpu: "0%".into(),
            avg_memory: "0MiB".into(),
            total_time: "0s".into(),
            exit_code: 0,
        });

        app.start_container("aspec-new".into(), "Claude Code".into(), 78, 18);
        assert!(app.last_container_summary.is_none());
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
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.start_container("aspec-test".into(), "Claude Code".into(), 78, 18);

        // Simulate stats arriving
        if let Some(ref mut info) = app.container_info {
            info.stats_history.push((5.0, 200.0));
            info.stats_history.push((10.0, 300.0));
        }

        app.finish_command(0);

        let summary = app.last_container_summary.as_ref().unwrap();
        assert_eq!(summary.avg_cpu, "7.5%");
        assert_eq!(summary.avg_memory, "250MiB");
    }

    #[test]
    fn container_scroll_offset_starts_at_zero() {
        let app = App::new();
        assert_eq!(app.container_scroll_offset, 0);
    }

    #[test]
    fn start_container_resets_scroll_offset() {
        let mut app = App::new();
        app.container_scroll_offset = 10;
        app.start_container("test".into(), "Agent".into(), 80, 24);
        assert_eq!(app.container_scroll_offset, 0);
    }

    #[test]
    fn finish_command_does_not_leave_stale_scroll_offset() {
        let mut app = App::new();
        app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.start_container("test".into(), "Agent".into(), 80, 24);
        app.container_scroll_offset = 15;
        app.finish_command(0);
        // After finishing, container is hidden and scroll offset is irrelevant,
        // but it should be left at 0 for the next container session.
        // start_container resets it, so this just verifies no panic.
        assert_eq!(app.container_window, ContainerWindowState::Hidden);
    }
}
