//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::generic;
use crate::compress::dedup_lines;
use crate::argv::{has_flag, has_any_flag};
use crate::ansi::strip_ansi;
use std::fmt::Write;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_flag(argv, "--show-current") {
        let s = strip_ansi(output);
        return s.trim().to_string();
    }
    if has_any_flag(argv, &["-d", "-D", "--delete"]) {
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
    compact(output)
}
pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut current: Option<String> = None;
    let mut locals: Vec<String> = Vec::new();
    let mut remotes: Vec<String> = Vec::new();
    let mut defaults: Vec<(String, String)> = Vec::new();
    for raw in s.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        // `* current` — single asterisk + space prefix.
        if let Some(rest) = line.strip_prefix("* ") {
            // Skip detached-HEAD descriptors like `* (HEAD detached at v1.0)`
            if rest.starts_with('(') {
                current = Some(rest.to_string());
            } else {
                current = Some(rest.trim().to_string());
            }
            continue;
        }
        let t = line.trim_start();
        if let Some(remote_name) = t.strip_prefix("remotes/") {
            // `origin/HEAD -> origin/main`: the remote's default branch,
            // kept as a mark on that branch's line, not as a line.
            if let Some((head, target)) = remote_name.split_once(" -> ") {
                if let Some(remote) = head.strip_suffix("/HEAD") {
                    defaults.push((remote.to_string(), target.to_string()));
                }
                continue;
            }
            remotes.push(remote_name.to_string());
        } else if !t.is_empty() {
            locals.push(t.to_string());
        }
    }
    // Drop remotes that mirror an existing local branch (avoids the
    // `origin/main` noise when `main` is already listed). The current
    // branch counts as a local too — caught by the #470 test fixture
    // `remote_that_mirrors_local_is_dropped`: `* main` + locals=[feature]
    // + remotes=[origin/main, origin/feature] used to leak `origin/main`
    // because the current branch wasn't in the dedup set.
    let mut local_set: std::collections::HashSet<String> = locals.iter().cloned().collect();
    if let Some(cur) = &current {
        // Skip detached-HEAD descriptors like `(HEAD detached at v1.0)`
        // — they're not branch names a remote could mirror.
        if !cur.starts_with('(') {
            local_set.insert(cur.clone());
        }
    }
    let remote_only: Vec<_> = remotes
        .iter()
        .filter(|r| {
            // Take the part after the first `/` (i.e. drop the remote
            // name like `origin/`) and compare against locals.
            let bare = r.split_once('/').map(|x| x.1).unwrap_or(r.as_str());
            !local_set.contains(bare)
        })
        .collect();
    // A mirror is not listed again, but WHERE the branch exists stays on
    // the local line: which branches are pushed is what `git branch -a` is
    // run for, and dropping the mirrors outright made `main` (on origin)
    // and a branch never pushed read the same.
    let on_remotes = |b: &str| -> String {
        let marks: Vec<String> = remotes
            .iter()
            .filter_map(|r| r.split_once('/').filter(|(_, bare)| *bare == b))
            .map(|(remote, _)| {
                let is_default = defaults
                    .iter()
                    .any(|(dr, target)| dr == remote && target.split_once('/').is_some_and(|(_, t)| t == b));
                if is_default {
                    format!("{remote}, default")
                } else {
                    remote.to_string()
                }
            })
            .collect();
        if marks.is_empty() {
            String::new()
        } else {
            format!("  [on {}]", marks.join("; "))
        }
    };
    let mut out = String::with_capacity(256);
    if let Some(cur) = &current {
        let _ = writeln!(out, "* {cur}{}", on_remotes(cur));
    }
    for b in &locals {
        // Don't repeat current.
        if Some(b) != current.as_ref() {
            let _ = writeln!(out, "  {b}{}", on_remotes(b));
        }
    }
    if !remote_only.is_empty() {
        let _ = write!(out, "\nremote-only ({}):\n", remote_only.len());
        for r in remote_only.iter().take(10) {
            let _ = writeln!(out, "  {r}");
        }
        if remote_only.len() > 10 {
            let _ = writeln!(out, "  … +{} more", remote_only.len() - 10);
        }
    }
    if out.is_empty() {
        // Same misrepresentation guard as git_status: if the input
        // had substantive lines but we didn't match any (e.g.
        // `git branch --show-current` outputs just `main\n`, no
        // `*` prefix), return generic instead of "no branches".
        let any_substantive = s.lines().any(|l| !l.trim().is_empty());
        if any_substantive {
            return generic(output);
        }
        return "no branches".to_string();
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    // --- dispatch() arms ---

    #[test]
    fn show_current_returns_trimmed_branch_name() {
        let out = dispatch(&argv(&["git", "branch", "--show-current"]), "main\n");
        assert_eq!(out, "main", "must trim trailing newline");
    }

    #[test]
    fn delete_arm_dedups_lines() {
        // `git branch -d X Y X` would not actually produce dup output,
        // but the arm uses dedup_lines so identical lines collapse.
        let input = "Deleted branch foo\nDeleted branch foo\nDeleted branch bar\n";
        let out = dispatch(&argv(&["git", "branch", "-d", "foo", "bar"]), input);
        // The dedup_lines helper folds adjacent duplicates — exact
        // collapse format is governed by the helper, just verify
        // duplicates don't both survive verbatim.
        let foo_occurrences = out.matches("Deleted branch foo").count();
        assert!(foo_occurrences < 2, "duplicates not collapsed: {out}");
        assert!(out.contains("Deleted branch bar"));
    }

    // --- compact() core path ---

    #[test]
    fn single_current_branch_with_star_prefix() {
        let input = "* main\n";
        let out = compact(input);
        assert_eq!(out, "* main");
    }

    #[test]
    fn current_plus_locals_keeps_current_first_no_dup() {
        let input = "* main\n  feature-a\n  feature-b\n";
        let out = compact(input);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "* main");
        // Locals are listed after, indented; current must not appear twice.
        assert!(out.contains("  feature-a"));
        assert!(out.contains("  feature-b"));
        assert_eq!(out.matches("main").count(), 1, "current must not dup");
    }

    /// Real `git branch -a` (2026-10-03): the mirrors are not repeated, but
    /// the local line says where its branch is pushed — an unpushed branch
    /// has no mark — and which one is the remote's default.
    #[test]
    fn a_local_line_says_which_remote_holds_its_branch() {
        let input = "  hardening/1.1.10-hostile-input\n* main\n  web-tree-rearch\n  \
                     remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  \
                     remotes/origin/web-tree-rearch\n";
        assert_eq!(
            compact(input),
            "* main  [on origin, default]\n  hardening/1.1.10-hostile-input\n  web-tree-rearch  [on origin]"
        );
    }

    #[test]
    fn remote_that_mirrors_local_is_dropped() {
        // `git branch -a` lists `remotes/origin/main` AND `main`.
        // The mirror should be filtered out.
        let input = "* main\n  feature\n  remotes/origin/main\n  remotes/origin/feature\n";
        let out = compact(input);
        // Neither `origin/main` nor `origin/feature` should appear — both mirror locals.
        assert!(
            !out.contains("origin/main"),
            "mirrored remote leaked: {out}"
        );
        assert!(
            !out.contains("origin/feature"),
            "mirrored remote leaked: {out}"
        );
    }

    #[test]
    fn remote_only_branches_get_their_own_section() {
        let input = "* main\n  remotes/origin/feature-x\n  remotes/origin/feature-y\n";
        let out = compact(input);
        assert!(out.contains("remote-only (2):"), "missing section: {out}");
        assert!(out.contains("origin/feature-x"));
        assert!(out.contains("origin/feature-y"));
    }

    #[test]
    fn head_pointer_line_skipped() {
        // `remotes/origin/HEAD -> origin/main` is git's symbolic
        // reference, not a real branch — drop it.
        let input = "* main\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/feature\n";
        let out = compact(input);
        assert!(!out.contains("HEAD"), "HEAD pointer leaked: {out}");
        assert!(out.contains("origin/feature"));
    }

    #[test]
    fn detached_head_preserved_as_current() {
        // `(HEAD detached at v1.0)` must be kept as the current
        // descriptor; otherwise the agent thinks no branch is checked out.
        let input = "* (HEAD detached at v1.0)\n  main\n";
        let out = compact(input);
        assert!(out.contains("(HEAD detached at v1.0)"));
        assert!(out.starts_with("* "), "must keep current marker");
    }

    #[test]
    fn remote_only_caps_at_10_with_more_count() {
        let mut input = String::from("* main\n");
        for i in 0..15 {
            let _ = writeln!(input, "  remotes/origin/feat-{i}");
        }
        let out = compact(&input);
        assert!(out.contains("remote-only (15):"));
        // Cap = 10 listed, "… +5 more"
        let listed = out.lines().filter(|l| l.contains("origin/feat-")).count();
        assert_eq!(listed, 10, "must cap at 10 listed; got {listed}");
        assert!(out.contains("+5 more"));
    }

    #[test]
    fn empty_input_returns_no_branches() {
        assert_eq!(compact(""), "no branches");
        assert_eq!(compact("\n\n  \n"), "no branches", "whitespace-only too");
    }

    #[test]
    fn substantive_unmatched_input_falls_back_to_generic() {
        // e.g. `main\n` without `*` prefix (one of the `--show-current`
        // edge cases noted inline in the source).
        let out = compact("main\n");
        // Generic just returns the input (possibly stripped/trimmed).
        // We only assert we DIDN'T misrepresent it as "no branches".
        assert_ne!(out, "no branches");
        assert!(out.contains("main"));
    }
}
