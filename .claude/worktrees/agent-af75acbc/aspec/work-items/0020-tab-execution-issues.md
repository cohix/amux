# Work Item: Bug

Title: tab execution issues
Issue: issuelink

## Summary:
- all commands need to execute in a tab's proper working directory.


## Implementation Details:
- for each tab within the TUI, all subcommands must execute within the tab's working directory.
- currently, commands like `new`, `init`, `init --aspec` etc. do not execute within the current tab's working directory, they run in the directory where amux was launched. This is incorrect.
- determine an over-arching design-level fix to ensure this never happens again for current or future subcommands within the TUI.


## Edge Case Considerations:
- cover every single subcommand and all of their flags for tab-working-directory correctness

## Test Considerations:
- test to ensure no subcommands are directly tied to the execution directory, they should all execute in the tab's working directory
- ensure that CLI commands still execute wherever amux was launched. Put another way, when using CLI, working dir == current dir. When using TUI, working dir == tab's working dir.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- ensure modularity between CLI and TUI. avoid workaround hacks, find an overall pattern to ensure subcommands execute properly.
