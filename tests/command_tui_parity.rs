/// Integration tests verifying that command-mode and TUI-mode reuse the same underlying logic.
///
/// These tests call the shared `run_with_sink` / helper functions directly,
/// confirming that the same code paths are exercised regardless of execution mode.
use aspec::commands::auth::apply_auth_decision;
use aspec::commands::implement::find_work_item;
use aspec::commands::output::OutputSink;
use aspec::commands::{init, ready};
use aspec::tui::input::{autocomplete_suggestions, closest_subcommand};
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;

// ---------------------------------------------------------------------------
// 1. init output via sink matches the expected lines
// ---------------------------------------------------------------------------

#[tokio::test]
async fn init_via_sink_produces_output_lines() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);

    // run_with_sink from inside a git repo (the aspec-cli repo itself)
    let result = init::run_with_sink(aspec::cli::Agent::Claude, &sink).await;
    drop(result); // may succeed or fail; we only care that the sink was used.

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    // At minimum the function should have sent something to the sink.
    assert!(
        !messages.is_empty(),
        "Expected at least one output line from init"
    );
}

// ---------------------------------------------------------------------------
// 2. ready emits the "Checking" message before any failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ready_via_sink_emits_checking_message() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);

    let _ = ready::run_with_sink(&sink).await;

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let has_checking = messages.iter().any(|m| m.contains("Checking"));
    assert!(has_checking, "Expected 'Checking' in ready output, got: {:?}", messages);
}

// ---------------------------------------------------------------------------
// 2b. ready routes Docker build output through the sink (not just aspec's own messages)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ready_sink_captures_docker_build_output() {
    // Skip when Docker is not available.
    if !aspec::docker::is_daemon_running() {
        return;
    }

    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);

    let result = ready::run_with_sink(&sink).await;
    // If ready fails (e.g., missing Dockerfile.dev), we still got some messages.
    let _ = result;

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    // aspec sends ~7 explicit messages. Docker build output adds many more.
    // With captured output, the total should exceed the explicit messages.
    let has_docker_output = messages.iter().any(|m| {
        m.contains("DONE")
            || m.contains("CACHED")
            || m.contains("FROM")
            || m.contains("building")
            || m.contains("#")
    });

    // If Docker ran successfully, its build output must be in the sink.
    if messages.iter().any(|m| m.contains("Building Docker image")) {
        assert!(
            has_docker_output,
            "Docker build output must be routed through OutputSink for TUI scrolling. \
             Got {} messages: {:?}",
            messages.len(),
            &messages
        );
        assert!(
            messages.len() > 7,
            "Expected more than 7 lines when Docker build output is captured. \
             Got {} lines — stderr/stdout may not be piped through the sink. Messages: {:?}",
            messages.len(),
            &messages
        );
    }
}

// ---------------------------------------------------------------------------
// 3. find_work_item is shared — same function used in command and TUI mode
// ---------------------------------------------------------------------------

#[test]
fn find_work_item_used_in_both_modes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let work_items_dir = root.join("aspec/work-items");
    std::fs::create_dir_all(&work_items_dir).unwrap();
    std::fs::write(work_items_dir.join("0002-some-feature.md"), "# test").unwrap();

    // This is the exact same function called by both `commands::implement::run()`
    // and `tui::mod::launch_implement()`.
    let path = find_work_item(&root, 2).unwrap();
    assert!(path.ends_with("0002-some-feature.md"));

    // Ensure missing work item returns the same error in both modes.
    let err = find_work_item(&root, 99).unwrap_err();
    assert!(err.to_string().contains("99"));
}

// ---------------------------------------------------------------------------
// 4. Unknown command → closest suggestion (TUI input logic)
// ---------------------------------------------------------------------------

#[test]
fn unknown_command_suggests_closest_subcommand() {
    assert_eq!(closest_subcommand("implemnt"), Some("implement".into()));
    assert_eq!(closest_subcommand("redy"), Some("ready".into()));
    assert_eq!(closest_subcommand("int"), Some("init".into()));
    // Exact match returns None (no correction needed).
    assert_eq!(closest_subcommand("ready"), None);
}

#[test]
fn autocomplete_returns_matching_subcommands() {
    let sug = autocomplete_suggestions("im");
    assert_eq!(sug, vec!["implement"]);

    let sug = autocomplete_suggestions("r");
    assert_eq!(sug, vec!["ready"]);

    let sug = autocomplete_suggestions("init ");
    assert!(sug.iter().any(|s: &String| s.contains("--agent")));
}

// ---------------------------------------------------------------------------
// 5. Auth decision is persisted and re-read correctly
// ---------------------------------------------------------------------------

#[test]
fn auth_apply_decision_saves_config() {
    let tmp = TempDir::new().unwrap();

    // Accept → saved as true.
    apply_auth_decision(tmp.path(), "claude", true).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(true));

    // Decline → saved as false (overwrites).
    apply_auth_decision(tmp.path(), "claude", false).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(false));
}
