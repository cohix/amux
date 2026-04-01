use anyhow::{bail, Result};

/// A single step in a multi-agent workflow.
#[derive(Debug, Clone)]
pub struct WorkflowStep {
    /// Unique name of this step (from `## Step: <name>` heading).
    pub name: String,
    /// Names of steps that must complete before this step can run.
    pub depends_on: Vec<String>,
    /// The raw prompt template string (may contain `{{...}}` variables).
    pub prompt_template: String,
}

/// Parse a workflow Markdown file into an optional title and ordered list of steps.
///
/// Format:
/// ```markdown
/// # Optional Title
///
/// ## Step: step-name
/// Depends-on: other-step
/// Prompt: The prompt template text, which may span multiple lines.
/// ```
pub fn parse_workflow(content: &str) -> Result<(Option<String>, Vec<WorkflowStep>)> {
    let mut title: Option<String> = None;
    let mut steps: Vec<WorkflowStep> = Vec::new();

    let mut current_name: Option<String> = None;
    let mut current_depends: Vec<String> = Vec::new();
    let mut current_body = String::new();
    let mut in_prompt = false;

    for line in content.lines() {
        // Top-level title (only before any step headings).
        if line.starts_with("# ") && title.is_none() && current_name.is_none() {
            title = Some(line[2..].trim().to_string());
            continue;
        }

        // Step heading: flush previous step, start a new one.
        if line.starts_with("## Step:") {
            flush_step(
                &mut steps,
                &mut current_name,
                &mut current_depends,
                &mut current_body,
                &mut in_prompt,
            );
            let raw = line["## Step:".len()..].trim();
            current_name = Some(raw.to_string());
            continue;
        }

        // Skip other H2 headings (they end the current step).
        if line.starts_with("## ") && current_name.is_some() {
            flush_step(
                &mut steps,
                &mut current_name,
                &mut current_depends,
                &mut current_body,
                &mut in_prompt,
            );
            continue;
        }

        if current_name.is_some() {
            let trimmed = line.trim();

            // Depends-on field.
            if trimmed.starts_with("Depends-on:") && !in_prompt {
                let deps_str = trimmed["Depends-on:".len()..].trim();
                current_depends = deps_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                continue;
            }

            // Prompt: field — everything after this is the prompt body.
            if (trimmed == "Prompt:" || trimmed.starts_with("Prompt: ")) && !in_prompt {
                in_prompt = true;
                let rest = trimmed["Prompt:".len()..].trim();
                if !rest.is_empty() {
                    current_body.push_str(rest);
                    current_body.push('\n');
                }
                continue;
            }

            if in_prompt {
                current_body.push_str(line);
                current_body.push('\n');
            }
        }
    }

    // Flush the final step.
    flush_step(
        &mut steps,
        &mut current_name,
        &mut current_depends,
        &mut current_body,
        &mut in_prompt,
    );

    if steps.is_empty() {
        bail!(
            "Workflow file contains no steps. \
             Define steps with '## Step: <name>' headings."
        );
    }

    Ok((title, steps))
}

fn flush_step(
    steps: &mut Vec<WorkflowStep>,
    current_name: &mut Option<String>,
    current_depends: &mut Vec<String>,
    current_body: &mut String,
    in_prompt: &mut bool,
) {
    if let Some(name) = current_name.take() {
        steps.push(WorkflowStep {
            name,
            depends_on: std::mem::take(current_depends),
            prompt_template: current_body.trim().to_string(),
        });
        current_body.clear();
        *in_prompt = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_step_no_deps() {
        let md = "## Step: plan\nPrompt: Do the thing.\n";
        let (title, steps) = parse_workflow(md).unwrap();
        assert!(title.is_none());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "plan");
        assert!(steps[0].depends_on.is_empty());
        assert_eq!(steps[0].prompt_template, "Do the thing.");
    }

    #[test]
    fn parse_title_and_steps() {
        let md = "# My Workflow\n\n## Step: plan\nPrompt: Plan it.\n\n## Step: implement\nDepends-on: plan\nPrompt: Implement it.\n";
        let (title, steps) = parse_workflow(md).unwrap();
        assert_eq!(title.as_deref(), Some("My Workflow"));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].depends_on, vec!["plan"]);
    }

    #[test]
    fn parse_missing_depends_on_gives_empty_vec() {
        let md = "## Step: solo\nPrompt: Do stuff.\n";
        let (_title, steps) = parse_workflow(md).unwrap();
        assert!(steps[0].depends_on.is_empty());
    }

    #[test]
    fn parse_multiple_depends_on() {
        let md = "## Step: merge\nDepends-on: plan, implement\nPrompt: Merge.\n";
        let (_title, steps) = parse_workflow(md).unwrap();
        assert_eq!(steps[0].depends_on, vec!["plan", "implement"]);
    }

    #[test]
    fn parse_empty_file_returns_error() {
        let result = parse_workflow("# Title only\n");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("no steps"));
    }

    #[test]
    fn parse_multiline_prompt() {
        let md = "## Step: plan\nPrompt: Line one.\nLine two.\nLine three.\n";
        let (_title, steps) = parse_workflow(md).unwrap();
        let prompt = &steps[0].prompt_template;
        assert!(prompt.contains("Line one."));
        assert!(prompt.contains("Line two."));
        assert!(prompt.contains("Line three."));
    }

    #[test]
    fn parse_prompt_with_inline_content() {
        let md = "## Step: plan\nPrompt: Do this: {{work_item_content}}\n";
        let (_title, steps) = parse_workflow(md).unwrap();
        assert!(steps[0].prompt_template.contains("{{work_item_content}}"));
    }

    #[test]
    fn parse_multiple_steps_preserves_order() {
        let md = "## Step: a\nPrompt: A.\n\n## Step: b\nDepends-on: a\nPrompt: B.\n\n## Step: c\nDepends-on: b\nPrompt: C.\n";
        let (_title, steps) = parse_workflow(md).unwrap();
        assert_eq!(steps[0].name, "a");
        assert_eq!(steps[1].name, "b");
        assert_eq!(steps[2].name, "c");
    }

    #[test]
    fn parse_step_with_no_prompt_section() {
        // A step without a "Prompt:" line should have an empty template.
        let md = "## Step: plan\nDepends-on: nothing\n";
        // This is valid — prompt_template will be empty.
        let result = parse_workflow(md);
        // parse_workflow only fails on empty file, not empty prompt
        assert!(result.is_ok());
        let (_t, steps) = result.unwrap();
        assert_eq!(steps[0].prompt_template, "");
    }
}
