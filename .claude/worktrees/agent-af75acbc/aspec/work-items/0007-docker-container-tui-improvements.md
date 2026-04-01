# Work Item: Feature

Title: Docker Container TUI Improvements
Issue: issuelink

## Summary:
- simplify the codebase by handling simple text output and interactive sessions in two seperate TUI components
- improve the interactive code agent experience by enabling full interactivity
- improve user experience of running containers within the TUI by adding additional metadata from Docker

## User Stories

### User Story 1:
As a: user

I want to:
view an isolated "container-window" whenever a container is running within the TUI

So I can:
get a clear sense of what is running within a container, get a more consistent interactive agent experience, and get Docker metadata.


## Implementation Details:
- the TUI must now implement two seperate "windows"
	- The "Outer" window (the currently implemented one) which is dedicated to showing only streaming text output
	- The new "Container window" which overlays above the outer window and is dedicated to only output from containers launched by aspec, focused on interactivity.
- Whenever aspec launches a container to run a coding agent, a new "Container window" must appear within the "outer window".
- The outer window implementation must be updated to be optimized for only plain-text streaming output from commands.
- The container window must be implemented to be optimized for interactive experiences like Claude's interactive mode. It must prioritize displaying TUI interfaces and behave properly with carriage returns, spinners, full-screen TUI apps, and more.
- The container window must capture the user's input whenever it is open and maximized, and all keyboard input must be sent to the process running in the container window, prioritizing a "native-feeling" interactive experience.
- Whenever the container window is open, aspec must be querying the Docker daemon at 5-second intervals in the background, and update the container window's title bar (top right) with the following information: Container name, CPU used, Memory used, Total runtime (no titles for each stat, e.g. "curious_muskrat | 5% | 200mb | 12m")
- The container window should have a green border, and a lock icon in its title bar (top left) with the name of the coding agent running and "(containerized)" - e.g. "🔒 Claude Code (containerized)"
- Whenever the container process quits for any reason, the container window should close and the focus returned to the outer window.
- A summary of the most-recently used container (code agent used, container name, avg CPU, avg memory, total time, exit code) should be displayed below the outer window as a rounded rectangle with a dashed-line faded-green border (1-line text height). This should only appear after a container window has been used, and should dissapear before any new container windows are opened.
- The container window should only take up 90% of the outer window when maximized, and be anchored to the bottom of the outer window (with a few px of buffer). This should allow the Docker command output by aspec immediately before a container launch should still be visible above the container window.
- The user must be able to "minimize" the container window to allow for reading the output from the outer window:
	- Whenever a container window is active, the user can press Esc to "minimize" the container window.
	- Whenever the container window is minimized, it should appear as a 1-text-line-tall rounded rectangle below outer window, and should display the title bar text (agent name, Docker stats) within the rectangle. Its border should still be green.
	- If the user presses the `c` key while the outer window is in focus, the container window should return to its normal "open" state and continue behaving as it normally would.
	- If a container window is minimized, it does not capture any user input. The arrow keys, b/e keys, and c keys should operate as described, and pressing Esc a second time should return focus to the command text box.
	- If the user is focused on the command text box, pressing the up arrow should focus back to the outer window as it normally does, regardless if the container window is minimized or maximized.


## Edge Case Considerations:
- The container window should always be "on top" of the outer window, and should be prioritized for focus and user input whenever it is maximized and the command text box is not in focus.
- Only one container window should be present at any given time.
- The container window should only ever be used to handle input/output from a launched container, and no container output should ever go to any other location outside of the container window.
- This entire work item applies to only the aspec TUI. The normal command behaviour should not be modified
- The agent auto-authorization behaviour should be reviewed, and ensure that the user preference is saved to and read from the git project local `aspec/aspec-cli.json` file. If the auto-authorization approval flag is true within aspec-cli.json, the user does not need to be prompted for agent auto-authorization. If the flag is false, do not automatically pass auth. If the flag is not set, prompt the user and save their preference to the project-local file.

## Test Considerations:
- test that the "outer window" is optimized for streaming normal-text output
- test that the "container window" is optimized for interactive TUI-like full-screen experiences, spinners, etc.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- double and triple check the agent auto-authorization implementation to ensure it is reliable, follows the users project-local preferences, and supports both system keychain OAuth token and environment variable API token pass-through.
