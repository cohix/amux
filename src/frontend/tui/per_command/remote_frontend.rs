//! TUI hooks for the remote command family.

use crate::command::commands::remote::RemoteCommandFrontend;
use crate::frontend::tui::command_frontend::TuiCommandFrontend;

impl RemoteCommandFrontend for TuiCommandFrontend {}
