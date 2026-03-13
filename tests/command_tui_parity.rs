/// Integration tests verifying that command-mode and TUI-mode reuse the same underlying logic.
///
/// These tests call the shared `run_with_sink` / helper functions directly,
/// confirming that the same code paths are exercised regardless of execution mode.
use aspec::commands::auth::apply_auth_decision;
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
    let _ = ready::run_with_sink(&sink, mount_path, vec![], &opts).await;

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

#[tokio::test]
async fn ready_sink_routes_all_output() {
    // Skip when Docker is not available.
    if !aspec::docker::is_daemon_running() {
        return;
    }

    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);

    let mount_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let opts = ReadyOptions::default();
    let result = ready::run_with_sink(&sink, mount_path, vec![], &opts).await;
    let _ = result;

    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    // Must include project-specific image tag references.
    let has_image_ref = messages
        .iter()
        .any(|m| m.contains("aspec-") && m.contains(":latest"));
    assert!(
        has_image_ref,
        "Expected project-specific image tag in output. Got: {:?}",
        messages
    );

    // Without --refresh, should have skip message.
    let has_skip = messages
        .iter()
        .any(|m| m.contains("Skipping Dockerfile audit"));
    assert!(
        has_skip,
        "Expected skip message in output. Got: {:?}",
        messages
    );
}

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

#[tokio::test]
async fn ready_no_refresh_skips_audit_with_message() {
    if !aspec::docker::is_daemon_running() {
        return;
    }
    let git_root = match aspec::commands::init::find_git_root() {
        Some(r) => r,
        None => return,
    };
    if !git_root.join("Dockerfile.dev").exists() {
        return;
    }

    let (tx, mut rx) = unbounded_channel::<String>();
    let sink = OutputSink::Channel(tx);
    let opts = ReadyOptions { refresh: false, non_interactive: false };
    let _ = ready::run_with_sink(&sink, git_root.clone(), vec![], &opts).await;
    let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let has_skip = messages.iter().any(|m| m.contains("Skipping"));
    let has_tip = messages.iter().any(|m| m.contains("--refresh"));
    assert!(has_skip, "Expected skip message. Got: {:?}", messages);
    assert!(has_tip, "Expected --refresh tip. Got: {:?}", messages);
}

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
fn autocomplete_ready_shows_new_flags() {
    let sug = autocomplete_suggestions("ready ");
    assert!(sug.iter().any(|s| s.contains("--refresh")));
    assert!(sug.iter().any(|s| s.contains("--non-interactive")));
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
    let args = aspec::docker::build_run_args("img", "/repo", &[], &env, None);
    assert!(args.contains(&"-e".to_string()));
    assert!(args.contains(&"ANTHROPIC_API_KEY=sk-test".to_string()));
}

#[test]
fn display_args_mask_env_var_values() {
    let env = vec![("ANTHROPIC_API_KEY".into(), "sk-secret".into())];
    let args = aspec::docker::build_run_args_display("img", "/repo", &[], &env, None);
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
    let claude = agent_entrypoint("claude", 1);
    assert_eq!(claude.len(), 2);
    assert_eq!(claude[0], "claude");
    assert!(claude[1].contains("work item 0001"));
    assert!(claude[1].contains("Iterate until the build succeeds"));

    let codex = agent_entrypoint("codex", 2);
    assert_eq!(codex[0], "codex");
    assert!(codex[1].contains("work item 0002"));

    let opencode = agent_entrypoint("opencode", 3);
    assert_eq!(opencode[0], "opencode");
    assert_eq!(opencode[1], "run");
    assert!(opencode[2].contains("work item 0003"));
}

#[test]
fn implement_entrypoint_non_interactive_for_each_agent() {
    let claude = agent_entrypoint_non_interactive("claude", 1);
    assert_eq!(claude[0], "claude");
    assert_eq!(claude[1], "-p");
    assert!(claude[2].contains("work item 0001"));

    let codex = agent_entrypoint_non_interactive("codex", 2);
    assert_eq!(codex[0], "codex");
    assert_eq!(codex[1], "--quiet");
    assert!(codex[2].contains("work item 0002"));

    let opencode = agent_entrypoint_non_interactive("opencode", 3);
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
// 8. ReadyOptions defaults
// ---------------------------------------------------------------------------

#[test]
fn ready_options_default_no_refresh_no_non_interactive() {
    let opts = ReadyOptions::default();
    assert!(!opts.refresh);
    assert!(!opts.non_interactive);
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
