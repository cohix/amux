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

    // --- Channels for text-based command output (init/ready) ---
    pub output_rx: UnboundedReceiver<String>,
    /// Cloned into OutputSink::Channel when launching non-PTY commands.
    pub output_tx: UnboundedSender<String>,
    /// Fires once when the current non-PTY command exits.
    pub exit_rx: Option<tokio::sync::oneshot::Receiver<i32>>,

    // --- Pending TUI state before launching a command (used by dialogs) ---
    pub pending_work_item: Option<u32>,
    pub pending_mount_path: Option<PathBuf>,

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
            output_rx,
            output_tx,
            exit_rx: None,
            pending_work_item: None,
            pending_mount_path: None,
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
        self.exit_rx = None;
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
                    let stripped = strip_ansi_escapes::strip(&bytes);
                    let text = String::from_utf8_lossy(&stripped).to_string();
                    for part in text.split('\n') {
                        let trimmed = part.trim_end_matches('\r');
                        if !trimmed.is_empty() {
                            self.push_output(trimmed.to_string());
                        }
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
}
