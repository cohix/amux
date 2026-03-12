use crate::tui::state::{App, Dialog, ExecutionPhase, Focus};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Top-level render function: draws the full TUI for one frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Outer layout: execution section (top) + status bar + command box + suggestions.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // execution window (grows)
            Constraint::Length(1), // status / hint bar
            Constraint::Length(3), // command input box
            Constraint::Length(1), // autocomplete suggestions
        ])
        .split(area);

    draw_exec_window(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);
    draw_command_box(frame, app, chunks[2]);
    draw_suggestions(frame, app, chunks[3]);

    // Dialogs are drawn on top (centered, floating).
    if app.dialog != Dialog::None {
        draw_dialog(frame, app, area);
    }
}

// --- Execution window ---

fn draw_exec_window(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = app.window_border_color();
    let border_style = Style::default().fg(border_color);

    // Calculate how many lines fit in the window (subtract borders).
    let inner_height = area.height.saturating_sub(2) as usize;
    let total = app.output_lines.len();

    let phase_label = match &app.phase {
        ExecutionPhase::Idle => " aspec ".to_string(),
        ExecutionPhase::Running { command } => format!(" ● running: {} ", command),
        ExecutionPhase::Done { command } => format!(" ✓ done: {} ", command),
        ExecutionPhase::Error { command, exit_code } => {
            format!(" ✗ error: {} (exit {}) ", command, exit_code)
        }
    };

    let block = Block::default()
        .title(phase_label)
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let lines: Vec<Line> = if app.output_lines.is_empty() {
        if matches!(app.phase, ExecutionPhase::Idle) {
            vec![
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  Welcome to aspec.",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::styled(
                    "  Running `aspec ready` to check your environment...",
                    Style::default().fg(Color::DarkGray),
                )]),
            ]
        } else {
            vec![]
        }
    } else {
        // Manual slicing: compute exactly which lines are visible.
        // scroll_offset=0 → show the newest (bottom) lines.
        // scroll_offset=N → show N lines further toward the top.
        let max_scroll = total.saturating_sub(inner_height);
        let effective_offset = app.scroll_offset.min(max_scroll);
        let start = max_scroll.saturating_sub(effective_offset);
        let end = total.min(start + inner_height);
        app.output_lines[start..end]
            .iter()
            .map(|l| Line::from(l.as_str()))
            .collect()
    };

    let para = Paragraph::new(lines).block(block);

    frame.render_widget(para, area);
}

// --- Status / hint bar ---

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let spans: Vec<Span> = match (&app.phase, &app.focus) {
        // Running + window selected: Esc to deselect.
        (ExecutionPhase::Running { .. }, Focus::ExecutionWindow) => vec![Span::styled(
            " Press Esc to deselect the window ",
            Style::default().fg(Color::Yellow),
        )],

        // Running + command box: ↑ to focus the window.
        (ExecutionPhase::Running { .. }, Focus::CommandBox) => vec![Span::styled(
            " Press ↑ to focus the window ",
            Style::default().fg(Color::DarkGray),
        )],

        // Done + window selected: Esc to deselect; ↑/↓ to scroll; b/e to jump.
        (ExecutionPhase::Done { .. }, Focus::ExecutionWindow) => vec![Span::styled(
            " ↑/↓ scroll  ·  b/e jump  ·  Esc deselect ",
            Style::default().fg(Color::DarkGray),
        )],

        // Done + command box: ↑ to focus the window.
        (ExecutionPhase::Done { .. }, Focus::CommandBox) => vec![Span::styled(
            " Press ↑ to focus the window ",
            Style::default().fg(Color::DarkGray),
        )],

        // Error + window selected: exit code + Esc + scroll hint.
        (ExecutionPhase::Error { exit_code, .. }, Focus::ExecutionWindow) => vec![
            Span::styled(
                format!(" Exit code: {} ", exit_code),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ·  ↑/↓ scroll  ·  b/e jump  ·  Esc deselect ",
                Style::default().fg(Color::DarkGray),
            ),
        ],

        // Error + command box: exit code always visible + ↑ to focus.
        (ExecutionPhase::Error { exit_code, .. }, Focus::CommandBox) => vec![
            Span::styled(
                format!(" Exit code: {} ", exit_code),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ·  Press ↑ to focus the window ",
                Style::default().fg(Color::DarkGray),
            ),
        ],

        _ => vec![],
    };

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black));
    frame.render_widget(bar, area);
}

// --- Command input box ---

fn draw_command_box(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.focus == Focus::CommandBox
        && !matches!(app.phase, ExecutionPhase::Running { .. });

    let border_color = if is_active {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .title(if is_active { " command " } else { " command (inactive) " })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    // Show error or current input.
    let content = if let Some(ref err) = app.input_error {
        vec![Line::from(vec![Span::styled(
            format!("  {}", err),
            Style::default().fg(Color::Red),
        )])]
    } else {
        let prefix = Span::styled("> ", Style::default().fg(Color::Cyan));
        let text = Span::raw(app.input.replace('\n', "↵"));
        vec![Line::from(vec![prefix, text])]
    };

    let para = Paragraph::new(content).block(block);
    frame.render_widget(para, area);

    // Render cursor when active.
    if is_active && app.input_error.is_none() {
        let cursor_x = area.x + 1 + 2 + app.cursor_col as u16; // border + "> "
        let cursor_y = area.y + 1; // inside border
        if cursor_x < area.x + area.width - 1 {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

// --- Autocomplete suggestions ---

fn draw_suggestions(frame: &mut Frame, app: &App, area: Rect) {
    if app.suggestions.is_empty() || app.focus != Focus::CommandBox {
        return;
    }

    let spans: Vec<Span> = app
        .suggestions
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            let sep = if i == 0 {
                Span::raw("  ")
            } else {
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray))
            };
            vec![
                sep,
                Span::styled(s.as_str(), Style::default().fg(Color::Cyan)),
            ]
        })
        .collect();

    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().fg(Color::DarkGray));

    frame.render_widget(para, area);
}

// --- Modal dialogs ---

fn draw_dialog(frame: &mut Frame, app: &App, area: Rect) {
    let (title, body) = match &app.dialog {
        Dialog::QuitConfirm => (
            " Quit aspec? ",
            "  Are you sure you want to quit? [y/n]  ".to_string(),
        ),
        Dialog::MountScope { git_root, cwd } => (
            " Mount Scope ",
            format!(
                "  Git root: {}\n  CWD:      {}\n\n  Mount Git root (r) or CWD only (c)? [r/c]  ",
                git_root.display(),
                cwd.display()
            ),
        ),
        Dialog::AgentAuth { agent, git_root } => (
            " Agent Credentials ",
            format!(
                "  Mount {} credentials into the container?\n  (saved for this repo: {})\n\n  [y/n]  ",
                agent,
                git_root.display()
            ),
        ),
        Dialog::None => return,
    };

    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let popup_height = 7u16.min(area.height.saturating_sub(4));
    let popup = centered_rect(popup_width, popup_height, area);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(body.as_str())
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup);
}

/// Return a centered rectangle of the given size within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::App;
    use ratatui::{backend::TestBackend, Terminal};

    /// Helper: render the app into a TestBackend and return the text content
    /// of the execution window's inner area (excluding borders).
    fn render_exec_window_lines(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw(f, app))
            .unwrap();
        let buf = terminal.backend().buffer();
        // The exec window occupies the top area. Layout: Min(5), Len(1), Len(3), Len(1).
        // So exec window height = total_height - 5 (status bar + cmd box + suggestions).
        let exec_height = height.saturating_sub(5);
        // Inner area excludes borders (1 row top, 1 row bottom, 1 col left, 1 col right).
        let inner_top = 1u16;
        let inner_left = 1u16;
        let inner_width = width.saturating_sub(2);
        let inner_rows = exec_height.saturating_sub(2);

        let mut lines = Vec::new();
        for row in inner_top..(inner_top + inner_rows) {
            let mut line = String::new();
            for col in inner_left..(inner_left + inner_width) {
                let cell = &buf[(col, row)];
                line.push_str(cell.symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    #[test]
    fn scroll_changes_visible_content_in_done_state() {
        let mut app = App::new();
        // Terminal: 40 wide, 15 tall → exec window = 15-5=10 rows → inner = 8 rows
        // Add 20 lines of output so there's content to scroll through.
        for i in 0..20 {
            app.output_lines.push(format!("line {}", i));
        }
        app.phase = ExecutionPhase::Done {
            command: "ready".into(),
        };
        app.focus = Focus::ExecutionWindow;

        // scroll_offset=0 → should show the LAST 8 lines (lines 12-19).
        app.scroll_offset = 0;
        let view0 = render_exec_window_lines(&app, 40, 15);
        assert!(
            view0.iter().any(|l| l.contains("line 19")),
            "scroll_offset=0 should show line 19 (newest). Got: {:?}",
            view0
        );
        assert!(
            !view0.iter().any(|l| l.contains("line 0")),
            "scroll_offset=0 should NOT show line 0 (oldest). Got: {:?}",
            view0
        );

        // scroll_offset=5 → should show earlier content.
        app.scroll_offset = 5;
        let view5 = render_exec_window_lines(&app, 40, 15);
        assert!(
            view5.iter().any(|l| l.contains("line 7")),
            "scroll_offset=5 should show line 7. Got: {:?}",
            view5
        );

        // The two views must differ.
        assert_ne!(
            view0, view5,
            "Scrolling must change the visible content"
        );

        // scroll_offset=max → should show the FIRST lines.
        app.scroll_offset = 20;
        let view_top = render_exec_window_lines(&app, 40, 15);
        assert!(
            view_top.iter().any(|l| l.contains("line 0")),
            "scroll_offset=max should show line 0 (oldest). Got: {:?}",
            view_top
        );
    }
}
