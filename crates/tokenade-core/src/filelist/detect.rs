//! Is this output a listing of files, and of which shape — `find`, `tree`,
//! `ls -laR`? Split out of `filelist.rs` when the file crossed the
//! 800-line guard: detection changes for its own reasons (a false tree, a
//! dependency tree), the rollup for others.

/// Does a box-drawn tree list PACKAGES rather than files?
///
/// `cargo tree`, `pnpm why`, `poetry show --tree` and `pipdeptree` draw the
/// same `├──` / `└──` characters as `tree(1)`, and the file-listing rollup
/// claimed all of them. What it then printed was not merely a worse fold, it
/// was untrue: measured 2026-09-04 on `cargo tree` in this repository, the
/// answer opened with "1034 file(s) across 2 directories" and an extension
/// histogram whose commonest entry was **`.1`, 13.7%** — the middle field of
/// the version numbers, read as a file extension.
///
/// The discriminator is the node label. A dependency tree's nodes carry a
/// version (`regex v1.12.3`, `serde 1.0.228`); a file tree's nodes are file
/// names, and a name whose first two dot-separated fields are both numbers is
/// rare enough that half of them being so means packages.
pub(super) fn looks_like_dependency_tree(head: &str) -> bool {
    let labels: Vec<&str> = head
        .lines()
        .take(200)
        .filter_map(|l| l.rsplit_once("── ").map(|(_, label)| label.trim()))
        .filter(|l| !l.is_empty())
        .collect();
    // No labels at all is not evidence of packages: a tree drawn without the
    // space after the branch (`├──src`) yields none, and `0 * 2 >= 0` would
    // otherwise call every one of them a dependency tree and cost the file
    // listing its fold.
    if labels.is_empty() {
        return false;
    }
    let versioned = labels.iter().filter(|l| carries_a_version(l)).count();
    versioned * 2 >= labels.len()
}

/// Is any whitespace-separated token of `label` a version number?
///
/// `v1.12.3`, `1.0.228` and npm's `@scope/pkg@1.2.0-beta.10` yes;
/// `libLLVM.so.21.1` no, because its first field is not a number — which is
/// what keeps a real file tree out. The three package managers an agent meets
/// spell the join differently and all three draw the same tree, so the token
/// is tried whole and again after its last `@`.
pub(super) fn carries_a_version(label: &str) -> bool {
    label.split_whitespace().any(|tok| {
        // After the LAST `@`, or the whole token when there is none — the two
        // are the same string for a token without one, so testing the token
        // as well is a condition no fixture can distinguish.
        is_version(tok.rsplit_once('@').map(|(_, v)| v).unwrap_or(tok))
    })
}

pub(super) fn is_version(tok: &str) -> bool {
    {
        let tok = tok.strip_prefix('v').unwrap_or(tok);
        let mut fields = tok.split('.');
        let (Some(major), Some(minor)) = (fields.next(), fields.next()) else {
            return false;
        };
        !major.is_empty()
            && major.bytes().all(|b| b.is_ascii_digit())
            && minor.bytes().next().is_some_and(|b| b.is_ascii_digit())
    }
}

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);

    // Strong signal #1: tree-drawing characters — unless the nodes are
    // packages rather than files.
    if is_tree_drawing(head) {
        return !looks_like_dependency_tree(head);
    }

    // Strong signal #2: `ls -laR` shape — `total N` line + permission strings.
    let has_total = head.contains("\ntotal ") || head.starts_with("total ");
    // The `total ` + permissions combo used to fire on a plain `ls -la`
    // on a small dir (3 entries) — the compactor then dropped most of
    // the info as "0 file(s) in 0 dirs" (regression caught 2026-05-28
    // via debug log). Real `ls -laR` runs on a project always produce
    // ≥20 permission lines OR multiple `total ` lines (one per dir);
    // we require one of those to keep the signal honest.
    let perm_count = head.matches("\n-rw").count()
        + head.matches("\ndrwx").count()
        + head.matches("\nlrwx").count()
        + usize::from(head.starts_with("-rw"))
        + usize::from(head.starts_with("drwx"));
    let total_count = head.matches("\ntotal ").count() + usize::from(head.starts_with("total "));
    if has_total && (total_count >= 2 || perm_count >= 20) {
        return true;
    }
    if is_named_long_listing(head) {
        return true;
    }

    // Moderate signal #3: `find` paths-per-line shape. Require ≥10 lines that
    // look like paths AND those make up ≥75 % of non-blank head lines (so a
    // build log with a few paths doesn't false-trigger).
    let mut path_lines = 0usize;
    let mut total_lines = 0usize;
    for line in head.lines().take(60) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        total_lines += 1;
        if looks_like_path(t) {
            path_lines += 1;
        }
    }
    path_lines >= 10 && total_lines > 0 && (path_lines * 100) / total_lines >= 75
}

/// A line is "path-like" if it starts with `./` or `/` and contains no
/// whitespace inside. Accepts single-component paths (`./Makefile`,
/// `./Dockerfile`) as well as deep paths.
pub(super) fn looks_like_path(s: &str) -> bool {
    if !(s.starts_with("./") || s.starts_with('/') || is_windows_path(s)) {
        return false;
    }
    // No internal whitespace — find output never has spaces unless the path
    // itself contains them; in that case `find -print` adds no protection
    // so the line genuinely has multiple whitespace-separated chunks, which
    // we can't disambiguate from prose without a real path-resolution check.
    if s.contains([' ', '\t']) {
        return false;
    }
    // Must have at least one char after the `./` or `/` prefix.
    s.len() > 2
}

/// `C:\\Windows\\x.mui`, `.\\src\\a.rs`: a Windows path. A real 300-path
/// `Get-ChildItem -Recurse … | Select-Object FullName` (2026-10-05) was not
/// recognised as a listing and went to the stride-sampler: one path in two
/// dropped, "158 lines elided … in 79 runs of 2".
pub(crate) fn is_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        || s.starts_with(".\\")
}

/// A listing line in a text ALREADY recognised as a path list: a path may
/// hold a space there. Measured 2026-10-03, `mdfind -onlyin /System/Library
/// -name .plist` (10 826 paths): the 53 with a space (`Active Directory.plist`,
/// `Address Book Plug-Ins/…`) were left out of the rollup's count and printed
/// raw under its "no individual path is listed below". Detection keeps the
/// strict [`looks_like_path`] — prose must not make a text a listing — and
/// what reads as a message (`/x: Permission denied`, a `grep` hit, an
/// `ls -l` arrow, a `dir:` header) is still not a path.
pub(super) fn listing_line_is_path(s: &str) -> bool {
    looks_like_path(s)
        || ((s.starts_with("./") || s.starts_with('/'))
            && s.len() > 2
            && !s.contains('\t')
            && !s.contains(": ")
            && !s.contains(" -> ")
            && !s.ends_with(':'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Find,
    Tree,
    LsLaR,
}

/// **A text that MENTIONS a tree is not a tree.**
///
/// The signal was one `├──` anywhere in the first 16 KB. Measured 2026-10-02:
/// `awk 'NR>=469 && NR<=574' filelist.rs | nl -ba` — a hundred lines of Rust,
/// two of which spell `"── "` in a string literal — came back as `tree, 2
/// file(s) across 1 directory — a rollup`, the two lines dropped as "files".
/// A README carrying a project layout in a code block is the same shape. What
/// `tree(1)` prints is drawing on nearly every line: the root, then one node
/// per line, then the `N directories, M files` count. So: at least five drawn
/// lines, and drawn lines making most of the non-blank ones.
pub(super) fn is_tree_drawing(head: &str) -> bool {
    let mut drawn = 0usize;
    let mut non_blank = 0usize;
    for line in head.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_blank += 1;
        if t.contains("├──") || t.contains("└──") || t.starts_with('│') {
            drawn += 1;
        }
    }
    drawn >= 5 && drawn * 10 >= non_blank * 6
}

pub(super) fn detect_shape(text: &str) -> Shape {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    if is_tree_drawing(head) {
        return Shape::Tree;
    }
    let has_total = head.contains("\ntotal ") || head.starts_with("total ");
    let has_perms = head.contains("\n-rw")
        || head.contains("\ndrwx")
        || head.starts_with("-rw")
        || head.starts_with("drwx");
    if (has_total && has_perms) || is_named_long_listing(head) {
        return Shape::LsLaR;
    }
    Shape::Find
}

/// `ls -l a b c …` — files NAMED on the command line — prints no `total`
/// line, so the `total`-gated tests never saw it. The tar detector used to
/// take it (and cut the size and date into the name); once it learned that a
/// link count signs `ls`, nothing took it and 400 lines went through raw
/// (2026-10-03, compat bench `telemetry`: no fold, so no event to send).
/// Read by [`detect`] AND [`detect_shape`]: the first fix taught only
/// `detect`, and the rollup, asking the shape, still answered `Find` and
/// handed the text back unchanged.
fn is_named_long_listing(head: &str) -> bool {
    let nonempty = head.lines().filter(|l| !l.trim().is_empty()).count();
    let long = head
        .lines()
        .filter(|l| crate::archive::is_ls_long_line(l))
        .count();
    long >= 20 && long * 10 >= nonempty * 9
}
