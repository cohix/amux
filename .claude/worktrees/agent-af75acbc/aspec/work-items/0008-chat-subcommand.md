# Work Item: Feature

Title: Chat Subcommand
Issue: issuelink

## Summary:
- add a new `chat` subcommand which opens the configured agent without a pre-configured prompt.

## User Stories

### User Story 1:
As a: user

I want to:
- run `aspec chat` or `chat` within the aspec TUI to start a fresh containerized chat session with my agent

So I can:
have a freeform chat with my containerized agent


## Implementation Details:
- the `chat` subcommand should start the configured agent in a container with no initial prompt
- the agent should always be launched in interactive mode (as usual), unless the `--non-interactive` flag is passed
- the agent should be auto-authenticated using the pre-existing methods within the codebase. Use existing methods, do not change existing logic
- ensure the containerized agents are configured correctly using existing methods
- refactor the implement subcommand and modularize the implementation such that `aspec implement` and `aspec chat` use the same underlying code paths
	- aspec chat launches the agent with no pre-configured prompt
	- aspec implement launches the agent with the pre-configured implement prompt for the given work item


## Edge Case Considerations:
- ensure the subcommand works properly across CLI (`aspec chat`) and TUI (run aspec to launch TUI, then run `chat` subcommand).

## Test Considerations:
- test CLI/TUI parity
- test the modular implement/chat shared implementation for both use-cases

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- clean up and warnings in the build, even if they came before this work item.