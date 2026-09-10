//! Pure-presentation helpers for the CLI frontend.
//!
//! Color/hyperlink decisions, exit-code maps for typed outcome variants,
//! and any other terminal-styling logic that does NOT depend on Layer 2
//! semantics belongs here.

use std::io::IsTerminal;

/// `true` when stdin is connected to a TTY. Drives the CLI frontend's
/// safe-default fallback for interactive prompts when stdin is piped.
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}
