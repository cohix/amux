//! `SquadCommandFrontend` impl for the TUI — the task-creation interview
//! (BLOCKER-3, §9.3) and the persistent-directory delete confirmation
//! (BLOCKER-2, §9.2), both driven through `ask_dialog`, exactly as
//! `NewCommandFrontend` collects a workflow.
//!
//! These COLLECT input only. No validation or scheduling decision is made
//! here; the answers flow to Layer 2, which builds the `CreateTask` and
//! reaches `LocalTaskGateway::validate_create` for every rejection.

use std::path::{Path, PathBuf};

use crate::command::commands::squad::commands::{SquadCommandFrontend, TaskWorkspaceChoice};
use crate::command::error::CommandError;
use crate::data::fs::task_store::MountScope;
use crate::frontend::tui::command_frontend::TuiCommandFrontend;
use crate::frontend::tui::dialogs::{DialogRequest, DialogResponse};

/// The task-description modal's title. Deliberately short — it is drawn into
/// the dialog rect's upper border, where a long sentence overflows or clips.
/// The full instruction lives in the dialog body (`ask_task_description`'s
/// prompt), which wraps and is always readable.
pub const TASK_DESCRIPTION_TITLE: &str = "New squad task description";

impl TuiCommandFrontend {
    /// One optional-field edit prompt (WI 0110): prefilled with the current
    /// value, cleared box means "fall back to the squad default".
    ///
    /// The TUI can express `Option<Option<_>>` without a sentinel token the way
    /// the CLI needs one: the box arrives holding the current value, so
    /// *emptying* it is an unambiguous "clear this", and leaving it alone is
    /// "keep this".
    fn ask_edited_optional(
        &mut self,
        title: &str,
        noun: &str,
        current: Option<&str>,
    ) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: title.to_string(),
            prompt: format!("Leader {noun} (clear the box to use the squad default):"),
            default_text: current.map(str::to_string),
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }
}

impl SquadCommandFrontend for TuiCommandFrontend {
    /// The TUI runs in the user's own terminal, so the process's current
    /// directory is theirs and the mount-scope question can be put to them.
    fn is_local_user_session(&self) -> bool {
        true
    }

    /// The same dismissable notice the squad tab raises on a first start,
    /// with the `[c]`/`[z]` copy actions the key snippet needs. Sent, not
    /// asked: a notice has no answer, so blocking the command thread on it
    /// would only stall the command that minted the key.
    fn show_key_setup(
        &mut self,
        setup: &crate::command::commands::squad::supervisor::SquadKeySetup,
    ) {
        let _ = self.dialog_tx.send(DialogRequest::KeySetupNotice {
            title: "squad authentication".to_string(),
            body: setup.body.clone(),
            copy_key: setup.key.clone(),
            copy_zshrc_snippet: setup.zshrc_snippet.clone(),
        });
    }

    fn ask_task_name(&mut self) -> Result<String, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Task name".into(),
            prompt: "Enter the task slug:".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            _ => Err(CommandError::Aborted),
        }
    }

    /// One freeform description covering both halves of a task — when it fires
    /// and what to do about it — through the same multiline editor
    /// `new spec --interview` uses. A one-line box could not hold either half.
    fn ask_task_description(&mut self) -> Result<String, CommandError> {
        let response = self.ask_dialog(DialogRequest::MultilineInput {
            title: TASK_DESCRIPTION_TITLE.into(),
            // The border title is a short label; the full instruction lives
            // here in the body, pre-wrapped (the multiline dialog renders its
            // prompt without wrapping).
            prompt: "Describe the new squad task including its triggering conditions\n\
                     and how squad should handle the task each time it is triggered.\n\
                     (Ctrl+Enter to submit)"
                .into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) => Ok(t),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_workspace_choice(&mut self) -> Result<TaskWorkspaceChoice, CommandError> {
        let response = self.ask_dialog(DialogRequest::KindSelect {
            title: "Task Workspace".into(),
            options: vec![
                ("1".into(), "Default Task Workspace".into()),
                ("2".into(), "Custom Folder / Repo".into()),
            ],
        })?;
        match response {
            DialogResponse::Char('2') | DialogResponse::Index(1) => {
                Ok(TaskWorkspaceChoice::CustomFolderOrRepo)
            }
            DialogResponse::Char('1') | DialogResponse::Index(0) => {
                Ok(TaskWorkspaceChoice::DefaultTaskWorkspace)
            }
            _ => Err(CommandError::Aborted),
        }
    }

    fn confirm_non_git_workspace(&mut self, path: &Path) -> Result<bool, CommandError> {
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Not a Git repository".into(),
            body: format!(
                "{} is not the root of a Git repository.\n\n\
                 Keep this path? (No = choose a different one)",
                path.display()
            ),
        })?;
        match response {
            DialogResponse::Yes => Ok(true),
            DialogResponse::No => Ok(false),
            // Dismissing is not "No": "No" asks for a different path, while
            // Esc abandons the interview outright (WI 0106's interrupted-
            // interview rule). Nothing may be persisted after it.
            _ => Err(CommandError::Aborted),
        }
    }

    fn confirm_parent_directory_workspace(
        &mut self,
        path: &Path,
        current_dir: &Path,
    ) -> Result<bool, CommandError> {
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Mount a parent directory?".into(),
            body: format!(
                "{} is a parent of {}.\n\n\
                 Mount it anyway? (No = choose a different one)",
                path.display(),
                current_dir.display()
            ),
        })?;
        match response {
            DialogResponse::Yes => Ok(true),
            DialogResponse::No => Ok(false),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_overlay(&mut self, existing: &[String]) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: format!("Overlays ({} added)", existing.len()),
            prompt: "Add an overlay? [dir()/ssh()/env()/skill() syntax, blank to finish]:".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            // A *blank submission* means "no more overlays" and ends the loop.
            // A dismissal does not: it abandons the interview, and nothing may
            // be persisted after it.
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_interval(&mut self) -> Result<String, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Evaluation interval".into(),
            prompt: "How often to evaluate (e.g. 6h, 1d):".into(),
            default_text: Some("6h".into()),
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            // Submitting an empty box takes the documented default; dismissing
            // the dialog abandons the interview.
            DialogResponse::Text(_) => Ok("6h".to_string()),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_repo(&mut self) -> Result<PathBuf, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Custom Folder / Repo".into(),
            prompt: "Folder or repository to bind this task to (Enter for current dir):".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(PathBuf::from(t.trim())),
            DialogResponse::Text(_) => std::env::current_dir().map_err(|error| {
                CommandError::Other(format!("cannot resolve current dir: {error}"))
            }),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_agent(&mut self) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Leader agent".into(),
            prompt: "Leader agent (optional, Enter to skip):".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_model(&mut self) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Leader model".into(),
            prompt: "Leader model (optional, Enter to skip):".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_task_mount_scope(&mut self) -> Result<MountScope, CommandError> {
        // "Yes" mounts the whole git root; "No" mounts the current directory
        // only. The default (git root) is the safer, more useful scope.
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Mount scope".into(),
            body: "Mount the entire git root? (No = current directory only)".into(),
        })?;
        match response {
            DialogResponse::No => Ok(MountScope::Cwd),
            DialogResponse::Yes => Ok(MountScope::GitRoot),
            _ => Err(CommandError::Aborted),
        }
    }

    // ── Task agent pool (WI 0110) ──────────────────────────────────────

    fn ask_use_global_squad_config(&mut self) -> Result<bool, CommandError> {
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Agents and models".into(),
            body: "A global squad configuration exists.\n\n\
                   Use those settings for this task? \
                   (No = give this task its own agents and models)"
                .into(),
        })?;
        match response {
            DialogResponse::Yes => Ok(true),
            DialogResponse::No => Ok(false),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_agent_model(
        &mut self,
        agent: &str,
        existing: &[String],
    ) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: format!("Models for {agent} ({} added)", existing.len()),
            prompt: format!("Add a model {agent} may use (blank to finish):"),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            // A blank submission ends the loop; a dismissal abandons the
            // interview, exactly as in the overlay step.
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_additional_agent(
        &mut self,
        existing: &[String],
    ) -> Result<Option<String>, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: format!("Available agents ({} added)", existing.len()),
            prompt: "Add another agent this task may use (blank to finish):".into(),
            default_text: None,
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            DialogResponse::Text(_) => Ok(None),
            _ => Err(CommandError::Aborted),
        }
    }

    // ── Task edit (WI 0110) ────────────────────────────────────────────

    fn ask_edited_description(&mut self, current: &str) -> Result<String, CommandError> {
        let response = self.ask_dialog(DialogRequest::MultilineInput {
            title: "Edit squad task description".into(),
            prompt: "Describe when this task fires and what squad should do.\n\
                     (Ctrl+Enter to submit)"
                .into(),
            default_text: Some(current.to_string()),
        })?;
        match response {
            DialogResponse::Text(t) => Ok(t),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_edited_interval(&mut self, current: &str) -> Result<String, CommandError> {
        let response = self.ask_dialog(DialogRequest::TextInput {
            title: "Evaluation interval".into(),
            prompt: "How often to evaluate (e.g. 6h, 1d):".into(),
            default_text: Some(current.to_string()),
        })?;
        match response {
            DialogResponse::Text(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            // A cleared box keeps the current value rather than meaning zero.
            DialogResponse::Text(_) => Ok(current.to_string()),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_edited_agent(&mut self, current: Option<&str>) -> Result<Option<String>, CommandError> {
        self.ask_edited_optional("Leader agent", "agent", current)
    }

    fn ask_edited_model(&mut self, current: Option<&str>) -> Result<Option<String>, CommandError> {
        self.ask_edited_optional("Leader model", "model", current)
    }

    fn ask_replace_overlays(&mut self, current: &[String]) -> Result<bool, CommandError> {
        let shown = if current.is_empty() {
            "(none)".to_string()
        } else {
            current.join(", ")
        };
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Overlays".into(),
            body: format!("Current overlays: {shown}\n\nReplace them? (No = leave them alone)"),
        })?;
        match response {
            DialogResponse::Yes => Ok(true),
            DialogResponse::No => Ok(false),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_replace_agent_pool(
        &mut self,
        current: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Result<bool, CommandError> {
        let shown = if current.is_empty() {
            "(inherits the global squad settings)".to_string()
        } else {
            current
                .iter()
                .map(|(agent, models)| format!("{agent} = {}", models.join(", ")))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: "Agents and models".into(),
            body: format!("Current agents:\n{shown}\n\nReplace them? (No = leave them alone)"),
        })?;
        match response {
            DialogResponse::Yes => Ok(true),
            DialogResponse::No => Ok(false),
            _ => Err(CommandError::Aborted),
        }
    }

    fn ask_delete_task_dir(&mut self, name: &str, path: &Path) -> Result<bool, CommandError> {
        let response = self.ask_dialog(DialogRequest::YesNo {
            title: format!("Delete {name} directory?"),
            body: format!(
                "Also delete the persistent task directory {}?",
                path.display()
            ),
        })?;
        Ok(matches!(response, DialogResponse::Yes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::per_command::mount_scope::tests::make_frontend;

    /// Answer the next dialog with `response`, on a helper thread, so the
    /// blocking `ask_dialog` call under test can complete.
    fn answer_with(
        req_rx: std::sync::mpsc::Receiver<DialogRequest>,
        resp_tx: std::sync::mpsc::Sender<DialogResponse>,
        response: DialogResponse,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let _req = req_rx.recv().unwrap();
            resp_tx.send(response).unwrap();
        })
    }

    /// Dismissing any interview dialog abandons task creation. Nothing may be
    /// persisted after an interrupted interview (WI 0106's edge case), so no
    /// step is allowed to quietly substitute a default and let the remaining
    /// prompts carry on to `gateway.create`.
    #[test]
    fn dismissing_any_interview_step_aborts_instead_of_taking_a_default() {
        macro_rules! assert_dismissal_aborts {
            ($label:expr, $call:expr) => {{
                let (mut frontend, req_rx, resp_tx) = make_frontend();
                let handle = answer_with(req_rx, resp_tx, DialogResponse::Dismissed);
                #[allow(clippy::redundant_closure_call)]
                let result = ($call)(&mut frontend);
                handle.join().unwrap();
                assert!(
                    matches!(result, Err(CommandError::Aborted)),
                    "{} must abort when its dialog is dismissed",
                    $label
                );
            }};
        }

        assert_dismissal_aborts!("the name step", |f: &mut TuiCommandFrontend| f
            .ask_task_name()
            .map(|_| ()));
        assert_dismissal_aborts!("the description step", |f: &mut TuiCommandFrontend| f
            .ask_task_description()
            .map(|_| ()));
        assert_dismissal_aborts!("the interval step", |f: &mut TuiCommandFrontend| f
            .ask_task_interval()
            .map(|_| ()));
        assert_dismissal_aborts!("the workspace-choice step", |f: &mut TuiCommandFrontend| f
            .ask_task_workspace_choice()
            .map(|_| ()));
        assert_dismissal_aborts!("the custom-path step", |f: &mut TuiCommandFrontend| f
            .ask_task_repo()
            .map(|_| ()));
        assert_dismissal_aborts!(
            "the not-a-repository warning",
            |f: &mut TuiCommandFrontend| f
                .confirm_non_git_workspace(std::path::Path::new("/tmp"))
                .map(|_| ())
        );
        assert_dismissal_aborts!(
            "the parent-directory warning",
            |f: &mut TuiCommandFrontend| f
                .confirm_parent_directory_workspace(
                    std::path::Path::new("/tmp"),
                    std::path::Path::new("/tmp/sub")
                )
                .map(|_| ())
        );
        assert_dismissal_aborts!("the overlay step", |f: &mut TuiCommandFrontend| f
            .ask_task_overlay(&[])
            .map(|_| ()));
        assert_dismissal_aborts!("the agent step", |f: &mut TuiCommandFrontend| f
            .ask_task_agent()
            .map(|_| ()));
        assert_dismissal_aborts!("the model step", |f: &mut TuiCommandFrontend| f
            .ask_task_model()
            .map(|_| ()));
        assert_dismissal_aborts!("the mount-scope step", |f: &mut TuiCommandFrontend| f
            .ask_task_mount_scope()
            .map(|_| ()));
    }

    /// A *blank submission* is still a real answer: it keeps the documented
    /// default for optional steps and ends the overlay loop. Only dismissal
    /// aborts.
    #[test]
    fn a_blank_submission_still_means_the_documented_default() {
        let (mut frontend, req_rx, resp_tx) = make_frontend();
        let handle = answer_with(req_rx, resp_tx, DialogResponse::Text(String::new()));
        assert_eq!(frontend.ask_task_interval().unwrap(), "6h");
        handle.join().unwrap();

        let (mut frontend, req_rx, resp_tx) = make_frontend();
        let handle = answer_with(req_rx, resp_tx, DialogResponse::Text("  ".into()));
        assert_eq!(frontend.ask_task_overlay(&[]).unwrap(), None);
        handle.join().unwrap();

        let (mut frontend, req_rx, resp_tx) = make_frontend();
        let handle = answer_with(req_rx, resp_tx, DialogResponse::Text(String::new()));
        assert_eq!(frontend.ask_task_agent().unwrap(), None);
        handle.join().unwrap();
    }
}
