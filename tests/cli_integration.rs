/// Integration tests for the aspec CLI binary.
///
/// These tests invoke the compiled binary to validate end-to-end behaviour
/// across multiple components.
use std::process::Command;

fn aspec() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aspec"))
}

#[test]
fn help_exits_successfully() {
    let output = aspec().arg("--help").output().expect("failed to run aspec");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("aspec"));
}

#[test]
fn version_exits_successfully() {
    let output = aspec().arg("--version").output().expect("failed to run aspec");
    assert!(output.status.success());
}

#[test]
fn implement_missing_work_item_prints_error() {
    let output = aspec()
        .args(["implement", "9999"])
        .output()
        .expect("failed to run aspec");
    // Should fail (non-zero exit) because work item 9999 does not exist.
    assert!(!output.status.success());
}

#[test]
fn implement_accepts_four_digit_work_item() {
    let output = aspec()
        .args(["implement", "0099"])
        .output()
        .expect("failed to run aspec");
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
    let output = aspec()
        .args(["ready", "--help"])
        .output()
        .expect("failed to run aspec");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--refresh"),
        "ready --help should mention --refresh flag"
    );
}

#[test]
fn ready_help_shows_non_interactive_flag() {
    let output = aspec()
        .args(["ready", "--help"])
        .output()
        .expect("failed to run aspec");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--non-interactive"),
        "ready --help should mention --non-interactive flag"
    );
}

#[test]
fn implement_help_shows_non_interactive_flag() {
    let output = aspec()
        .args(["implement", "--help"])
        .output()
        .expect("failed to run aspec");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--non-interactive"),
        "implement --help should mention --non-interactive flag"
    );
}

#[test]
fn new_help_shows_subcommand() {
    let output = aspec()
        .args(["--help"])
        .output()
        .expect("failed to run aspec");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("new"),
        "help should mention 'new' subcommand"
    );
}
