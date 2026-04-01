# Work Item: Feature

Title: Ready Build and NoCache Flags
Issue: issuelink

## Summary:
- new --build and --no-cache flags for `ready` subcommand

## User Stories

### User Story 1:
As a: user

I want to:
rebuild my aspec dev container without needing to re-audit it.

So I can:
get a fresh image when the Dockerfile is updated


## Implementation Details:
- add a --build flag to `aspec ready`, `aspec` and `ready` within the TUI.
	- when passed, the development container should be built from the current Dockerfile.dev in the project
	- if `--build` is passed to `aspec`, the `ready` command that is run upon TUI startup should also be given the `--build` flag
	- use existing container image naming and logic, just update the ready subcommand workflow to build the new image even if one exists already
- add a --no-cache flag to `aspec ready`, `aspec`, and `ready within the TUI
	- when passed, the development container build should include `--no-cache` as a flag for `docker build`
	- if passed, the `--no-cache` flag should be passed along to any invocation of `docker build`, whether `--build` is passed or not


## Edge Case Considerations:
- any time `ready` or `init` subcommands find that no Dockerfile.dev is present, apply the appropriate template for the correct agent, and then initiate a build of the development container, even if one with the correct name exists within Docker.
- if both `--refresh` and `--build` are passed, ignore `--build` since the container is automatically rebuilt after the code agent audit already
- if both `--refresh` and `--no-cache` are passed, ensure `--no-cache` is also passed to `docker build`.


## Test Considerations:
- ensure parity between CLI and TUI versions of the subcommand
- ensure flags passed to `aspec` are passed into the `ready` subcommand that is automatically run when the TUI starts.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
