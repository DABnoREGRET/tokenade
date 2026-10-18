//! TTY-only progress messages.
//!
//! Long Tokenade operations (indexing, semantic build, model download)
//! print a status line to stderr so a user running `tokenade ask "X"`
//! in their shell sees "I'm working, not hung". The TTY check makes
//! the path silent in every non-interactive context:
//!
//! - MCP server (Claude Code talks to us over stdio JSON-RPC): silent.
//!   Host UIs do not display arbitrary text from MCP servers in their
//!   spinner — we used to push `notifications/progress` messages but
//!   they were never surfaced. Removed.
//! - Hook subprocesses (PostToolUse Bash wrap): silent. Stderr piped.
//! - Background / CI: silent.
//!
//! Net effect: zero tokens consumed by progress chatter; CLI users still
//! get the feedback they need on their own terminal.

use std::io::{IsTerminal, Write};

/// Print `message` to stderr if and only if stderr is a real terminal.
/// Used both directly (`announce`) and by the [`progress!`] macro.
pub fn announce(message: &str) {
    if std::io::stderr().is_terminal() {
        let _ = writeln!(std::io::stderr(), "{message}");
    }
}

/// Macro variant that mirrors `format!` so existing call sites such as
/// `progress!("[tokenade] indexing {} files…", n)` keep compiling.
#[macro_export]
macro_rules! progress {
    ($($arg:tt)*) => {
        $crate::progress::announce(&format!($($arg)*))
    };
}

/// Legacy alias kept for callers that already used `eprint` explicitly.
pub fn eprint(s: &str) {
    announce(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_no_panic_without_tty() {
        // cargo test pipes stderr → no-op path is exercised.
        announce("not visible");
        eprint("also not visible");
    }
}
