# Work Item: Feature

Title: stuck tab detection
Issue: issuelink

## Summary:
- add the ability to detect when a tab's container is "stuck" and alert the user

## User Stories

### User Story 1:
As a: user

I want to:
know when a tab within the TUI is "stuck"

So I can:
go and help fix it


## Implementation Details:
- for each open tab in the TUI that is running a container, monitor the output going into the VT100 terminal emulator
- if a tab's container output is completely inactive for over 60 seconds (i.e. the process running in the container has not created any output whatsoever in over 60s), turn the tab itself yellow.
- when the user switches to the yellow tab, or provides any input whatsoever into the tab or its container, turn the tab color back to what it was previously (blue, grey, green, purple).
- when the tab turns yellow, add a warning symbol or emoji into the subcommand title within the tab itself (i.e. `⚠️ implement 001`).


## Edge Case Considerations:
- ensure any interaction whatsoever with a yellow tab (switching to it, typing any command, providing any input to the container terminal) reverts the color back to its appropriate state. The color is a warning and should be cleared as soon as the user checks on it or takes any action.

## Test Considerations:
- ensure the correct non-stuck state is returned to after the user checks on or interacts with a yellow tab.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- this is a TUI only change, don't change any CLI code.
