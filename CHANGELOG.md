# Changelog

## Unreleased

- **WI 0113 F-08:** Ctrl-T now uses the same `open_or_workdir_fallback`
  policy as the API for non-Git directories, confirmed by the developer as
  the intended single policy (2026-09-08). This is stricter than the old
  TUI behavior in one respect: only "no git root found" falls back silently
  to using the directory as its own root. Any other session-open error
  (e.g. a corrupted `.git` directory, a git-resolution failure, a malformed
  `.awman/config.json`) is now surfaced as "Failed to open session: ..." and
  the tab does not open, instead of being silently masked.
