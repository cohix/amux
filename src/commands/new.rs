use crate::commands::init::find_git_root;
use crate::commands::output::OutputSink;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Command-mode entry point: runs `new` interactively via stdin/stdout.
pub async fn run() -> Result<()> {
    run_with_sink(&OutputSink::Stdout, None, None).await
}

/// Core logic shared between command mode and TUI mode.
///
/// When `kind` or `title` are `None`, the user is prompted interactively
/// (stdin for command mode, or pre-supplied for TUI mode).
pub async fn run_with_sink(
    out: &OutputSink,
    kind: Option<WorkItemKind>,
    title: Option<String>,
) -> Result<()> {
    let git_root = find_git_root().context("Not inside a Git repository")?;

    // Locate the template.
    let template_path = find_template(&git_root)?;
    let template_content =
        std::fs::read_to_string(&template_path).context("Failed to read template file")?;

    // Determine the next sequential number.
    let work_items_dir = template_path
        .parent()
        .context("Template has no parent directory")?;
    let next_number = next_work_item_number(work_items_dir)?;

    // Get work item kind.
    let kind = match kind {
        Some(k) => k,
        None => prompt_kind(out)?,
    };

    // Get work item title.
    let title = match title {
        Some(t) => t,
        None => prompt_title(out)?,
    };

    // Build the filename.
    let slug = slugify(&title);
    let filename = format!("{:04}-{}.md", next_number, slug);
    let file_path = work_items_dir.join(&filename);

    // Build the file content from the template.
    let content = apply_template(&template_content, &kind, &title);

    std::fs::write(&file_path, &content)
        .with_context(|| format!("Failed to write {}", file_path.display()))?;

    out.println(format!("Created work item: {}", file_path.display()));

    // Try to open in VS Code if running inside the VS Code terminal.
    if is_vscode_terminal() {
        open_in_vscode(&file_path);
        out.println(format!("Opened {} in VS Code.", filename));
    }

    Ok(())
}

/// The three types of work items.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkItemKind {
    Feature,
    Bug,
    Task,
}

impl WorkItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkItemKind::Feature => "Feature",
            WorkItemKind::Bug => "Bug",
            WorkItemKind::Task => "Task",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "feature" | "f" | "1" => Some(WorkItemKind::Feature),
            "bug" | "b" | "2" => Some(WorkItemKind::Bug),
            "task" | "t" | "3" => Some(WorkItemKind::Task),
            _ => None,
        }
    }
}

/// Find the work items template, searching the git root.
pub fn find_template(git_root: &Path) -> Result<PathBuf> {
    let path = git_root.join("aspec/work-items/0000-template.md");
    if path.exists() {
        return Ok(path);
    }
    bail!(
        "Template not found at {}. \
         Download it from https://github.com/cohix/aspec/raw/refs/heads/main/aspec/work-items/0000-template.md \
         and place it in your project's aspec/work-items/ directory.",
        path.display()
    )
}

/// Scan the work-items directory and determine the next sequential number.
pub fn next_work_item_number(work_items_dir: &Path) -> Result<u32> {
    let mut max_number: u32 = 0;

    if work_items_dir.exists() {
        for entry in std::fs::read_dir(work_items_dir)
            .with_context(|| format!("Failed to read {}", work_items_dir.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(num) = parse_work_item_number(&name_str) {
                if num > max_number {
                    max_number = num;
                }
            }
        }
    }

    Ok(max_number + 1)
}

/// Parse a work item number from a filename like "0002-some-feature.md".
pub fn parse_work_item_number(filename: &str) -> Option<u32> {
    let prefix = filename.split('-').next()?;
    prefix.parse::<u32>().ok()
}

/// Convert a user-provided title to a filename slug.
/// Lowercase, replace spaces with hyphens, remove non-alphanumeric/non-hyphen chars.
pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
}

/// Apply substitutions to the template content.
pub fn apply_template(template: &str, kind: &WorkItemKind, title: &str) -> String {
    let mut result = String::new();
    for line in template.lines() {
        if line.starts_with("# Work Item:") {
            result.push_str(&format!("# Work Item: {}", kind.as_str()));
        } else if line.starts_with("Title:") {
            result.push_str(&format!("Title: {}", title));
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
}

/// Check if we're running inside a VS Code integrated terminal.
pub fn is_vscode_terminal() -> bool {
    std::env::var("TERM_PROGRAM").map_or(false, |v| v == "vscode")
}

/// Open a file in VS Code using the `code` CLI.
pub fn open_in_vscode(path: &Path) {
    let _ = std::process::Command::new("code")
        .arg("--reuse-window")
        .arg(path)
        .spawn();
}

/// Prompt the user to select a work item kind (command mode via stdin).
fn prompt_kind(out: &OutputSink) -> Result<WorkItemKind> {
    out.println("Select work item type:");
    out.println("  1) Feature");
    out.println("  2) Bug");
    out.println("  3) Task");
    out.print("Choice [1/2/3]: ");

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;

    WorkItemKind::from_str(&input).context("Invalid choice. Please enter 1, 2, or 3.")
}

/// Prompt the user to provide a title (command mode via stdin).
fn prompt_title(out: &OutputSink) -> Result<String> {
    out.print("Work item title: ");

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;

    let title = input.trim().to_string();
    if title.is_empty() {
        bail!("Title cannot be empty.");
    }
    Ok(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My New Feature"), "my-new-feature");
    }

    #[test]
    fn slugify_removes_special_chars() {
        assert_eq!(slugify("Fix: the bug!"), "fix-the-bug");
    }

    #[test]
    fn slugify_preserves_numbers() {
        assert_eq!(slugify("Add step 2 support"), "add-step-2-support");
    }

    #[test]
    fn slugify_empty_string() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_multiple_spaces() {
        assert_eq!(slugify("a  b   c"), "a--b---c");
    }

    #[test]
    fn parse_work_item_number_valid() {
        assert_eq!(parse_work_item_number("0001-some-feature.md"), Some(1));
        assert_eq!(parse_work_item_number("0042-fix-bug.md"), Some(42));
        assert_eq!(parse_work_item_number("0000-template.md"), Some(0));
    }

    #[test]
    fn parse_work_item_number_invalid() {
        assert_eq!(parse_work_item_number("readme.md"), None);
        assert_eq!(parse_work_item_number(""), None);
    }

    #[test]
    fn next_work_item_number_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let num = next_work_item_number(tmp.path()).unwrap();
        assert_eq!(num, 1);
    }

    #[test]
    fn next_work_item_number_with_existing() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("0000-template.md"), "").unwrap();
        std::fs::write(tmp.path().join("0001-first.md"), "").unwrap();
        std::fs::write(tmp.path().join("0003-third.md"), "").unwrap();
        let num = next_work_item_number(tmp.path()).unwrap();
        assert_eq!(num, 4);
    }

    #[test]
    fn next_work_item_number_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("does-not-exist");
        let num = next_work_item_number(&nonexistent).unwrap();
        assert_eq!(num, 1);
    }

    #[test]
    fn apply_template_replaces_header_and_title() {
        let template = "# Work Item: [Feature | Bug | Task]\n\nTitle: title\nIssue: issuelink\n";
        let result = apply_template(template, &WorkItemKind::Bug, "Fix login crash");
        assert!(result.contains("# Work Item: Bug"));
        assert!(result.contains("Title: Fix login crash"));
        assert!(!result.contains("[Feature | Bug | Task]"));
        assert!(!result.contains("Title: title\n"));
    }

    #[test]
    fn apply_template_preserves_other_content() {
        let template = "# Work Item: [Feature | Bug | Task]\n\nTitle: title\n\n## Summary:\n- summary\n";
        let result = apply_template(template, &WorkItemKind::Task, "My Task");
        assert!(result.contains("## Summary:"));
        assert!(result.contains("- summary"));
    }

    #[test]
    fn work_item_kind_from_str_variants() {
        assert_eq!(WorkItemKind::from_str("feature"), Some(WorkItemKind::Feature));
        assert_eq!(WorkItemKind::from_str("Feature"), Some(WorkItemKind::Feature));
        assert_eq!(WorkItemKind::from_str("f"), Some(WorkItemKind::Feature));
        assert_eq!(WorkItemKind::from_str("1"), Some(WorkItemKind::Feature));
        assert_eq!(WorkItemKind::from_str("bug"), Some(WorkItemKind::Bug));
        assert_eq!(WorkItemKind::from_str("Bug"), Some(WorkItemKind::Bug));
        assert_eq!(WorkItemKind::from_str("b"), Some(WorkItemKind::Bug));
        assert_eq!(WorkItemKind::from_str("2"), Some(WorkItemKind::Bug));
        assert_eq!(WorkItemKind::from_str("task"), Some(WorkItemKind::Task));
        assert_eq!(WorkItemKind::from_str("Task"), Some(WorkItemKind::Task));
        assert_eq!(WorkItemKind::from_str("t"), Some(WorkItemKind::Task));
        assert_eq!(WorkItemKind::from_str("3"), Some(WorkItemKind::Task));
        assert_eq!(WorkItemKind::from_str("invalid"), None);
    }

    #[test]
    fn work_item_kind_as_str() {
        assert_eq!(WorkItemKind::Feature.as_str(), "Feature");
        assert_eq!(WorkItemKind::Bug.as_str(), "Bug");
        assert_eq!(WorkItemKind::Task.as_str(), "Task");
    }

    #[test]
    fn find_template_in_git_root() {
        let tmp = TempDir::new().unwrap();
        let work_items = tmp.path().join("aspec/work-items");
        std::fs::create_dir_all(&work_items).unwrap();
        std::fs::write(work_items.join("0000-template.md"), "# template").unwrap();
        let result = find_template(tmp.path());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("0000-template.md"));
    }

    #[test]
    fn find_template_missing_returns_error() {
        let tmp = TempDir::new().unwrap();
        let result = find_template(tmp.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Template not found"));
        assert!(err.contains("https://github.com"));
    }

    #[tokio::test]
    async fn run_with_sink_creates_work_item_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Set up a git repo and template.
        std::fs::create_dir(root.join(".git")).unwrap();
        let work_items = root.join("aspec/work-items");
        std::fs::create_dir_all(&work_items).unwrap();
        std::fs::write(
            work_items.join("0000-template.md"),
            "# Work Item: [Feature | Bug | Task]\n\nTitle: title\nIssue: issuelink\n",
        )
        .unwrap();

        // Use channel sink.
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);

        // We need to temporarily change CWD for find_git_root.
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(root).unwrap();

        let result = run_with_sink(
            &sink,
            Some(WorkItemKind::Feature),
            Some("My New Feature".to_string()),
        )
        .await;

        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok(), "run_with_sink failed: {:?}", result.err());

        // Verify file was created.
        let created = work_items.join("0001-my-new-feature.md");
        assert!(created.exists(), "Work item file should exist");

        let content = std::fs::read_to_string(&created).unwrap();
        assert!(content.contains("# Work Item: Feature"));
        assert!(content.contains("Title: My New Feature"));

        // Verify output was sent.
        let messages: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            messages.iter().any(|m| m.contains("Created work item")),
            "Expected creation message, got: {:?}",
            messages
        );
    }

    #[test]
    fn is_vscode_terminal_when_not_set() {
        // In test environment, TERM_PROGRAM is unlikely to be "vscode".
        // We just verify the function doesn't panic.
        let _ = is_vscode_terminal();
    }
}
