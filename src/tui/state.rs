use crate::commands::ready::{ReadyContext, ReadyOptions, ReadySummary};
use crate::tui::pty::PtySession;
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
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
        non_interactive: bool,
    },
    Implement {
        work_item: u32,
        non_interactive: bool,
    },
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
    }

    /// Process raw PTY output bytes, handling carriage returns (`\r`) correctly.
    ///
    /// Terminal applications use `\r` (without `\n`) to move the cursor back to
    /// column 0 so the next output overwrites the current line — this is how
    /// spinners and progress indicators work. `\r\n` is treated as a newline.
    ///
    /// The method maintains `pty_line_buffer` (the current incomplete line) and
    /// a "live line" at the end of `output_lines` that is updated in-place
    /// until a `\n` finalises it.
    fn process_pty_data(&mut self, bytes: &[u8]) {
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
                // Previous \r was a bare carriage return → finalise (not clear)
                // so content from full-screen apps (cursor movement after \r) is preserved.
                self.finalise_pty_line();
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
                            // Bare \r → finalise current line (not clear) so content
                            // from full-screen apps using cursor movement is preserved.
                            self.finalise_pty_line();
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

    /// Finalise the current PTY line buffer: push it to output_lines
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
                    self.process_pty_data(&bytes);
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

        // Second chunk: \r finalises "Thinking..." then "Done!" becomes live line
        app.process_pty_data(b"\rDone!      ");
        assert_eq!(app.output_lines, vec!["Thinking...", "Done!      "]);
        assert!(app.pty_live_line);

        // Newline finalises the line
        app.process_pty_data(b"\n");
        assert_eq!(app.output_lines, vec!["Thinking...", "Done!      "]);
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
        // Multiple carriage returns in one chunk — each \r finalises the line
        app.process_pty_data(b"frame1\rframe2\rframe3\n");
        assert_eq!(app.output_lines, vec!["frame1", "frame2", "frame3"]);
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
        // bare \r finalised "old text" as its own line, then "new text" was finalized.
        assert_eq!(app.output_lines, vec!["old text", "new text"]);
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
}
