//! **The choices made at `tokenade install` survive whatever re-runs it.**
//!
//! Nothing kept them: the SessionStart self-repair, `upgrade` and
//! `install.sh` (which re-runs a BARE `tokenade install` on every update)
//! restored the defaults — hooks put back after `--no-hook`, debug log
//! turned back on after `--no-debug-log`, auto-compaction threshold put back
//! after `--no-autocompact`, agents excluded by `--only` rewired. Reported by
//! a user who saw what they had disabled come back every day.
//!
//! We keep the ARGV of the choices, not a structure: the same parser
//! (`parse_install_flags`) re-reads it, so a choice cannot mean one thing
//! at install and another at repair.

use std::path::{Path, PathBuf};

/// Durable choices. Not `--yes`, `--dry-run`, `--no-cleanup` (specific to one
/// run), nor `--autocompact-pct` (already kept by its marker in
/// settings.json, which can also recognise a hand-edited value).
const REMEMBERED_BOOL: &[&str] = &[
    "--no-hook",
    "--no-wrap-mcps",
    "--with-claude-md",
    "--with-brevity-skill",
    "--per-project",
    "--no-debug-log",
    "--no-autocompact",
];
const REMEMBERED_VALUE: &[&str] = &["--profile", "--debug-log", "--only"];

/// `--defaults`: forget the remembered choices for THIS install and the next ones.
pub(crate) const RESET_FLAG: &str = "--defaults";

fn options_path(home: &Path) -> PathBuf {
    home.join(".tokenade").join("install-options.json")
}

/// The remembered argv, empty when none was ever saved (or it is unreadable).
pub(crate) fn saved_install_args(home: &Path) -> Vec<String> {
    std::fs::read_to_string(options_path(home))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v["args"].as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

pub(crate) fn save_install_args(home: &Path, effective: &[String]) {
    let p = options_path(home);
    tokenade_core::platform::ensure_parent_dir(&p);
    let body = serde_json::json!({ "args": remembered_subset(effective) });
    let _ = crate::write_json_atomic(&p, &body.to_string());
}

fn remembered_subset(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if REMEMBERED_BOOL.contains(&a) {
            out.push(a.to_string());
        } else if REMEMBERED_VALUE.contains(&a) {
            if let Some(v) = args.get(i + 1) {
                out.push(a.to_string());
                out.push(v.clone());
            }
            i += 1;
        }
        i += 1;
    }
    out
}

/// The flag on the command line that cancels a remembered one.
fn overridden_by(saved: &str, rest: &[String]) -> bool {
    let has = |f: &str| rest.iter().any(|a| a == f);
    has(saved)
        || match saved {
            "--no-hook" => has("--with-hook"),
            "--no-debug-log" => has("--debug-log"),
            "--debug-log" => has("--no-debug-log"),
            _ => false,
        }
}

/// The argv this run acts on: the explicit `rest`, then every remembered
/// choice it does not override. Returns `(effective, inherited)` — the second
/// so the installer can SAY which choices it kept.
pub(crate) fn merge_saved_install_args(
    rest: &[String],
    saved: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut effective = rest.to_vec();
    let mut inherited = Vec::new();
    if rest.iter().any(|a| a == RESET_FLAG) {
        effective.retain(|a| a != RESET_FLAG);
        return (effective, inherited);
    }
    let mut i = 0;
    while i < saved.len() {
        let flag = saved[i].as_str();
        let value = REMEMBERED_VALUE
            .contains(&flag)
            .then(|| saved.get(i + 1).cloned())
            .flatten();
        if !overridden_by(flag, rest) {
            effective.push(flag.to_string());
            inherited.push(flag.to_string());
            if let Some(v) = &value {
                effective.push(v.clone());
                inherited.push(v.clone());
            }
        }
        i += if value.is_some() { 2 } else { 1 };
    }
    (effective, inherited)
}

/// Remembered choices, as the background paths (self-heal, upgrade) need them.
pub(crate) struct SavedInstallChoices {
    pub no_hook: bool,
    pub per_project: bool,
    pub no_debug_log: bool,
    pub no_autocompact: bool,
    pub only: Option<String>,
}

pub(crate) fn saved_install_choices(home: &Path) -> SavedInstallChoices {
    let args = saved_install_args(home);
    let has = |f: &str| args.iter().any(|a| a == f);
    SavedInstallChoices {
        no_hook: has("--no-hook"),
        per_project: has("--per-project"),
        no_debug_log: has("--no-debug-log"),
        no_autocompact: has("--no-autocompact"),
        only: args
            .iter()
            .position(|a| a == "--only")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.to_lowercase()),
    }
}

impl SavedInstallChoices {
    /// Did the user's install cover `agent`? `--only` is the one restriction;
    /// its deprecated aliases resolve as the installer resolves them.
    pub(crate) fn covers(&self, agent: tokenade_core::detect::Agent) -> bool {
        match self.only.as_deref() {
            None => true,
            Some("windsurf" | "devin") => agent.as_str() == "devin-desktop",
            // `--only t3-code` wires Claude Code: T3's driver IS `claude`.
            Some("t3-code") => {
                agent == tokenade_core::detect::Agent::ClaudeCode || agent.as_str() == "t3-code"
            }
            Some(want) => agent.as_str() == want,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_bare_install_inherits_every_remembered_choice() {
        let saved = s(&["--no-hook", "--only", "codex", "--no-autocompact"]);
        let (eff, inh) = merge_saved_install_args(&[], &saved);
        assert_eq!(eff, saved);
        assert_eq!(inh, saved);
    }

    #[test]
    fn an_explicit_flag_overrides_its_remembered_counterpart() {
        let saved = s(&["--no-hook", "--only", "codex", "--no-debug-log"]);
        let rest = s(&["--with-hook", "--only", "cursor", "--debug-log", "/x"]);
        let (eff, inh) = merge_saved_install_args(&rest, &saved);
        assert_eq!(eff, rest, "nothing remembered survives its override");
        assert!(inh.is_empty());
    }

    #[test]
    fn defaults_forgets_everything_and_is_not_passed_on() {
        let saved = s(&["--no-hook", "--per-project"]);
        let (eff, inh) = merge_saved_install_args(&s(&["--defaults", "--yes"]), &saved);
        assert_eq!(eff, s(&["--yes"]));
        assert!(inh.is_empty());
    }

    #[test]
    fn only_durable_choices_are_remembered() {
        let args = s(&[
            "--yes",
            "--dry-run",
            "--no-hook",
            "--only",
            "codex",
            "--autocompact-pct",
            "60",
            "--no-cleanup",
            "--profile",
            "coding",
        ]);
        assert_eq!(
            remembered_subset(&args),
            s(&["--no-hook", "--only", "codex", "--profile", "coding"])
        );
    }

    #[test]
    fn every_remembered_flag_is_an_install_flag_of_the_same_kind() {
        // A name mistyped here would be remembered by no one; a bool listed as
        // a value flag would swallow the flag after it.
        for f in REMEMBERED_BOOL {
            assert!(
                super::super::INSTALL_BOOL_FLAGS.contains(f),
                "{f} is not an install bool flag"
            );
            assert_eq!(remembered_subset(&s(&[f])), s(&[f]));
        }
        for f in REMEMBERED_VALUE {
            assert!(
                super::super::INSTALL_VALUE_FLAGS.contains(f),
                "{f} is not an install value flag"
            );
            assert_eq!(remembered_subset(&s(&[f, "v"])), s(&[f, "v"]));
        }
        assert!(super::super::INSTALL_BOOL_FLAGS.contains(&RESET_FLAG));
    }

    #[test]
    fn choices_round_trip_through_the_file() {
        let home = std::env::temp_dir().join(format!("tok-instopts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        assert!(saved_install_args(&home).is_empty());
        save_install_args(
            &home,
            &s(&["--yes", "--no-debug-log", "--only", "Claude-Code"]),
        );
        let c = saved_install_choices(&home);
        assert!(c.no_debug_log && !c.no_hook && !c.per_project && !c.no_autocompact);
        assert!(c.covers(tokenade_core::detect::Agent::ClaudeCode));
        assert!(!c.covers(tokenade_core::detect::Agent::Qwen));
        let _ = std::fs::remove_dir_all(&home);
    }
}
