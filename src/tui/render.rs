use crate::tui::state::{
    App, TabState, ContainerWindowState, Dialog, ExecutionPhase, Focus, LastContainerSummary,
};
use crate::workflow::{StepStatus, WorkflowState};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Top-level render function: draws the full TUI for one frame.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Vertical split: tab bar (3 rows) + main content area.
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);
    let tab_bar_area = vert[0];
    let main_area = vert[1];

    draw_tab_bar(frame, app, tab_bar_area);

    let tab = app.active_tab_mut();

    // Determine if we need a minimized container bar or a summary bar.
    let show_minimized_bar = tab.container_window == ContainerWindowState::Minimized;
    let show_summary = !show_minimized_bar
        && tab.container_window == ContainerWindowState::Hidden
        && tab.last_container_summary.is_some();
    let extra_bar_height = if show_minimized_bar || show_summary { 3 } else { 0 };

    // Determine workflow strip height (0 if no workflow active).
    let workflow_strip_height = if tab.workflow.is_some() {
        workflow_strip_height(tab.workflow.as_ref().unwrap())
    } else {
        0
    };

    let mut constraint_list: Vec<Constraint> = vec![Constraint::Min(5)];
    if extra_bar_height > 0 {
        constraint_list.push(Constraint::Length(extra_bar_height));
    }
    if workflow_strip_height > 0 {
        constraint_list.push(Constraint::Length(workflow_strip_height));
    }
    constraint_list.push(Constraint::Length(1)); // status bar
    constraint_list.push(Constraint::Length(3)); // command box
    constraint_list.push(Constraint::Length(1)); // suggestions

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraint_list)
        .split(main_area);

    let mut idx = 0usize;
    let exec_area = chunks[idx];
    idx += 1;

    draw_exec_window(frame, tab, exec_area);

    if extra_bar_height > 0 {
        if show_minimized_bar {
            draw_minimized_container_bar(frame, tab, chunks[idx]);
        } else if show_summary {
            draw_container_summary(frame, tab.last_container_summary.as_ref().unwrap(), chunks[idx]);
        }
        idx += 1;
    }

    if workflow_strip_height > 0 {
        if let Some(wf) = tab.workflow.as_ref() {
            draw_workflow_strip(frame, wf, tab.workflow_current_step.as_deref(), chunks[idx]);
        }
        idx += 1;
    }

    draw_status_bar(frame, tab, chunks[idx]);
    idx += 1;
    draw_command_box(frame, tab, chunks[idx]);
    idx += 1;
    draw_suggestions(frame, tab, chunks[idx]);

    if tab.container_window == ContainerWindowState::Maximized {
        draw_container_window(frame, tab, exec_area);
    }

    if tab.dialog != Dialog::None {
        draw_dialog(frame, tab, area);
    }
}

/// Calculate the inner dimensions of the container window for a given terminal size.
///
/// This mirrors the layout used in `draw_container_window` so the vt100 parser
/// and PTY are sized to match the actual rendered area.
///
/// `extra_rows` accounts for additional UI strips that reduce the exec area height
/// (e.g. the workflow status strip). Pass 0 when no extra strips are visible.
pub fn calculate_container_inner_size(term_cols: u16, term_rows: u16, extra_rows: u16) -> (u16, u16) {
    // No sidebar. Tab bar takes 3 rows at top.
    // Fixed rows below tab bar: status(1) + cmd(3) + suggest(1) = 5.
    let exec_height = term_rows.saturating_sub(3 + 5 + extra_rows);
    // Container window: 95% of exec area, centered.
    let container_height = (exec_height * 95 / 100).max(5);
    let container_width = (term_cols * 95 / 100).max(10);
    // Inner area excludes borders.
    let inner_rows = container_height.saturating_sub(2);
    let inner_cols = container_width.saturating_sub(2);
    (inner_cols, inner_rows)
}

// --- Tab bar (horizontal, top) ---

fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    const TAB_WIDTH: u16 = 20;
    for (i, tab) in app.tabs.iter().enumerate() {
        let x = area.x + (i as u16) * TAB_WIDTH;
        if x + TAB_WIDTH > area.x + area.width {
            break;
        }
        let is_active = i == app.active_tab_idx;
        // All tabs share the same 3-row height, flush to the top of the tab bar area.
        let tab_area = Rect { x, y: area.y, width: TAB_WIDTH, height: 3 };
        let color = tab.tab_color();
        let project = tab.tab_project_name();
        let subcmd = tab.tab_subcommand_label(TAB_WIDTH);

        let (border_style, title_style, content_style) = if is_active {
            (
                Style::default().fg(color),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                Style::default().fg(color),
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
            )
        };

        let title_text = if is_active {
            format!(" ➡ {} ", project)
        } else {
            format!(" {} ", project)
        };

        let borders = if is_active {
            Borders::TOP | Borders::LEFT | Borders::RIGHT
        } else {
            Borders::ALL
        };

        let block = Block::default()
            .title(Span::styled(title_text, title_style))
            .borders(borders)
            .border_type(BorderType::Rounded)
            .border_style(border_style);

        let content = Paragraph::new(Line::from(Span::styled(
            format!(" {} ", subcmd),
            content_style,
        )))
        .block(block);
        frame.render_widget(content, tab_area);
    }
}

// --- Execution window (outer window) ---

fn draw_exec_window(frame: &mut Frame, tab: &TabState, area: Rect) {
    let border_color = tab.window_border_color();
    let border_style = Style::default().fg(border_color);

    // Calculate how many visual rows fit in the window (subtract borders).
    let inner_height = area.height.saturating_sub(2) as usize;

    let phase_label = match &tab.phase {
        ExecutionPhase::Idle => " amux ".to_string(),
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

    let inner_width = area.width.saturating_sub(2) as usize; // exclude borders

    let lines: Vec<Line> = if tab.output_lines.is_empty() {
        if matches!(tab.phase, ExecutionPhase::Idle) {
            vec![
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  Welcome to amux.",
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::from(vec![Span::styled(
                    "  Running `amux ready` to check your environment...",
                    Style::default().fg(Color::DarkGray),
                )]),
            ]
        } else {
            vec![]
        }
    } else {
        tab.output_lines
            .iter()
            .map(|l| Line::from(l.as_str()))
            .collect()
    };

    // Calculate how many visual rows the content takes, using display width
    // (via Line::width()) instead of byte length.
    let total_visual: usize = if inner_width == 0 {
        lines.len()
    } else {
        lines
            .iter()
            .map(|l| {
                let w = l.width();
                if w == 0 { 1 } else { (w + inner_width - 1) / inner_width }
            })
            .sum()
    };
    let max_scroll = total_visual.saturating_sub(inner_height);
    let effective_offset = tab.scroll_offset.min(max_scroll);
    let scroll_y = max_scroll.saturating_sub(effective_offset);

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));
    frame.render_widget(para, area);
}

// --- Container window (overlay on top of outer window) ---

fn draw_container_window(frame: &mut Frame, tab: &mut TabState, outer_area: Rect) {
    // Container window takes 95% of the outer window's width and height, centered.
    let container_height = (outer_area.height * 95 / 100).max(5);
    let container_width = (outer_area.width * 95 / 100).max(10);
    let offset_x = (outer_area.width.saturating_sub(container_width)) / 2;
    let offset_y = (outer_area.height.saturating_sub(container_height)) / 2;
    let container_area = Rect {
        x: outer_area.x + offset_x,
        y: outer_area.y + offset_y,
        width: container_width,
        height: container_height,
    };

    // Clear the area under the container window.
    frame.render_widget(Clear, container_area);

    // Build title strings.
    let agent_name = tab
        .container_info
        .as_ref()
        .map(|i| i.agent_display_name.as_str())
        .unwrap_or("Agent");
    let left_title = format!(" \u{1F512} {} (containerized) ", agent_name);

    let right_title = build_stats_title(tab);

    let mut block = Block::default()
        .title(Line::from(left_title).alignment(Alignment::Left))
        .title(Line::from(right_title).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));

    // Show scroll indicator when viewing scrollback.
    if tab.container_scroll_offset > 0 {
        let scroll_hint = format!(" \u{2191} scrollback ({} lines up) ", tab.container_scroll_offset);
        block = block.title(
            Line::from(Span::styled(scroll_hint, Style::default().fg(Color::Yellow)))
                .alignment(Alignment::Center),
        );
    }

    let inner = block.inner(container_area);
    frame.render_widget(block, container_area);

    // Render the vt100 terminal emulator screen into the inner area.
    if let Some(ref mut parser) = tab.vt100_parser {
        if tab.container_scroll_offset > 0 {
            // set_scrollback() supports offsets up to the screen row count.
            // Cap to the screen row count to avoid overflow in the vt100 grid.
            let max_safe = parser.screen().size().0 as usize;
            let offset = tab.container_scroll_offset.min(max_safe);
            if offset > 0 {
                parser.set_scrollback(offset);
                render_vt100_screen_no_cursor(frame, parser.screen(), inner);
                parser.set_scrollback(0);
            } else {
                render_vt100_screen(frame, parser.screen(), inner);
            }
        } else {
            render_vt100_screen(frame, parser.screen(), inner);
        }
    }
}

/// Render a vt100 screen into a ratatui buffer area, preserving colors,
/// bold/italic/underline, and cursor position.
fn render_vt100_screen(frame: &mut Frame, screen: &vt100::Screen, area: Rect) {
    let buf = frame.buffer_mut();
    let rows = area.height as usize;
    let cols = area.width as usize;
    let screen_rows = screen.size().0 as usize;
    let screen_cols = screen.size().1 as usize;

    for row in 0..rows.min(screen_rows) {
        let mut col = 0;
        while col < cols.min(screen_cols) {
            let cell = screen.cell(row as u16, col as u16);
            let x = area.x + col as u16;
            let y = area.y + row as u16;

            if let Some(cell) = cell {
                let contents = cell.contents();
                let mut style = Style::default();
                style = style.fg(convert_vt100_color(cell.fgcolor()));
                style = style.bg(convert_vt100_color(cell.bgcolor()));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                if contents.is_empty() {
                    buf[(x, y)].set_symbol(" ").set_style(style);
                } else {
                    buf[(x, y)].set_symbol(&contents).set_style(style);
                }
            }

            col += 1;
        }
    }

    // Render cursor position.
    if !screen.hide_cursor() {
        let (cursor_row, cursor_col) = screen.cursor_position();
        let cx = area.x + cursor_col;
        let cy = area.y + cursor_row;
        if cx < area.x + area.width && cy < area.y + area.height {
            frame.set_cursor_position((cx, cy));
        }
    }
}

/// Render a vt100 screen into a ratatui buffer area without showing the cursor.
/// Used when viewing scrollback history.
fn render_vt100_screen_no_cursor(frame: &mut Frame, screen: &vt100::Screen, area: Rect) {
    let buf = frame.buffer_mut();
    let rows = area.height as usize;
    let cols = area.width as usize;
    let screen_rows = screen.size().0 as usize;
    let screen_cols = screen.size().1 as usize;

    for row in 0..rows.min(screen_rows) {
        let mut col = 0;
        while col < cols.min(screen_cols) {
            let cell = screen.cell(row as u16, col as u16);
            let x = area.x + col as u16;
            let y = area.y + row as u16;

            if let Some(cell) = cell {
                let contents = cell.contents();
                let mut style = Style::default();
                style = style.fg(convert_vt100_color(cell.fgcolor()));
                style = style.bg(convert_vt100_color(cell.bgcolor()));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                if contents.is_empty() {
                    buf[(x, y)].set_symbol(" ").set_style(style);
                } else {
                    buf[(x, y)].set_symbol(&contents).set_style(style);
                }
            }

            col += 1;
        }
    }
}

/// Convert a vt100 color to a ratatui color.
fn convert_vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

// --- Minimized container bar ---

fn draw_minimized_container_bar(frame: &mut Frame, tab: &TabState, area: Rect) {
    let agent_name = tab
        .container_info
        .as_ref()
        .map(|i| i.agent_display_name.as_str())
        .unwrap_or("Agent");
    let stats_title = build_stats_title(tab);

    let content = format!(
        "\u{1F512} {} | {}",
        agent_name,
        stats_title.trim()
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));

    let para = Paragraph::new(Line::from(vec![Span::styled(
        format!(" {}", content),
        Style::default().fg(Color::Green),
    )]))
    .block(block);

    frame.render_widget(para, area);
}

// --- Container summary bar (after container exits) ---

fn draw_container_summary(frame: &mut Frame, summary: &LastContainerSummary, area: Rect) {
    let exit_text = if summary.exit_code == 0 {
        "exit 0".to_string()
    } else {
        format!("exit {}", summary.exit_code)
    };

    let content = format!(
        " {} | {} | avg {} | avg {} | {} | {}",
        summary.agent_display_name,
        summary.container_name,
        summary.avg_cpu,
        summary.avg_memory,
        summary.total_time,
        exit_text,
    );

    // Use a custom border set with dashed lines for the summary.
    let border_set = ratatui::symbols::border::Set {
        top_left: "╭",
        top_right: "╮",
        bottom_left: "╰",
        bottom_right: "╯",
        horizontal_top: "╌",
        horizontal_bottom: "╌",
        vertical_left: "┆",
        vertical_right: "┆",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border_set)
        .border_style(Style::default().fg(Color::DarkGray));

    let color = if summary.exit_code == 0 {
        Color::DarkGray
    } else {
        Color::Red
    };

    let para = Paragraph::new(Line::from(vec![Span::styled(
        content,
        Style::default().fg(color),
    )]))
    .block(block);

    frame.render_widget(para, area);
}

/// Build the right-side title for the container window: "name | cpu | mem | time"
fn build_stats_title(tab: &TabState) -> String {
    let info = match &tab.container_info {
        Some(i) => i,
        None => return String::new(),
    };

    let elapsed = info.start_time.elapsed().as_secs();
    let time_str = crate::tui::state::format_duration(elapsed);

    if let Some(ref stats) = info.latest_stats {
        format!(
            " {} | {} | {} | {} ",
            stats.name, stats.cpu_percent, stats.memory, time_str
        )
    } else {
        format!(" {} | ... | ... | {} ", info.container_name, time_str)
    }
}

// --- Status / hint bar ---

fn draw_status_bar(frame: &mut Frame, tab: &TabState, area: Rect) {
    let spans: Vec<Span> = match (&tab.phase, &tab.focus, &tab.container_window) {
        // Container maximized + window focused: Esc to minimize, scroll for history.
        (ExecutionPhase::Running { .. }, Focus::ExecutionWindow, ContainerWindowState::Maximized) => {
            vec![Span::styled(
                " Esc minimize  ·  scroll ↕ history ",
                Style::default().fg(Color::Yellow),
            )]
        }

        // Container minimized + window focused: hints for scrolling + c to restore.
        (ExecutionPhase::Running { .. }, Focus::ExecutionWindow, ContainerWindowState::Minimized) => {
            vec![Span::styled(
                " ↑/↓ scroll  ·  b/e jump  ·  c restore container  ·  Esc deselect ",
                Style::default().fg(Color::DarkGray),
            )]
        }

        // Running + window selected (no container): Esc to deselect.
        (ExecutionPhase::Running { .. }, Focus::ExecutionWindow, ContainerWindowState::Hidden) => {
            vec![Span::styled(
                " Press Esc to deselect the window ",
                Style::default().fg(Color::Yellow),
            )]
        }

        // Running + command box: ↑ to focus the window.
        (ExecutionPhase::Running { .. }, Focus::CommandBox, _) => vec![Span::styled(
            " Press ↑ to focus the window ",
            Style::default().fg(Color::DarkGray),
        )],

        // Done + window selected: Esc to deselect; ↑/↓ to scroll; b/e to jump.
        (ExecutionPhase::Done { .. }, Focus::ExecutionWindow, _) => vec![Span::styled(
            " ↑/↓ scroll  ·  b/e jump  ·  Esc deselect ",
            Style::default().fg(Color::DarkGray),
        )],

        // Done + command box: ↑ to focus the window.
        (ExecutionPhase::Done { .. }, Focus::CommandBox, _) => vec![Span::styled(
            " Press ↑ to focus the window ",
            Style::default().fg(Color::DarkGray),
        )],

        // Error + window selected: exit code + Esc + scroll hint.
        (ExecutionPhase::Error { exit_code, .. }, Focus::ExecutionWindow, _) => vec![
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
        (ExecutionPhase::Error { exit_code, .. }, Focus::CommandBox, _) => vec![
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

fn draw_command_box(frame: &mut Frame, tab: &TabState, area: Rect) {
    let is_running = matches!(tab.phase, ExecutionPhase::Running { .. });
    let is_active = tab.focus == Focus::CommandBox && !is_running;

    let border_color = if is_active { Color::Cyan } else { Color::DarkGray };

    let title = if is_active { " command " } else { " command (inactive) " };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let content = if is_running && tab.focus == Focus::CommandBox {
        // Blocked: show hint about creating new tab
        vec![Line::from(vec![Span::styled(
            "  Press Ctrl+T to run another command in a new tab",
            Style::default().fg(Color::DarkGray),
        )])]
    } else if let Some(ref err) = tab.input_error {
        vec![Line::from(vec![Span::styled(
            format!("  {}", err),
            Style::default().fg(Color::Red),
        )])]
    } else {
        let prefix = Span::styled("> ", Style::default().fg(Color::Cyan));
        let text = Span::raw(tab.input.replace('\n', "↵"));
        vec![Line::from(vec![prefix, text])]
    };

    let para = Paragraph::new(content).block(block);
    frame.render_widget(para, area);

    if is_active && tab.input_error.is_none() {
        let cursor_x = area.x + 1 + 2 + tab.cursor_col as u16;
        let cursor_y = area.y + 1;
        if cursor_x < area.x + area.width - 1 {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

// --- Autocomplete suggestions ---

fn draw_suggestions(frame: &mut Frame, tab: &TabState, area: Rect) {
    if tab.focus != Focus::CommandBox {
        return;
    }
    if tab.suggestions.is_empty() {
        // Show the current working directory when no suggestions.
        let cwd_str = tab.cwd.to_string_lossy();
        let para = Paragraph::new(Line::from(vec![
            Span::styled("  cwd: ", Style::default().fg(Color::DarkGray)),
            Span::styled(cwd_str.as_ref().to_string(), Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(para, area);
        return;
    }

    let spans: Vec<Span> = tab
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

    let para = Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(para, area);
}

// --- Modal dialogs ---

fn draw_dialog(frame: &mut Frame, tab: &TabState, area: Rect) {
    // Special dialogs that do their own rendering.
    if let Dialog::NewInterviewSummary { kind, title, work_item_number, summary, cursor_pos } = &tab.dialog {
        draw_interview_summary_dialog(frame, kind, title, *work_item_number, summary, *cursor_pos, area);
        return;
    }

    let (title, body) = match &tab.dialog {
        Dialog::CloseTabConfirm => (
            " Ctrl+C pressed ",
            " Press ctrl-c again to quit amux, ctrl-t to close the current tab, or esc to cancel  ".to_string(),
        ),
        Dialog::NewTabDirectory { input } => (
            " New Tab — Working Directory ",
            format!(
                "  Enter working directory for new tab:\n\n  > {}\n\n  [Enter to confirm, Esc to cancel]  ",
                input
            ),
        ),
        Dialog::QuitConfirm => (
            " Ctrl+C pressed ",
            " Press ctrl-c again to quit amux, or esc to cancel  ".to_string(),
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
        Dialog::NewKindSelect { .. } => (
            " New Work Item — Type ",
            "  Select work item type:\n\n  1) Feature\n  2) Bug\n  3) Task\n  4) Enhancement\n\n  [1/2/3/4 or Esc to cancel]  ".to_string(),
        ),
        Dialog::NewTitleInput { kind, title: item_title, .. } => (
            " New Work Item — Title ",
            format!(
                "  Type: {}\n\n  Enter title: {}\n\n  [Enter to confirm, Esc to cancel]  ",
                kind.as_str(),
                item_title
            ),
        ),
        Dialog::ClawsReadyHasForked => (
            " Claws Ready — Fork ",
            "  Have you already forked nanoclaw on GitHub?\n\n  1) Yes\n  2) No (fork first)\n\n  [1/2 or Esc to cancel]  ".to_string(),
        ),
        Dialog::ClawsReadyUsernameInput { username } => (
            " Claws Ready — GitHub Username ",
            format!(
                "  Enter your GitHub username (fork owner):\n\n  > {}\n\n  [Enter to confirm, Esc to cancel]  ",
                username
            ),
        ),
        Dialog::ClawsAuditConfirm => (
            " Claws Init — Agent Audit ",
            "  amux will launch your code agent inside the container to configure\n  \
nanoclaw for containerized networking.\n\n  \
Allow the agent to work (could take up to 15m). When it finishes,\n  \
run /setup in the same agent session — no need to reattach.\n  \
The container continues running after you close the session.\n\n  \
Press y or 1 to accept and launch the agent,\n  \
or n or 2 (or Esc) to cancel.  ".to_string(),
        ),
        Dialog::ClawsReadyDockerSocketWarning => (
            " Claws Ready — Docker Socket Warning ",
            "  The nanoclaw container will be mounted to the host\n  Docker socket (like --allow-docker).\n  This grants elevated access to Docker.\n\n  Accept Docker socket access? [1=yes/2=no]  ".to_string(),
        ),
        Dialog::ClawsReadyOfferRestartStopped { container_id, name, created } => (
            " Claws Ready — Restart Stopped Container ",
            format!(
                "  Found a stopped nanoclaw container:\n\n  Name:    {}\n  ID:      {}\n  Created: {}\n\n  Start this stopped container? [1=yes/2=no]  ",
                name,
                &container_id[..container_id.len().min(12)],
                created,
            ),
        ),
        Dialog::ClawsRestartFailedOfferFresh { container_id } => (
            " Claws Ready — Restart Failed ",
            format!(
                "  Failed to start container {}.\n  The bind-mount sources (e.g. claude.json) may have been\n  cleaned up since the container was created.\n\n  Delete this container and start a fresh one? [1=yes/2=no]  ",
                &container_id[..container_id.len().min(12)],
            ),
        ),
        Dialog::ClawsReadyOfferStart => (
            " Claws Ready — Run Fresh Container ",
            format!(
                "  Run a fresh '{}' container? [1=yes/2=no]  ",
                crate::commands::claws::NANOCLAW_CONTROLLER_NAME,
            ),
        ),
        Dialog::ClawsReadySudoConfirm { password } => (
            " Claws Ready — Sudo Password ",
            format!(
                "  Clone to {} failed: permission denied.\n  Enter your sudo password to retry with sudo.\n\n  Password: {}\n\n  [Enter to confirm, Esc to cancel]  ",
                crate::commands::claws::nanoclaw_path_str(),
                "*".repeat(password.len()),
            ),
        ),
        Dialog::WorkflowStepConfirm { completed_step, next_steps } => (
            " Workflow Step Complete ",
            format!(
                "  Step '{}' completed successfully.\n\n  Next step(s): {}\n\n  \
                 [Enter/y] Advance to next step  [q/n/Esc] Pause workflow  ",
                completed_step,
                if next_steps.is_empty() { "none".to_string() } else { next_steps.join(", ") }
            ),
        ),
        Dialog::WorkflowStepError { failed_step, error } => (
            " Workflow Step Failed ",
            format!(
                "  Step '{}' failed.\n  Error: {}\n\n  \
                 [r/1] Retry step  [q/n/Esc] Pause workflow  ",
                failed_step,
                if error.len() > 60 { &error[..60] } else { error.as_str() }
            ),
        ),
        Dialog::None => return,
        // NewInterviewSummary is handled by the early return above — this arm is unreachable.
        Dialog::NewInterviewSummary { .. } => return,
    };

    let popup_width = 72u16.min(area.width.saturating_sub(4));
    // Height = line count + 2 border rows, capped to terminal height.
    let line_count = body.chars().filter(|&c| c == '\n').count() as u16 + 1;
    let popup_height = (line_count + 2).max(5).min(area.height.saturating_sub(4));
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

fn draw_interview_summary_dialog(
    frame: &mut Frame,
    kind: &crate::commands::new::WorkItemKind,
    title: &str,
    work_item_number: u32,
    summary: &str,
    cursor_pos: usize,
    area: Rect,
) {
    // Compute popup size: 80% width, 60% height, min 16 rows
    let popup_width = ((area.width as u32 * 80 / 100) as u16).min(82).max(40);
    let popup_height = ((area.height as u32 * 60 / 100) as u16)
        .max(16)
        .min(area.height.saturating_sub(4));
    let popup = centered_rect(popup_width, popup_height, area);

    frame.render_widget(Clear, popup);

    // Outer block
    let outer_block = Block::default()
        .title(" Interview Summary ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = outer_block.inner(popup);
    frame.render_widget(outer_block, popup);

    if inner.height < 6 {
        return;
    }

    // Header rows: info + blank + instructions + blank
    let header_height = 4u16;
    let footer_height = 1u16;
    let text_area_height = inner.height.saturating_sub(header_height + footer_height);

    // Layout inside the popup inner area
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(text_area_height.max(3)),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let header_area = layout[0];
    let text_outer_area = layout[1];
    let footer_area = layout[2];

    // Render header
    let header_text = vec![
        Line::from(vec![Span::styled(
            format!("  {:04} — {}: {}", work_item_number, kind.as_str(), title),
            Style::default().fg(Color::Cyan),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Describe the work item. The code agent will complete the details.",
            Style::default().fg(Color::DarkGray),
        )]),
        Line::from(""),
    ];
    let header_para = Paragraph::new(header_text);
    frame.render_widget(header_para, header_area);

    // Render text area with border
    let text_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::White));
    let text_inner = text_block.inner(text_outer_area);
    frame.render_widget(text_block, text_outer_area);

    // Split summary into logical lines
    let lines: Vec<&str> = summary.split('\n').collect();

    // Compute cursor logical row and col (in chars)
    let before_cursor = &summary[..cursor_pos.min(summary.len())];
    let cursor_logical_row = before_cursor.chars().filter(|&c| c == '\n').count();
    let last_newline_byte = before_cursor.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let cursor_logical_col = before_cursor[last_newline_byte..].chars().count();

    // Text width for wrapping (-1 for the " " prefix)
    let text_width = (text_inner.width.saturating_sub(1) as usize).max(1);

    // Build visual lines and find cursor visual position
    let mut all_visual_lines: Vec<String> = Vec::new();
    let mut cursor_visual_row = 0usize;
    let mut cursor_visual_col = 0usize;

    for (logical_row, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let line_char_len = chars.len();
        let num_visual = if line_char_len == 0 { 1 } else { (line_char_len + text_width - 1) / text_width };

        if logical_row == cursor_logical_row {
            cursor_visual_row = all_visual_lines.len() + cursor_logical_col / text_width;
            cursor_visual_col = cursor_logical_col % text_width;
        }

        for chunk_idx in 0..num_visual {
            let start = chunk_idx * text_width;
            let end = (start + text_width).min(line_char_len);
            let chunk: String = chars[start..end].iter().collect();
            all_visual_lines.push(chunk);
        }
    }

    // Scroll to keep cursor visible
    let visible_rows = text_inner.height as usize;
    let scroll_start = if cursor_visual_row >= visible_rows {
        cursor_visual_row + 1 - visible_rows
    } else {
        0
    };

    // Render visible visual lines
    let visible_lines: Vec<Line> = all_visual_lines
        .iter()
        .skip(scroll_start)
        .take(visible_rows)
        .map(|line| Line::from(format!(" {}", line)))
        .collect();

    let text_para = Paragraph::new(visible_lines);
    frame.render_widget(text_para, text_inner);

    // Place cursor
    let cursor_visible_row = cursor_visual_row.saturating_sub(scroll_start);
    let cx = text_inner.x + 1 + cursor_visual_col as u16; // +1 for the " " prefix
    let cy = text_inner.y + cursor_visible_row as u16;
    if cx < text_inner.x + text_inner.width && cy < text_inner.y + text_inner.height {
        frame.set_cursor_position((cx, cy));
    }

    // Render footer
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("  Ctrl+S / Ctrl+Enter to submit", Style::default().fg(Color::Green)),
        Span::raw("  ·  "),
        Span::styled("Esc to cancel", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(footer, footer_area);
}

/// Return a centered rectangle of the given size within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}

// ─── Workflow strip ───────────────────────────────────────────────────────────

/// Compute the height in rows needed for the workflow status strip.
/// Returns 0 if there are no parallel groups (sequential only = 1 box row = 3 rows),
/// and up to 3 box-rows tall (9 rows max) for parallel groups.
pub fn workflow_strip_height(wf: &WorkflowState) -> u16 {
    let max_parallel = max_parallel_group_size(wf);
    // Cap at 3 box-rows. Each box is 3 rows tall.
    let rows = max_parallel.min(3) as u16;
    rows * 3
}

/// Return the size of the largest parallel group (steps sharing same depends_on set).
fn max_parallel_group_size(wf: &WorkflowState) -> usize {
    use std::collections::HashMap;
    let mut group_sizes: HashMap<Vec<String>, usize> = HashMap::new();
    for step in &wf.steps {
        let mut key = step.depends_on.clone();
        key.sort();
        *group_sizes.entry(key).or_insert(0) += 1;
    }
    group_sizes.values().copied().max().unwrap_or(1)
}

/// Render the workflow status strip.
///
/// The strip shows steps arranged left-to-right in topological order.
/// Parallel steps (same depends_on) are stacked vertically (max 3 rows), with
/// subsequent ones indented slightly to show they'll run sequentially.
pub fn draw_workflow_strip(
    frame: &mut Frame,
    wf: &WorkflowState,
    current_step: Option<&str>,
    area: Rect,
) {
    // Layout: one column per topological "level" (unique depends_on set in order).
    // Build ordered columns.
    let columns = build_workflow_columns(wf);
    if columns.is_empty() {
        return;
    }

    let num_cols = columns.len() as u16;
    // Distribute all available width across columns.
    // Reserve 1 char per inter-column arrow (N-1 arrows for N columns).
    // Each column box gets an equal share of the remaining space; the last
    // column absorbs any remainder so the strip always fills the full width.
    let arrow_chars = num_cols.saturating_sub(1);
    let box_space = area.width.saturating_sub(arrow_chars);
    let base_col_w = (box_space / num_cols).max(4);
    // Stride = box width + 1 arrow char between columns.
    let col_stride = base_col_w + 1;

    // Render each column.
    for (col_idx, col_steps) in columns.iter().enumerate() {
        let col_x = area.x + col_idx as u16 * col_stride;
        if col_x >= area.x + area.width {
            break; // Out of space — stop rendering columns.
        }

        // Last column gets any remaining space so the strip fills the full width.
        let this_col_w = if col_idx + 1 == columns.len() {
            (area.x + area.width).saturating_sub(col_x)
        } else {
            base_col_w
        };

        let visible_rows = (area.height / 3).max(1) as usize;
        let steps_to_show: Vec<_> = col_steps.iter().take(visible_rows).collect();
        let hidden_count = col_steps.len().saturating_sub(visible_rows);

        for (row_idx, step_name) in steps_to_show.iter().enumerate() {
            let row_y = area.y + row_idx as u16 * 3;
            if row_y + 3 > area.y + area.height {
                break;
            }

            // Indent by row_idx if there are multiple steps in column (parallel group).
            let indent = if col_steps.len() > 1 { row_idx as u16 } else { 0 };
            let box_x = (col_x + indent).min(area.x + area.width.saturating_sub(4));
            let box_w = this_col_w.saturating_sub(indent).max(4);

            let step = wf.get_step(step_name).unwrap();
            let is_current = current_step == Some(step_name.as_str());

            let (label, style) = step_box_label_and_style(step_name, &step.status, is_current, box_w);

            let box_area = Rect {
                x: box_x,
                y: row_y,
                width: box_w,
                height: 3,
            };

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(style);

            let para = Paragraph::new(label)
                .block(block)
                .style(style);
            frame.render_widget(para, box_area);

            // Render arrow between columns (→) in the 1-char gap after the box.
            if col_idx + 1 < columns.len() && row_idx == 0 {
                let arrow_x = col_x + this_col_w; // immediately after the box area
                if arrow_x < area.x + area.width {
                    let arrow_area = Rect { x: arrow_x, y: row_y + 1, width: 1, height: 1 };
                    let arrow = Paragraph::new("→")
                        .style(Style::default().fg(Color::DarkGray));
                    frame.render_widget(arrow, arrow_area);
                }
            }
        }

        // Show "+ N more" if there are hidden steps.
        if hidden_count > 0 {
            let last_row = steps_to_show.len().saturating_sub(1);
            let row_y = area.y + last_row as u16 * 3;
            if row_y + 3 <= area.y + area.height {
                let indent = last_row as u16;
                let box_x = (col_x + indent).min(area.x + area.width.saturating_sub(4));
                let box_w = this_col_w.saturating_sub(indent).max(4);
                let box_area = Rect { x: box_x, y: row_y, width: box_w, height: 3 };
                let more_label = format!("+ {} more…", hidden_count);
                let para = Paragraph::new(more_label)
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::DarkGray)))
                    .style(Style::default().fg(Color::DarkGray));
                frame.render_widget(para, box_area);
            }
        }
    }
}

/// Build topological columns for the workflow:
/// each column is a Vec of step names that share the same `depends_on` set,
/// in file order.
fn build_workflow_columns(wf: &WorkflowState) -> Vec<Vec<String>> {
    use std::collections::HashMap;

    // Map each depends_on signature (sorted deps) → column index, preserving order.
    let mut sig_to_col: HashMap<Vec<String>, usize> = HashMap::new();
    let mut columns: Vec<Vec<String>> = Vec::new();

    // Process steps in topological order (preserved by file order for same-level steps).
    let topo = crate::workflow::dag::topological_order(
        &wf.steps.iter().map(|s| crate::workflow::parser::WorkflowStep {
            name: s.name.clone(),
            depends_on: s.depends_on.clone(),
            prompt_template: String::new(),
        }).collect::<Vec<_>>(),
    );

    for step_name in &topo {
        let step = match wf.get_step(step_name) {
            Some(s) => s,
            None => continue,
        };
        let mut sig = step.depends_on.clone();
        sig.sort();
        let col_idx = *sig_to_col.entry(sig).or_insert_with(|| {
            columns.push(Vec::new());
            columns.len() - 1
        });
        columns[col_idx].push(step_name.clone());
    }

    columns
}

/// Return (label_text, style) for a step box.
fn step_box_label_and_style(
    name: &str,
    status: &StepStatus,
    is_current: bool,
    _box_width: u16,
) -> (String, Style) {
    const MAX_NAME: usize = 12;
    let truncated_name = if name.len() > MAX_NAME {
        format!("{}…", &name[..MAX_NAME.saturating_sub(1)])
    } else {
        name.to_string()
    };

    let (status_char, style) = match status {
        StepStatus::Pending => (
            "○",
            Style::default().fg(Color::DarkGray),
        ),
        StepStatus::Running => (
            "●",
            Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ),
        StepStatus::Done => (
            "✓",
            Style::default().fg(Color::Green),
        ),
        StepStatus::Error(_) => (
            "✗",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };

    let style = if is_current {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };

    // Fit label in box: " ● name "
    let label = format!(" {} {} ", status_char, truncated_name);
    (label, style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::App;
    use ratatui::{backend::TestBackend, Terminal};

    fn new_app() -> App {
        App::new(std::path::PathBuf::new())
    }

    /// Helper: render the app into a TestBackend and return the text content
    /// of the execution window's inner area (excluding borders).
    /// No sidebar. Tab bar is 3 rows at top. Exec window starts after tab bar.
    fn render_exec_window_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        // Tab bar takes 3 rows. Exec window height = total - 3 (tab bar) - 5 (status+cmd+suggest).
        let tab_bar_height = 3u16;
        let exec_height = height.saturating_sub(tab_bar_height + 5);
        let inner_top = tab_bar_height + 1; // after tab bar + top border
        let inner_left = 1u16;              // no sidebar, just left border
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
        let mut app = new_app();
        // Terminal: 40 wide, 18 tall
        // exec window = 18 - 3 (tab bar) - 5 (status+cmd+suggest) = 10 rows → inner = 8 rows
        // Add 20 lines of output so there's content to scroll through.
        for i in 0..20 {
            app.active_tab_mut().output_lines.push(format!("line {}", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Done {
            command: "ready".into(),
        };
        app.active_tab_mut().focus = Focus::ExecutionWindow;

        // scroll_offset=0 → should show the LAST 8 lines (lines 12-19).
        app.active_tab_mut().scroll_offset = 0;
        let view0 = render_exec_window_lines(&mut app, 40, 18);
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

        // scroll_offset=5 → should show earlier content (lines 7-14 with 8 inner rows).
        app.active_tab_mut().scroll_offset = 5;
        let view5 = render_exec_window_lines(&mut app, 40, 18);
        assert!(
            view5.iter().any(|l| l.contains("line 8")),
            "scroll_offset=5 should show line 8. Got: {:?}",
            view5
        );

        // The two views must differ.
        assert_ne!(
            view0, view5,
            "Scrolling must change the visible content"
        );

        // scroll_offset=max → should show the FIRST lines.
        app.active_tab_mut().scroll_offset = 20;
        let view_top = render_exec_window_lines(&mut app, 40, 18);
        assert!(
            view_top.iter().any(|l| l.contains("line 0")),
            "scroll_offset=max should show line 0 (oldest). Got: {:?}",
            view_top
        );
    }

    #[test]
    fn unicode_lines_do_not_cause_scroll_overshoot() {
        let mut app = new_app();
        // Box-drawing chars: "─" is 3 bytes but 1 display column.
        for i in 0..10 {
            app.active_tab_mut().output_lines.push(format!("──── step {} ────", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Done {
            command: "ready".into(),
        };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().scroll_offset = 0;

        // 40 wide, 18 tall → exec_height = 9, inner = 7 rows.
        let view = render_exec_window_lines(&mut app, 40, 18);
        assert!(
            view.iter().any(|l| l.contains("step 9")),
            "Newest line must be visible with Unicode content. Got: {:?}",
            view
        );
    }

    #[test]
    fn container_summary_renders_after_container_exit() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Done { command: "implement 0001".into() };
        app.active_tab_mut().last_container_summary = Some(LastContainerSummary {
            agent_display_name: "Claude Code".into(),
            container_name: "amux-test".into(),
            avg_cpu: "5.0%".into(),
            avg_memory: "200MiB".into(),
            total_time: "12m".into(),
            exit_code: 0,
        });

        // Render with enough space to include the summary bar.
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        // Collect all text from the buffer to verify summary content appears.
        let mut all_text = String::new();
        for row in 0..20 {
            for col in 0..80 {
                let cell = &buf[(col, row)];
                all_text.push_str(cell.symbol());
            }
        }
        assert!(
            all_text.contains("Claude Code"),
            "Summary should contain agent name. Got buffer text."
        );
        assert!(
            all_text.contains("amux-test"),
            "Summary should contain container name."
        );
    }

    #[test]
    fn container_window_renders_when_maximized() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        // Use size matching what TestBackend(80,25) would produce.
        let (inner_cols, inner_rows) = calculate_container_inner_size(80, 25, 0);
        app.active_tab_mut().start_container("amux-test".into(), "Claude Code".into(), inner_cols, inner_rows);

        // Feed data through the vt100 parser.
        if let Some(ref mut parser) = app.active_tab_mut().vt100_parser {
            parser.process(b"Hello from container\r\n");
        }

        let backend = TestBackend::new(80, 25);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        let mut all_text = String::new();
        for row in 0..25 {
            for col in 0..80 {
                let cell = &buf[(col, row)];
                all_text.push_str(cell.symbol());
            }
        }
        // Container window should show agent name and "containerized".
        assert!(
            all_text.contains("containerized"),
            "Container window should show '(containerized)' label"
        );
        // Container output should be visible via vt100 rendering.
        assert!(
            all_text.contains("Hello from container"),
            "Container output should be rendered in the container window"
        );
    }

    #[test]
    fn minimized_container_bar_renders() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        app.active_tab_mut().start_container("amux-test".into(), "Claude Code".into(), 78, 18);
        app.active_tab_mut().container_window = ContainerWindowState::Minimized;

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        let mut all_text = String::new();
        for row in 0..20 {
            for col in 0..80 {
                let cell = &buf[(col, row)];
                all_text.push_str(cell.symbol());
            }
        }
        assert!(
            all_text.contains("Claude Code"),
            "Minimized bar should contain agent name"
        );
    }

    #[test]
    fn calculate_container_inner_size_reasonable_values() {
        let (cols, rows) = calculate_container_inner_size(80, 25, 0);
        // exec_height = 25 - 3 (tab bar) - 5 (status+cmd+suggest) = 17
        // container_height = 17 * 95 / 100 = 16
        // container_width = 80 * 95 / 100 = 76
        // inner_rows = 16 - 2 = 14
        // inner_cols = 76 - 2 = 74
        assert_eq!(cols, 74);
        assert_eq!(rows, 14);
    }

    #[test]
    fn container_window_is_95_percent_and_centered() {
        // Verify the container window occupies 95% of content area and is centered.
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        let (inner_cols, inner_rows) = calculate_container_inner_size(100, 30, 0);
        app.active_tab_mut().start_container("test".into(), "Agent".into(), inner_cols, inner_rows);

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        // No sidebar. exec_height = 30 - 4 (tab bar) - 5 = 21
        // container_width = 100 * 95/100 = 95, container_height = 21 * 95/100 = 19
        // offset_x = (100 - 95)/2 = 2, offset_y = (21 - 19)/2 = 1
        // abs_x = 0 + 2 = 2, abs_y = 4 (tab bar) + 1 (offset_y) = 5
        // Border at (2, 5)
        let corner = buf[(2, 5)].symbol().to_string();
        assert!(
            corner == "╭" || corner == "│" || corner == "─",
            "Container border character should appear at centered position. Got: '{}'",
            corner
        );
    }

    #[test]
    fn vt100_set_scrollback_basic() {
        // Verify basic vt100 set_scrollback behavior.
        let mut parser = vt100::Parser::new(5, 20, 100);
        for i in 0..20 {
            parser.process(format!("line {}\r\n", i).as_bytes());
        }
        // After 20 lines in a 5-row screen, 15 lines should be in scrollback.
        // scrollback() returns the current position (0 when normal view).
        assert_eq!(parser.screen().scrollback(), 0);

        parser.set_scrollback(5);
        assert_eq!(parser.screen().scrollback(), 5);
        // cell(0,0) should access scrollback content.
        let cell = parser.screen().cell(0, 0);
        assert!(cell.is_some(), "cell(0,0) should be valid with scrollback=5");

        parser.set_scrollback(0);
        assert_eq!(parser.screen().scrollback(), 0);
    }

    #[test]
    fn container_scrollback_renders_older_content() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        let (inner_cols, inner_rows) = calculate_container_inner_size(80, 25, 0);
        app.active_tab_mut().start_container("test".into(), "Agent".into(), inner_cols, inner_rows);

        // Feed enough data to create scrollback: write many lines to push
        // content into the scrollback buffer.
        if let Some(ref mut parser) = app.active_tab_mut().vt100_parser {
            for i in 0..50 {
                parser.process(format!("scrollback line {}\r\n", i).as_bytes());
            }
        }

        // At offset 0, the latest lines should be visible.
        app.active_tab_mut().container_scroll_offset = 0;
        let backend = TestBackend::new(80, 25);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut text_at_0 = String::new();
        for row in 0..25 {
            for col in 0..80 {
                text_at_0.push_str(buf[(col, row)].symbol());
            }
        }
        assert!(
            text_at_0.contains("scrollback line 49"),
            "At offset 0 the latest line should be visible"
        );

        // Scroll up by a safe amount (capped at screen rows = inner_rows).
        let max_safe = inner_rows as usize;
        app.active_tab_mut().container_scroll_offset = max_safe;
        let backend = TestBackend::new(80, 25);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut text_scrolled = String::new();
        for row in 0..25 {
            for col in 0..80 {
                text_scrolled.push_str(buf[(col, row)].symbol());
            }
        }
        // When scrolled max_safe lines up, the most recent line should not be visible.
        assert!(
            !text_scrolled.contains("scrollback line 49"),
            "At max scroll the latest line should NOT be visible"
        );
        // Should show earlier content from scrollback.
        assert!(
            text_scrolled.contains("scrollback line"),
            "Should show scrollback content when scrolled up"
        );
    }

    #[test]
    fn container_scroll_indicator_shown_when_scrolled() {
        let mut app = new_app();
        app.active_tab_mut().phase = ExecutionPhase::Running { command: "implement 0001".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        let (inner_cols, inner_rows) = calculate_container_inner_size(80, 25, 0);
        app.active_tab_mut().start_container("test".into(), "Agent".into(), inner_cols, inner_rows);

        // Feed data to create scrollback.
        if let Some(ref mut parser) = app.active_tab_mut().vt100_parser {
            for i in 0..50 {
                parser.process(format!("line {}\r\n", i).as_bytes());
            }
        }

        // Use a scroll offset within the safe range (≤ screen rows).
        app.active_tab_mut().container_scroll_offset = (inner_rows as usize).min(10);
        let backend = TestBackend::new(80, 25);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut all_text = String::new();
        for row in 0..25 {
            for col in 0..80 {
                all_text.push_str(buf[(col, row)].symbol());
            }
        }
        assert!(
            all_text.contains("scrollback"),
            "Scroll indicator should appear when scrolled up. Got buffer text."
        );
    }

    #[test]
    fn outer_window_scroll_unaffected_by_container_changes() {
        // Verify that the outer execution window scrolling still works correctly
        // even when container-related state is present.
        let mut app = new_app();
        for i in 0..20 {
            app.active_tab_mut().output_lines.push(format!("outer line {}", i));
        }
        app.active_tab_mut().phase = ExecutionPhase::Done { command: "ready".into() };
        app.active_tab_mut().focus = Focus::ExecutionWindow;
        // Container is hidden (default) — this should not affect outer scrolling.
        app.active_tab_mut().container_scroll_offset = 5; // stale value, should be irrelevant

        app.active_tab_mut().scroll_offset = 0;
        let view_bottom = render_exec_window_lines(&mut app, 40, 18);
        assert!(
            view_bottom.iter().any(|l| l.contains("outer line 19")),
            "Outer window should show newest line at offset 0. Got: {:?}",
            view_bottom
        );

        app.active_tab_mut().scroll_offset = 10;
        let view_scrolled = render_exec_window_lines(&mut app, 40, 18);
        assert!(
            !view_scrolled.iter().any(|l| l.contains("outer line 19")),
            "Outer window should not show newest line at offset 10. Got: {:?}",
            view_scrolled
        );
    }

    #[test]
    fn tab_bar_renders_at_top() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/myproject"));
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        // Top-left corner of the first tab's rounded border should be at (0, 0).
        let corner = buf[(0, 0)].symbol().to_string();
        assert!(
            corner == "╭" || corner == "─",
            "Tab bar border at (0,0): '{}'",
            corner
        );
        // Row 3 should be the start of the exec window border (tab bar is 3 rows).
        let exec_border = buf[(0, 3)].symbol().to_string();
        assert!(
            exec_border == "╭" || exec_border == "─" || exec_border == " ",
            "Exec window border or space should start at row 3. Got: '{}'",
            exec_border
        );
    }

    #[test]
    fn single_tab_renders_without_panic() {
        // With exactly one tab (active_tab_idx == 0), the tab bar must render
        // without any out-of-bounds access and the active tab must use the
        // open-bottom (no bottom border) style.
        let mut app = new_app();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        // Must not panic.
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        // The active tab occupies columns 0..20, rows 0..3.
        // With Borders::TOP | LEFT | RIGHT (no bottom), row 2 of the tab area
        // should NOT contain a horizontal border character at the bottom line.
        // Row 0 is the top border (╭ … ╮), row 2 is the bottom of the tab block.
        // For an active tab, the bottom row should be blank (space), not "─".
        let bottom_left = buf[(0, 2)].symbol().to_string();
        assert_ne!(
            bottom_left, "╰",
            "Active tab bottom-left corner should not appear (no bottom border). Got: '{}'",
            bottom_left
        );
        // The top border should still be present.
        let top_left = buf[(0, 0)].symbol().to_string();
        assert!(
            top_left == "╭" || top_left == "─",
            "Active tab top border should be present. Got: '{}'",
            top_left
        );
    }

    #[test]
    fn active_tab_has_no_bottom_border_inactive_tabs_do() {
        // With two tabs, the active tab suppresses its bottom border while the
        // inactive tab retains its full border (Borders::ALL).
        let mut app = new_app();
        // Add a second tab by pushing a new TabState.
        let second = crate::tui::state::TabState::new(std::path::PathBuf::new());
        app.tabs.push(second);
        assert_eq!(app.tabs.len(), 2);
        app.active_tab_idx = 0; // first tab is active

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        // Tab width is 20. Active tab: cols 0..20. Inactive tab: cols 20..40.
        // Row 2 is the bottom row of the 3-row tab area.
        // Active tab (col 0, row 2): no bottom border → should not be "╰".
        let active_bottom_left = buf[(0, 2)].symbol().to_string();
        assert_ne!(
            active_bottom_left, "╰",
            "Active tab must not have a bottom-left corner. Got: '{}'",
            active_bottom_left
        );

        // Inactive tab (col 20, row 2): should have a bottom border "╰".
        let inactive_bottom_left = buf[(20, 2)].symbol().to_string();
        assert_eq!(
            inactive_bottom_left, "╰",
            "Inactive tab must have a bottom-left corner. Got: '{}'",
            inactive_bottom_left
        );
    }

    #[test]
    fn workflow_strip_renders_without_panic() {
        use crate::workflow::{WorkflowState, WorkflowStepState, StepStatus};

        let mut app = new_app();

        // Build a minimal WorkflowState with three steps (plan → implement → review).
        let steps = vec![
            WorkflowStepState {
                name: "plan".to_string(),
                depends_on: vec![],
                prompt_template: "Plan the work.".to_string(),
                status: StepStatus::Done,
                container_id: None,
            },
            WorkflowStepState {
                name: "implement".to_string(),
                depends_on: vec!["plan".to_string()],
                prompt_template: "Implement it.".to_string(),
                status: StepStatus::Running,
                container_id: Some("abc123".to_string()),
            },
            WorkflowStepState {
                name: "review".to_string(),
                depends_on: vec!["implement".to_string()],
                prompt_template: "Review the changes.".to_string(),
                status: StepStatus::Pending,
                container_id: None,
            },
        ];

        let wf = WorkflowState {
            title: Some("Test Workflow".to_string()),
            steps,
            workflow_hash: "deadbeef".to_string(),
            work_item: 27,
            workflow_name: "test-workflow".to_string(),
        };

        app.active_tab_mut().workflow = Some(wf);
        app.active_tab_mut().workflow_current_step = Some("implement".to_string());

        // Render — must not panic.
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        // Collect all rendered text.
        let buf = terminal.backend().buffer();
        let mut all_text = String::new();
        for row in 0..30 {
            for col in 0..80 {
                all_text.push_str(buf[(col, row)].symbol());
            }
        }

        // The workflow strip should contain at least one of the step names.
        assert!(
            all_text.contains("plan") || all_text.contains("impl") || all_text.contains("review"),
            "Workflow strip should render step names. Buffer text: {:?}",
            &all_text[..all_text.len().min(200)]
        );
    }

    #[test]
    fn workflow_strip_height_matches_parallel_groups() {
        use crate::workflow::{WorkflowState, WorkflowStepState, StepStatus};

        // Single-column workflow (linear chain): height should be 3 (1 row of boxes).
        let linear_steps = vec![
            WorkflowStepState {
                name: "a".to_string(),
                depends_on: vec![],
                prompt_template: String::new(),
                status: StepStatus::Pending,
                container_id: None,
            },
            WorkflowStepState {
                name: "b".to_string(),
                depends_on: vec!["a".to_string()],
                prompt_template: String::new(),
                status: StepStatus::Pending,
                container_id: None,
            },
        ];
        let wf_linear = WorkflowState {
            title: None,
            steps: linear_steps,
            workflow_hash: "h".to_string(),
            work_item: 1,
            workflow_name: "w".to_string(),
        };
        let h = workflow_strip_height(&wf_linear);
        assert!(h >= 3, "Strip height for linear workflow should be at least 3. Got: {}", h);

        // Parallel group (both b and c depend on a): height should accommodate stacking.
        let parallel_steps = vec![
            WorkflowStepState {
                name: "a".to_string(),
                depends_on: vec![],
                prompt_template: String::new(),
                status: StepStatus::Pending,
                container_id: None,
            },
            WorkflowStepState {
                name: "b".to_string(),
                depends_on: vec!["a".to_string()],
                prompt_template: String::new(),
                status: StepStatus::Pending,
                container_id: None,
            },
            WorkflowStepState {
                name: "c".to_string(),
                depends_on: vec!["a".to_string()],
                prompt_template: String::new(),
                status: StepStatus::Pending,
                container_id: None,
            },
        ];
        let wf_parallel = WorkflowState {
            title: None,
            steps: parallel_steps,
            workflow_hash: "h".to_string(),
            work_item: 1,
            workflow_name: "w".to_string(),
        };
        let h_parallel = workflow_strip_height(&wf_parallel);
        assert!(
            h_parallel >= h,
            "Strip height for parallel workflow ({}) should be >= linear ({})",
            h_parallel,
            h
        );
    }
}
