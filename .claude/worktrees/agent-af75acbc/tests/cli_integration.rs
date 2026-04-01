/// Integration tests for the amux CLI binary.
///
/// These tests invoke the compiled binary to validate end-to-end behaviour
/// across multiple components.
use std::process::Command;

fn amux() -> Command {
    Command::new(env!("CARGO_BIN_EXE_amux"))
}

#[test]
fn help_exits_successfully() {
    let output = amux().arg("--help").output().expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("amux"));
}

#[test]
fn version_exits_successfully() {
    let output = amux().arg("--version").output().expect("failed to run amux");
    assert!(output.status.success());
}

#[test]
fn implement_missing_work_item_prints_error() {
    let output = amux()
        .args(["implement", "9999"])
        .output()
        .expect("failed to run amux");
    // Should fail (non-zero exit) because work item 9999 does not exist.
    assert!(!output.status.success());
}

#[test]
fn implement_accepts_four_digit_work_item() {
    let output = amux()
        .args(["implement", "0099"])
        .output()
        .expect("failed to run amux");
    // Should fail because work item 0099 doesn't exist, but the input should be accepted.
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should report the work item is missing, not an invalid number error.
    assert!(
        stderr.contains("work item") || stderr.contains("0099") || stderr.contains("99"),
        "Expected work-item-not-found error, got: {}",
        stderr
    );
}

#[test]
fn ready_help_shows_refresh_flag() {
    let output = amux()
        .args(["ready", "--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--refresh"),
        "ready --help should mention --refresh flag"
    );
}

#[test]
fn ready_help_shows_non_interactive_flag() {
    let output = amux()
        .args(["ready", "--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--non-interactive"),
        "ready --help should mention --non-interactive flag"
    );
}

#[test]
fn implement_help_shows_non_interactive_flag() {
    let output = amux()
        .args(["implement", "--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--non-interactive"),
        "implement --help should mention --non-interactive flag"
    );
}

#[test]
fn new_help_shows_subcommand() {
    let output = amux()
        .args(["specs", "--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("new"),
        "specs --help should mention 'new' subcommand"
    );
}

#[test]
fn chat_help_shows_subcommand() {
    let output = amux()
        .args(["--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("chat"),
        "help should mention 'chat' subcommand"
    );
}

#[test]
fn chat_help_shows_non_interactive_flag() {
    let output = amux()
        .args(["chat", "--help"])
        .output()
        .expect("failed to run amux");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--non-interactive"),
        "chat --help should mention --non-interactive flag"
    );
}
