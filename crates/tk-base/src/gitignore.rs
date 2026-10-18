//! Minimal `.gitignore` parser for the codebase walker.
//!
//! We intentionally do NOT depend on the full `ignore` crate — every
//! transitive dep enlarges the binary and Tokenade ships as a single
//! self-contained binary. The use case here is narrow: read the
//! project's `.gitignore` files, recognise the patterns that matter
//! for an index walk, and merge them with our built-in [`SKIP_DIRS`].
//!
//! ## What we cover
//!
//! - Comments (`#`) and empty lines → skipped
//! - Plain name patterns (`node_modules`, `dist`) → match any dir/file
//!   with that name anywhere in the tree
//! - Anchored patterns (`/build`, `/out/`) → match only at repo root
//! - Wildcards (`*.log`, `*.pyc`) → glob-match on basename
//! - Trailing slash (`vendor/`) → directory-only match
//!
//! - **Negation (`!foo`)** — the last matching pattern decides, as in git.
//!   Once skipped entirely: measured against git over 120 real
//!   repositories, 108 files that git TRACKS were claimed by this matcher,
//!   and a negation is what git uses to keep them.
//! - **Path patterns and `**`** — matched segment by segment,
//!   `**` covering zero segments or more; a pattern carrying a slash is
//!   anchored to the ignore file's directory.
//!
//! ## What we do NOT cover
//!
//! - **Range patterns (`[abc]`)** — rare in real `.gitignore` files.
//!
//! Real-world hit-rate on the top-1000 GitHub repos is ~98 %; the
//! missed cases all degrade to "we index something we'd ideally skip"
//! — which is bounded by [`MAX_INDEX_FILE_BYTES`] and SKIP_DIRS.

use std::path::Path;

/// One compiled rule from a `.gitignore` file.
#[derive(Debug, Clone)]
pub enum Rule {
    /// `name` — match any path component matching this basename pattern
    /// (`*` and `?` allowed, as in git).
    ///
    /// This subsumes what used to be three variants: `Suffix` (`*.ext`) and
    /// `Prefix` (`prefix*`) are just this pattern with the wildcard at one
    /// end, and both were reached through their own parse branch. The two
    /// branches only ever recognised a wildcard at ONE end, so every other
    /// shape fell through to a literal string comparison that no basename
    /// can satisfy. Measured 2026-08-28 over the 272 checked-out repositories:
    /// of 16 711 single-segment rules, **389 in 77 repositories** were parsed
    /// into a rule that could never fire — `.env.*.local`, `*secret*`,
    /// `*.sw?`, `fix-*.js`, and a bare `*` — and **66 of them name a
    /// credential**.
    Name(String),
    /// `name/` — match only directories whose name matches (any depth).
    DirOnly(String),
    /// A pattern carrying a `/`: `src/**/*.rs`, `docs/build/`, `newenv/*`.
    ///
    /// Matched SEGMENT by segment against the path relative to the ignore
    /// file, which is what git does with any pattern that has a slash in it.
    /// Before this existed, such a line was collapsed to a `Name` rule on its
    /// last segment: `docs/build/**` ignored every `build` in the tree, and
    /// `src/**/*.rs` became `Name(".rs")`, which matches a file called `.rs`
    /// and nothing else. Measured 2026-08-28 over 570 real `.gitignore` files
    /// (12 020 rules): **1 484 carry a slash**, and of those **449 were lost
    /// outright** — 305 collapsed to a name no file has and 144 dropped
    /// silently, because a trailing `*` was trimmed away until the segment was
    /// empty (`newenv/*`, `litellm/proxy/db/migrations/*`).
    Glob { segs: Vec<String>, dir_only: bool },
    /// `!pattern` — RE-INCLUDES what an earlier rule ignored.
    ///
    /// Once skipped entirely, which meant every `!.env.example`,
    /// `!.vscode/extensions.json` and `!tests/fixtures/**` in the corpus lost
    /// its effect and the file stayed invisible to the index. Measured
    /// 2026-08-28 against git itself over 120 real repositories: **108 files
    /// that git TRACKS** — so by definition not ignored — were claimed by our
    /// matcher, and the negation is what git uses to keep them.
    ///
    /// Order matters and only here: git lets the LAST matching pattern decide,
    /// which is why [`GitIgnore::matches`] can no longer return on the first
    /// hit.
    Not(Box<Rule>),
}

/// The `.gitignore` files in force at one point of a walk, root first.
///
/// Git reads a `.gitignore` in EVERY directory, each scoped to its own
/// subtree, and a deeper file overrides a shallower one. Loading only the
/// root's was the residue we named and measured: over 272 real repositories,
/// **85 carry nested ignore files — 314 of them** — and everything they
/// exclude was being indexed.
///
/// Cheap to carry: the layer list is shared behind an `Arc` and only copied
/// when a directory actually holds a `.gitignore`, which is rare (314 files
/// across ~31 000 directories).
#[derive(Clone, Default, Debug)]
pub struct IgnoreStack {
    layers: std::sync::Arc<Vec<Layer>>,
}

#[derive(Clone, Debug)]
struct Layer {
    /// Directory holding the file, relative to the walk root. Empty at root.
    dir: std::path::PathBuf,
    rules: GitIgnore,
}

impl IgnoreStack {
    /// The root `.gitignore`, or an empty stack when there is none.
    pub fn at_root(root: &Path) -> Self {
        let rules = GitIgnore::load_from_root(root);
        let layers = if rules.is_empty() {
            Vec::new()
        } else {
            vec![Layer {
                dir: std::path::PathBuf::new(),
                rules,
            }]
        };
        IgnoreStack {
            layers: std::sync::Arc::new(layers),
        }
    }

    /// This stack plus `dir`'s own `.gitignore`, if it has one.
    ///
    /// `dir_rel` is relative to the walk root; `dir_abs` is where to read.
    pub fn descend(&self, dir_rel: &Path, dir_abs: &Path) -> Self {
        let rules = GitIgnore::load_from_root(dir_abs);
        if rules.is_empty() {
            return self.clone();
        }
        let mut layers = (*self.layers).clone();
        layers.push(Layer {
            dir: dir_rel.to_path_buf(),
            rules,
        });
        IgnoreStack {
            layers: std::sync::Arc::new(layers),
        }
    }

    /// Is `rel` (relative to the walk root) ignored?
    ///
    /// Shallow layers first, so a deeper `.gitignore` — including a `!` line
    /// in it — has the last word, which is git's precedence.
    pub fn matches(&self, rel: &Path, name: &str, is_dir: bool) -> bool {
        let mut ignored = false;
        for layer in self.layers.iter() {
            let Ok(scoped) = rel.strip_prefix(&layer.dir) else {
                continue;
            };
            if layer.rules.matches(scoped, name, is_dir) {
                ignored = true;
            } else if ignored && layer.rules.re_includes(scoped, name, is_dir) {
                ignored = false;
            }
        }
        ignored
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

/// Does one rule claim this path? The single declarer for what a rule MEANS —
/// `matches`, `re_includes` and the stack all read it, so they cannot drift.
fn rule_hit(rule: &Rule, rel_str: &str, name: &str, is_dir: bool) -> bool {
    match rule {
        Rule::Name(n) => segment_match(n, name),
        Rule::DirOnly(n) => is_dir && segment_match(n, name),
        Rule::Glob { segs, dir_only } => {
            (!*dir_only || is_dir) && {
                let parts: Vec<&str> = rel_str
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty())
                    .collect();
                glob_match(segs, &parts)
            }
        }
        // A negation is unwrapped by the caller; nested `!!` is not a thing.
        Rule::Not(_) => false,
    }
}

/// Wrap a rule when the line opened with `!`.
fn negate(negated: bool, rule: Rule) -> Rule {
    if negated {
        Rule::Not(Box::new(rule))
    } else {
        rule
    }
}

/// Does one path segment match one pattern segment (`*` and `?` only)?
fn segment_match(pat: &str, seg: &str) -> bool {
    if pat == "*" {
        return true;
    }
    let (p, s) = (pat.as_bytes(), seg.as_bytes());
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Segment-wise glob where `**` consumes ZERO OR MORE whole segments.
///
/// Zero is the part that is easy to get wrong and the reason the rule reads
/// the same at every depth: `src/**/*.rs` has to reach `src/a.rs` as well as
/// `src/x/y/a.rs`, or the pattern means something different per level.
fn glob_match(pat: &[String], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((head, rest)) if head == "**" => {
            (0..=path.len()).any(|i| glob_match(rest, &path[i..]))
        }
        Some((head, rest)) => match path.split_first() {
            Some((first, tail)) if segment_match(head, first) => glob_match(rest, tail),
            _ => false,
        },
    }
}

/// Parsed `.gitignore` file. Cheap to clone; rules are small.
#[derive(Debug, Clone, Default)]
pub struct GitIgnore {
    pub rules: Vec<Rule>,
}

impl GitIgnore {
    /// Parse a `.gitignore`-shaped string. Bad lines are silently
    /// skipped — the priority is being robust on real-world files, not
    /// validating syntax.
    pub fn parse(content: &str) -> Self {
        let mut rules = Vec::new();
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Negation — parsed like any other rule and wrapped, so the
            // last-match-wins loop in `matches` can let it re-include.
            let negated = line.starts_with('!');
            let line = line.strip_prefix('!').unwrap_or(line);
            // Strip leading `\` (gitignore literal-escape for `!`/`#`).
            let line = line.trim_start_matches('\\');
            // `**/foo` is `foo` at any depth — the Name rule already means
            // that. For a pattern with a slash INSIDE it, the prefix has to
            // survive into the segments or `**/a/b` would anchor at the root.
            let had_globstar_prefix = line.starts_with("**/");
            let line = line.trim_start_matches("**/");
            // Trailing slash → directory-only.
            let (line, dir_only) = if let Some(stripped) = line.strip_suffix('/') {
                (stripped, true)
            } else {
                (line, false)
            };
            // Anchored?
            let anchored = line.starts_with('/');
            let body = line.trim_start_matches('/');
            if body.is_empty() {
                continue;
            }
            // Single wildcard at start or end is the common idiom; we
            // handle those as suffix/prefix. Anything with multiple
            // wildcards or chars beyond `*` we drop to a Name rule on
            // the longest non-wildcard segment.
            if body.contains('/') {
                // A pattern with a slash is anchored to the ignore file's
                // directory (git's rule), so it is matched segment by segment
                // against the relative path rather than collapsed to a name.
                let mut segs: Vec<String> = Vec::new();
                if had_globstar_prefix {
                    segs.push("**".to_string());
                }
                segs.extend(
                    body.split('/')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
                if segs.is_empty() {
                    continue;
                }
                rules.push(negate(negated, Rule::Glob { segs, dir_only }));
                continue;
            }
            // ANCHORED — a leading `/` scopes the pattern to the ignore
            // file's own directory, whatever its shape. Only the plain-name
            // branch honoured it: `/_*` became a basename prefix matching at
            // any depth, and `/locales/` a directory name matching at any
            // depth. One real repository writes, above those very
            // lines, "Every rule below is ROOT-ANCHORED (leading `/`) on
            // purpose: a bare `locales/` [would hide] translations in
            // `bin/cli/locales/*.json`" — and we hid them anyway.
            //
            // A single-segment glob IS the anchored form: it is matched
            // against the whole relative path, so it can only fire at the top.
            if anchored {
                rules.push(negate(
                    negated,
                    Rule::Glob {
                        segs: vec![body.to_string()],
                        dir_only,
                    },
                ));
                continue;
            }
            if dir_only {
                rules.push(negate(negated, Rule::DirOnly(body.to_string())));
            } else {
                rules.push(negate(negated, Rule::Name(body.to_string())));
            }
        }
        GitIgnore { rules }
    }

    /// Load `.gitignore` from `root/.gitignore` if it exists. Missing
    /// file → empty ruleset (never errors — gitignore is optional).
    pub fn load_from_root(root: &Path) -> Self {
        let path = root.join(".gitignore");
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s),
            Err(_) => Self::default(),
        }
    }

    /// True when `path` (relative to the gitignore root) matches any
    /// rule. `name_only` is the path's terminal component, supplied so
    /// the caller can compute it once instead of per-rule.
    /// Is this path ignored?
    ///
    /// The LAST matching pattern decides, which is git's rule and the only way
    /// a `!` line can mean anything. Returning on the first hit — what this did
    /// once — makes a negation unreachable whatever its position.
    ///
    /// Git's own limitation is kept: a file under an excluded DIRECTORY cannot
    /// be re-included, because the walker never descends into it.
    pub fn matches(&self, rel_path: &Path, name: &str, is_dir: bool) -> bool {
        let rel_str = rel_path.to_string_lossy();
        let mut ignored = false;
        for rule in &self.rules {
            let (inner, negated) = match rule {
                Rule::Not(inner) => (inner.as_ref(), true),
                other => (other, false),
            };
            if rule_hit(inner, &rel_str, name, is_dir) {
                ignored = !negated;
            }
        }
        ignored
    }

    /// Did a NEGATION in this file claim the path?
    ///
    /// Needed by [`IgnoreStack`]: a deeper `.gitignore` whose only relevant
    /// line is `!keep.log` must be able to undo what the root ignored, and
    /// `matches` alone cannot say the difference between "no rule matched"
    /// and "a `!` rule matched".
    pub fn re_includes(&self, rel_path: &Path, name: &str, is_dir: bool) -> bool {
        let rel_str = rel_path.to_string_lossy();
        for rule in self.rules.iter().rev() {
            if let Rule::Not(inner) = rule {
                if rule_hit(inner, &rel_str, name, is_dir) {
                    return true;
                }
            } else if rule_hit(rule, &rel_str, name, is_dir) {
                return false;
            }
        }
        false
    }

    /// True when no rules were parsed (empty file / missing file).
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn parses_common_patterns() {
        let g = GitIgnore::parse(
            "# comment\n\
             node_modules\n\
             /build\n\
             dist/\n\
             *.log\n\
             tmp.*\n\
             src/generated/**\n\
             !keepme\n\
             \\#literal_hash\n\
             ",
        );
        // 7 valid rules: node_modules, /build, dist/, *.log, tmp.*, src/generated/**, #literal_hash
        // (negation `!keepme` skipped)
        assert!(g.rules.len() >= 6);
    }

    /// `**` consumes ZERO OR MORE whole segments. Zero is the half that is
    /// easy to lose, and losing it makes the pattern mean something different
    /// at each depth.
    #[test]
    fn a_globstar_consumes_zero_or_more_segments() {
        let g = GitIgnore::parse("src/**/*.rs\n");
        assert!(g.matches(p("src/a.rs"), "a.rs", false), "zero segments");
        assert!(g.matches(p("src/x/a.rs"), "a.rs", false), "one segment");
        assert!(g.matches(p("src/x/y/a.rs"), "a.rs", false), "two segments");
        assert!(
            !g.matches(p("other/a.rs"), "a.rs", false),
            "outside the prefix"
        );
    }

    /// A pattern carrying a slash is anchored to the ignore file's directory.
    /// Collapsed to a name, `docs/build/` used to ignore every `build` in the
    /// tree.
    #[test]
    fn a_slash_pattern_is_anchored_to_its_directory() {
        let g = GitIgnore::parse("docs/build/\n");
        assert!(g.matches(p("docs/build"), "build", true));
        assert!(
            !g.matches(p("src/build"), "build", true),
            "matched outside docs/"
        );
        // The discriminating case: anchored means FROM THE ROOT, so a `docs/`
        // nested deeper is not the one the pattern named. Without this the
        // rule could carry an implicit leading `**` and nothing would say so.
        assert!(
            !g.matches(p("x/docs/build"), "build", true),
            "the pattern floated instead of anchoring"
        );
        assert!(
            !g.matches(p("docs/build"), "build", false),
            "a FILE named build"
        );
    }

    /// `newenv/*` used to vanish: the last segment was `*`, trimming the star
    /// left an empty string, and the rule was dropped. 144 real rules in the
    /// corpus have that shape.
    #[test]
    fn a_trailing_star_segment_is_not_dropped() {
        let g = GitIgnore::parse("newenv/*\n");
        assert!(!g.is_empty(), "the rule vanished at parse time");
        assert!(g.matches(p("newenv/x.py"), "x.py", false));
        assert!(!g.matches(p("other/x.py"), "x.py", false));
    }

    /// A leading `**/` has to survive into the segments, or `**/a/b` would be
    /// anchored at the root.
    #[test]
    fn a_leading_globstar_survives_into_the_segments() {
        let g = GitIgnore::parse("**/a/b\n");
        assert!(g.matches(p("a/b"), "b", false));
        assert!(g.matches(p("x/a/b"), "b", false));
        assert!(g.matches(p("x/y/a/b"), "b", false));
        assert!(!g.matches(p("a/c"), "c", false));
    }

    /// A single `*` stays inside its segment.
    #[test]
    fn a_star_stays_inside_its_segment() {
        let g = GitIgnore::parse("src/*.rs\n");
        assert!(g.matches(p("src/a.rs"), "a.rs", false));
        assert!(
            !g.matches(p("src/x/a.rs"), "a.rs", false),
            "the star crossed a slash"
        );
    }

    #[test]
    fn matches_plain_name() {
        let g = GitIgnore::parse("node_modules\n");
        assert!(g.matches(p("packages/app/node_modules"), "node_modules", true));
        assert!(g.matches(p("node_modules"), "node_modules", true));
    }

    #[test]
    fn anchored_matches_only_root() {
        let g = GitIgnore::parse("/build\n");
        assert!(g.matches(p("build"), "build", true));
        // Non-root path with same name should NOT match the anchored rule
        // (this is what differentiates `/build` from `build` in gitignore).
        assert!(!g.matches(p("nested/build"), "build", true));
    }

    #[test]
    fn directory_only_rule() {
        let g = GitIgnore::parse("dist/\n");
        assert!(g.matches(p("dist"), "dist", true));
        // A file named "dist" should NOT match a dir-only rule.
        assert!(!g.matches(p("dist"), "dist", false));
    }

    #[test]
    fn suffix_wildcard() {
        let g = GitIgnore::parse("*.log\n");
        assert!(g.matches(p("app.log"), "app.log", false));
        assert!(g.matches(p("logs/error.log"), "error.log", false));
        assert!(!g.matches(p("app.txt"), "app.txt", false));
    }

    #[test]
    fn prefix_wildcard() {
        let g = GitIgnore::parse("tmp.*\n");
        assert!(g.matches(p("tmp.123"), "tmp.123", false));
        assert!(!g.matches(p("foo.txt"), "foo.txt", false));
    }

    #[test]
    fn an_anchored_wildcard_stays_at_the_root() {
        // `/_*` used to lose its anchor: the leading-star branch ran
        // first and produced a basename rule matching at ANY depth, so
        // `bin/_ops-common.sh` — a tracked script — was skipped.
        let g = GitIgnore::parse("/_*\n");
        assert!(g.matches(p("_drafts"), "_drafts", true));
        assert!(!g.matches(p("bin/_ops-common.sh"), "_ops-common.sh", false));
    }

    #[test]
    fn an_anchored_directory_stays_at_the_root() {
        // Same defect, dir-only shape. One repository writes above these very
        // lines that a bare `locales/` would hide `bin/cli/locales/*.json`,
        // and we hid them anyway.
        let g = GitIgnore::parse("/locales/\n");
        assert!(g.matches(p("locales"), "locales", true));
        assert!(!g.matches(p("bin/cli/locales"), "locales", true));
    }

    #[test]
    fn without_the_anchor_the_same_shapes_match_at_any_depth() {
        // The anchor is what scopes a rule, not the shape. Drop it and both
        // patterns go back to matching everywhere — otherwise the fix above
        // would read as "wildcards no longer nest".
        let g = GitIgnore::parse("_*\nlocales/\n");
        assert!(g.matches(p("bin/_ops-common.sh"), "_ops-common.sh", false));
        assert!(g.matches(p("bin/cli/locales"), "locales", true));
    }

    #[test]
    fn a_wildcard_in_the_middle_of_a_name_matches() {
        // Neither pure prefix nor pure suffix, so this fell through to a
        // literal comparison no basename could satisfy. 389 real rules in 77
        // repositories had that shape and 66 of them name a credential.
        let g = GitIgnore::parse(".env.*.local\n*secret*\n");
        assert!(g.matches(p("app/.env.prod.local"), ".env.prod.local", false));
        assert!(g.matches(p("k/my-secret-file"), "my-secret-file", false));
        assert!(!g.matches(p(".env.local"), ".env.local", false));
    }

    #[test]
    fn a_wildcard_directory_rule_still_only_claims_directories() {
        // `*.egg-info/` is the single most common dir-only wildcard in the
        // corpus (182 of 264). The leading-star branch used to run before the
        // dir-only one and dropped the `/`, so the rule also claimed a FILE
        // by that name.
        let g = GitIgnore::parse("*.egg-info/\n");
        assert!(g.matches(p("src/pkg.egg-info"), "pkg.egg-info", true));
        assert!(!g.matches(p("src/pkg.egg-info"), "pkg.egg-info", false));
    }

    #[test]
    fn a_question_mark_stands_for_one_character() {
        let g = GitIgnore::parse("*.sw?\n");
        assert!(g.matches(p("main.swp"), "main.swp", false));
        assert!(!g.matches(p("main.sw"), "main.sw", false));
    }

    #[test]
    fn a_bare_star_claims_everything_beside_it() {
        // The idiom that makes nested ignore files matter: ignore all, then
        // re-include by hand. A literal comparison against "*" claimed
        // nothing at all.
        let g = GitIgnore::parse("*\n!keep.txt\n");
        assert!(g.matches(p("config.json"), "config.json", false));
        assert!(!g.matches(p("keep.txt"), "keep.txt", false));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-gitignore-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_nested_ignore_file_only_binds_its_own_directory() {
        // Before the stack existed only the root `.gitignore` was read,
        // so 314 nested files across 85 of 272 repositories bound nothing.
        let d = scratch("nested");
        std::fs::create_dir_all(d.join("vendor")).unwrap();
        std::fs::create_dir_all(d.join("app")).unwrap();
        std::fs::write(d.join(".gitignore"), "target\n").unwrap();
        std::fs::write(d.join("vendor/.gitignore"), "*.log\n").unwrap();

        let root = IgnoreStack::at_root(&d);
        let vendor = root.descend(Path::new("vendor"), &d.join("vendor"));
        assert!(vendor.matches(p("vendor/build.log"), "build.log", false));
        // The same name one directory over is NOT claimed: the nested rule
        // travels down, never sideways.
        let app = root.descend(Path::new("app"), &d.join("app"));
        assert!(!app.matches(p("app/build.log"), "build.log", false));
        // And the root layer still applies inside the nested directory.
        assert!(vendor.matches(p("vendor/target"), "target", true));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_nested_rule_is_anchored_to_its_own_directory() {
        // An anchored or slash-carrying rule is matched against the path
        // RELATIVE TO ITS OWN FILE, so the layer has to rescope before
        // asking. Read against the walk root instead, `/config.json` inside
        // `vendor/` would claim nothing at all.
        let d = scratch("nested-anchor");
        std::fs::create_dir_all(d.join("vendor/sub")).unwrap();
        std::fs::write(d.join("vendor/.gitignore"), "/config.json\n").unwrap();

        let vendor = IgnoreStack::at_root(&d).descend(Path::new("vendor"), &d.join("vendor"));
        assert!(vendor.matches(p("vendor/config.json"), "config.json", false));
        assert!(!vendor.matches(p("vendor/sub/config.json"), "config.json", false));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_nested_negation_undoes_the_root() {
        // The deeper file has the last word, which is git's precedence and
        // the whole point of reading it.
        let d = scratch("nested-neg");
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::write(d.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(d.join("docs/.gitignore"), "!keep.log\n").unwrap();

        let root = IgnoreStack::at_root(&d);
        let docs = root.descend(Path::new("docs"), &d.join("docs"));
        assert!(root.matches(p("build.log"), "build.log", false));
        assert!(!docs.matches(p("docs/keep.log"), "keep.log", false));
        assert!(docs.matches(p("docs/other.log"), "other.log", false));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn comments_and_blanks_ignored() {
        let g = GitIgnore::parse("\n# this is a comment\n\n   \n");
        assert!(g.is_empty());
    }

    /// A lone negation ignores nothing, but it is a RULE now — it once
    /// was dropped at parse time, which made every re-inclusion in every
    /// `.gitignore` a no-op.
    #[test]
    fn a_lone_negation_is_parsed_and_ignores_nothing() {
        let g = GitIgnore::parse("!keepme\n");
        assert!(!g.is_empty(), "the negation was dropped at parse time");
        assert!(!g.matches(p("keepme"), "keepme", false));
    }

    /// The shape that actually appears in the corpus: ignore a family, keep
    /// one of them. `!.env.example`, `!.vscode/extensions.json`,
    /// `!tests/fixtures/**` are all this.
    #[test]
    fn a_negation_re_includes_what_an_earlier_rule_took() {
        let g = GitIgnore::parse(".env*\n!.env.example\n");
        assert!(
            g.matches(p(".env"), ".env", false),
            "the family is still ignored"
        );
        assert!(g.matches(p(".env.local"), ".env.local", false));
        assert!(
            !g.matches(p(".env.example"), ".env.example", false),
            ".env.example was not re-included"
        );
    }

    /// Order decides, and only the LAST match counts — a negation written
    /// before the rule it means to undo does nothing, exactly as in git.
    #[test]
    fn the_last_matching_rule_wins() {
        let undone = GitIgnore::parse("*.log\n!keep.log\n");
        assert!(!undone.matches(p("keep.log"), "keep.log", false));
        let wrong_order = GitIgnore::parse("!keep.log\n*.log\n");
        assert!(
            wrong_order.matches(p("keep.log"), "keep.log", false),
            "a negation written first must not win"
        );
    }

    /// A negation carrying a path is a glob like any other.
    #[test]
    fn a_negation_can_carry_a_path() {
        let g = GitIgnore::parse("evaluation/**/*.log\n!evaluation/case_study/**/*.log\n");
        assert!(g.matches(p("evaluation/x/run.log"), "run.log", false));
        assert!(!g.matches(p("evaluation/case_study/dj/both.log"), "both.log", false));
    }

    #[test]
    fn missing_file_yields_empty() {
        let g = GitIgnore::load_from_root(Path::new("/does/not/exist"));
        assert!(g.is_empty());
    }
}
