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
