# Work Item: Feature

Title: claw command expansion
Issue: issuelink

## Summary:
- expand the `amux claws` subcommands to be less complicated

## User Stories

### User Story 1:
As a: user

I want to:
run `amux claws init`, `amux claws ready` and `amux claws chat`

So I can:
set up, monitor, and configure my claw agents


## Implementation Details:
- the current `claws ready` command will be split into 3:
	- `claws init` will be responsible for the first-run setup wizard that currently exists in `claws ready`
	- `claws ready` will no longer run any first run or setup, and will only run the appropriate status checks and show the results table (as it does today after first run)
	- `claws chat` will check the nanoclaw container is running, and then attach to it interactively and launch the code agent in a freeform chat, no pre-made prompt. In the TUI this will open a container window and the experience should just be like every other container-running command (except it's attaching to a running background container instead of launching a new one)
- ensure all `claws *` subcommands show as purple tabs in the TUI
- ensure the audit container in first run is INTERACTIVE, FOREGROUND, PROMPTED, and the main nanoclaw container is BACKGROUND, DETACHED, NO PROMPT. Both containers are run during the setup, but they are different.


## Edge Case Considerations:
- if `claws ready` is run and nanoclaw is not even downloaded to `$HOME/.nanoclaw`, suggest to the user to run `claws init`
- if `claws ready` is run and nanoclaw IS downloaded but the container is NOT running, interactively offer to start the container, if the user agrees then launch it in the background (-d) as it was done in the first run


## Test Considerations:
- test for CLI/TUI parity

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
