//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::ansi::strip_ansi;
use std::fmt::Write;

pub fn dispatch(argv: &[String], output: &str) -> String {
    // argv[2] is the stash subcommand (e.g. `git stash list`).
    let sub = argv.get(2).map(String::as_str).unwrap_or("list");
    match sub {
        "show" | "show-detail" => super::diff::dispatch(&[], output),
        "pop" | "apply" | "drop" | "clear" | "push" | "save" | "create" | "store" => {
            let s = strip_ansi(output);
            s.trim().to_string()
        }
        _ => compact(output),
    }
}
/// The branch a `WIP on main` / `On main` prefix names.
fn branch_of(kind: &str) -> Option<&str> {
    kind.strip_prefix("WIP on ").or_else(|| kind.strip_prefix("On "))
}

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    // Source format: `stash@{N}: <kind> on <branch>: <hash> <subject>`, or
    // `stash@{N}: On <branch>: <message>` for `git stash push -m`. The hash is
    // kept on purpose: the agent can `git show` it without applying the stash.
    let mut parsed: Vec<(String, Option<String>, String)> = Vec::new();
    for raw in s.lines() {
        let t = raw.trim_end();
        if t.is_empty() {
            continue;
        }
        match t.split_once(": ") {
            Some((head, rest)) => match rest.split_once(": ") {
                Some((kind, subject)) if branch_of(kind).is_some() => {
                    parsed.push((head.to_string(), Some(kind.to_string()), subject.to_string()))
                }
                _ => parsed.push((head.to_string(), None, rest.to_string())),
            },
            None => parsed.push((t.to_string(), None, String::new())),
        }
    }
    if parsed.is_empty() {
        return "no stashes".to_string();
    }
    // **The branch is boilerplate only when it is the SAME everywhere.** Said
    // once then; kept per line otherwise — which stash belongs to which branch
    // decides where it can be applied. The subject is never cut: it was
    // clipped at 80 characters mid-word with no mark (measured 2026-10-02, a
    // one-stash list ending `— sauveg`), on a list capped at 20 anyway.
    let branches: std::collections::BTreeSet<&str> =
        parsed.iter().filter_map(|(_, k, _)| k.as_deref().and_then(branch_of)).collect();
    let shared = (branches.len() == 1 && parsed.iter().all(|(_, k, _)| k.is_some()))
        .then(|| branches.iter().next().copied())
        .flatten();
    let entries: Vec<String> = parsed
        .iter()
        .map(|(head, kind, subject)| match kind {
            Some(k) if shared.is_none() => format!("{head}  {k}: {subject}"),
            _ if subject.is_empty() => head.clone(),
            _ => format!("{head}  {subject}"),
        })
        .collect();
    let count = entries.len();
    let on = shared.map(|b| format!(", all on `{b}`")).unwrap_or_default();
    let mut out = format!("[tokenade:git-stash] {count} stash(es){on}\n\n");
    for e in entries.iter().take(20) {
        out.push_str(e);
        out.push('\n');
    }
    if entries.len() > 20 {
        let _ = write!(out, "\n… +{} more\n", entries.len() - 20);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // --- dispatch() arms ---

    #[test]
    fn dispatch_pop_just_trims() {
        // Status-style commands (pop/apply/drop/clear/push/save/create/store)
        // get minimal handling — strip ANSI + trim.
        let input = "\x1b[32mDropped refs/stash@{0}\x1b[0m\n";
        let out = dispatch(&argv(&["git", "stash", "pop"]), input);
        assert_eq!(out, "Dropped refs/stash@{0}");
    }

    #[test]
    fn dispatch_default_no_subcommand_lists() {
        // `git stash` (no subcommand) defaults to `list`.
        let input = "stash@{0}: WIP on main: abc1234 fix\nstash@{1}: WIP on dev: def5678 other\n";
        let out = dispatch(&argv(&["git", "stash"]), input);
        assert!(out.contains("stash@{0}"));
        assert!(out.contains("2 stash(es)"));
    }

    // --- compact() core path ---

    #[test]
    fn empty_input_returns_no_stashes() {
        assert_eq!(compact(""), "no stashes");
        assert_eq!(compact("\n\n"), "no stashes");
    }

    #[test]
    fn single_stash_keeps_index_hash_and_subject() {
        // Doc-fix #473: hash IS kept (not dropped). The original
        // comment said "drop the hash" but the impl always kept it —
        // and that's actually useful (agent can `git show <hash>`).
        let input = "stash@{0}: WIP on main: abc1234 Fix the bug\n";
        let out = compact(input);
        assert!(out.contains("stash@{0}"), "index lost: {out}");
        assert!(out.contains("abc1234"), "hash dropped — was supposed to be kept");
        assert!(out.contains("Fix the bug"), "subject lost: {out}");
        // The kind+branch boilerplate IS dropped — and the branch said once.
        assert!(!out.contains("WIP on main"), "kind+branch leaked: {out}");
        assert!(out.contains("all on `main`"), "{out}");
    }

    #[test]
    fn subject_with_internal_colons_preserved() {
        // splitn(2, ": ") on rest means only the FIRST ": " is the
        // separator; everything after is kept verbatim (including
        // colons in the subject like "Fix: missing import").
        let input = "stash@{0}: WIP on main: abc1234 Fix: missing import\n";
        let out = compact(input);
        assert!(out.contains("Fix: missing import"), "colon-in-subject lost: {out}");
    }

    #[test]
    fn count_and_cap_at_twenty() {
        let mut input = String::new();
        for i in 0..25 {
            let _ = writeln!(input, "stash@{{{i}}}: WIP on main: hash{i} msg {i}");
        }
        let out = compact(&input);
        assert!(out.contains("25 stash(es)"), "wrong count: {out}");
        let listed = out.lines().filter(|l| l.starts_with("stash@{")).count();
        assert_eq!(listed, 20, "cap at 20 listed, got {listed}");
        assert!(out.contains("+5 more"));
    }

    /// Two branches: each line keeps its own. A long message stays whole.
    #[test]
    fn the_branch_stays_when_it_changes_and_the_message_is_not_cut() {
        let long = "arbre main: travail non commité d'avant le merge llm-proxy (2026-08-22) — sauvegarde aussi dans /tmp/x/main-worktree-backup";
        let input = format!("stash@{{0}}: On main: {long}\nstash@{{1}}: WIP on feature-x: abc1234 essai\n");
        let out = compact(&input);
        assert!(out.contains(long), "{out}");
        assert!(out.contains("On main: arbre") && out.contains("WIP on feature-x: abc1234 essai"), "{out}");
        assert!(!out.contains("all on"), "{out}");
        // The measured shape: ONE stash, branch stated up front, message whole.
        let out = compact(&format!("stash@{{0}}: On main: {long}\n"));
        assert!(out.contains(long) && out.contains("all on `main`"), "{out}");
    }
}
