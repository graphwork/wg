//! Telegram command parser and executor for WG.
//!
//! Reuses the same command parsing logic as [`crate::matrix_commands`] since
//! the command vocabulary is identical. This module provides Telegram-specific
//! help text formatting and the entry point for command dispatch from the
//! Telegram listener.

use std::path::Path;

use crate::matrix_commands::{self, MatrixCommand};

/// Parse a Telegram message into a WG command.
///
/// Delegates to the shared parser in [`matrix_commands`]. The command syntax
/// is identical: `claim <task>`, `done <task>`, `status`, etc.
pub fn parse(message: &str) -> Option<MatrixCommand> {
    MatrixCommand::parse(message)
}

/// Execute a parsed command against WG, returning the response text.
pub fn execute(workgraph_dir: &Path, command: &MatrixCommand, sender: &str) -> String {
    matrix_commands::execute_command(workgraph_dir, command, sender)
}

/// Generate Telegram-formatted help text.
pub fn help_text() -> String {
    "📋 *WG commands*\n\n\
     • `claim <task>` \\- Claim a task\n\
     • `claim <task> as <actor>` \\- Claim for someone\n\
     • `done <task>` \\- Mark done\n\
     • `fail <task> [reason]` \\- Mark failed\n\
     • `input <task> <text>` \\- Add a log entry\n\
     • `unclaim <task>` \\- Release a task\n\
     • `ready` \\- List ready tasks\n\
     • `status` \\- Project status\n\
     • `help` \\- This help\n\n\
     Prefix with `wg` if needed \\(e\\.g\\. `wg claim task\\-1`\\)"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_delegates_to_shared_parser() {
        let cmd = parse("claim my-task").unwrap();
        assert!(matches!(cmd, MatrixCommand::Claim { .. }));
    }

    #[test]
    fn parse_with_prefix() {
        let cmd = parse("wg status").unwrap();
        assert!(matches!(cmd, MatrixCommand::Status));
    }

    #[test]
    fn parse_ignores_regular_text() {
        assert!(parse("hello world").is_none());
    }

    #[test]
    fn help_text_is_nonempty() {
        let text = help_text();
        assert!(text.contains("WG commands"));
    }
}
