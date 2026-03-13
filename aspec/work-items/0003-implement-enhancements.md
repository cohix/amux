# Work Item: Enhancement

Title: Implement command enhancements
Issue: issuelink

## Summary:
- Small improvements to the 'implement' subcommand.

## User Stories

### User Story 1:
As a: user

I want to:
implement a work item using the `implement` subcommand in the aspec CLI or interactive TUI

So I can:
work safely with my code agents within a containerized environment


## Implementation Details:
- Ensure the `implement` subcommand uses the best practices for Docker container execution for this project: permissioned auto-auth for code agents, system keychain OR env var extraction of authorization tokens, proper handling of stdin, stdout, and stderr.
- Ensure there is parity between the command when it's executed via CLI command and within the interactive TUI.
- When prompting the code agent to implement the work item, the prompt should read "Implement work item XXXX. Iterate until the build succeeds. Implement tests as descibed in the work item and the project aspec. Iterate until tests are comprehensive and pass. Write documentation as described in the project aspec. Ensure final build and test success."
- When Claude is the configured agent, for all subcommands within aspec (both CLI and TUI), ensure the use of Claude's interactive mode. The initial prompt should be passed as configured by the subcommand, but then the user should be able to interact with Claude via the TUI just as they would within their terminal, meaning that both inputs via keyboard and outputs should behave as they would outside of aspec. When the user is satisfied, they must quit the interactive mode with ctrl+c, after which aspec can continue its work.
- The user must be able to scroll within the TUI window while a process is running, and all keyboard input including keyboard shortcuts and arrow keys must work within the TUI while a process is running.
- The input for the implement subcommand should be a 4-digit number, such as 0001. Eg: `aspec implement 0002`.


## Edge Case Considerations:
- Ensure long lines are handled properly within the TUI (line wrapping).
- Re-validate the scrolling (arrow keys) and jumping (b/e keys) behaviour of the TUI is still valid and functioning. 
- If the user's input is being captured by the running process, ensure that the process input takes precedence over window navigation.
- Enable mouse scrolling within the TUI to ensure the user can always navigate the window, even if the running process is capturing keyboard input.
- Ensure that typing, arrow keys and keyboard shortcuts such as ctrl+o will work within the TUI and are passed properly to the running process.

## Test Considerations:
- Ensure parity between the CLI and TUI versions of the subcommand. The only difference should be Claude being

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.