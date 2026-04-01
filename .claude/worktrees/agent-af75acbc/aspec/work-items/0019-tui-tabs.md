# Work Item: Feature

Title: TUI tabs
Issue: issuelink

## Summary:
- allow multiple 'tabs' to exist within the TUI to allow running multiple concurrent 'sessions'

## User Stories

### User Story 1:
As a: user

I want to:
open multiple tabs within the TUI

So I can:
run and monitor several concurrent agents at once.


## Implementation Details:
- the amux TUI will now allow for multiple "tabs" that are displayed along the top edge of the TUI. These tabs will be similar to browser tabs in that they are each completely independent of one another and allows the user to switch between multiple running `sessions`.
- each tab gets its own `outer window` and `container window`. When the user switches between tabs, the TUI will show the outer and container windows associated with each tab. The behaviour within a single tab is identical to how the TUI currently operates (including Esc, c key, up/down arrows, minimzed container bar, recent container bar, etc.)
- The tabs and tab top bar should be graphically distinct (i.e. each tab is a rounded rectangle shown in a new top bar above the outer window and/or container window). Tabs start on the left of the top bar and more are added to the right as the user creates them.
- when the TUI is launched, there is only one tab, shown on the top lefthand side. The tab itself should show the name of the project the tab is associated with (like 'amux' if the tab's working directory is the amux git repo) in the tab rectangle's title.
- If a subcommand is running, the subcommand name (or the name of the most recently finished subcommand) should be shown INSIDE the tab rectangle itslf (e.g. `chat`). 
- the user presses `ctrl+t` to create a new tab. When a new tab is created, it is added immediately to the right of the current tab. When a new tab is created, the user is immediately switched to it.
- when a new tab opens, its outer window and command text box are shown. The user is prompted via dialog wether they wish the new tab to run in the current CWD, or another directory. If they choose another directory, they must type in the directory.
- each tab is associated with a "working directory", and all commands are operated against that directory. Each tab may have a different working directory, and several tabs may share the same working directory. The full path of the working directory for the current tab should be displayed directly below the command text box whenever there are no subcommand suggestions being shown.
- EVERY OPERATION including `ready` checks, container execution, `aspec` template downloading, audit agents, etc. should be run in the tab's working directory as if `amux` was launched in that directory.
- if the user chooses to open a tab in a working directory that is not already open in another tab, the `ready` subcommand should be run within that working directory, just as if the user opened the TUI freshly. If another tab exists with the same working directory, skip the ready command and print a message stating as such.
- the user nagivates between tabs using `ctrl+a` to move to tab to the left of the current tab and `ctrl+d` to move to the tab to the right of the current tab.
- the tab's color should reflect its current subcommand's status. An idle tab should be grey (no subcommand running). A tab running a subcommand should be blue. A tab with an actively running container should be green. A tab that has encountered an error that is unresolved should be red. A tab running any of the `claws` subcommands (even if a container is running) should be purple.
- the currently active tab should have a solid background of its appropriate color (grey, blue, green, purple, or red). All non-active tabs should have their outline colored appropriately but no color background.
- each tab may only have a single running subcommand, and a single running container.
- if the user presses `Esc` to focus the command text box while a subcommand is running (or Esc twice if a container is running) within the current tab, display the text `Press ctrl+t to run another command in a new tab` within the command text box, but disable it so that the user cannot type within it.
- if a tab is idle (no subcommand or container running), the user can press ctrl+c, a dialog will ask the user if they want to close the current tab, exit the TUI, or cancel. Allow the user to choose, and respond appropriately. If there is only one open tab, the current ctrl+c dialog/behaviour should be used.


## Edge Case Considerations:
- this is a TUI ONLY change. Little to no changes should be made to CLI logic.
- ensure the container state (running maximized, running minimized, container summary bar) is persisted for each tab so when a user leaves and returns to a tab, it is in the same state it was left.
- ensure user input is only bound to the command text box or container virtual terminal for the CURRENTLY ACTIVE tab. The user's input should not be sent to any other tab or tab's container that is not currently active.
- The VT100 session for each container should continue to operate even if the container's tab is not currently being shown. If the user leaves and then returns to a particular tab, the running container's virtual terminal should be still running and be up to date with the command that was running in the background.

## Test Considerations:
- ensure each tab is completely independent of each other
- ensure that user input can be attached/detached from each tab and its container so that only one tab at a time is recieving user input.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- maintain modular code between TUI and CLI.
- aim for memory and CPU efficiency when dealing with multiple tabs, subcommands, and container instances, but do not sacrifice user experience for efficiency.
