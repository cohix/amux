# Work Item: Enhancement

Title: virtual terminal scrolling and size
Issue: issuelink

## Summary:
- the virtual terminal used for the "container window" can be made larger, and it should allow for scrolling with the mouse

## User Stories

### User Story 1:
As a: user

I want to:
be able to scroll within the virtual terminal presented within the container window.

So I can:
view previous output of the running agent.


## Implementation Details:
- investigate the virtual terminal implementation that exists within the container window in the TUI. determine if there is a way to enable mouse scrolling in a reasonable way that retains all of its current functionality (full screen tui apps, colors, spinners, etc) and full interactivity.
- the container window can be made larger, have it take up 95% of the outer window's width and height, centered.


## Edge Case Considerations:
- ensure the container window resizes properly when the real terminal changes sizes
- DO NOT lose any of the virtual terminal's current functionality or interactivity. terminate your attempt if anything will become degraded in the attempt to add scrolling.

## Test Considerations:
- ensure the outer window is not affected

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
