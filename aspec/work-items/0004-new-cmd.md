# Work Item: Feature

Title: New Command
Issue: issuelink

## Summary:
- the `new` subcommand creates a new work-item in the current Git root based on the `aspec/work-items/000-template.md`

## User Stories

### User Story 1:
As a: user

I want to:
Automatically create a work item using a template

So I can:
Avoid manually working with files


## Implementation Details:
- Running `aspec new` or `new` within the aspec TUI should find the `aspec/work-items/0000-template.md` (searching up to the git root if needed), and create a new work item file in the work-items directory.
- First, scan the work-items folder and parse the filenames of the existing files, determine the next sequential number.
- Then, prompt the user to select whether the work item is a feature, bug, or task.
- Then, prompt the user to provide a name for the work item.
- Finally, create the new file by copying the template file, and using the appropriate file name pattern (XXXX-title-of-item.md) where XXXX is the next sequential number, and title-of-item is derived from the "nice" title that the user provided (lowercase it, replace spaces with hyphens, remove all non-number and non-letter characters)
- Replace the line in the template that contains `# Work Item: [Feature | Bug | Task]` with `# Work Item: X` where X is the user's choice of Feature, Bug, or Task
- Replace the line in the template that contains `Title: title` with `Title: X` where X is the title the user provided.
- Detect if `aspec` is running within the builtin VSCode terminal, and if so, open the newly created file in the current editor window as a new tab.


## Edge Case Considerations:
- If no template is found in the current git root, offer to download it from https://github.com/cohix/aspec/raw/refs/heads/main/aspec/work-items/0000-template.md. If the user accepts, download the file from Github and perform the same steps as above.

## Test Considerations:
- considerations

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.