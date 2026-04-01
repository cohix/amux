# Work Item: Feature

Title: allow docker daemon access with flag
Issue: issuelink

## Summary:
- optionally allow aspec-launched containers to access the docker daemon socket.

## User Stories

### User Story 1:
As a: user

I want to:
allow some containers to access the docker daemon socket

So I can:
allow code agents to build and run Docker containers


## Implementation Details:
- if the `--allow-docker` flag is passed to implement, chat, or ready subcommands, launch the agent container with access to the host docker daemon socket by mounting it via volume
- ensure the volume mount location is correct for the platform aspec is running on (linux, windows, macos)


## Edge Case Considerations:
- before launching a container with `--allow-docker`, check that the host's Docker Daemon is running and accessible and that the socket file exists
- whenever docker access is being allowed, print the docker daemon check results and a warning that the docker socket is being mounted in the container in the outer window before launching the container window

## Test Considerations:
- add tests for OS support (socket file paths etc.)

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- ensure the implementation of this flag is consistent with other docker-related flags and uses a shared implementation across all of the subcommands (implement, chat, ready)
