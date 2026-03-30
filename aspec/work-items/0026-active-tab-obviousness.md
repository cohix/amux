# Work Item: Enhancement

Title: Active Tab Obviousness
Issue: N/A

## Summary:
- The active tab in the TUI is not always visually obvious. When a tab's assigned color is grey or a dark neutral, the current bold + arrow indicator is insufficient contrast to distinguish it from inactive tabs. The active tab must be unmistakably clear regardless of the tab's color.

## User Stories

### User Story 1:
As a: user

I want to:
immediately see which tab is active at a glance, no matter what color that tab has been assigned

So I can:
quickly orient myself when switching between multiple worktrees without having to read tab labels carefully

### User Story 2:
As a: user

I want to:
inactive tabs to recede visually so my focus is drawn to the active one

So I can:
reduce cognitive overhead when managing many simultaneous agent sessions

### User Story 3:
As a: user

I want to:
the active tab to look "open" or "raised" compared to closed/flat inactive tabs

So I can:
use spatial metaphors I already understand from standard tabbed UI conventions

## Implementation Details:

 **Bottom border suppression on active tab**: Render the active tab block with `Borders::TOP | Borders::LEFT | Borders::RIGHT` (omit `Borders::BOTTOM`). This makes the active tab appear to open into the content area below, reinforcing the tab metaphor.

## Edge Case Considerations:

- **Single tab**: With only one tab, the tab bar still renders correctly — the single active tab shows full height with no bottom border. No visual regression.


## Test Considerations:

- **Single tab renders without panic**: Render with `app.tabs.len() == 1` and `active_tab_idx == 0`; assert no out-of-bounds.


## Codebase Integration:
- Follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- All changes are isolated to `src/tui/render.rs` in the `draw_tab_bar` function; no new modules or public API changes required.
- The `TabState` helpers (`tab_color`, `tab_project_name`, `tab_subcommand_label`) in `src/tui/mod.rs` are read-only dependencies — do not modify them.
- Follow the existing pattern of using `Modifier::BOLD` and `BorderType::Rounded` rather than introducing new border types.
