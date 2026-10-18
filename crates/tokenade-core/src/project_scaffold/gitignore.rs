//! **Adding a line to someone else's `.gitignore`.**
//!
//! Without it, tokenade's local state ends up committed — and a gains
//! registry shared in a company repo is a leak. The line is added only once,
//! never twice, and never in a repo that already carries it in another form.

use super::*;

/// Append `.tokenade/` to the global gitignore at
/// `~/.config/git/ignore` (XDG default since git 1.7.6) so EVERY
/// repo on this machine ignores tokenade's per-project cache,
/// including repos that haven't been MCP-initialized yet.
///
/// Returns true on touch, false on no-op (already present, no HOME,
/// write failed). Creates the parent dir as needed — git auto-reads
/// `~/.config/git/ignore` without further configuration on any git
/// install that respects XDG (everything since 2011).
pub fn ensure_global_gitignored() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let global = PathBuf::from(&home)
        .join(".config")
        .join("git")
        .join("ignore");
    let existing = std::fs::read_to_string(&global).unwrap_or_default();
    // Two paths to cover: `.tokenade/` (per-project cache, always
    // gitignored) AND `.fastembed_cache/` (legacy per-project model
    // dir — pre-2026-05-29 installs created these. Even though we now
    // pin the cache globally, an OLD project might still have one
    // before its first MCP session triggers the migration).
    let tokenade_needles = [".tokenade", ".tokenade/", "/.tokenade", "/.tokenade/"];
    // **The backup copies dirtied the user's `git status`.**
    // `paths::backup_once` writes `<file>.tokenade-original` NEXT TO the file
    // it saves — including `.mcp.json` and the rules files, which live in the
    // repo. Reported by a tester on 2026-09-11, in these words:
    // "it creates a lot of files, lots of changes, not ignored
    // (.mcp.json.tokenade-original)". The copy keeps its reason to exist — it
    // is what makes the install reversible — but it has no business in what
    // the user is about to commit.
    //
    // Written HERE, in the GLOBAL excludes, and not in the project's
    // `.gitignore`: that one is a TRACKED file, so adding to it would be one
    // more change to commit — the cure would be the disease. The global
    // excludes cover every repo and show up in none.
    let sidecar_needles = ["*.tokenade-original", "*.tokenade-backup-*"];
    let fastembed_needles = [
        ".fastembed_cache",
        ".fastembed_cache/",
        "/.fastembed_cache",
        "/.fastembed_cache/",
    ];
    let has_tokenade = existing
        .lines()
        .any(|l| tokenade_needles.contains(&l.trim()));
    let has_fastembed = existing
        .lines()
        .any(|l| fastembed_needles.contains(&l.trim()));
    let has_sidecars = sidecar_needles
        .iter()
        .all(|n| existing.lines().any(|l| l.trim() == *n));
    if has_tokenade && has_fastembed && has_sidecars {
        return false;
    }
    if let Some(parent) = global.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let sep = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let mut snippet = String::from(sep);
    if !has_tokenade {
        snippet.push_str(
            "\n# tokenade — per-project cache / scaffold flag / dedup refs (global)\n.tokenade/\n",
        );
    }
    if !has_fastembed {
        snippet.push_str("\n# tokenade — legacy per-project embedding model cache (now global)\n.fastembed_cache/\n");
    }
    if !has_sidecars {
        snippet.push_str("\n# tokenade — original copies kept so the install can be reverted\n");
        for n in sidecar_needles {
            snippet.push_str(n);
            snippet.push('\n');
        }
    }
    let new_content = format!("{existing}{snippet}");
    std::fs::write(&global, new_content).is_ok()
}

/// Append `.tokenade/` to the project's `.gitignore` so the cache /
/// scaffold-flag don't pollute commits. Returns true if we touched
/// the file (added the entry); false on no-op (already present, or
/// not a git repo, or write failed).
///
/// SAFE: never overwrites the user's existing rules. Just appends
/// our line at the end, on its own line, with a comment so a
/// reader can grep our origin marker.
pub fn ensure_gitignored(root: &Path) -> bool {
    // Only act inside a git repo — touching .gitignore in a
    // non-git directory is intrusive (the file would never be
    // consulted).
    if !root.join(".git").exists() {
        return false;
    }
    let gitignore = root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    // Detect existing entries. Accept any of the common forms. We
    // cover .tokenade/ (always gitignored) AND .fastembed_cache/
    // (legacy per-project; even after migration, old commits may
    // still have it so the rule belongs here).
    let tokenade_needles = [".tokenade", ".tokenade/", "/.tokenade", "/.tokenade/"];
    let fastembed_needles = [
        ".fastembed_cache",
        ".fastembed_cache/",
        "/.fastembed_cache",
        "/.fastembed_cache/",
    ];
    let has_tokenade = existing
        .lines()
        .any(|l| tokenade_needles.contains(&l.trim()));
    let has_fastembed = existing
        .lines()
        .any(|l| fastembed_needles.contains(&l.trim()));
    if has_tokenade && has_fastembed {
        return false;
    }
    let sep = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let mut snippet = String::from(sep);
    if !has_tokenade {
        snippet.push_str(
            "\n# tokenade — per-project cache / scaffold flag / dedup refs\n.tokenade/\n",
        );
    }
    if !has_fastembed {
        snippet.push_str("\n# tokenade — legacy per-project embedding model cache (now global)\n.fastembed_cache/\n");
    }
    let new_content = format!("{existing}{snippet}");
    // Repo-scoped: `.gitignore` is a file the checked-out repository controls,
    // so it can arrive as a symlink pointing anywhere. Same class as the
    // scaffold write — refuse rather than append through it.
    crate::paths::write_atomic_repo_scoped(root, &gitignore, &new_content).is_ok()
}
