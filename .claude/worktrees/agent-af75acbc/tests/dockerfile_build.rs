/// Integration tests that build each agent's Dockerfile template using Docker.
///
/// These tests verify that the template Dockerfiles produce valid images
/// in their default/template states. They require a running Docker daemon
/// and network access, and are skipped if Docker is unavailable.
use std::process::Command;

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn build_template(template_path: &str, tag: &str) {
    if !docker_available() {
        eprintln!("Docker not available, skipping Dockerfile build test");
        return;
    }

    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            tag,
            "-f",
            template_path,
            ".",
        ])
        .status()
        .expect("failed to invoke docker build");

    assert!(
        status.success(),
        "docker build failed for template: {}",
        template_path
    );

    // Clean up the test image.
    let _ = Command::new("docker")
        .args(["rmi", tag])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[test]
fn build_claude_template() {
    build_template("templates/Dockerfile.claude", "amux-test-claude:latest");
}

#[test]
fn build_codex_template() {
    build_template("templates/Dockerfile.codex", "amux-test-codex:latest");
}

#[test]
fn build_opencode_template() {
    build_template("templates/Dockerfile.opencode", "amux-test-opencode:latest");
}
