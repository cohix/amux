# Work Item: Feature

Title: plan flag
Issue: issuelink

## Summary:
- add a --plan flag to chat and implement subcommands

## User Stories

### User Story 1:
As a: user

I want to:
pass --plan to `chat` or `implement` subcommands in aspec CLI and TUI

So I can:
run my code agent in plan mode.


## Implementation Details:
- add a `--plan` flag to the `chat` and `implement` subcommands
- when passed, the `--plan` flag should cause the agent running within the container to be initialized in plan mode (exact implementation is agent dependent)


## Edge Case Considerations:
- ensure parity between CLI and TUI

## Test Considerations:
- test that each agent gets configured in its own plan mode properly based on its documentation

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
