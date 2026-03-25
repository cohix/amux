# Work Item: Feature

Title: dashboard subcommand
Issue: issuelink

## Summary:
- a new `status` subcommand which shows the state of all agents amux is running

## User Stories

### User Story 1:
As a: user

I want to:
run the `status` subcommand 

So I can:
to get a full picture of my amux agent universe


## Implementation Details:
- add a new subcommand, `status`: it outputs two tables:
	- one showing all of the active code agent containers (title: CODE AGENTS) (any running container with amux- prefix should work). The table columns show which Git project the container is mounted to, which code agent it's running (go find the amux config for the project), and all the Docker stats normally shown in the TUI
	- the other showing status of Nanoclaw and its containers (any container name with nanoclaw in the name should be included). Show the same Docker stats.
- if there are no code agents and/or no nanoclaw running, show a reasonable "empty state" and suggest the commands needed to get each type started.
- add a `--watch` command which refreshes the table every 5s
	- if run from the CLI. Use carriage returns etc to make the full output update in-place rather than just be output again.
	- if run from the TUI, it should be shown in the tab's outer window, and every time it refreshes, the window's contents should be wiped so that the tables appear to update in place, even though the outer window doesn't support carriage returns. This is a rare CLI/TUI mismatch of behaviour, be sure to handle it modularly within code.
	- if the `amux` TUI is launched within a folder that is not a Git repo, run `status --watch` instead of `ready`.


## Edge Case Considerations:
- considerations

## Test Considerations:
- considerations

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
