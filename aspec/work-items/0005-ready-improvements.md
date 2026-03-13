# Work Item: Enhancement

Title: ready subcommand enhancements
Issue: issuelink

## Summary:
- improve the utility of the ready command

## User Stories

### User Story 1:
As a: user

I want to:
run the ready command with optional --refresh flag and streaming Docker Build output

So I can:
optimize my workflow by skipping unneeded steps


## Implementation Details:
- for the `aspec` base command and `aspec ready`, add a new `--refresh` flag which optionally runs the Dockerfile agent audit. If the flag is not present, print a line indicating that the Dockerfile audit is being skipped. If the flag is passed, use the configured agent to audit the codebase and Dockerfile as it does now.
- whenever an interactive code agent is launched, first print a large noticable ascii-art decorated notice letting the user know that the agent is launching in interactive mode and they will need to quit the agent (via ctrl-c or other) when the agent's work is complete
- for any command which can launch an interactive agent (including the base `aspec` command), add a `--non-interactive` flag which causes the configured agent to be launched in "print" mode (or whatever non-interactive mode it supports). Interactive mode should remain the default whenever the flag is not passed. Update all subcommands in command mode and TUI mode to match.
- currently, output from `docker build` when running `aspec ready` does not stream its output. The full output is printed at the end, making it appear frozen. Fix this issue, and allow output from all Docker Build commands stream its output.
- Whenever the `ready` subcommand is run (as a command or in TUI`), present a summary table at the end of the output showing everything that was checked and each step/component's status (such as Docker: running, Dev image: exists, Refresh: skipped), etc.


## Edge Case Considerations:
- Add appropriate hints to outputs/TUI whenever the `--refresh` or `--non-interactive` flags could be used, as a "tip".

## Test Considerations:
- test parity between command mode and TUI mode.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- ensure to maximize modularity of all code and avoid duplication between command mode and TUI mode wherever possible.