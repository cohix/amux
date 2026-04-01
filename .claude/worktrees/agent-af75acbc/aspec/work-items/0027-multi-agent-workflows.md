# Work Item: Feature

Title: Multi-agent workflows
Issue: issuelink

## Summary:
- Allow users to define reusable multi-agent workflows as Markdown files that amux can coordinate when implementing a work item. amux parses the workflow into a DAG of steps, injects per-step prompts mixed with work-item context into each agent container invocation, pauses for user confirmation between steps, and renders the workflow graph live in the TUI so the user can see step statuses at a glance.

## User Stories

### User Story 1:
As a: user

I want to:
define a multi-step workflow in a Markdown file and pass it to `amux implement` so that different agent runs each handle a focused phase of the work (e.g. plan → implement → test → review)

So I can:
break complex work items into discrete, auditable stages without having to manually re-prompt the agent at each phase

### User Story 2:
As a: user

I want to:
see a visual representation of the workflow DAG inside the TUI, with each step coloured by its status (pending, running, done, errored)

So I can:
immediately understand where the workflow is, which steps are blocked, and whether any step has failed — without leaving the TUI

### User Story 3:
As a: user

I want to:
be prompted for explicit confirmation before amux advances from one completed workflow step to the next

So I can:
review the agent's output at each stage, decide whether to continue or abort, and maintain control over the overall implementation process


## Implementation Details:

### Workflow file format
- A workflow is a plain Markdown file the user writes and owns
- Top-level `# Title` is optional and used only for display
- Steps are defined as level-2 headings: `## Step: <name>`
- Each step block supports the following fields (parsed from the body text below the heading):
  - `Depends-on: <step-name>[, <step-name>...]` — zero or more upstream steps; omit for root steps
  - `Prompt:` — everything following this keyword (until the next heading) is the step's prompt template
- Prompt templates may include substitution variables:
  - `{{work_item_content}}` amux replaces it with the full text of the target work item file at runtime
  - `{{work_item_number}}` amux replaces it with the work item number (like 0026)
  - `{{work_item_section:[name]}}` amux replaces it with the contents of the section from the work item markdown with the provided header name (i.e. everything from directly after the header to the beginning of the next header in the markdown file), e.g. `{{work_item_section:[Implementation details]}} would find the `Implementation details` header in the work item markdown and pull out all of the text between that header and the next one (H1s and H2s only).
- Create an example workflow markdown file based on the example below for testing in `./aspec/workflows/implement-feature.md`
- Example workflow:
  ```markdown
  # Implement Feature Workflow

  ## Step: plan
  Prompt: Read the following work item and produce a detailed implementation plan. Do not write any code yet. Write the plan to `./aspec/work-items/plans/{{work_item_number}}-short-description.md.

  {{work_item_content}}

  ## Step: implement
  Depends-on: plan
  Prompt: Implement the work item {{work_item_number}} according to the plan produced in the previous step. Iterate until the work item is comprehensively implemented, the build succeeds, and all tests pass.

  Follow the plan provided and compare against the work item implementation spec: {{work_item_section:[Implementation details]}}

  ## Step: review
  Depends-on: implement
  Prompt: Review the changes made in the implement step for correctness, security, and style. Suggest improvements if needed. Ensure all edge cases are considered:

  {{work_item_section:[Edge case considerations]}}
  
  Ensure tests are implemented as described below:

  {{work_item_section:[Test considerations]}}

  ## Step: docs
  Depends-on: implement
  Prompt: Write comprehensive documentation for work item {{work_item_number}}, following the plan that was previously written and following guidelines from the project aspec.
  ```

### DAG construction (`src/workflow/mod.rs` — new module)
- New `src/workflow/` module with `mod.rs`, `parser.rs`, `dag.rs`
- `parser.rs`: parse a workflow `.md` file into a `Vec<WorkflowStep>` (name, depends_on, prompt_template)
- `dag.rs`: build an adjacency list DAG from the parsed steps; return an error if cycles are detected (DFS cycle check); expose a `ready_steps(completed: &HashSet<String>) -> Vec<String>` helper that returns steps whose dependencies are all in `completed`
- `WorkflowState` struct: `steps: IndexMap<String, StepStatus>` where `StepStatus` is `Pending | Running | Done | Error(String)`
- `WorkflowState::next_ready(&self) -> Vec<String>` — delegates to `dag.ready_steps`

### CLI changes (`src/cli.rs`)
- Add `--workflow` optional argument to the `Implement` subcommand:
  ```
  /// Path to a workflow Markdown file. If omitted, the work item is implemented in a single agent run with the current prompt, unchanged.
  #[arg(long)]
  workflow: Option<PathBuf>,
  ```

### Command mode (`src/commands/implement.rs`)
- `run()`: when `workflow` is `Some`, call `run_workflow()` instead of the single-step `run_agent_with_sink`
- `run_workflow()`:
  1. Parse the workflow file; build `WorkflowState`
  2. Loop: find ready steps via `WorkflowState::next_ready()`
  3. For each ready step (sequentially for now): build the combined prompt (step template + work item content substituted), set step to `Running`, call `run_agent_with_sink`
  4. On agent exit with success: set step to `Done`; print step summary; prompt user (`[Enter] to advance, [q] to abort`)
  5. On agent exit with error: set step to `Error`; print error; prompt user to retry or abort
  6. Repeat until all steps are `Done` or user aborts

### TUI integration (`src/tui/`)
- Extend `TabState` in `src/tui/state.rs` with `workflow: Option<WorkflowState>`
- The `implement` TUI flow (launched from `TuiTabAction::Implement`) must accept an optional `workflow_path: Option<PathBuf>` and initialise `tab.workflow` when provided, then launch the first step in an agent container using existing methods (FOREGROUND, PTY, PROMPTED)
- Step advancement in TUI mode: after an agent run finishes (tab enters `ExecutionPhase::Done`), if a workflow is active, show a new `Dialog::WorkflowStepConfirm { step_name: String, next_steps: Vec<String> }` that asks the user to confirm advancing; on confirmation, launch the next step's agent container
- `src/tui/render.rs`: when `tab.workflow` is `Some`, render a workflow status strip below the execution window, above the command text box (but inside the tab's outer border). Each step is rendered as a short, wide rounded rectangle with the step name within the rectangle; edges between dependent steps are shown as arrows. The width of the step rectangles should depend on how many steps there are (but don't allow them to get too small to read their step titles).
- If multiple steps could run in parallel, (i.e. both depends-on the same step), render them above one another in the workflow status strip (based on the order they appear in the original markdown). Do not yet allow for parallel step execution, execute the steps one at a time, based on the order they appear in original markdown. Each step in a "parallel group" should be rendered slightly indended from the one above to visually indicate that they will be executed sequentially.
- If there are more than two "parallel steps" in one group, the 3rd box (at the bottom, indented two steps) should only show "+ 2 more..." to indicate there are more steps in the "parallel group". As steps in a "parallel group" complete, they get "collapsed" into the topmost box in the "group" (i.e. a green box showing "plan, implement" as its label to indicate the first two steps in the group are complete, with in-progress and incomplete steps shown below, indented).
- Step status colours:
  - Pending: grey / dim
  - Running: blue / bold
  - Stuck: yellow / bold
  - Done: green / dim
  - Error: red / bold
- The strip height should be capped (e.g. 3 step boxes high, 3 rows per box) and scroll horizontally if the DAG is wide; a simple left-to-right topological layout is sufficient. If there are not any parallel step groups, the status strip should only be one box high. Grow in height only if needed for "parallel groups".

### Workflow state persistence
- `WorkflowState` should be serialisable to JSON so that a partially-completed workflow can be resumed if amux restarts (save to `$GITROOT/.amux/workflows/<repo-hash>-<work-item>-<workflow-name>.json`)
- On `implement --workflow=...`, check for an existing state file and offer to resume or restart.
  - Always prompt the user if they wish to resume a previous workflow run or if they want to start fresh. Delete the workflowstate file and create a fresh one if they choose to restart, do not edit the existing file.
  - in the json file, store a sha256 hash of the original workflow markdown file. 
    - If the workflow file being loaded has a different hash, notify the user that the workflow has changed from its previous run, and therefore the workflow should be restarted from the beginning. Include a "restart from beginning" and "continue anyways (could be dangerous)" option. 
    - If the user chooses to continue anyways, attempt to re-run the workflow despite the changed workflow file, but first check to ensure the step titles and depends-on are all the same as the saved workflow state. If they do not all match, notify the user that resuming with a changed workflow file is not possible and it will need to be re-started, allow them to accept or cancel.
  - if resuming a workflow and a step was recorded in workflowState as 'running', show the user a dialog indicating which step was running previously, and ask them if they would like to start that step over, or proceed to the next step. Resume the workflow from their desired place.
  - in the JSON file, include the container ID which was used to implement each step in the workflow. If a step gets retried, over-write with the latest container ID.


## Edge Case Considerations:
- **Cycle detection**: if the workflow DAG contains a cycle, return a clear error before any agent runs
- **Missing step references**: if a `Depends-on` names a step that does not exist in the file, return an error at parse time
- **Empty workflow**: a workflow file with zero steps should be rejected with a helpful message
- **Unknown substitution variables**: warn but do not fail if `{work_item_content}` is absent from a step's prompt template (the step's prompt is used verbatim)
- **Work item not found**: propagate the existing `find_work_item` error before reading the workflow
- **Workflow file not found / unreadable**: return a clear `anyhow` error with the path and helpful error
- **Agent failure mid-workflow**: do not automatically advance; surface the error status in both TUI and command mode and prompt the user for retry or abort
- **Parallel-ready steps**: v1 executes ready steps sequentially in topological order; document this limitation. True parallel execution (multiple containers) is out of scope for this work item. Ensure they are rendered as "grouped but sequential" using the stacked-but-indented method described above in the TUI workflow state strip.
- **Very large workflows**: cap the TUI strip render at a reasonable width; truncate step names longer than 12 chars with `…` if needed. Allow for a "+ 3 more steps..." box at the bottom of a many-step "parallel group" OR at the far right of a many-step workflow. Do not render more than 3 step-boxes in height.
- **Resume prompt on stale state file**: if the workflow definition has changed since the state was saved (hash, step names, or depends-on differ), allow the user to choose what to do, as described above.


## Test Considerations:
- **Unit — parser**: test parsing of a valid multi-step workflow; test that missing `Depends-on` produces an empty dependency list; test that a malformed file returns an error
- **Unit — DAG**: test cycle detection with a simple A→B→A cycle; test `ready_steps` returns only steps whose dependencies are met; test topological ordering
- **Unit — prompt substitution**: test that `{work_item_content}` is replaced with the work item text; test that a prompt with no placeholder is returned unchanged
- **Unit — WorkflowState transitions**: test `Pending → Running → Done` happy path; test `Running → Error` on non-zero exit
- **Unit — CLI flag parsing**: test that `--workflow path/to/wf.md` is parsed into `Some(PathBuf)` on `Implement`; test that omitting the flag yields `None`
- **Integration — command mode**: create a temp Git repo with a work item and a two-step workflow file; run `implement --workflow=...` with a mock agent that exits 0; assert both steps are marked `Done`
- **Integration — resume**: write a partial `WorkflowState` JSON file and verify amux offers to resume; verify starting fresh discards the old state
- **End-to-end — TUI**: verify the workflow strip renders without panicking when `WorkflowState` is populated (render snapshot test using `ratatui::backend::TestBackend`)


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec
- New `src/workflow/` module must be declared in `src/lib.rs` and kept small/modular, consistent with existing modules (`src/commands/`, `src/docker/`, etc.)
- Reuse `find_work_item` from `src/commands/implement.rs` to resolve the work item path
- Reuse `run_agent_with_sink` from `src/commands/agent.rs` for each workflow step's container run — do not duplicate container-launch logic. Workflow step containers run FOREGROUND, ATTACHED, PROMPTED.
- Follow the `OutputSink` pattern for directing per-step output to the TUI PTY or stdout.
- New `Dialog::WorkflowStepConfirm` variant follows the existing `Dialog` enum pattern in `src/tui/state.rs`; input handling belongs in `src/tui/input.rs`, rendering in `src/tui/render.rs`
- Workflow state serialisation should use `serde_json`, consistent with `src/config/mod.rs` (which already uses `serde` + `serde_json`)
- All new public functions must have unit tests in the same file following the `#[cfg(test)] mod tests` pattern used throughout the codebase
- Document workflow files, parsing, workflow state, etc. in their own "using workflows" file seperate from getting started and usage. Add command/flag definitions to usage doc, and reference new workflows doc file in getting started, but contain a detailed guide-style doc for workflows in its own new file.
