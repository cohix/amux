# Work Item: Bug

Title: Docker Build Doesn't Stream Output
Issue: issuelink

## Summary:
- when running the `ready` subcommand, the output from `docker build` is not streamed into the TUI window, resembling an apparent hang.

## User Stories

### User Story 1:
As a: user

I want to:
view streaming logs of docker builds within aspec

So I can:
understand the progression of Docker builds in real time


## Implementation Details:
- when running the `ready` subcommand within the TUI, enable live streaming for the output of `docker build`
- be sure to capture both stdout and stderr
- retain text color if possible, but do not sacrifice streaming and proper text display for color support.


## Edge Case Considerations:
- ensure all call sites for docker builds in the TUI support streaming logs

## Test Considerations:
- ensure docker build command outputs stream properly and are not buffered

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- do not touch the container window virtual terminal implementation, this is for the "outer window" only.
