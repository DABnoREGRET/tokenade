//! A compactor that builds its own output must say when it cuts.
//!
//! 845 of the 947 command modules delegate their rendering to a shared helper
//! — an archetype (`fold_middle`, `keep_head_and_say`) or a family helper
//! (`git_listing::fold`, `docker_table::fold_table`) — and those declare by
//! construction: change the helper and every caller changes with it.
//!
//! **102 build their whole output on their own**, and nothing shared protects
//! them: `last`, `ls_table`, `fuser`, `getfacl`, `dpkg_list`, `ss`, `nmap`,
//! `traceroute`, `grep`, `arp` and the rest. MEASURED 2026-09-06, all 102
//! satisfy the rule today — this guard exists to keep it that way, because the
//! sweep that found them was written only after two of their neighbours had
//! already shipped a silent cut (`git blame` dropping author and date,
//! `cargo install` dropping the line that explains a failure).
//!
//! Why a source sweep rather than calling each module: the dispatch is spread
//! across per-family wrappers in `filter/*.rs`, so there is no registry to
//! iterate. Building one is a refactor; this is a ratchet.
//!
//! The predicate had to be corrected twice while being written, and both
//! corrections are load-bearing:
//!   * `…` and `...` ARE declarations. A sweep that only recognised words
//!     accused `serp.rs`, whose `truncate` appends an ellipsis.
//!   * a cut inside a DETECTOR delivers nothing. `serp.rs` reads 200_000
//!     characters to look for an anti-bot phrase; that is not a fold.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// The body of the module's compaction entry point, `#[cfg(test)]` stripped.
///
/// Stripped by the ONE declarer, `srcsweep::production_lines`: splitting at the
/// first `#[cfg(test)]` hid everything below it, and a module whose entry point
/// sits after an interleaved test block simply left this guard.
fn compact_body(src: &str) -> Option<String> {
    let prod: String = tokenade_core::srcsweep::production_lines(src)
        .into_iter()
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n");
    let prod = prod.as_str();
    let at = prod
        .find("fn try_compact")
        .or_else(|| prod.find("pub fn compact"))?;
    let open = prod[at..].find('{')? + at;
    let mut depth = 0usize;
    for (i, c) in prod[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(prod[open + 1..open + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Does the body hand off to another module? Those inherit that module's
/// honesty and are not this guard's business.
fn delegates(body: &str) -> bool {
    for kw in ["super::", "crate::", "self::"] {
        let mut from = 0;
        while let Some(i) = body[from..].find(kw) {
            let start = from + i + kw.len();
            let rest = &body[start..];
            let path_len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
                .unwrap_or(rest.len());
            if rest[path_len..].starts_with('(') {
                return true;
            }
            from = start;
        }
    }
    false
}

const CUTS: &[&str] = &[
    ".take(",
    ".truncate(",
    "keep_rows",
    "MAX_ROWS",
    "MAX_LINES",
    "MAX_ENTRIES",
];

/// Every spelling of "and it said so", including the two that a word-only
/// predicate misses.
const DECLARES: &[&str] = &[
    "[tokenade:",
    "[disclose:",
    "elided",
    "not shown",
    "hidden",
    "folded",
    "withheld",
    "omitted",
    "truncated",
    "…",
    "...",
];

fn cuts(body: &str) -> bool {
    CUTS.iter().any(|p| body.contains(p))
}

fn declares(body: &str) -> bool {
    DECLARES.iter().any(|p| body.contains(p))
}

/// The rule on source that is not in the tree, so weakening it shows up here
/// even when every module is clean.
#[test]
fn the_rule_reads_delegation_ellipsis_and_a_bare_cut() {
    let bare = "let out: Vec<_> = lines.take(20).collect(); out.join(\"\\n\")";
    assert!(cuts(bare) && !declares(bare), "a bare cut must be reported");

    let with_ellipsis = "let out: String = s.chars().take(80).collect(); out.push('…'); out";
    assert!(
        cuts(with_ellipsis) && declares(with_ellipsis),
        "an ellipsis IS the declaration"
    );

    let with_banner =
        "let out = lines.take(20); format!(\"[tokenade:x] {n} line(s) elided\\n{out}\")";
    assert!(declares(with_banner));

    assert!(delegates(
        "super::git_listing::fold(text, 20, \"a\", \"b\")"
    ));
    assert!(delegates(
        "crate::cmd::linux::cargo::build::compact(output)"
    ));
    assert!(
        !delegates("let n = super::HEAD; text.lines().take(n).collect::<Vec<_>>()"),
        "naming a constant from another module is not handing the work to it"
    );
    assert!(!delegates("text.lines().take(20).collect::<Vec<_>>()"));
}

#[test]
fn every_hand_rolled_compactor_that_cuts_says_so() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates/tokenade-core/src/cmd"), &mut files);
    assert!(
        files.len() > 500,
        "the sweep found only {} sources — it stopped seeing the tree it guards",
        files.len()
    );

    let mut hand_rolled = 0usize;
    let mut offenders = Vec::new();
    for path in &files {
        if path.file_name().and_then(|n| n.to_str()) == Some("archetypes.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(body) = compact_body(&src) else {
            continue;
        };
        if delegates(&body) {
            continue;
        }
        hand_rolled += 1;
        if cuts(&body) && !declares(&body) {
            offenders.push(
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }

    // Anti-vacuity: if delegation detection ever swallows the population, the
    // guard would pass by having nothing left to check.
    assert!(
        hand_rolled >= 60,
        "only {hand_rolled} hand-rolled compactor(s) found; 102 were measured on 2026-09-06, \
         so the delegation predicate has gone too wide and this guard now checks almost nothing"
    );
    assert!(
        offenders.is_empty(),
        "a compactor that builds its own output cut without saying so — add the count and the \
         way back, the way `warnings (N)` and `[tokenade:git-blame]` do:\n  {}",
        offenders.join("\n  ")
    );
}
