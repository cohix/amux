# Work Item: Task

Title: init and ready refinements
Issue: issuelink

## Summary:
- small refinements to the init and ready commands to make everything more intuitive

## User Stories

### User Story 1:
As a: user

I want to:
run `init`, `ready`, and `claws ready` subcommands more flexibly with choices about what happens via flags

So I can:
more easily get started and understand the use of amux


## Implementation Details:
- the `init` subcommand should gain a new flag, `--aspec`:
	- if `--aspec` is passed, download the `aspec` templates to the current project as currently happens
	- if `--aspec` is NOT passed, skip downloading the `aspec` templates, note that it was skipped in the summary table
- the `init` command should NOT overwrite any existing Dockerfile.dev: 
	- If Dockerfile.dev already exists in the project, do not replace it, instead interactively offer to run the agent audit container with an explanation of what it will do. Only proceed if the user agrees, skip agent audit if they decline.
	- If Dockerfile.dev DOES NOT exist in the project, download it and interactively offer to run the agent audit container with an explanation of what it will do. Only proceed if the user agrees, skip agent audit if they decline.
- the `ready` subcommand should no longer automatically add a Dockerfile.dev if it's missing:
	- if `ready` is run and no Dockerfile.dev exists, interactively ask the user if they'd like to add a Dockerfile.dev and run the agent audit container, first explaining what it does. Only proceed if the user agrees, skip download and agent audit if they decline.
	- if there is no Dockerfile.dev and the user declines to create one, the result of the `ready` command should be a failure, and the summary table should show that Dockerfile.dev is missing.
	- if there is no `aspec` folder present in the project, add that note to the summary table (aspec: missing), and add a note to the output that the user can run `amux init --aspec` to add an aspec folder
- At the end of `init`, provide a summary table of everything that happened, and include a prominent "what's next?" below the table (with ASCII art title) explaining that the user can run `amux` to start the interactive TUI, and then use the `chat`, `new`, and `implement` commands, with a short summary of what each one does.
- during the `ready` command, add a new check step which executes a NON-CONTAINERIZED command to the user's code agent of choice sending a simple greeting such as "hello" to 1) check if the agent is installed on the host and 2) to refresh any oauth token that may be used. 
	- create 50 random simple greetings and randomly select one to pass to the agent each time `ready` is run.
	- run the configured agent in non-interactive mode so that it just prints its response and then exits, such as `claude --print "hello"`.
	- include in the summary table whether the local agent is installed and working/authenticated or not (e.g. Claude: installed & authenticated). Do not fail the command if the agent is not installed on the host.


## Edge Case Considerations:
- if `ready` is run and there is an existing Dockerfile.dev, but its contents exactly match one of the default templates, offer to run the agent audit container. If the contents do not match any of the templates, do not offer to run the agent.
- any time a new Dockerfile.dev is created OR the audit agent container is run, always build the Dockerfile.dev image. 
- If a new Dockerfile.dev is created, do not build it until the user has been offered to run the audit agent container. If they decline the audit, build the Dockerfile. If they accept the audit, run the audit container and then build the Dockerfile afterwards.

## Test Considerations:
- test for CLI/TUI parity

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- use existing conventions for asking user questions/permission in the CLI and TUI.
