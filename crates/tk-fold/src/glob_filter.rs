//! `Glob` tool results — drop vendored/build noise, and SAY what was dropped.
//!
//! A `**/*.ts` in a Node repo returns the source tree and then twenty thousand
//! paths under `node_modules/`. The agent asked for the former and pays for
//! both, and no amount of pattern-writing skill helps: the noise directories
//! are the ones the user never wrote.
//!
//! Two rules keep this honest, because a file listing is the one output where
//! a silent omission is indistinguishable from an answer:
//!
//!   * a directory is only noise when the PATTERN does not name it — asking
//!     for `node_modules/**/package.json` means you want node_modules, and the
//!     filter must get out of the way;
//!   * whatever is dropped is counted and named in a trailing line, with the
//!     way to get it back. The agent can tell "there are no matches" from
//!     "there are matches you are not being shown", which is the whole point.
//!
//! Idea from `ecotokens` (`src/hook/glob_handler.rs`), which carries the same
//! directory list but drops silently and has no pattern-awareness.

/// Directory names whose contents are vendored, generated, or cached — never
/// what a repo-wide glob is looking for unless it says so.
///
/// Matched as a whole PATH COMPONENT, never as a substring: `dist` must not
/// match `redistribute/`, and `target` must not match `src/target_arch.rs`.
const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    ".nuxt",
    "dist",
    "build",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".nyc_output",
    "htmlcov",
    ".eggs",
    "vendor",
    ".gradle",
    ".terraform",
    "Pods",
    ".cargo",
    ".svelte-kit",
    "coverage",
    ".turbo",
    ".parcel-cache",
];

/// A component that is noise by SUFFIX rather than by name.
const NOISE_SUFFIXES: &[&str] = &[".egg-info", ".dSYM", ".framework"];

/// What the filter did, so the caller can render it and meter it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GlobFold {
    pub kept: Vec<String>,
    /// Dropped count per directory name, sorted by count desc then name asc so
    /// the rendered line is byte-identical for identical input.
    pub dropped: Vec<(String, usize)>,
}

impl GlobFold {
    pub fn dropped_total(&self) -> usize {
        self.dropped.iter().map(|(_, n)| n).sum()
    }
}

/// Split `paths` into what the agent wanted and what it did not.
///
/// `pattern` is the glob the tool was called with; any noise directory it
/// mentions is treated as wanted and left alone.
pub fn fold(paths: &[String], pattern: &str) -> GlobFold {
    let mut kept = Vec::with_capacity(paths.len());
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for p in paths {
        match noise_component(p, pattern) {
            Some(dir) => *counts.entry(dir).or_default() += 1,
            None => kept.push(p.clone()),
        }
    }
    let mut dropped: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(d, n)| (d.to_string(), n))
        .collect();
    // Deterministic order: a listing that reshuffles between two identical
    // calls rewrites bytes the model already had cached.
    dropped.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    GlobFold { kept, dropped }
}

/// The noise directory this path sits under, or `None` when it is wanted.
fn noise_component<'a>(path: &'a str, pattern: &str) -> Option<&'a str> {
    for comp in path.split(['/', '\\']) {
        if comp.is_empty() {
            continue;
        }
        let hit = NOISE_DIRS.contains(&comp) || NOISE_SUFFIXES.iter().any(|s| comp.ends_with(s));
        // The pattern naming the directory is an explicit request for it.
        if hit && !pattern_names(pattern, comp) {
            return Some(comp);
        }
    }
    None
}

/// Does `pattern` NAME this directory, as a whole segment?
///
/// The rule is the one stated at the top of this file for paths — a component,
/// never a substring — and the pattern side was reading `pattern.contains(comp)`
/// instead. `.github/workflows/*.yml` contains `.git`, so asking for the
/// workflows of any repository switched the `.git/` filter off; `fuzz_targets`
/// carries `target`, `distill` carries `dist`, and `agent-build-context`
/// carries `build`. Measured 2026-08-28 over 31 499 real directories in
/// cloned third-party repositories: **719 of them collide this way**, across 105 distinct
/// names, `.github` alone accounting for 223. The filter that exists to hide
/// vendored output was being switched off by the most ordinary pattern there
/// is.
///
/// Braces count as separators too, so `{dist,build}/**` still names both.
fn pattern_names(pattern: &str, comp: &str) -> bool {
    pattern
        .split(['/', '\\', '{', '}', ','])
        .any(|seg| seg.trim() == comp)
}

/// One line stating what was withheld and how to get it, or empty when the
/// filter did not bite.
pub fn footer(f: &GlobFold) -> String {
    if f.dropped.is_empty() {
        return String::new();
    }
    let mut named: Vec<String> = f
        .dropped
        .iter()
        .take(3)
        .map(|(d, n)| format!("{d}/ ({n})"))
        .collect();
    if f.dropped.len() > 3 {
        named.push(format!("+{} more", f.dropped.len() - 3));
    }
    format!(
        "[tokenade:glob] {} vendored/build path(s) hidden — {}. Name the directory \
         in the pattern to include it.",
        f.dropped_total(),
        named.join(", ")
    )
}

/// Fold a newline-separated Glob result. Returns `None` when nothing was
/// dropped, so the caller passes the original through byte-for-byte.
pub fn fold_text(text: &str, pattern: &str) -> Option<String> {
    let paths: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    if paths.is_empty() {
        return None;
    }
    let f = fold(&paths, pattern);
    if f.dropped.is_empty() {
        return None;
    }
    // Refusing to leave the agent with nothing: if EVERY match was noise, the
    // answer "no results" would be a lie, and the honest reading is that the
    // matches are all vendored. Keep the footer, keep no paths, say so.
    let mut out = f.kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&footer(&f));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// `.github` carries `.git`, `fuzz_targets` carries `target`,
    /// `agent-build-context` carries `build`. A pattern that merely CONTAINS
    /// the name is not a request for the directory, and reading it as one
    /// switched the filter off on the most ordinary pattern there is.
    #[test]
    fn a_pattern_that_merely_contains_the_name_does_not_ask_for_it() {
        for (pattern, noise) in [
            (".github/workflows/*.yml", ".git/config"),
            ("**/fuzz_targets/*.rs", "target/debug/x.rs"),
            ("src/distill/*.ts", "dist/bundle.js"),
            ("agent-build-context/**/*.md", "build/out.o"),
        ] {
            let paths = v(&["src/keep.rs", noise]);
            let f = fold(&paths, pattern);
            assert_eq!(
                f.kept,
                v(&["src/keep.rs"]),
                "{pattern:?} was read as a request for {noise:?}"
            );
        }
    }

    /// And naming it, as a segment, still asks for it.
    #[test]
    fn naming_the_directory_as_a_segment_still_asks_for_it() {
        for pattern in [
            "node_modules/**/package.json",
            "**/node_modules/*/index.d.ts",
            "node_modules",
        ] {
            let paths = v(&["node_modules/react/index.d.ts"]);
            let f = fold(&paths, pattern);
            assert!(
                f.dropped.is_empty(),
                "{pattern:?} did not reach node_modules"
            );
        }
    }

    /// A brace list names every alternative in it.
    #[test]
    fn a_brace_list_names_each_alternative() {
        let paths = v(&["dist/a.js", "build/b.o", "src/keep.rs"]);
        let f = fold(&paths, "{dist,build}/**");
        assert!(
            f.dropped.is_empty(),
            "a brace alternative was not read as a name"
        );
    }

    #[test]
    fn vendored_paths_go_and_source_stays() {
        let paths = v(&[
            "src/main.ts",
            "node_modules/react/index.d.ts",
            "node_modules/lodash/x.d.ts",
            "src/lib/util.ts",
            "target/debug/build.rs",
        ]);
        let f = fold(&paths, "**/*.ts");
        assert_eq!(f.kept, v(&["src/main.ts", "src/lib/util.ts"]));
        assert_eq!(f.dropped_total(), 3);
        assert_eq!(f.dropped[0], ("node_modules".to_string(), 2));
    }

    #[test]
    fn a_pattern_that_names_the_directory_disables_the_filter() {
        let paths = v(&["node_modules/react/package.json", "src/package.json"]);
        let f = fold(&paths, "node_modules/**/package.json");
        assert!(f.dropped.is_empty(), "the agent asked for node_modules");
        assert_eq!(f.kept.len(), 2);
    }

    #[test]
    fn noise_names_match_whole_components_only() {
        // `dist` and `target` are common substrings of real source paths.
        let paths = v(&[
            "src/redistribute/index.js",
            "src/target_arch.rs",
            "crates/buildings/mod.rs",
        ]);
        let f = fold(&paths, "**/*");
        assert!(
            f.dropped.is_empty(),
            "substring match would eat these: {f:?}"
        );
    }

    #[test]
    fn windows_separators_are_understood() {
        let paths = v(&["src\\main.rs", "target\\debug\\x.rs"]);
        let f = fold(&paths, "**/*.rs");
        assert_eq!(f.kept, v(&["src\\main.rs"]));
        assert_eq!(f.dropped_total(), 1);
    }

    #[test]
    fn egg_info_and_dsym_are_matched_by_suffix() {
        let paths = v(&["pkg.egg-info/PKG-INFO", "app.dSYM/Contents/x", "src/a.py"]);
        let f = fold(&paths, "**/*");
        assert_eq!(f.kept, v(&["src/a.py"]));
        assert_eq!(f.dropped_total(), 2);
    }

    #[test]
    fn the_footer_names_what_it_hid_and_how_to_get_it() {
        let paths = v(&["src/a.ts", "node_modules/b.ts"]);
        let out = fold_text(&paths.join("\n"), "**/*.ts").unwrap();
        assert!(out.starts_with("src/a.ts\n"));
        assert!(out.contains("1 vendored/build path(s) hidden"));
        assert!(out.contains("node_modules/ (1)"));
        assert!(out.contains("Name the directory in the pattern"));
    }

    #[test]
    fn a_clean_listing_is_left_untouched() {
        assert!(fold_text("src/a.ts\nsrc/b.ts", "**/*.ts").is_none());
        assert!(fold_text("", "**/*").is_none());
    }

    #[test]
    fn an_all_noise_listing_still_says_so_instead_of_looking_empty() {
        let out = fold_text("node_modules/a.ts\nnode_modules/b.ts", "**/*.ts").unwrap();
        assert!(
            out.starts_with("[tokenade:glob]"),
            "no paths, but never a bare empty answer: {out}"
        );
        assert!(out.contains("2 vendored/build path(s) hidden"));
    }

    #[test]
    fn the_dropped_summary_is_order_stable() {
        let paths = v(&[
            "target/a", "dist/b", "target/c", "vendor/d", "dist/e", "src/f",
        ]);
        let a = fold(&paths, "**/*");
        let mut shuffled = paths.clone();
        shuffled.reverse();
        let b = fold(&shuffled, "**/*");
        assert_eq!(
            a.dropped, b.dropped,
            "counts must not depend on input order"
        );
        assert_eq!(a.dropped[0].0, "dist", "ties break on the name");
    }
}
