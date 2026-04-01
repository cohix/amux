# Work Item: Feature

Title: basic TUI
Issue: issuelink

## Summary:
- Implement the basic TUI experience for "interactive" mode.

## User Stories

### User Story 1:
As a: user

I want to:
Enter the aspec "interactive" mode by executing the `aspec` CLI with no arguments.
The TUI environment should allow user to execute any of the `aspec` CLI's subcommands such as `init`, `implement`, `ready`, etc.

So I can:
interact with the aspec tool in a dedicated, intuitive, interactive session.


## Implementation Details:
- The TUI interface should be presented as a simple REPL. The user types their command in the command text box, which is statically/persistently placed and appears at the bottom of the interface.
- As the user types their command, suggestions appear (autofilling things like subcommands, available flags, etc).
- When the user presses 'enter', the command is executed. The command should use the exact same logic, flags, etc. as the associated command as if it were executed via `aspec` in "command mode".
- When the command is executed, the command text box should become disabled, and the executed command text should appear at the top of the TUI window, and remain there statically while output scrolls below.
- The output of the running command should show in a "window" just below the text of the currently running command. This "window" should allow for full interactivity with the running command, as if it were the user's own terminal.
- For commands that execute a Docker container, such as `implement`, the container's output should appear in the window. The user should be able to interact with whatever is running within the window such as typing commands, using the arrow keys, etc. 
- For any command executed within a Docker container, whether via "command" mode or "interactive" mode, their agent of choice should be automatically logged in using the credentials saved on the host machine. This can be accomplished by mounting the appropriate directory holding the credentials on the host machine into the container, but only after asking the user permission (should be done only once per Git root, and the permission saved in the .aspec.json file, 'autoAgentAuthAccepted': true)
- If the window is "selected", it should be outlined in blue for running commands, or green for completed-successful commands, or red for completed-error commands. If it is "unselected" (i.e. the user is typing in the comand text box or presses the Esc key to deselect the window), the window should be outlined in grey.
- While the window is selected in the completed-successful or completed-error state, the up/down arrow keys should scroll the window's contents up and down, showing the entirety of the command's output. While any command is running, all input from the keyboard should be passed to the running process.
- If the window is unselected, the user's input should not be passed into the window's running process and should instead be passed to the "outer" interface, such as the command text box and global commands such as "q for quit".
- While the window is selected, a message below should remind the user that they can press Esc to deselect the window.
- If the window is not selected, a message below should remind the user they can press the up arrow key to move from the command text box to the window.
- When a command completes, the execution window remains in read-only mode and can be scrolled using the arrow keys to show the full output of the command.
- If there was an error code returned from the executed command, it is shown in red text below the execution window, above the command text box. The window outline should turn red.
- When the current command completes, the command text box becomes enabled again and automatically selected so that the user can type their next command. The window automatically becomes deselected (grey outline, or red for error).
- Shift+enter should create a newline within the command text box, rather than executing the current command.
- If the user tries to enter any command other than an `aspec` subcommand, give an error message, clear the command text box, and suggest the subcommand that is most closely related to what they typed (to help with accidental typos).


## Edge Case Considerations:
- At the beginning of each session (i.e. immediately after the TUI is launched), run the `ready` subcommand automatically. Display the output in the "window" as with every other command.
- If the user does not have the "window" selected, and presses 'q' or 'ctl+c" to exit the TUI, ask them if they're sure in a pop-up, and allow them to press y to confirm or n to cancel and remain in the TUI.
- If the user is not logged in to their chosen agent on their host machine, or does not have the chosen agent installed at all, or has chosen not to automatically login their agent (autoAgentAuthAccepted=false in the config file), they should be prompted to log in. If accepted, the login command for the specified agent should be executed within the development docker container, and the appropriate folder where credentials are saved should be created on the host machine and then mounted into the container such that it can be shared across invocations. As with the previous point, if the user chooses to log in in via this prompt, they should then be prompted whether automatic agent login should be enabled, following the same logic as above. If the user chooses not to log in, the command should be canceled.
- Regardless of the final outcome of each command (finshed successfully or with error), user should be able to scroll the full output within the window.
- When a new command is executed via the command text box, clear the contents of the window before beginning to display the new command's output.

## Test Considerations:
- Create integration tests to ensure that the same logic can be reused between the command module and the interactive TUI module.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- Reuse all logic from the commands/ module to avoid duplication of code across "command" mode and "interactive" mode.