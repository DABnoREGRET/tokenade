//! **A hand-written table that no test walks.**
//!
//! This project's memory names the class: "a hand-enumerated guard lets the
//! next variant through". The repo has **137** tables of six entries or more,
//! and the day of 2026-09-13 saw four of them bite — four command names the
//! binary never had, twenty-seven commands whose alias decided otherwise than
//! they did, `mcp-wrap` missing from the list of commands that talk to a
//! machine, and eight HTML void elements out of fourteen.
//!
//! The rule is weak on purpose: it is enough that a test NAMES the table. A
//! test that names it almost always walks it, and the strong rule — "a test
//! iterates over it" — is not decidable by reading the source. What it buys
//! is the ratchet: a NEW table arrives with its test, or it is listed below
//! and the counter goes up, which shows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Today's tables that no test names. **The list must SHRINK.**
///
/// Each entry is a debt, not an exemption: the table decides something and
/// nothing says what it decides. The twenty-four whose entries REAPPEAR in
/// another file are the most urgent — two places answering the same question
/// end up answering differently, and that is what had just happened to the
/// HTML void elements.
const UNTESTED: &[(&str, &str)] = &[
    // An INSTRUMENT (`attribution --transcripts`): the list says what it
    // counts as a false name, it decides nothing for a user.
    ("LAUNCHERS", "tokenade-bench/src/bin/attribution.rs"),
    ("ARTICLE_TYPES", "tokenade-core/src/site_readers/article.rs"),
    ("ATTRS", "tk-base/src/redact/cookies.rs"),
    ("BUILD_DIR_FALLBACK", "tokenade-core/src/index/walk.rs"),
    ("BUILD_OR_TEST_PROGRAMS", "tokenade-cli/src/wrap/spawn.rs"),
    ("CHAT", "tk-agents/src/style.rs"),
    ("CLAUDE_HOOKS", "tokenade-cli/src/install_cli/hooks.rs"),
    ("CODE_EXTS", "tokenade-cli/src/hookwrap/read_hook.rs"),
    (
        "CODE_INTEL_TOOLS",
        "tokenade-core/src/code_intel_heuristic.rs",
    ),
    ("CODE_WORDS", "tk-base/src/redact/keyword_assignments.rs"),
    ("COMMAND_WRAPPERS", "tokenade-cli/src/wrap/spawn.rs"),
    (
        "COMPLAINT_PHRASES",
        "tokenade-bench/src/transcripts/refusal.rs",
    ),
    ("COOKIE_ATTR_KEYS", "tk-base/src/redact/cookies.rs"),
    ("DATA_EXT", "tokenade-core/src/ranking.rs"),
    ("DIRECTIVES", "tk-formats/src/makefile.rs"),
    ("DIR_MARKERS", "tokenade-core/src/ranking.rs"),
    ("DROPPED_HTML_BLOCKS", "tk-formats/src/markdown.rs"),
    ("DROP_BLOCKS", "tk-formats/src/yaml.rs"),
    ("ENGINE_HOSTS", "tokenade-core/src/serp/quality.rs"),
    ("ENTRY_TYPES", "tk-formats/src/bibtex.rs"),
    ("ERROR_KEYWORDS", "tk-fold/src/compress.rs"),
    ("EVENT_KEYS", "tk-formats/src/ndjson.rs"),
    ("EXTS", "tokenade-bench/src/benches/misc.rs"),
    ("EXTS", "tokenade-core/src/claude_md_audit/render.rs"),
    ("FENCE_LANGUAGES", "tk-formats/src/markdown.rs"),
    ("FIRST_PARTY", "tokenade-core/src/model_costs.rs"),
    ("FOLD_STATES", "tokenade-core/src/cmd/linux/netstat.rs"),
    ("FORMAT_NAMES", "tokenade-cli/src/main.rs"),
    (
        "FUNCTION_WORDS",
        "tokenade-core/src/disclose/scoped/retrieval.rs",
    ),
    ("HARD_SKIP_DIRS", "tokenade-core/src/index/walk.rs"),
    ("HCL_BLOCKS", "tokenade-core/src/hcl.rs"),
    ("HIDDEN_STEMS", "tokenade-core/src/web_tree/hidden.rs"),
    ("HOP_BY_HOP", "tokenade-cli/src/llm_proxy/http.rs"),
    ("HOST_RECORD_FORMS", "tokenade-core/src/dns.rs"),
    ("IMPLIED_FLAGS", "tokenade-core/src/cmd/linux/ip.rs"),
    ("JUNK_DIRS", "tokenade-core/src/ranking.rs"),
    ("KEYS", "tk-formats/src/ndjson.rs"),
    ("KEYS", "tokenade-core/src/skeleton.rs"),
    ("KEY_SUFFIXES", "tokenade-core/build.rs"),
    ("KEY_TOKENS", "tokenade-core/src/cmd/linux/ip.rs"),
    ("KEY_TYPES", "tk-formats/src/ssh_keys.rs"),
    ("KNOWN_STATES", "tokenade-core/src/cmd/linux/netstat.rs"),
    ("LINE_OPENERS", "tk-formats/src/source_signals.rs"),
    ("LOCKFILES", "tokenade-core/src/ranking.rs"),
    ("LONG_BUT_PUBLIC", "tk-formats/src/env.rs"),
    ("LSBLK_COLUMNS", "tokenade-core/src/cmd/linux/lsblk.rs"),
    ("METHODS", "tk-formats/src/openapi.rs"),
    ("MULTI_PART_SUFFIXES", "tk-base/src/url.rs"),
    (
        "NEEDLES",
        "tokenade-cli/src/hooks/webfetch_pretool/serve.rs",
    ),
    ("NEEDS_SHORT_PAGE", "tk-formats/src/edge_error.rs"),
    ("NEVER_DEDUP", "tokenade-core/src/dom_dedup.rs"),
    ("NOISE_DIR", "tokenade-core/src/diff.rs"),
    ("NOISE_DIRS", "tk-fold/src/glob_filter.rs"),
    ("NOISE_FS", "tokenade-core/src/cmd/linux/mount.rs"),
    ("NOISE_NAME", "tokenade-core/src/diff.rs"),
    ("NOISE_PREFIXES", "tokenade-core/src/cmd/linux/env.rs"),
    (
        "NU_BUILTIN_COLLISIONS",
        "tokenade-cli/src/install/shell_proxy/denylist.rs",
    ),
    ("OUR_OWN_ECHO", "tokenade-bench/src/transcripts.rs"),
    ("PACKAGE_FACT_WORDS", "tokenade-core/src/web_tree/prune.rs"),
    ("PAIRED_FLAG_TRIGGERS", "tk-base/src/redact/core.rs"),
    ("PAYLOAD_KEYS", "tk-formats/src/json/fold.rs"),
    ("PREFIXES", "tokenade-cli/src/hooks/replay.rs"),
    ("PROTOS", "tokenade-core/src/cmd/linux/netstat.rs"),
    ("READERS", "tokenade-core/src/disclose.rs"),
    ("ROUTING_PREFIXES", "tokenade-core/src/model_costs.rs"),
    (
        "SCOPE_STOPWORDS",
        "tokenade-core/src/disclose/scoped/retrieval.rs",
    ),
    ("SECRET_MARKERS", "tk-formats/src/env.rs"),
    ("SERP_HEADERS", "tokenade-core/src/web_fetch.rs"),
    ("SPACED", "tokenade-core/src/code_strip/comments.rs"),
    ("SQL_CLAUSES", "tokenade-core/src/sql_source.rs"),
    ("SQL_KEYWORDS", "tokenade-core/src/sql_source.rs"),
    ("SRC_EXT", "tokenade-core/src/ranking.rs"),
    (
        "STATUSLINE_HOSTS",
        "tokenade-cli/src/install/statusline_hosts.rs",
    ),
    (
        "STRUCTURED_DATA_EXTS",
        "tokenade-cli/src/hookwrap/read_hook.rs",
    ),
    (
        "STRUCTURE_KINDS",
        "tokenade-core/src/file_search/language.rs",
    ),
    ("TELEMETRY_KEYS", "tk-formats/src/json/walk.rs"),
    ("TEXT_DATA_EXTS", "tokenade-cli/src/hookwrap/read_hook.rs"),
    ("TRACKING_KEYS", "tokenade-core/src/search.rs"),
    (
        "TRIAL_LANGUAGES",
        "tokenade-core/src/file_search/language.rs",
    ),
    ("UNAMBIGUOUS", "tk-formats/src/edge_error.rs"),
    ("VOID", "tk-formats/src/html_nesting.rs"),
    ("WRAP_SHELL_BUILTINS", "tokenade-cli/src/wrap/route.rs"),
    (
        "YOUTUBE_INVIDIOUS_INSTANCES",
        "tokenade-core/src/site_readers/youtube.rs",
    ),
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

fn rust_files() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![crates_dir()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    let rel = p
                        .strip_prefix(crates_dir())
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, s));
                }
            }
        }
    }
    out
}

/// `(name, file)` of every table of six entries or more.
fn tables(files: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (rel, s) in files {
        if rel.contains("/tests/") {
            continue; // a fixture table is not a production table
        }
        let mut from = 0usize;
        while let Some(at) = s[from..].find("const ") {
            let start = from + at + "const ".len();
            from = start;
            let name: String = s[start..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if name.len() < 4 {
                continue;
            }
            // `… : … = &[` — the anchor is the `= &[`, not the `[` of the TYPE.
            let rest = &s[start + name.len()..];
            let Some(eq) = rest.find("= &[") else {
                continue;
            };
            if rest[..eq].contains('\n') || rest[..eq].contains(';') {
                continue;
            }
            let body_at = start + name.len() + eq + "= &[".len();
            let mut depth = 1i32;
            let mut k = body_at;
            let b = s.as_bytes();
            while depth > 0 && k < b.len() {
                match b[k] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                k += 1;
            }
            let body = &s[body_at..k.saturating_sub(1)];
            let entries = body
                .split('"')
                .skip(1)
                .step_by(2)
                .filter(|x| x.len() >= 2)
                .count();
            if entries >= 6 {
                out.push((name, rel.clone()));
            }
        }
    }
    out
}

/// Occurrences of `needle` as a WHOLE word.
///
/// A substring is not enough: `SEVERAL_READERS_ON_PURPOSE`, in a neighbouring
/// guard, contains `READERS` and so declared tested a table of that name that
/// is not.
fn word_hits(hay: &str, needle: &str) -> usize {
    let b = hay.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut n = 0;
    let mut from = 0;
    while let Some(at) = hay[from..].find(needle) {
        let i = from + at;
        let before_ok = i == 0 || !ident(b[i - 1]);
        let j = i + needle.len();
        let after_ok = j >= b.len() || !ident(b[j]);
        if before_ok && after_ok {
            n += 1;
        }
        from = i + needle.len();
    }
    n
}

#[test]
fn every_hand_written_table_is_named_by_a_test_or_declared() {
    let files = rust_files();
    assert!(files.len() > 1_000, "only {} files read", files.len());
    let tabs = tables(&files);
    assert!(
        tabs.len() >= 100,
        "only {} tables found — the sweep got lost",
        tabs.len()
    );

    // Named by a test: either in a file of `tests/`, or in the TEST half of a
    // production file.
    // "Named in the TEST half" is decided by `srcsweep`, the declarer —
    // writing a `split("#[cfg(test)]")` here would be the duplicate another
    // guard of this directory forbids, and it refused it on the first try.
    // One more occurrence in the WHOLE file than in its production half:
    // there is one in the test half.
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for (rel, s) in &files {
        // THIS file does not count: `UNTESTED` spells the 78 names, so it
        // declared itself the test of each of them. It is the shape the
        // repo's convention calls "a needle that finds itself" — and it bit
        // here on the first try, announcing that the whole debt was paid.
        if rel.ends_with(concat!("no_table_ships", "_without_a_test.rs")) {
            continue;
        }
        let whole_is_test = rel.contains("/tests/") || rel.ends_with("_tests.rs");
        let production = if whole_is_test {
            String::new()
        } else {
            tokenade_core::srcsweep::production_source(s)
        };
        for (name, _) in &tabs {
            let n = name.as_str();
            if word_hits(s, n) > word_hits(&production, n) {
                named.insert(n);
            }
        }
    }

    let declared: BTreeSet<&str> = UNTESTED.iter().map(|(n, _)| *n).collect();
    let mut fresh = Vec::new();
    for (name, rel) in &tabs {
        if named.contains(name.as_str()) || declared.contains(name.as_str()) {
            continue;
        }
        fresh.push(format!("    (\"{name}\", \"{rel}\"),"));
    }
    fresh.sort();
    fresh.dedup();
    assert!(
        fresh.is_empty(),
        "these tables decide something and no test says what. Write \
         the test that WALKS them, or list them in `UNTESTED`:\n{}",
        fresh.join("\n")
    );

    // And the reverse direction: a paid debt leaves the list.
    let live: BTreeSet<&str> = tabs.iter().map(|(n, _)| n.as_str()).collect();
    let paid: Vec<&str> = UNTESTED
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !live.contains(n) || named.contains(n))
        .collect();
    assert!(
        paid.is_empty(),
        "these tables have a test (or no longer exist): remove them from `UNTESTED`, \
         otherwise the list becomes decoration again:\n  {}",
        paid.join("\n  ")
    );
}

#[test]
fn the_untested_list_is_counted_and_shrinking() {
    // 86 at the first count, on 2026-09-13.
    assert!(
        UNTESTED.len() <= 86,
        "the debt of untested tables has grown: {} entries, ceiling 86",
        UNTESTED.len()
    );
}
