# Work Item: Feature

Title: Add Dockerfile audit to the `ready` command.
Issue: issuelink

## Summary:
- The `aspec ready` command and `ready` subcommand within the TUI should now launch the configured code agent to update Dockerfile.dev to ensure it has all of the tools needed to work on the current project's codebase.

## User Stories

### User Story 1:
As a: user

I want to:
keep Dockerfile.dev updated to ensure it has every build and test tool needed for my project

So I can:
successfully work with my code agent within a docker container


## Implementation Details:
- When `ready` is run, insert a new step after checking the Docker daemon, but before building Dockerfile.dev
- If the Docker daemon is ready, check to see if an `aspec-{projectname}:latest` image exists (where projectname is the name of the folder containing the current Git root)
- If that image does exist, launch it and prompt the configured agent as follows: "scan this project and determine every tool needed to build, run, and test it per the local development workflows defined in the aspec. Modify Dockerfile.dev to ensure that all of those tools, at the correct version, get installed when the Dockerfile is built"
- Once the configured agent is done modifying Dockerfile.dev, build it again and tag it as `aspec-{projectname}:latest` where projectname is the name of the folder containing the current Git root
- Each time a Docker container is launched as part of the ready subcommand, the standard agent auto-authentication check and flow should be performed, and the standard file mounting rules should be followed. If the command is run from a subdirectory, ask the user if only the current subdirectory should be mounted, or if the Git root should be mounted. Ensure the code agent is always executed in the correct directory containing whatever the user chose to mount.
- Any time a Docker container is run across any subcommand or TUI subcommand, show the full Docker CLI command being run (print in regular command mode, include in status bar in TUI mode)
- Validate that the aspec tool understand where the authentication data for each available agent type is stored on the host machine, and ensure that they are mounted in the correct place within the Docker container if and only if the user has accepted agent auto authentication.


## Edge Case Considerations:
- If Dockerfile.dev does not exist, or if there is no image called `aspec-{projectname}:latest` available via the Docker daemon, initialize a new Dockerfile.dev using the appropriate template for the desired agent and build it with the tag `aspec-{projectname}:latest. Then, run the new container with the prompt above as normal.
- if any `aspec` subcommand is passed the `--auth-from-env` flag, use the agent-appropriate environment variable from the host to extract the authentication token for the agent running in the container. If that flag is NOT passed, then automatically extract the authentication token from the system keychain and pass it to the container. If `--auth-from-env` is NOT passed, the user must either have previously accepted automatic agent authentication, or they must be prompted if automatic auth is allowed. Save the automatic auth preference in the project-local storage as usual.

## Test Considerations:
- considerations

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- ensure that the output from any Docker container is properly funneled into the TUI (stdout AND stderr)