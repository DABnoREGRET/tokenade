//! Interactive prompts for `tokenade install` and friends.
//!
//! Two execution modes, one code path:
//!
//! 1. **TTY** (the user runs `tokenade install` themselves) — we print
//!    each proposed action with a y/N prompt; the user types a key.
//! 2. **Non-TTY** (an AI agent invoked us, our stdin/stdout are pipes)
//!    — we never prompt (we'd hang waiting for input that never comes).
//!    Instead, [`Decision::auto`] returns the "safe default" (`No` for
//!    destructive actions) UNLESS the caller passed `--yes` upstream,
//!    in which case the action runs without confirmation.
//!
//! This way the same install code path works in both situations and the
//! invariant "no destructive action without explicit consent" holds.

use std::io::{BufRead, IsTerminal, Write};

/// What the caller wanted the prompt to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Action approved by the user (TTY 'y'), by `--yes`, or by an
    /// explicit caller-side override.
    Yes,
    /// Action declined by the user (TTY 'n' / Enter / EOF), or by the
    /// safe default for a non-interactive run.
    No,
}

/// Caller's hint about consent flags from CLI args.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsentFlags {
    /// `--yes` / `-y` — accept everything reasonable. Implies non-
    /// interactive even on a TTY (the user already opted in).
    pub assume_yes: bool,
    /// `--dry-run` — never actually do anything; prompts still print
    /// so the user sees the plan, but the outcome is always `No`.
    pub dry_run: bool,
}

/// Ask the user yes/no. `default_no` controls the Enter / empty-input
/// outcome and which letter is uppercased in the prompt.
///
/// Output goes to stderr (so it doesn't mix with structured stdout from
/// `--json`-flavoured callers). Input is read from stdin one line at a
/// time. EOF / read error always → `No`.
pub fn ask(question: &str, default_no: bool, flags: ConsentFlags) -> Decision {
    // `--yes` wins immediately. We still echo the question so the user
    // sees what we're about to do.
    if flags.assume_yes {
        let _ = writeln!(std::io::stderr(), "{question} [--yes → YES]");
        return Decision::Yes;
    }
    // `--dry-run` always declines. Same: echo for visibility.
    if flags.dry_run {
        let _ = writeln!(std::io::stderr(), "{question} [--dry-run → NO]");
        return Decision::No;
    }
    // Non-TTY without --yes: safe default. We print the proposal so the
    // caller (AI agent or pipeline) can see it.
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        let default_label = if default_no { "no" } else { "yes" };
        let _ = writeln!(
            std::io::stderr(),
            "{question} [non-interactive, default: {default_label}]"
        );
        return if default_no {
            Decision::No
        } else {
            Decision::Yes
        };
    }

    // Interactive path. Loop until we get a clear answer or EOF.
    let stdin = std::io::stdin();
    let mut input = String::new();
    loop {
        let prompt_letters = if default_no { "y/N" } else { "Y/n" };
        let _ = write!(std::io::stderr(), "{question} [{prompt_letters}] ");
        let _ = std::io::stderr().flush();
        input.clear();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => return Decision::No, // EOF
            Ok(_) => {}
            Err(_) => return Decision::No,
        }
        match input.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "o" | "oui" => return Decision::Yes,
            "n" | "no" | "non" => return Decision::No,
            "" => {
                return if default_no {
                    Decision::No
                } else {
                    Decision::Yes
                }
            }
            other => {
                let _ = writeln!(
                    std::io::stderr(),
                    "  unrecognised answer `{other}`, please type y or n"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_short_circuits_to_yes() {
        let f = ConsentFlags {
            assume_yes: true,
            dry_run: false,
        };
        assert_eq!(ask("q", true, f), Decision::Yes);
        assert_eq!(ask("q", false, f), Decision::Yes);
    }

    #[test]
    fn dry_run_short_circuits_to_no() {
        let f = ConsentFlags {
            assume_yes: false,
            dry_run: true,
        };
        assert_eq!(ask("q", true, f), Decision::No);
        assert_eq!(ask("q", false, f), Decision::No);
    }

    // The non-tty fallback is exercised under `cargo test` (no terminal).
    #[test]
    fn non_tty_uses_default() {
        let f = ConsentFlags::default();
        assert_eq!(ask("q", true, f), Decision::No);
        assert_eq!(ask("q", false, f), Decision::Yes);
    }
}
