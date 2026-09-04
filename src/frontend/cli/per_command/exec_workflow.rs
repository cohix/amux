//! `ExecWorkflowCommandFrontend` impl for the CLI.
//!
//! All supertraits (`UserMessageSink`, `AgentFrontend`, `WorkflowFrontend`,
//! `MountScopeFrontend`, `AgentSetupFrontend`, `AgentAuthFrontend`,
//! `WorktreeLifecycleFrontend`) are implemented elsewhere in
//! `src/frontend/cli/`; this file only carries the trait's own methods
//! (PTY gating, the summary, and the resume prompts).

use crate::command::commands::exec_workflow::{
    ExecWorkflowCommandFrontend, WorkflowResumeDecision, WorkflowResumePrompt, WorkflowSummary,
};
use crate::command::error::CommandError;
use crate::data::message::{MessageLevel, UserMessage, UserMessageSink};

use crate::frontend::cli::command_frontend::CliFrontend;

impl ExecWorkflowCommandFrontend for CliFrontend {
    fn set_pty_active(&mut self, active: bool) {
        self.messages.set_pty_active(active);
    }

    fn report_workflow_summary(&mut self, summary: &WorkflowSummary) {
        self.write_message(UserMessage {
            level: MessageLevel::Info,
            text: format!(
                "workflow summary — {}/{} steps OK ({} failed)",
                summary.steps_completed,
                summary.steps_completed + summary.steps_failed,
                summary.steps_failed
            ),
        });
    }

    fn ask_workflow_resume(
        &mut self,
        prompt: &WorkflowResumePrompt,
    ) -> Result<WorkflowResumeDecision, CommandError> {
        // Without a TTY, keep the old non-interactive default: preserve the
        // saved work and pick up where the previous run stopped, rather than
        // discarding it and re-running every step.
        if self.non_interactive {
            return Ok(prompt.resume_from_stop_point());
        }
        eprintln!("awman: {}", prompt.title);
        for line in prompt.body.lines() {
            eprintln!("  {line}");
        }
        for (i, label) in prompt.choice_labels().iter().enumerate() {
            eprintln!("  [{}] {label}", i + 1);
        }
        eprintln!("  [f] {}", prompt.fresh_label);

        let mut buf = String::new();
        if std::io::stdin().read_line(&mut buf).is_err() {
            return Ok(WorkflowResumeDecision::Fresh);
        }
        Ok(buf
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .and_then(|n| prompt.start_points.get(n - 1))
            .map(|p| WorkflowResumeDecision::ResumeFrom(p.name.clone()))
            .unwrap_or(WorkflowResumeDecision::Fresh))
    }

    fn notify_dynamic_workflow_resume_unavailable(
        &mut self,
        work_item: u32,
        reason: &str,
    ) -> Result<(), CommandError> {
        eprintln!(
            "awman: the worktree for work item {work_item:04} is still on disk, but the previous \
             dynamic workflow cannot be resumed: {reason}"
        );
        if self.non_interactive {
            return Ok(());
        }
        eprintln!("awman: press Enter to start a fresh dynamic workflow.");
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
        Ok(())
    }
}
