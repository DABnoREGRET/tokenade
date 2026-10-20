//! One-line invitation to star the repository, shown once per machine.
//!
//! Shown at the end of a successful `tokenade install`, or by `tokenade
//! dashboard` once there are saved tokens to show — whichever a person sees
//! first; after that, never again (sentinel `star-invite-shown` in
//! `~/.tokenade`). Never shown when stdout is not a terminal or under CI, and
//! `TOKENADE_NO_STAR_INVITE=1` turns it off. Nothing here touches the network.

use std::io::IsTerminal;
use std::path::Path;

pub(crate) const LINE: &str = "Tokenade is free and open source. If it saves you tokens, a ⭐ helps others find it: https://github.com/pi-infected/tokenade";

const SHOWN: &str = "star-invite-shown";

/// Opt-out variable: any value but empty / `0` / `false` disables the line.
pub(crate) const OPT_OUT_ENV: &str = "TOKENADE_NO_STAR_INVITE";

fn truthy(v: Option<String>) -> bool {
    v.is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    })
}

/// The decision, with every input passed in so tests need no TTY or env.
/// Marks the machine BEFORE saying yes: if the sentinel cannot be written,
/// the line is not shown at all rather than shown on every run.
pub(crate) fn take_at(
    dir: &Path,
    stdout_is_tty: bool,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    if !stdout_is_tty || truthy(env("CI")) || truthy(env(OPT_OUT_ENV)) {
        return false;
    }
    let sentinel = dir.join(SHOWN);
    if sentinel.exists() {
        return false;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    std::fs::write(&sentinel, env!("CARGO_PKG_VERSION")).is_ok()
}

/// Print the line (stdout, after a blank line) if this machine has not seen it.
pub(crate) fn maybe_show() {
    let dir = tokenade_core::platform::tokenade_state_dir();
    if take_at(&dir, std::io::stdout().is_terminal(), |k| {
        std::env::var(k).ok()
    }) {
        println!("\n{LINE}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!("tk-star-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn shown_once_then_never() {
        let d = tmp("once");
        assert!(take_at(d.as_path(), true, no_env));
        assert!(!take_at(d.as_path(), true, no_env));
        assert!(!take_at(d.as_path(), true, no_env));
    }

    #[test]
    fn creates_missing_state_dir() {
        let d = tmp("mkdir");
        let state = d.as_path().join(".tokenade");
        assert!(take_at(&state, true, no_env));
        assert!(state.join(SHOWN).exists());
    }

    #[test]
    fn never_without_a_tty_and_does_not_consume_the_slot() {
        let d = tmp("tty");
        assert!(!take_at(d.as_path(), false, no_env));
        assert!(!d.as_path().join(SHOWN).exists());
        assert!(take_at(d.as_path(), true, no_env));
    }

    #[test]
    fn never_under_ci() {
        let d = tmp("ci");
        for v in ["true", "1", "yes"] {
            assert!(!take_at(d.as_path(), true, |k| (k == "CI").then(|| v.into())));
        }
        assert!(!d.as_path().join(SHOWN).exists());
        // CI=false / CI=0 is not CI.
        assert!(take_at(d.as_path(), true, |k| (k == "CI").then(|| "false".into())));
    }

    #[test]
    fn opt_out_env_disables_it() {
        let d = tmp("optout");
        assert!(!take_at(d.as_path(), true, |k| (k == OPT_OUT_ENV)
            .then(|| "1".into())));
        assert!(!d.as_path().join(SHOWN).exists());
        assert!(take_at(d.as_path(), true, |k| (k == OPT_OUT_ENV)
            .then(|| "0".into())));
    }

    #[test]
    fn line_has_no_tracking_parameters() {
        assert!(LINE.ends_with("https://github.com/pi-infected/tokenade"));
        assert!(!LINE.contains('?'));
    }
}
