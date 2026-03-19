/// Integration tests verifying that command-mode and TUI-mode reuse the same underlying logic.
///
/// These tests call the shared `run_with_sink` / helper functions directly,
/// confirming that the same code paths are exercised regardless of execution mode.
use aspec::commands::auth::{
    apply_auth_decision, agent_keychain_credentials, read_keychain_raw,
    AgentCredentials,
};
use aspec::commands::chat::{
    chat_entrypoint, chat_entrypoint_non_interactive,
};
use aspec::commands::implement::{
    agent_entrypoint, agent_entrypoint_non_interactive, find_work_item, implement_prompt,
    parse_work_item,
};
use aspec::commands::new::{
    apply_template, find_template, next_work_item_number, slugify, WorkItemKind,
};
use aspec::commands::output::OutputSink;
use aspec::commands::ready::{
    audit_entrypoint, audit_entrypoint_non_interactive, ReadyOptions, ReadySummary, StepStatus,
    print_summary, print_interactive_notice,
};
use aspec::commands::{init, new, ready};
use aspec::tui::input::{autocomplete_suggestions, closest_subcommand};
use std::path::PathBuf;
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

    let mount_path = PathBuf::from("/tmp");
    let opts = ReadyOptions::default();
    let _ = ready::run_with_sink(&sink, mount_path, vec![], &opts, None).await;

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let has_checking = messages.iter().any(|m| m.contains("Checking"));
    assert!(
        has_checking,
        "Expected 'Checking' in ready output, got: {:?}",
        messages
    );
}

// ---------------------------------------------------------------------------
// 2b. ready routes all output through the sink (status, image tag)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2c. ready audit entrypoint generates correct agent commands
// ---------------------------------------------------------------------------

#[test]
fn ready_audit_entrypoint_for_each_agent() {
    let claude = audit_entrypoint("claude");
    assert_eq!(claude.len(), 3);
    assert_eq!(claude[0], "claude");
    assert_eq!(claude[1], "--allowedTools=Edit,Write");
    assert!(claude[2].contains("scan this project"));

    let codex = audit_entrypoint("codex");
    assert_eq!(codex[0], "codex");
    assert!(codex[1].contains("scan this project"));

    let opencode = audit_entrypoint("opencode");
    assert_eq!(opencode[0], "opencode");
    assert_eq!(opencode[1], "run");
    assert!(opencode[2].contains("scan this project"));
}

// ---------------------------------------------------------------------------
// 2d. ready uses project-specific image tag
// ---------------------------------------------------------------------------

#[test]
fn ready_uses_project_specific_image_tag() {
    let tag = aspec::docker::project_image_tag(std::path::Path::new("/home/user/myproject"));
    assert_eq!(tag, "aspec-myproject:latest");
}

// ---------------------------------------------------------------------------
// 2e. ready non-interactive audit entrypoint
// ---------------------------------------------------------------------------

#[test]
fn ready_audit_entrypoint_non_interactive_for_each_agent() {
    let claude = audit_entrypoint_non_interactive("claude");
    assert_eq!(claude[0], "claude");
    assert_eq!(claude[1], "-p");
    assert_eq!(claude[2], "--allowedTools=Edit,Write");
    assert!(claude[3].contains("scan this project"));

    let codex = audit_entrypoint_non_interactive("codex");
    assert_eq!(codex[0], "codex");
    assert_eq!(codex[1], "--quiet");
    assert!(codex[2].contains("scan this project"));
}

// ---------------------------------------------------------------------------
// 2f. ready summary table
// ---------------------------------------------------------------------------

#[test]
fn ready_summary_table_outputs_all_rows() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);
    let summary = ReadySummary {
        docker_daemon: StepStatus::Ok("running".into()),
        dockerfile: StepStatus::Ok("exists".into()),
        dev_image: StepStatus::Ok("exists".into()),
        refresh: StepStatus::Skipped("use --refresh to run".into()),
        image_rebuild: StepStatus::Skipped("no refresh".into()),
    };
    print_summary(&sink, &summary);
    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let all = messages.join("\n");
    assert!(all.contains("Ready Summary"));
    assert!(all.contains("Docker daemon"));
    assert!(all.contains("Dockerfile.dev"));
    assert!(all.contains("Dev image"));
    assert!(all.contains("Refresh"));
    assert!(all.contains("Image rebuild"));
}

// ---------------------------------------------------------------------------
// 2g. ready skip message when no --refresh
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2h. interactive notice
// ---------------------------------------------------------------------------

#[test]
fn interactive_notice_contains_agent_info() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);
    print_interactive_notice(&sink, "claude");
    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let all = messages.join("\n");
    assert!(all.contains("INTERACTIVE"), "Missing INTERACTIVE label");
    assert!(all.contains("claude"), "Missing agent name");
    assert!(all.contains("Ctrl+C"), "Missing quit hint");
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

    let path = find_work_item(&root, 2).unwrap();
    assert!(path.ends_with("0002-some-feature.md"));

    let err = find_work_item(&root, 99).unwrap_err();
    assert!(err.to_string().contains("99"));
}

// ---------------------------------------------------------------------------
// 4. Unknown command -> closest suggestion (TUI input logic)
// ---------------------------------------------------------------------------

#[test]
fn unknown_command_suggests_closest_subcommand() {
    assert_eq!(closest_subcommand("implemnt"), Some("implement".into()));
    assert_eq!(closest_subcommand("redy"), Some("ready".into()));
    assert_eq!(closest_subcommand("int"), Some("init".into()));
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

#[test]
fn autocomplete_ready_shows_all_flags() {
    let sug = autocomplete_suggestions("ready ");
    assert!(sug.iter().any(|s| s.contains("--refresh")), "Missing --refresh");
    assert!(sug.iter().any(|s| s.contains("--build")), "Missing --build");
    assert!(sug.iter().any(|s| s.contains("--no-cache")), "Missing --no-cache");
    assert!(sug.iter().any(|s| s.contains("--non-interactive")), "Missing --non-interactive");
}

#[test]
fn autocomplete_implement_shows_non_interactive_flag() {
    let sug = autocomplete_suggestions("implement ");
    assert!(sug.iter().any(|s| s.contains("--non-interactive")));
}

// ---------------------------------------------------------------------------
// 5. Agent credentials are passed as env vars into the container
// ---------------------------------------------------------------------------

#[test]
fn agent_env_vars_passed_to_container() {
    let env = vec![("ANTHROPIC_API_KEY".into(), "sk-test".into())];
    let args = aspec::docker::build_run_args("img", "/repo", &[], &env, None, false);
    assert!(args.contains(&"-e".to_string()));
    assert!(args.contains(&"ANTHROPIC_API_KEY=sk-test".to_string()));
}

#[test]
fn display_args_mask_env_var_values() {
    let env = vec![("ANTHROPIC_API_KEY".into(), "sk-secret".into())];
    let args = aspec::docker::build_run_args_display("img", "/repo", &[], &env, None, false);
    assert!(
        args.contains(&"ANTHROPIC_API_KEY=***".to_string()),
        "Display args must mask env var values, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| a.contains("sk-secret")),
        "Display args must not contain actual secret"
    );
}

// ---------------------------------------------------------------------------
// 6. Auth decision is persisted and re-read correctly
// ---------------------------------------------------------------------------

#[test]
fn auth_apply_decision_saves_config() {
    let tmp = TempDir::new().unwrap();

    apply_auth_decision(tmp.path(), "claude", true).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(true));

    apply_auth_decision(tmp.path(), "claude", false).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(false));
}

// ---------------------------------------------------------------------------
// 7. Implement entrypoint and prompt (shared between CLI and TUI)
// ---------------------------------------------------------------------------

#[test]
fn implement_entrypoint_for_each_agent() {
    let claude = agent_entrypoint("claude", 1, false);
    assert_eq!(claude.len(), 2);
    assert_eq!(claude[0], "claude");
    assert!(claude[1].contains("work item 0001"));
    assert!(claude[1].contains("Iterate until the build succeeds"));

    let codex = agent_entrypoint("codex", 2, false);
    assert_eq!(codex[0], "codex");
    assert!(codex[1].contains("work item 0002"));

    let opencode = agent_entrypoint("opencode", 3, false);
    assert_eq!(opencode[0], "opencode");
    assert_eq!(opencode[1], "run");
    assert!(opencode[2].contains("work item 0003"));
}

#[test]
fn implement_entrypoint_non_interactive_for_each_agent() {
    let claude = agent_entrypoint_non_interactive("claude", 1, false);
    assert_eq!(claude[0], "claude");
    assert_eq!(claude[1], "-p");
    assert!(claude[2].contains("work item 0001"));

    let codex = agent_entrypoint_non_interactive("codex", 2, false);
    assert_eq!(codex[0], "codex");
    assert_eq!(codex[1], "--quiet");
    assert!(codex[2].contains("work item 0002"));

    let opencode = agent_entrypoint_non_interactive("opencode", 3, false);
    assert_eq!(opencode[0], "opencode");
    assert_eq!(opencode[1], "run");
    assert!(opencode[2].contains("work item 0003"));
}

#[test]
fn implement_prompt_contains_required_elements() {
    let prompt = implement_prompt(42);
    assert!(
        prompt.contains("Implement work item 0042"),
        "prompt: {}",
        prompt
    );
    assert!(
        prompt.contains("Iterate until the build succeeds"),
        "prompt: {}",
        prompt
    );
    assert!(
        prompt.contains("tests are comprehensive and pass"),
        "prompt: {}",
        prompt
    );
    assert!(
        prompt.contains("Write documentation"),
        "prompt: {}",
        prompt
    );
    assert!(
        prompt.contains("Ensure final build and test success"),
        "prompt: {}",
        prompt
    );
}

#[test]
fn parse_work_item_accepts_various_formats() {
    assert_eq!(parse_work_item("0001").unwrap(), 1);
    assert_eq!(parse_work_item("1").unwrap(), 1);
    assert_eq!(parse_work_item("42").unwrap(), 42);
    assert_eq!(parse_work_item("0042").unwrap(), 42);
    assert!(parse_work_item("abc").is_err());
    assert!(parse_work_item("").is_err());
}

// ---------------------------------------------------------------------------
// 8. ReadyOptions defaults and new fields
// ---------------------------------------------------------------------------

#[test]
fn ready_options_default_no_refresh_no_non_interactive() {
    let opts = ReadyOptions::default();
    assert!(!opts.refresh);
    assert!(!opts.build);
    assert!(!opts.no_cache);
    assert!(!opts.non_interactive);
}

#[test]
fn ready_options_build_flag() {
    let opts = ReadyOptions { build: true, ..Default::default() };
    assert!(opts.build);
    assert!(!opts.refresh);
    assert!(!opts.no_cache);
    assert!(!opts.non_interactive);
}

#[test]
fn ready_options_no_cache_flag() {
    let opts = ReadyOptions { no_cache: true, ..Default::default() };
    assert!(opts.no_cache);
    assert!(!opts.build);
}

#[test]
fn ready_options_build_and_no_cache() {
    let opts = ReadyOptions { build: true, no_cache: true, ..Default::default() };
    assert!(opts.build);
    assert!(opts.no_cache);
}

#[test]
fn ready_options_refresh_ignores_build() {
    // When refresh is true, build should be ignored per spec.
    // The run() function handles this, but ReadyOptions itself is just data.
    let opts = ReadyOptions { refresh: true, build: true, ..Default::default() };
    assert!(opts.refresh);
    assert!(opts.build); // ReadyOptions stores both; run() ignores build when refresh is true.
}

// ---------------------------------------------------------------------------
// 9. ReadySummary status variants
// ---------------------------------------------------------------------------

#[test]
fn ready_summary_status_variants() {
    assert_eq!(StepStatus::Pending, StepStatus::Pending);
    assert_ne!(StepStatus::Pending, StepStatus::Ok("ok".into()));
    assert_ne!(
        StepStatus::Ok("a".into()),
        StepStatus::Failed("b".into())
    );
    assert_ne!(
        StepStatus::Skipped("x".into()),
        StepStatus::Ok("x".into())
    );
}

// ---------------------------------------------------------------------------
// 10. New command: slugify produces correct filenames
// ---------------------------------------------------------------------------

#[test]
fn new_slugify_produces_valid_filenames() {
    assert_eq!(slugify("My New Feature"), "my-new-feature");
    assert_eq!(slugify("Fix: the bug!"), "fix-the-bug");
    assert_eq!(slugify("Add step 2 support"), "add-step-2-support");
    assert_eq!(slugify(""), "");
}

// ---------------------------------------------------------------------------
// 11. New command: next_work_item_number finds the correct next number
// ---------------------------------------------------------------------------

#[test]
fn new_next_work_item_number_sequential() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("0000-template.md"), "").unwrap();
    std::fs::write(tmp.path().join("0001-first.md"), "").unwrap();
    std::fs::write(tmp.path().join("0002-second.md"), "").unwrap();
    let num = next_work_item_number(tmp.path()).unwrap();
    assert_eq!(num, 3);
}

#[test]
fn new_next_work_item_number_with_gaps() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("0000-template.md"), "").unwrap();
    std::fs::write(tmp.path().join("0005-fifth.md"), "").unwrap();
    let num = next_work_item_number(tmp.path()).unwrap();
    assert_eq!(num, 6);
}

// ---------------------------------------------------------------------------
// 12. New command: apply_template replaces header and title
// ---------------------------------------------------------------------------

#[test]
fn new_apply_template_substitutions() {
    let template = "# Work Item: [Feature | Bug | Task]\n\nTitle: title\nIssue: issuelink\n\n## Summary:\n- summary\n";
    let result = apply_template(template, &WorkItemKind::Bug, "Fix login crash");
    assert!(result.contains("# Work Item: Bug"));
    assert!(result.contains("Title: Fix login crash"));
    assert!(result.contains("## Summary:"));
    assert!(!result.contains("[Feature | Bug | Task]"));
}

#[test]
fn new_apply_template_all_kinds() {
    let template = "# Work Item: [Feature | Bug | Task]\nTitle: title\n";
    for (kind, label) in [
        (WorkItemKind::Feature, "Feature"),
        (WorkItemKind::Bug, "Bug"),
        (WorkItemKind::Task, "Task"),
    ] {
        let result = apply_template(template, &kind, "Test");
        assert!(
            result.contains(&format!("# Work Item: {}", label)),
            "Expected kind '{}' in template output",
            label
        );
    }
}

// ---------------------------------------------------------------------------
// 13. New command: find_template returns correct path or error
// ---------------------------------------------------------------------------

#[test]
fn new_find_template_exists() {
    let tmp = TempDir::new().unwrap();
    let wi = tmp.path().join("aspec/work-items");
    std::fs::create_dir_all(&wi).unwrap();
    std::fs::write(wi.join("0000-template.md"), "# template").unwrap();
    let path = find_template(tmp.path()).unwrap();
    assert!(path.ends_with("0000-template.md"));
}

#[test]
fn new_find_template_missing_suggests_download() {
    let tmp = TempDir::new().unwrap();
    let err = find_template(tmp.path()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Template not found"));
    assert!(msg.contains("https://github.com/cohix/aspec"));
}

// ---------------------------------------------------------------------------
// 14. New command: WorkItemKind parsing
// ---------------------------------------------------------------------------

#[test]
fn new_work_item_kind_parsing() {
    assert_eq!(WorkItemKind::from_str("feature"), Some(WorkItemKind::Feature));
    assert_eq!(WorkItemKind::from_str("1"), Some(WorkItemKind::Feature));
    assert_eq!(WorkItemKind::from_str("bug"), Some(WorkItemKind::Bug));
    assert_eq!(WorkItemKind::from_str("2"), Some(WorkItemKind::Bug));
    assert_eq!(WorkItemKind::from_str("task"), Some(WorkItemKind::Task));
    assert_eq!(WorkItemKind::from_str("3"), Some(WorkItemKind::Task));
    assert_eq!(WorkItemKind::from_str("invalid"), None);
}

// ---------------------------------------------------------------------------
// 15. New command: run_with_sink creates a file (shared logic)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_via_sink_creates_work_item() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    let wi = root.join("aspec/work-items");
    std::fs::create_dir_all(&wi).unwrap();
    std::fs::write(
        wi.join("0000-template.md"),
        "# Work Item: [Feature | Bug | Task]\n\nTitle: title\nIssue: issuelink\n",
    )
    .unwrap();

    let (tx, mut rx) = unbounded_channel();
    let sink = OutputSink::Channel(tx);

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();

    let result = new::run_with_sink(
        &sink,
        Some(WorkItemKind::Task),
        Some("My New Task".to_string()),
    )
    .await;

    std::env::set_current_dir(original_dir).unwrap();

    assert!(result.is_ok(), "run_with_sink failed: {:?}", result.err());

    let created = wi.join("0001-my-new-task.md");
    assert!(created.exists(), "Work item file should exist");

    let content = std::fs::read_to_string(&created).unwrap();
    assert!(content.contains("# Work Item: Task"));
    assert!(content.contains("Title: My New Task"));

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(messages.iter().any(|m| m.contains("Created work item")));
}

// ---------------------------------------------------------------------------
// 16. New command: autocomplete includes "new"
// ---------------------------------------------------------------------------

#[test]
fn autocomplete_includes_new_subcommand() {
    let sug = autocomplete_suggestions("");
    assert!(sug.contains(&"new".to_string()), "Expected 'new' in suggestions");

    let sug = autocomplete_suggestions("ne");
    assert_eq!(sug, vec!["new"]);
}

#[test]
fn autocomplete_new_shows_hint() {
    let sug = autocomplete_suggestions("new ");
    assert!(
        sug.iter().any(|s| s.contains("new")),
        "Expected hint for 'new' command, got: {:?}",
        sug
    );
}

// ---------------------------------------------------------------------------
// 17. Container window state management
// ---------------------------------------------------------------------------

#[test]
fn container_window_lifecycle() {
    use aspec::tui::state::{App, ContainerWindowState, ExecutionPhase};

    let mut app = App::new();
    assert_eq!(app.container_window, ContainerWindowState::Hidden);

    // Start a container.
    app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
    app.start_container("aspec-test-123".into(), "Claude Code".into(), 78, 18);
    assert_eq!(app.container_window, ContainerWindowState::Maximized);

    // Minimize.
    app.container_window = ContainerWindowState::Minimized;
    assert_eq!(app.container_window, ContainerWindowState::Minimized);

    // Restore.
    app.container_window = ContainerWindowState::Maximized;
    assert_eq!(app.container_window, ContainerWindowState::Maximized);

    // Container exits → summary created, window hidden.
    app.finish_command(0);
    assert_eq!(app.container_window, ContainerWindowState::Hidden);
    assert!(app.last_container_summary.is_some());
    let summary = app.last_container_summary.as_ref().unwrap();
    assert_eq!(summary.exit_code, 0);
    assert_eq!(summary.agent_display_name, "Claude Code");
    assert_eq!(summary.container_name, "aspec-test-123");
}

// ---------------------------------------------------------------------------
// 18. Container window PTY output routing
// ---------------------------------------------------------------------------

#[test]
fn container_pty_output_routing() {
    use aspec::tui::state::{App, ExecutionPhase};

    let mut app = App::new();
    app.phase = ExecutionPhase::Running { command: "implement 0001".into() };

    // Without container window: PTY data goes to output_lines.
    app.process_pty_data(b"outer line\n");
    assert!(app.output_lines.iter().any(|l| l == "outer line"));
    assert!(app.vt100_parser.is_none());

    // Activate container window: PTY data goes to vt100 parser.
    app.start_container("aspec-test".into(), "Claude Code".into(), 80, 24);
    assert!(app.vt100_parser.is_some());
    // Feed data through the vt100 parser (simulating what tick() does).
    if let Some(ref mut parser) = app.vt100_parser {
        parser.process(b"container line\r\n");
    }
    let screen_text = app.vt100_parser.as_ref().unwrap().screen().contents();
    assert!(screen_text.contains("container line"), "vt100 screen should contain container output");
    // The outer window should still have "outer line" from before.
    assert!(app.output_lines.iter().any(|l| l == "outer line"));
}

// ---------------------------------------------------------------------------
// 19. Docker stats parsing
// ---------------------------------------------------------------------------

#[test]
fn docker_stats_parsing() {
    assert!((aspec::docker::parse_cpu_percent("5.23%") - 5.23).abs() < 0.001);
    assert!((aspec::docker::parse_memory_mb("200MiB") - 200.0).abs() < 0.1);
    assert!((aspec::docker::parse_memory_mb("1.5GiB") - 1536.0).abs() < 0.1);
}

// ---------------------------------------------------------------------------
// 20. Container name generation
// ---------------------------------------------------------------------------

#[test]
fn container_name_generation() {
    let name = aspec::docker::generate_container_name();
    assert!(name.starts_with("aspec-"));
}

// ---------------------------------------------------------------------------
// 21. Duration formatting
// ---------------------------------------------------------------------------

#[test]
fn duration_formatting() {
    use aspec::tui::state::format_duration;
    assert_eq!(format_duration(0), "0s");
    assert_eq!(format_duration(45), "45s");
    assert_eq!(format_duration(60), "1m");
    assert_eq!(format_duration(3600), "1h");
    assert_eq!(format_duration(5400), "1h 30m");
}

// ---------------------------------------------------------------------------
// 22. Agent display name
// ---------------------------------------------------------------------------

#[test]
fn agent_display_names() {
    use aspec::tui::state::agent_display_name;
    assert_eq!(agent_display_name("claude"), "Claude Code");
    assert_eq!(agent_display_name("codex"), "Codex");
    assert_eq!(agent_display_name("opencode"), "Opencode");
    assert_eq!(agent_display_name("unknown"), "unknown");
}

// ---------------------------------------------------------------------------
// 23. PTY args include container name
// ---------------------------------------------------------------------------

#[test]
fn pty_args_container_name() {
    let args = aspec::docker::build_run_args_pty(
        "img", "/repo", &[], &[], Some("aspec-test-42"), None, false,
    );
    assert!(args.contains(&"--name".to_string()));
    assert!(args.contains(&"aspec-test-42".to_string()));

    let args_no_name = aspec::docker::build_run_args_pty(
        "img", "/repo", &[], &[], None, None, false,
    );
    assert!(!args_no_name.contains(&"--name".to_string()));
}

// ---------------------------------------------------------------------------
// 24. Container summary with stats history averages
// ---------------------------------------------------------------------------

#[test]
fn container_summary_averages_stats() {
    use aspec::tui::state::{App, ExecutionPhase};

    let mut app = App::new();
    app.phase = ExecutionPhase::Running { command: "implement 0001".into() };
    app.start_container("aspec-test".into(), "Claude Code".into(), 78, 18);

    // Simulate stats history.
    if let Some(ref mut info) = app.container_info {
        info.stats_history.push((10.0, 100.0));
        info.stats_history.push((20.0, 200.0));
        info.stats_history.push((30.0, 300.0));
    }

    app.finish_command(0);

    let summary = app.last_container_summary.as_ref().unwrap();
    assert_eq!(summary.avg_cpu, "20.0%");
    assert_eq!(summary.avg_memory, "200MiB");
}

// ---------------------------------------------------------------------------
// 25. Auth auto-authorization reads project-local config
// ---------------------------------------------------------------------------

#[test]
fn auth_reads_project_local_config() {
    let tmp = TempDir::new().unwrap();

    // Initially None → prompt should be shown.
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, None);

    // Accept → saved to project-local config.
    apply_auth_decision(tmp.path(), "claude", true).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(true));

    // Check that the config file is at the correct path.
    let config_path = aspec::config::repo_config_path(tmp.path());
    assert!(config_path.exists());
    assert!(config_path.to_str().unwrap().contains("aspec/.aspec-cli.json"));

    // Decline → saved as false.
    apply_auth_decision(tmp.path(), "claude", false).unwrap();
    let config = aspec::config::load_repo_config(tmp.path()).unwrap();
    assert_eq!(config.auto_agent_auth_accepted, Some(false));
}

// ---------------------------------------------------------------------------
// 26. Keychain credentials returns single CLAUDE_CODE_OAUTH_TOKEN — no files
// ---------------------------------------------------------------------------

#[test]
fn keychain_credentials_uses_single_env_var() {
    let creds = agent_keychain_credentials("claude");
    // On a dev machine with keychain, exactly one env var should be set.
    // On CI without keychain, it returns empty.
    if !creds.env_vars.is_empty() {
        assert_eq!(creds.env_vars.len(), 1, "Should set exactly one env var");
        assert_eq!(creds.env_vars[0].0, "CLAUDE_CODE_OAUTH_TOKEN");
        // Value should be the raw access token string, not JSON.
        let val = &creds.env_vars[0].1;
        assert!(val.starts_with("sk-ant-oat"), "Token should be a raw OAuth token, got: {}", val);
    }
}

// ---------------------------------------------------------------------------
// 27. Keychain raw JSON includes both access and refresh tokens
// ---------------------------------------------------------------------------

#[test]
fn keychain_raw_json_has_required_fields() {
    if let Some(raw) = read_keychain_raw("claude") {
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let oauth = parsed.get("claudeAiOauth").expect("claudeAiOauth key should exist");
        assert!(oauth.get("accessToken").is_some(), "accessToken should exist");
        assert!(oauth.get("refreshToken").is_some(), "refreshToken should exist");
        assert!(oauth.get("expiresAt").is_some(), "expiresAt should exist");
    }
}

// ---------------------------------------------------------------------------
// 28. AgentCredentials default is empty
// ---------------------------------------------------------------------------

#[test]
fn agent_credentials_default_is_empty() {
    let creds = AgentCredentials::default();
    assert!(creds.env_vars.is_empty());
}

// ---------------------------------------------------------------------------
// 29. Docker args only mount workspace — no config dirs or credential files
// ---------------------------------------------------------------------------

#[test]
fn docker_args_without_settings_has_workspace_mount_only() {
    let env = vec![("ANTHROPIC_API_KEY".into(), "sk-ant-oat01-test".into())];
    let args = aspec::docker::build_run_args("img", "/repo", &[], &env, None, false);
    // Without host_settings or allow_docker, only the workspace mount should be present.
    let volume_mounts: Vec<&String> = args.windows(2)
        .filter(|w| w[0] == "-v")
        .map(|w| &w[1])
        .collect();
    assert_eq!(volume_mounts.len(), 1, "Expected exactly one volume mount (workspace only). Got: {:?}", volume_mounts);
    assert!(volume_mounts[0].contains(":/workspace"), "The only mount should be the workspace");
    assert!(!args.iter().any(|a| a.contains("credentials")), "Must not mount any credential files");
}

// ---------------------------------------------------------------------------
// 30. Docker display args mask env var values
// ---------------------------------------------------------------------------

#[test]
fn docker_display_args_mask_secrets() {
    let env = vec![("ANTHROPIC_API_KEY".into(), "sk-ant-oat01-secret".into())];
    let args = aspec::docker::build_run_args_display("img", "/repo", &[], &env, None, false);
    assert!(args.contains(&"ANTHROPIC_API_KEY=***".to_string()), "API key should be masked");
    assert!(!args.iter().any(|a| a.contains("sk-ant-oat01-secret")), "Secret must not appear");
}

// ---------------------------------------------------------------------------
// 31. Chat entrypoint for each agent
// ---------------------------------------------------------------------------

#[test]
fn chat_entrypoint_for_each_agent() {
    let claude = chat_entrypoint("claude", false);
    assert_eq!(claude.len(), 1);
    assert_eq!(claude[0], "claude");

    let codex = chat_entrypoint("codex", false);
    assert_eq!(codex.len(), 1);
    assert_eq!(codex[0], "codex");

    let opencode = chat_entrypoint("opencode", false);
    assert_eq!(opencode.len(), 1);
    assert_eq!(opencode[0], "opencode");
}

// ---------------------------------------------------------------------------
// 32. Chat entrypoint non-interactive for each agent
// ---------------------------------------------------------------------------

#[test]
fn chat_entrypoint_non_interactive_for_each_agent() {
    let claude = chat_entrypoint_non_interactive("claude", false);
    assert_eq!(claude[0], "claude");
    assert_eq!(claude[1], "-p");

    let codex = chat_entrypoint_non_interactive("codex", false);
    assert_eq!(codex[0], "codex");
    assert_eq!(codex[1], "--quiet");

    let opencode = chat_entrypoint_non_interactive("opencode", false);
    assert_eq!(opencode.len(), 1);
    assert_eq!(opencode[0], "opencode");
}

// ---------------------------------------------------------------------------
// 33. Chat entrypoint has no prompt (unlike implement)
// ---------------------------------------------------------------------------

#[test]
fn chat_entrypoint_has_no_prompt() {
    for agent in &["claude", "codex", "opencode"] {
        let chat_args = chat_entrypoint(agent, false);
        let impl_args = agent_entrypoint(agent, 1, false);

        // Chat should have fewer args than implement (no prompt).
        assert!(
            chat_args.len() < impl_args.len(),
            "Chat entrypoint for {} should be shorter than implement entrypoint ({} vs {})",
            agent,
            chat_args.len(),
            impl_args.len()
        );

        // Chat should not contain any prompt-like text.
        for arg in &chat_args {
            assert!(
                !arg.contains("Implement"),
                "Chat entrypoint for {} should not contain implement prompt",
                agent
            );
            assert!(
                !arg.contains("work item"),
                "Chat entrypoint for {} should not reference a work item",
                agent
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 34. Chat and implement share docker run arg construction
// ---------------------------------------------------------------------------

#[test]
fn chat_and_implement_share_docker_args() {
    let chat_ep = chat_entrypoint("claude", false);
    let impl_ep = agent_entrypoint("claude", 1, false);

    let chat_ep_refs: Vec<&str> = chat_ep.iter().map(String::as_str).collect();
    let impl_ep_refs: Vec<&str> = impl_ep.iter().map(String::as_str).collect();

    let chat_args = aspec::docker::build_run_args("img", "/repo", &chat_ep_refs, &[], None, false);
    let impl_args = aspec::docker::build_run_args("img", "/repo", &impl_ep_refs, &[], None, false);

    // Both should start with the same Docker flags.
    assert_eq!(chat_args[0], impl_args[0]); // "run"
    assert_eq!(chat_args[1], impl_args[1]); // "--rm"
    assert_eq!(chat_args[2], impl_args[2]); // "-it"

    // Both should use the same image.
    assert!(chat_args.contains(&"img".to_string()));
    assert!(impl_args.contains(&"img".to_string()));

    // Chat should have "claude" only; implement should have "claude" + prompt.
    let chat_post_image: Vec<&String> = chat_args.iter().skip_while(|a| *a != "img").skip(1).collect();
    let impl_post_image: Vec<&String> = impl_args.iter().skip_while(|a| *a != "img").skip(1).collect();
    assert_eq!(chat_post_image.len(), 1, "Chat: just the agent command");
    assert_eq!(impl_post_image.len(), 2, "Implement: agent + prompt");
}

// ---------------------------------------------------------------------------
// 35. Autocomplete includes chat subcommand
// ---------------------------------------------------------------------------

#[test]
fn autocomplete_includes_chat_subcommand() {
    let sug = autocomplete_suggestions("");
    assert!(
        sug.contains(&"chat".to_string()),
        "Expected 'chat' in suggestions"
    );

    let sug = autocomplete_suggestions("ch");
    assert_eq!(sug, vec!["chat"]);
}

#[test]
fn autocomplete_chat_shows_hints() {
    let sug = autocomplete_suggestions("chat ");
    assert!(
        sug.iter().any(|s| s.contains("chat")),
        "Expected hint for 'chat' command, got: {:?}",
        sug
    );
    assert!(
        sug.iter().any(|s| s.contains("--non-interactive")),
        "Expected --non-interactive hint for 'chat' command, got: {:?}",
        sug
    );
}

// ---------------------------------------------------------------------------
// 36. Chat PendingCommand variant
// ---------------------------------------------------------------------------

#[test]
fn pending_command_chat_variant() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: false };
    assert_eq!(cmd, PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: false });
    assert_ne!(cmd, PendingCommand::Chat { non_interactive: true, plan: false, allow_docker: false });
    assert_ne!(cmd, PendingCommand::None);
}

// ---------------------------------------------------------------------------
// 37. Chat entrypoint non-interactive has no prompt
// ---------------------------------------------------------------------------

#[test]
fn chat_entrypoint_non_interactive_has_no_prompt() {
    for agent in &["claude", "codex", "opencode"] {
        let chat_args = chat_entrypoint_non_interactive(agent, false);
        let impl_args = agent_entrypoint_non_interactive(agent, 1, false);

        // Chat non-interactive should not contain prompt text.
        for arg in &chat_args {
            assert!(
                !arg.contains("Implement"),
                "Chat non-interactive for {} should not contain prompt: {}",
                agent,
                arg
            );
        }

        // Chat should have fewer or equal args than implement.
        assert!(
            chat_args.len() <= impl_args.len(),
            "Chat non-interactive for {} should not be longer than implement ({} vs {})",
            agent,
            chat_args.len(),
            impl_args.len()
        );
    }
}

// ---------------------------------------------------------------------------
// 38. PendingCommand::Ready includes build and no_cache fields
// ---------------------------------------------------------------------------

#[test]
fn pending_command_ready_build_no_cache_fields() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Ready {
        refresh: false,
        build: true,
        no_cache: true,
        non_interactive: false,
        allow_docker: false,
    };
    assert_eq!(cmd, PendingCommand::Ready {
        refresh: false,
        build: true,
        no_cache: true,
        non_interactive: false,
        allow_docker: false,
    });
    // Different build flag should not match
    assert_ne!(cmd, PendingCommand::Ready {
        refresh: false,
        build: false,
        no_cache: true,
        non_interactive: false,
        allow_docker: false,
    });
}

// ---------------------------------------------------------------------------
// 39. Docker format_build_cmd_no_cache
// ---------------------------------------------------------------------------

#[test]
fn docker_format_build_cmd_no_cache() {
    let cmd = aspec::docker::format_build_cmd_no_cache("img:latest", "Dockerfile.dev", "/repo");
    assert!(cmd.contains("--no-cache"), "Should contain --no-cache flag");
    assert!(cmd.contains("img:latest"), "Should contain image tag");
    assert!(cmd.contains("Dockerfile.dev"), "Should contain dockerfile");
}

// ---------------------------------------------------------------------------
// 40. CLI parses --build and --no-cache flags for ready
// ---------------------------------------------------------------------------

#[test]
fn cli_ready_build_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "ready", "--build"]);
    match cli.command.unwrap() {
        Command::Ready { build, .. } => assert!(build),
        _ => panic!("expected ready"),
    }
}

#[test]
fn cli_ready_no_cache_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "ready", "--no-cache"]);
    match cli.command.unwrap() {
        Command::Ready { no_cache, .. } => assert!(no_cache),
        _ => panic!("expected ready"),
    }
}

#[test]
fn cli_ready_build_and_no_cache() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "ready", "--build", "--no-cache"]);
    match cli.command.unwrap() {
        Command::Ready { build, no_cache, .. } => {
            assert!(build);
            assert!(no_cache);
        }
        _ => panic!("expected ready"),
    }
}

#[test]
fn cli_ready_all_flags_combined() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "ready", "--refresh", "--build", "--no-cache", "--non-interactive"]);
    match cli.command.unwrap() {
        Command::Ready { refresh, build, no_cache, non_interactive, .. } => {
            assert!(refresh);
            assert!(build);
            assert!(no_cache);
            assert!(non_interactive);
        }
        _ => panic!("expected ready"),
    }
}

#[test]
fn cli_ready_defaults_all_false() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "ready"]);
    match cli.command.unwrap() {
        Command::Ready { refresh, build, no_cache, non_interactive, .. } => {
            assert!(!refresh);
            assert!(!build);
            assert!(!no_cache);
            assert!(!non_interactive);
        }
        _ => panic!("expected ready"),
    }
}

// ---------------------------------------------------------------------------
// Root-level flags forwarded to ready at TUI startup
// ---------------------------------------------------------------------------

/// Flags passed to `aspec` (no subcommand) are available on the Cli struct
/// and should be forwarded to the `ready` command when the TUI starts.
#[test]
fn root_build_flag_parsed_for_tui_startup() {
    use aspec::cli::Cli;
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "--build"]);
    assert!(cli.command.is_none(), "no subcommand when flags are on root");
    assert!(cli.build);
    assert!(!cli.no_cache);
    assert!(!cli.refresh);
}

#[test]
fn root_no_cache_flag_parsed_for_tui_startup() {
    use aspec::cli::Cli;
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "--no-cache"]);
    assert!(cli.command.is_none());
    assert!(cli.no_cache);
}

#[test]
fn root_refresh_flag_parsed_for_tui_startup() {
    use aspec::cli::Cli;
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "--refresh"]);
    assert!(cli.command.is_none());
    assert!(cli.refresh);
}

#[test]
fn root_all_flags_parsed_for_tui_startup() {
    use aspec::cli::Cli;
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "--build", "--no-cache", "--refresh"]);
    assert!(cli.command.is_none());
    assert!(cli.build);
    assert!(cli.no_cache);
    assert!(cli.refresh);
}

// ---------------------------------------------------------------------------
// Plan flag: CLI parsing
// ---------------------------------------------------------------------------

#[test]
fn cli_chat_plan_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "chat", "--plan"]);
    match cli.command.unwrap() {
        Command::Chat { plan, non_interactive, .. } => {
            assert!(plan);
            assert!(!non_interactive);
        }
        _ => panic!("expected chat"),
    }
}

#[test]
fn cli_implement_plan_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "implement", "0001", "--plan"]);
    match cli.command.unwrap() {
        Command::Implement { plan, work_item, non_interactive, .. } => {
            assert!(plan);
            assert_eq!(work_item, "0001");
            assert!(!non_interactive);
        }
        _ => panic!("expected implement"),
    }
}

#[test]
fn cli_chat_plan_and_non_interactive() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "chat", "--plan", "--non-interactive"]);
    match cli.command.unwrap() {
        Command::Chat { plan, non_interactive, .. } => {
            assert!(plan);
            assert!(non_interactive);
        }
        _ => panic!("expected chat"),
    }
}

#[test]
fn cli_implement_plan_and_non_interactive() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::parse_from(&["aspec", "implement", "42", "--plan", "--non-interactive"]);
    match cli.command.unwrap() {
        Command::Implement { plan, non_interactive, work_item, .. } => {
            assert!(plan);
            assert!(non_interactive);
            assert_eq!(work_item, "42");
        }
        _ => panic!("expected implement"),
    }
}

// ---------------------------------------------------------------------------
// Plan flag: agent entrypoint configuration per agent
// ---------------------------------------------------------------------------

#[test]
fn plan_flag_configures_claude_correctly() {
    // Claude uses --permission-mode plan
    let chat = chat_entrypoint("claude", true);
    assert!(chat.contains(&"--permission-mode".to_string()), "Claude chat should include --permission-mode");
    assert!(chat.contains(&"plan".to_string()), "Claude chat should include plan");

    let imp = agent_entrypoint("claude", 1, true);
    assert!(imp.contains(&"--permission-mode".to_string()), "Claude implement should include --permission-mode");
    assert!(imp.contains(&"plan".to_string()), "Claude implement should include plan");

    let chat_ni = chat_entrypoint_non_interactive("claude", true);
    assert!(chat_ni.contains(&"--permission-mode".to_string()), "Claude chat non-interactive should include --permission-mode");
    assert!(chat_ni.contains(&"plan".to_string()), "Claude chat non-interactive should include plan");

    let imp_ni = agent_entrypoint_non_interactive("claude", 1, true);
    assert!(imp_ni.contains(&"--permission-mode".to_string()), "Claude implement non-interactive should include --permission-mode");
    assert!(imp_ni.contains(&"plan".to_string()), "Claude implement non-interactive should include plan");
}

#[test]
fn plan_flag_configures_codex_correctly() {
    // Codex uses --approval-mode plan
    let chat = chat_entrypoint("codex", true);
    assert!(chat.contains(&"--approval-mode".to_string()), "Codex chat should include --approval-mode");
    assert!(chat.contains(&"plan".to_string()), "Codex chat should include plan");

    let imp = agent_entrypoint("codex", 2, true);
    assert!(imp.contains(&"--approval-mode".to_string()), "Codex implement should include --approval-mode");
    assert!(imp.contains(&"plan".to_string()), "Codex implement should include plan");
}

#[test]
fn plan_flag_ignored_for_opencode() {
    // Opencode has no plan mode; flag should be silently ignored.
    let chat_no_plan = chat_entrypoint("opencode", false);
    let chat_plan = chat_entrypoint("opencode", true);
    assert_eq!(chat_no_plan, chat_plan, "Opencode chat should be unchanged with --plan");

    let imp_no_plan = agent_entrypoint("opencode", 3, false);
    let imp_plan = agent_entrypoint("opencode", 3, true);
    assert_eq!(imp_no_plan, imp_plan, "Opencode implement should be unchanged with --plan");
}

#[test]
fn plan_false_does_not_add_flags() {
    // When plan=false, no plan flags should appear.
    for agent in &["claude", "codex", "opencode"] {
        let chat = chat_entrypoint(agent, false);
        assert!(!chat.contains(&"--permission-mode".to_string()), "No --permission-mode for {} with plan=false", agent);
        assert!(!chat.contains(&"--approval-mode".to_string()), "No --approval-mode for {} with plan=false", agent);

        let imp = agent_entrypoint(agent, 1, false);
        assert!(!imp.contains(&"--permission-mode".to_string()), "No --permission-mode for {} implement with plan=false", agent);
        assert!(!imp.contains(&"--approval-mode".to_string()), "No --approval-mode for {} implement with plan=false", agent);
    }
}

// ---------------------------------------------------------------------------
// Plan flag: PendingCommand variants include plan field
// ---------------------------------------------------------------------------

#[test]
fn pending_command_chat_plan_field() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Chat { non_interactive: false, plan: true, allow_docker: false };
    assert_eq!(cmd, PendingCommand::Chat { non_interactive: false, plan: true, allow_docker: false });
    assert_ne!(cmd, PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: false });
}

#[test]
fn pending_command_implement_plan_field() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Implement { work_item: 1, non_interactive: false, plan: true, allow_docker: false };
    assert_eq!(cmd, PendingCommand::Implement { work_item: 1, non_interactive: false, plan: true, allow_docker: false });
    assert_ne!(cmd, PendingCommand::Implement { work_item: 1, non_interactive: false, plan: false, allow_docker: false });
}

// ---------------------------------------------------------------------------
// Plan flag: autocomplete hints include --plan
// ---------------------------------------------------------------------------

#[test]
fn autocomplete_chat_shows_plan_hint() {
    let sug = autocomplete_suggestions("chat ");
    assert!(
        sug.iter().any(|s| s.contains("--plan")),
        "Expected --plan hint for 'chat' command, got: {:?}",
        sug
    );
}

#[test]
fn autocomplete_implement_shows_plan_hint() {
    let sug = autocomplete_suggestions("implement ");
    assert!(
        sug.iter().any(|s| s.contains("--plan")),
        "Expected --plan hint for 'implement' command, got: {:?}",
        sug
    );
}

// ---------------------------------------------------------------------------
// allow-docker flag: CLI parsing
// ---------------------------------------------------------------------------

#[test]
fn cli_implement_allow_docker_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "implement", "0001", "--allow-docker"]).unwrap();
    match cli.command.unwrap() {
        Command::Implement { allow_docker, work_item, .. } => {
            assert!(allow_docker, "Expected allow_docker=true");
            assert_eq!(work_item, "0001");
        }
        _ => panic!("Expected Implement command"),
    }
}

#[test]
fn cli_implement_no_allow_docker_by_default() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "implement", "0001"]).unwrap();
    match cli.command.unwrap() {
        Command::Implement { allow_docker, .. } => {
            assert!(!allow_docker, "Expected allow_docker=false by default");
        }
        _ => panic!("Expected Implement command"),
    }
}

#[test]
fn cli_chat_allow_docker_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "chat", "--allow-docker"]).unwrap();
    match cli.command.unwrap() {
        Command::Chat { allow_docker, .. } => {
            assert!(allow_docker, "Expected allow_docker=true");
        }
        _ => panic!("Expected Chat command"),
    }
}

#[test]
fn cli_chat_no_allow_docker_by_default() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "chat"]).unwrap();
    match cli.command.unwrap() {
        Command::Chat { allow_docker, .. } => {
            assert!(!allow_docker, "Expected allow_docker=false by default");
        }
        _ => panic!("Expected Chat command"),
    }
}

#[test]
fn cli_ready_allow_docker_flag() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "ready", "--allow-docker"]).unwrap();
    match cli.command.unwrap() {
        Command::Ready { allow_docker, .. } => {
            assert!(allow_docker, "Expected allow_docker=true");
        }
        _ => panic!("Expected Ready command"),
    }
}

#[test]
fn cli_ready_no_allow_docker_by_default() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "ready"]).unwrap();
    match cli.command.unwrap() {
        Command::Ready { allow_docker, .. } => {
            assert!(!allow_docker, "Expected allow_docker=false by default");
        }
        _ => panic!("Expected Ready command"),
    }
}

#[test]
fn cli_implement_allow_docker_with_plan() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "implement", "0042", "--allow-docker", "--plan"]).unwrap();
    match cli.command.unwrap() {
        Command::Implement { allow_docker, plan, work_item, .. } => {
            assert!(allow_docker);
            assert!(plan);
            assert_eq!(work_item, "0042");
        }
        _ => panic!("Expected Implement command"),
    }
}

#[test]
fn cli_chat_allow_docker_with_plan() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "chat", "--allow-docker", "--plan"]).unwrap();
    match cli.command.unwrap() {
        Command::Chat { allow_docker, plan, .. } => {
            assert!(allow_docker);
            assert!(plan);
        }
        _ => panic!("Expected Chat command"),
    }
}

#[test]
fn cli_ready_allow_docker_with_refresh() {
    use aspec::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aspec", "ready", "--allow-docker", "--refresh"]).unwrap();
    match cli.command.unwrap() {
        Command::Ready { allow_docker, refresh, .. } => {
            assert!(allow_docker);
            assert!(refresh);
        }
        _ => panic!("Expected Ready command"),
    }
}

// ---------------------------------------------------------------------------
// allow-docker flag: PendingCommand variants include allow_docker field
// ---------------------------------------------------------------------------

#[test]
fn pending_command_chat_allow_docker_field() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: true };
    assert_eq!(cmd, PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: true });
    assert_ne!(cmd, PendingCommand::Chat { non_interactive: false, plan: false, allow_docker: false });
}

#[test]
fn pending_command_implement_allow_docker_field() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Implement { work_item: 1, non_interactive: false, plan: false, allow_docker: true };
    assert_eq!(cmd, PendingCommand::Implement { work_item: 1, non_interactive: false, plan: false, allow_docker: true });
    assert_ne!(cmd, PendingCommand::Implement { work_item: 1, non_interactive: false, plan: false, allow_docker: false });
}

#[test]
fn pending_command_ready_allow_docker_field() {
    use aspec::tui::state::PendingCommand;

    let cmd = PendingCommand::Ready { refresh: false, build: false, no_cache: false, non_interactive: false, allow_docker: true };
    assert_eq!(cmd, PendingCommand::Ready { refresh: false, build: false, no_cache: false, non_interactive: false, allow_docker: true });
    assert_ne!(cmd, PendingCommand::Ready { refresh: false, build: false, no_cache: false, non_interactive: false, allow_docker: false });
}

// ---------------------------------------------------------------------------
// allow-docker flag: socket mount appears in docker run args
// ---------------------------------------------------------------------------

#[test]
fn allow_docker_adds_socket_mount_to_implement_run_args() {
    use aspec::docker::build_run_args;

    let socket_path = aspec::docker::docker_socket_path();
    let socket_str = socket_path.to_string_lossy().to_string();

    let args = build_run_args(
        "test-image",
        "/workspace",
        &["entrypoint.sh"],
        &[],
        None,
        true, // allow_docker
    );

    #[cfg(not(target_os = "windows"))]
    {
        let joined = args.join(" ");
        assert!(
            joined.contains(&socket_str),
            "Expected socket path {} in args: {:?}",
            socket_str,
            args
        );
    }
}

#[test]
fn no_allow_docker_omits_socket_mount_from_implement_run_args() {
    use aspec::docker::build_run_args;

    let socket_path = aspec::docker::docker_socket_path();
    let socket_str = socket_path.to_string_lossy().to_string();

    let args = build_run_args(
        "test-image",
        "/workspace",
        &["entrypoint.sh"],
        &[],
        None,
        false, // allow_docker
    );

    let joined = args.join(" ");
    assert!(
        !joined.contains(&socket_str),
        "Did not expect socket path {} in args without allow_docker: {:?}",
        socket_str,
        args
    );
}

// ---------------------------------------------------------------------------
// allow-docker flag: autocomplete hints include --allow-docker
// ---------------------------------------------------------------------------

#[test]
fn autocomplete_chat_shows_allow_docker_hint() {
    let sug = autocomplete_suggestions("chat ");
    assert!(
        sug.iter().any(|s| s.contains("--allow-docker")),
        "Expected --allow-docker hint for 'chat' command, got: {:?}",
        sug
    );
}

#[test]
fn autocomplete_implement_shows_allow_docker_hint() {
    let sug = autocomplete_suggestions("implement ");
    assert!(
        sug.iter().any(|s| s.contains("--allow-docker")),
        "Expected --allow-docker hint for 'implement' command, got: {:?}",
        sug
    );
}

#[test]
fn autocomplete_ready_shows_allow_docker_hint() {
    let sug = autocomplete_suggestions("ready ");
    assert!(
        sug.iter().any(|s| s.contains("--allow-docker")),
        "Expected --allow-docker hint for 'ready' command, got: {:?}",
        sug
    );
}
