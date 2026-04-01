pub mod dag;
pub mod parser;

use anyhow::{bail, Context, Result};
use parser::WorkflowStep;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use dag::{detect_cycle, ready_steps, validate_references};
pub use parser::parse_workflow;

// ─── Step status ─────────────────────────────────────────────────────────────

/// Lifecycle state of a single workflow step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error(String),
}

// ─── Per-step state ───────────────────────────────────────────────────────────

/// Persisted state for one step: includes the original definition plus runtime status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepState {
    pub name: String,
    pub depends_on: Vec<String>,
    pub prompt_template: String,
    pub status: StepStatus,
    /// Most-recent container ID used for this step (overwritten on retry).
    pub container_id: Option<String>,
}

// ─── Workflow state ───────────────────────────────────────────────────────────

/// Full, serialisable state of an in-progress (or completed) workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// Optional display title from the workflow file's `# Title` heading.
    pub title: Option<String>,
    /// Steps in the order they appear in the workflow file.
    pub steps: Vec<WorkflowStepState>,
    /// SHA-256 hex digest of the workflow file at the time this state was created.
    pub workflow_hash: String,
    /// Work item number (e.g. 27 for work item 0027).
    pub work_item: u32,
    /// Filename stem of the workflow file (used as part of the state-file name).
    pub workflow_name: String,
}

impl WorkflowState {
    /// Create a fresh workflow state from parsed steps.
    pub fn new(
        title: Option<String>,
        steps_parsed: Vec<WorkflowStep>,
        workflow_hash: String,
        work_item: u32,
        workflow_name: String,
    ) -> Self {
        let steps = steps_parsed
            .into_iter()
            .map(|s| WorkflowStepState {
                depends_on: s.depends_on,
                prompt_template: s.prompt_template,
                status: StepStatus::Pending,
                container_id: None,
                name: s.name,
            })
            .collect();

        Self {
            title,
            steps,
            workflow_hash,
            work_item,
            workflow_name,
        }
    }

    /// Return names of steps that are ready to run (all deps done, not yet started/done).
    pub fn next_ready(&self) -> Vec<String> {
        let completed = self.completed_set();
        // Exclude steps that are currently Running or Error (not safe to re-run automatically).
        let blocked: HashSet<String> = self
            .steps
            .iter()
            .filter(|s| {
                matches!(s.status, StepStatus::Running | StepStatus::Error(_))
            })
            .map(|s| s.name.clone())
            .collect();

        let pseudo_steps: Vec<WorkflowStep> = self
            .steps
            .iter()
            .filter(|s| !blocked.contains(&s.name))
            .map(|s| WorkflowStep {
                name: s.name.clone(),
                depends_on: s.depends_on.clone(),
                prompt_template: s.prompt_template.clone(),
            })
            .collect();

        ready_steps(&pseudo_steps, &completed)
    }

    /// Return the set of step names whose status is `Done`.
    pub fn completed_set(&self) -> HashSet<String> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Returns `true` when every step is `Done`.
    pub fn all_done(&self) -> bool {
        self.steps.iter().all(|s| s.status == StepStatus::Done)
    }

    /// Update the status of the named step (no-op if the name is not found).
    pub fn set_status(&mut self, name: &str, status: StepStatus) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.name == name) {
            step.status = status;
        }
    }

    /// Record the container ID used for a step (overwrites previous value on retry).
    pub fn set_container_id(&mut self, name: &str, container_id: String) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.name == name) {
            step.container_id = Some(container_id);
        }
    }

    /// Look up a step by name.
    pub fn get_step(&self, name: &str) -> Option<&WorkflowStepState> {
        self.steps.iter().find(|s| s.name == name)
    }

    /// Return the names of steps that were in `Running` state when the state was saved
    /// (indicating an interrupted run).
    pub fn interrupted_running_steps(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Running)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Returns step names that are in a "parallel group" for a given step:
    /// all steps that share exactly the same set of depends_on values.
    /// Returns them in file order.
    pub fn parallel_group_for(&self, step_name: &str) -> Vec<String> {
        let target = match self.get_step(step_name) {
            Some(s) => s,
            None => return vec![step_name.to_string()],
        };
        let target_deps = &target.depends_on;
        self.steps
            .iter()
            .filter(|s| &s.depends_on == target_deps)
            .map(|s| s.name.clone())
            .collect()
    }
}

// ─── SHA-256 helper ───────────────────────────────────────────────────────────

/// Compute the SHA-256 hash of `data`, returned as a lowercase hex string.
pub fn sha256_hex(data: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

// ─── File I/O helpers ─────────────────────────────────────────────────────────

/// Read a workflow file, compute its hash, and parse + validate its contents.
/// Returns `(hash, title, steps)`.
pub fn load_workflow_file(path: &Path) -> Result<(String, Option<String>, Vec<WorkflowStep>)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read workflow file: {}", path.display()))?;
    let hash = sha256_hex(&content);
    let (title, steps) = parse_workflow(&content)?;
    validate_references(&steps)?;
    detect_cycle(&steps)?;
    Ok((hash, title, steps))
}

/// Return the file-system path where the workflow state JSON is stored.
///
/// Format: `$GITROOT/.amux/workflows/<repo-hash8>-<work-item>-<workflow-name>.json`
pub fn workflow_state_path(git_root: &Path, work_item: u32, workflow_name: &str) -> PathBuf {
    let repo_hash = &sha256_hex(&git_root.to_string_lossy())[..8];
    git_root
        .join(".amux/workflows")
        .join(format!("{}-{:04}-{}.json", repo_hash, work_item, workflow_name))
}

/// Persist the workflow state to its JSON file, creating the directory if needed.
pub fn save_workflow_state(git_root: &Path, state: &WorkflowState) -> Result<()> {
    let path = workflow_state_path(git_root, state.work_item, &state.workflow_name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create workflow state directory: {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json)
        .with_context(|| format!("Cannot write workflow state: {}", path.display()))?;
    Ok(())
}

/// Load and deserialise a workflow state file.
pub fn load_workflow_state(path: &Path) -> Result<WorkflowState> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read workflow state: {}", path.display()))?;
    let state: WorkflowState = serde_json::from_str(&content)
        .with_context(|| format!("Cannot parse workflow state: {}", path.display()))?;
    Ok(state)
}

// ─── Prompt substitution ──────────────────────────────────────────────────────

/// Substitute template variables in a prompt string.
///
/// Variables:
/// - `{{work_item_number}}` → zero-padded four-digit work item number
/// - `{{work_item_content}}` → full text of the work item file
/// - `{{work_item_section:[name]}}` → content of a named H1/H2 section
///
/// Unknown variables are left in place (with a logged warning in tests only).
pub fn substitute_prompt(template: &str, work_item: u32, work_item_content: &str) -> String {
    let mut result = template.to_string();
    result = result.replace("{{work_item_number}}", &format!("{:04}", work_item));
    result = result.replace("{{work_item_content}}", work_item_content);

    // Handle {{work_item_section:[name]}} substitutions iteratively.
    loop {
        if let Some(start) = result.find("{{work_item_section:[") {
            if let Some(rel_end) = result[start..].find("]}}") {
                let name_start = start + "{{work_item_section:[".len();
                let name_end = start + rel_end;
                let section_name = result[name_start..name_end].to_string();
                let section_content = extract_section(work_item_content, &section_name);
                let token = format!("{{{{work_item_section:[{}]}}}}", section_name);
                result = result.replacen(&token, &section_content, 1);
            } else {
                break; // Malformed token — leave the rest as-is.
            }
        } else {
            break;
        }
    }

    result
}

/// Extract the body of a named H1 or H2 section from Markdown.
///
/// Matches the section heading case-insensitively.  Returns everything from
/// the line *after* the heading to the line *before* the next H1/H2 heading
/// (or end of file), trimmed.
fn extract_section(content: &str, section_name: &str) -> String {
    let target = section_name.trim().to_lowercase();
    let mut in_section = false;
    let mut body = String::new();

    for line in content.lines() {
        if line.starts_with("## ") || line.starts_with("# ") {
            let heading_raw = if line.starts_with("## ") {
                line[3..].trim()
            } else {
                line[2..].trim()
            };
            let heading = heading_raw.trim_end_matches(':');
            if heading.to_lowercase() == target {
                in_section = true;
                continue;
            } else if in_section {
                break; // Next heading — stop collecting.
            }
        } else if in_section {
            body.push_str(line);
            body.push('\n');
        }
    }

    body.trim().to_string()
}

/// Validate that the step names and depends-on values of a reloaded workflow match the saved state.
/// Used when resuming with a changed (but user-accepted) workflow file.
pub fn validate_resume_compatibility(
    saved: &WorkflowState,
    new_steps: &[WorkflowStep],
) -> Result<()> {
    if saved.steps.len() != new_steps.len() {
        bail!(
            "Cannot resume: the workflow now has {} steps but the saved state has {}.",
            new_steps.len(),
            saved.steps.len()
        );
    }
    for (saved_step, new_step) in saved.steps.iter().zip(new_steps.iter()) {
        if saved_step.name != new_step.name {
            bail!(
                "Cannot resume: step order changed — expected '{}' but found '{}'.",
                saved_step.name,
                new_step.name
            );
        }
        if saved_step.depends_on != new_step.depends_on {
            bail!(
                "Cannot resume: step '{}' depends-on changed from {:?} to {:?}.",
                saved_step.name,
                saved_step.depends_on,
                new_step.depends_on
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_step(name: &str, deps: &[&str], prompt: &str) -> WorkflowStep {
        WorkflowStep {
            name: name.to_string(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            prompt_template: prompt.to_string(),
        }
    }

    // ─── WorkflowState::next_ready ─────────────────────────────────────────────

    #[test]
    fn next_ready_pending_no_deps() {
        let state = WorkflowState::new(
            None,
            vec![make_step("plan", &[], "p"), make_step("impl", &["plan"], "i")],
            "hash".into(),
            1,
            "wf".into(),
        );
        let ready = state.next_ready();
        assert_eq!(ready, vec!["plan"]);
    }

    #[test]
    fn next_ready_unlocks_after_done() {
        let mut state = WorkflowState::new(
            None,
            vec![make_step("plan", &[], "p"), make_step("impl", &["plan"], "i")],
            "hash".into(),
            1,
            "wf".into(),
        );
        state.set_status("plan", StepStatus::Done);
        let ready = state.next_ready();
        assert_eq!(ready, vec!["impl"]);
    }

    #[test]
    fn next_ready_empty_when_all_done() {
        let mut state = WorkflowState::new(
            None,
            vec![make_step("plan", &[], "p"), make_step("impl", &["plan"], "i")],
            "hash".into(),
            1,
            "wf".into(),
        );
        state.set_status("plan", StepStatus::Done);
        state.set_status("impl", StepStatus::Done);
        assert!(state.next_ready().is_empty());
        assert!(state.all_done());
    }

    #[test]
    fn next_ready_skips_running_and_error_steps() {
        let mut state = WorkflowState::new(
            None,
            vec![make_step("a", &[], ""), make_step("b", &[], "")],
            "hash".into(),
            1,
            "wf".into(),
        );
        state.set_status("a", StepStatus::Running);
        // "b" is still pending with no deps, so it appears ready.
        // "a" is running — excluded.
        let ready = state.next_ready();
        assert!(!ready.contains(&"a".to_string()));
        assert!(ready.contains(&"b".to_string()));
    }

    // ─── State transitions ─────────────────────────────────────────────────────

    #[test]
    fn state_transitions_pending_running_done() {
        let mut state = WorkflowState::new(
            None,
            vec![make_step("plan", &[], "p")],
            "hash".into(),
            1,
            "wf".into(),
        );
        assert_eq!(state.get_step("plan").unwrap().status, StepStatus::Pending);
        state.set_status("plan", StepStatus::Running);
        assert_eq!(state.get_step("plan").unwrap().status, StepStatus::Running);
        state.set_status("plan", StepStatus::Done);
        assert_eq!(state.get_step("plan").unwrap().status, StepStatus::Done);
    }

    #[test]
    fn state_transition_running_to_error() {
        let mut state = WorkflowState::new(
            None,
            vec![make_step("plan", &[], "p")],
            "hash".into(),
            1,
            "wf".into(),
        );
        state.set_status("plan", StepStatus::Running);
        state.set_status("plan", StepStatus::Error("exit 1".into()));
        match &state.get_step("plan").unwrap().status {
            StepStatus::Error(msg) => assert_eq!(msg, "exit 1"),
            _ => panic!("expected Error"),
        }
    }

    // ─── Prompt substitution ───────────────────────────────────────────────────

    #[test]
    fn substitute_work_item_number() {
        let result = substitute_prompt("Item {{work_item_number}}", 27, "");
        assert_eq!(result, "Item 0027");
    }

    #[test]
    fn substitute_work_item_content() {
        let result = substitute_prompt("Content: {{work_item_content}}", 1, "Hello world");
        assert_eq!(result, "Content: Hello world");
    }

    #[test]
    fn substitute_no_placeholder_unchanged() {
        let result = substitute_prompt("Just a plain prompt.", 1, "content");
        assert_eq!(result, "Just a plain prompt.");
    }

    #[test]
    fn substitute_work_item_section() {
        let content = "# Title\n\n## Implementation Details\nDo the thing.\nMore details.\n\n## Other\nIgnored.\n";
        let result = substitute_prompt(
            "Details: {{work_item_section:[Implementation Details]}}",
            1,
            content,
        );
        assert!(result.contains("Do the thing."));
        assert!(result.contains("More details."));
        assert!(!result.contains("Ignored."));
    }

    #[test]
    fn substitute_section_case_insensitive() {
        let content = "## IMPL DETAILS\nStuff.\n";
        let result = substitute_prompt("{{work_item_section:[impl details]}}", 1, content);
        assert!(result.contains("Stuff."));
    }

    #[test]
    fn substitute_section_with_trailing_colon_in_heading() {
        // Work item headings often have a trailing colon (e.g. "## Implementation Details:")
        // but workflow templates reference them without (e.g. {{work_item_section:[Implementation Details]}}).
        let content = "## Implementation Details:\nDo the thing.\n## Other:\nIgnored.\n";
        let result = substitute_prompt(
            "{{work_item_section:[Implementation Details]}}",
            1,
            content,
        );
        assert!(result.contains("Do the thing."), "got: {result}");
        assert!(!result.contains("Ignored."));
    }

    // ─── validate_resume_compatibility ────────────────────────────────────────

    #[test]
    fn resume_compat_same_steps_ok() {
        let state = WorkflowState::new(
            None,
            vec![make_step("a", &[], ""), make_step("b", &["a"], "")],
            "hash".into(),
            1,
            "wf".into(),
        );
        let new_steps = vec![make_step("a", &[], "different"), make_step("b", &["a"], "ok")];
        assert!(validate_resume_compatibility(&state, &new_steps).is_ok());
    }

    #[test]
    fn resume_compat_different_step_count_err() {
        let state = WorkflowState::new(
            None,
            vec![make_step("a", &[], "")],
            "hash".into(),
            1,
            "wf".into(),
        );
        let new_steps = vec![make_step("a", &[], ""), make_step("b", &[], "")];
        assert!(validate_resume_compatibility(&state, &new_steps).is_err());
    }

    #[test]
    fn resume_compat_different_name_err() {
        let state = WorkflowState::new(
            None,
            vec![make_step("original", &[], "")],
            "hash".into(),
            1,
            "wf".into(),
        );
        let new_steps = vec![make_step("renamed", &[], "")];
        assert!(validate_resume_compatibility(&state, &new_steps).is_err());
    }

    // ─── sha256_hex ────────────────────────────────────────────────────────────

    #[test]
    fn sha256_hex_is_deterministic() {
        assert_eq!(sha256_hex("hello"), sha256_hex("hello"));
    }

    #[test]
    fn sha256_hex_differs_on_different_input() {
        assert_ne!(sha256_hex("hello"), sha256_hex("world"));
    }

    #[test]
    fn sha256_hex_correct_length() {
        assert_eq!(sha256_hex("test").len(), 64);
    }
}
