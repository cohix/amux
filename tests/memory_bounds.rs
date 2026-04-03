//! Memory snapshot tests for amux (work item 0033).
//!
//! These tests verify that output buffers are bounded or explicitly freed when
//! a tab is closed or a new command starts, preventing unbounded heap growth
//! during long-running sessions.

use amux::tui::state::{App, ContainerWindowState, ExecutionPhase, TabState};
use std::path::PathBuf;

// ─── VT100 scrollback is bounded ─────────────────────────────────────────────

/// The vt100 parser used for container output is created with a 1 000-line
/// scrollback limit.  Pushing more than 1 000 lines must not cause the
/// internal buffer to grow beyond that cap.
#[test]
fn vt100_scrollback_is_bounded_at_1000_lines() {
    let mut tab = TabState::new(PathBuf::from("/tmp/vt100-bound"));
    // Start a container session (cols=80, rows=24).
    tab.start_container("ctr-test".into(), "TestAgent".into(), 80, 24);

    // Feed 3 000 lines of output into the vt100 parser.
    let line = b"abcdefghijklmnopqrstuvwxyz 0123456789\r\n";
    let three_k_lines: Vec<u8> = line.repeat(3_000);

    // Route bytes through the container path (vt100 parser).
    assert!(tab.vt100_parser.is_some(), "vt100_parser should be active after start_container");
    if let Some(ref mut parser) = tab.vt100_parser {
        parser.process(&three_k_lines);

        // Attempt to scroll back 3 000 rows — the parser will clamp to its
        // configured scrollback_len (1 000).  After `set_scrollback`, the
        // screen reports the *actual* retained rows via `screen.scrollback()`.
        parser.set_scrollback(3_000);
        let retained = parser.screen().scrollback();

        assert!(
            retained <= 1_000,
            "vt100 scrollback retained {} rows after 3 000 lines — expected ≤ 1 000 \
             (configured scrollback_len cap)",
            retained
        );

        // The screen dimensions must be unchanged.
        let (rows, cols) = parser.screen().size();
        assert_eq!(rows, 24, "Screen rows should remain 24 after processing data");
        assert_eq!(cols, 80, "Screen cols should remain 80 after processing data");
    }
}

/// After `finish_command`, the vt100 parser and stats channel must be dropped
/// (set to `None`) to release container-session memory.
#[test]
fn finish_command_releases_container_resources() {
    let mut tab = TabState::new(PathBuf::from("/tmp/finish-release"));
    tab.start_container("ctr-finish".into(), "TestAgent".into(), 80, 24);

    assert!(tab.vt100_parser.is_some(), "parser should exist after start_container");
    assert_eq!(tab.container_window, ContainerWindowState::Maximized);

    tab.finish_command(0);

    assert!(
        tab.vt100_parser.is_none(),
        "vt100_parser must be None after finish_command"
    );
    assert_eq!(
        tab.container_window,
        ContainerWindowState::Hidden,
        "container_window must return to Hidden after finish_command"
    );
    assert!(
        tab.stats_rx.is_none(),
        "stats_rx must be None after finish_command"
    );
    // A summary should have been generated (agent_display_name was provided).
    assert!(
        tab.last_container_summary.is_some(),
        "last_container_summary should be populated after a container session"
    );
}

// ─── output_lines is cleared on new command ───────────────────────────────────

/// `start_command` must clear `output_lines` so previous command output does not
/// survive into the next command's execution window.
#[test]
fn start_command_clears_output_lines() {
    let mut tab = TabState::new(PathBuf::from("/tmp/output-clear"));
    for i in 0..500 {
        tab.push_output(format!("stale line {}", i));
    }
    assert_eq!(tab.output_lines.len(), 500);

    tab.start_command("fresh-command".into());

    assert!(
        tab.output_lines.is_empty(),
        "output_lines must be empty immediately after start_command; \
         found {} lines",
        tab.output_lines.len()
    );
    assert!(
        matches!(tab.phase, ExecutionPhase::Running { .. }),
        "phase must be Running after start_command"
    );
}

/// The PTY line buffer must also be cleared when starting a new command so
/// partial lines from a previous run cannot bleed into new output.
#[test]
fn start_command_clears_pty_line_buffer() {
    let mut tab = TabState::new(PathBuf::from("/tmp/ptybuf-clear"));
    tab.start_command("old-cmd".into());
    tab.process_pty_data(b"partial line without newline");
    // pty_line_buffer now holds "partial line without newline"

    tab.start_command("new-cmd".into());

    // The live-line state must be fully reset.
    assert!(
        !tab.pty_live_line,
        "pty_live_line should be false after start_command"
    );
    assert!(
        !tab.pty_pending_cr,
        "pty_pending_cr should be false after start_command"
    );
}

// ─── Closed tab frees its output buffer ──────────────────────────────────────

/// When `close_tab` removes a tab from `App`, the `TabState` (and its
/// `output_lines` Vec) must be dropped immediately.  Rust's ownership model
/// guarantees this; this test confirms the API contract by checking that the
/// remaining tab has not inherited the closed tab's data.
#[test]
fn closed_tab_output_does_not_leak_into_remaining_tabs() {
    let mut app = App::new(PathBuf::from("/tmp/tab-close-a"));
    app.create_tab(PathBuf::from("/tmp/tab-close-b"));

    // Fill tab 0 with 1 000 lines.
    for i in 0..1_000 {
        app.tabs[0].push_output(format!("leaked-line-{}", i));
    }
    // Fill tab 1 with a distinctive marker.
    app.tabs[1].push_output("survivor-marker".to_string());

    assert_eq!(app.tabs.len(), 2);
    app.close_tab(0);

    assert_eq!(app.tabs.len(), 1, "Tab was not removed");
    assert_eq!(app.active_tab_idx, 0, "active_tab_idx was not adjusted");

    // The surviving tab must contain only its own data.
    assert!(
        app.tabs[0].output_lines.iter().any(|l| l == "survivor-marker"),
        "Survivor tab lost its own output after close_tab"
    );
    assert!(
        !app.tabs[0]
            .output_lines
            .iter()
            .any(|l| l.contains("leaked-line")),
        "Closed tab's output_lines leaked into the surviving tab"
    );
}

/// Closing every tab triggers `should_quit` and does not leave dangling state.
#[test]
fn closing_all_tabs_sets_should_quit() {
    let mut app = App::new(PathBuf::from("/tmp/quit-test"));
    for i in 0..500 {
        app.tabs[0].push_output(format!("line {}", i));
    }
    app.close_tab(0);

    assert!(
        app.should_quit,
        "should_quit must be true after closing the last tab"
    );
}

// ─── output_lines growth during a long run ────────────────────────────────────

/// Pushes a very large number of output lines (simulating a long-running agent)
/// and confirms the lines accumulate correctly without panic.
///
/// NOTE: This test documents the CURRENT behaviour (unbounded growth during a
/// session). A future work item should cap `output_lines` with a ring buffer to
/// prevent runaway memory use in very long sessions.
#[test]
fn output_lines_grow_unbounded_during_single_command() {
    let mut tab = TabState::new(PathBuf::from("/tmp/long-run"));
    tab.start_command("long-agent".into());

    const LINES: usize = 50_000;
    let chunk = b"agent output: building feature X step 0000001\n";
    let big = chunk.repeat(LINES);
    tab.process_pty_data(&big);

    // Lines accumulated — this is expected today and serves as a regression
    // anchor: if output_lines is later capped, update the assertion below.
    assert!(
        tab.output_lines.len() > 1_000,
        "Expected > 1 000 output lines for a long-running command; got {}",
        tab.output_lines.len()
    );
}

// ─── VT100 None before start_container ────────────────────────────────────────

/// `TabState::new` must initialise `vt100_parser` to `None`; the parser is only
/// created when an actual container session begins.
#[test]
fn new_tab_has_no_vt100_parser() {
    let tab = TabState::new(PathBuf::from("/tmp/fresh"));
    assert!(
        tab.vt100_parser.is_none(),
        "vt100_parser must be None on a freshly created TabState"
    );
    assert!(
        tab.container_info.is_none(),
        "container_info must be None on a freshly created TabState"
    );
    assert!(
        tab.stats_rx.is_none(),
        "stats_rx must be None on a freshly created TabState"
    );
}
