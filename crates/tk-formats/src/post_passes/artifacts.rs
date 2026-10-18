//! Build-artifact noise in a file listing.
//!
//! `target/`, `node_modules/`, `.venv/` and friends: thousands of paths an
//! agent never asked about, dropped only when the command was a listing and
//! the argv did not name the artifact directory itself.

use crate::argv::basename;

/// Command whose stdout is a plain file/path listing (so a dropped line is a
/// dropped path, never prose). `git` qualifies only for its listing subcommands.
pub(super) fn is_file_listing_cmd(argv: &[String]) -> bool {
    let prog = match argv.first() {
        Some(p) => basename(p),
        None => return false,
    };
    match prog.as_str() {
        "find" | "fd" | "fdfind" | "locate" | "mlocate" | "plocate" | "ls" | "tree" | "dir" => true,
        "git" => argv.iter().any(|a| a == "ls-files" || a == "ls-tree"),
        _ => false,
    }
}

/// True when an argument explicitly names a build-artifact pattern — the user
/// is hunting for exactly the paths #3 would otherwise hide, so passthrough.
pub(super) fn argv_targets_artifact(argv: &[String]) -> bool {
    const NEEDLES: [&str; 7] = [
        "__pycache__",
        ".pyc",
        ".pyo",
        ".DS_Store",
        "egg-info",
        "node_modules",
        ".git",
    ];
    argv.iter()
        .skip(1)
        .any(|a| NEEDLES.iter().any(|n| a.contains(n)))
}

/// A single listing line is pure build-artifact noise. Takes the last
/// whitespace-delimited token as the path candidate (covers `ls -l` columns
/// and `tree`'s drawing prefix as well as bare `find` lines).
pub(super) fn line_is_build_artifact(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return false;
    }
    let path = trimmed
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or(trimmed);
    is_build_artifact_path(path)
}

/// Always-noise predicate on a path: a compiled-Python file, a `.DS_Store`,
/// or any component that is `__pycache__`, an exact `.git` dir, or an
/// `*.egg-info` dir. Exact-component matching keeps `.gitignore`/`.github`
/// (and any real `.py`) safe.
pub(super) fn is_build_artifact_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return false;
    }
    if path.ends_with(".pyc") || path.ends_with(".pyo") {
        return true;
    }
    path.split('/').any(|seg| {
        matches!(seg, "__pycache__" | ".DS_Store" | ".git") || seg.ends_with(".egg-info")
    })
}
