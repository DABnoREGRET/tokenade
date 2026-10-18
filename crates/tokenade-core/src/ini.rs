//! INI / `.conf` / `.properties` compactor.
//!
//! Covers the family of "old-school config files" that don't fit TOML
//! (which already has its own compactor): `*.ini`, `*.conf`,
//! `*.cfg`, `*.properties`, `sshd_config`, `nginx.conf`, `php.ini`,
//! systemd unit files, `mysql.cnf`, etc.
//!
//! What it strips:
//!   * full-line `#`, `;`, and `//` comments
//!   * inline `#` / `;` comment tails (quote-aware so URLs and PHP
//!     fragments survive)
//!   * blank-line runs → collapse to one
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * `[section]` headers (and nginx-style `block {` lines)
//!   * every `key = value`, `key: value`, `key value` line
//!   * the in-string content of quoted values (`#` inside `"..."` is
//!     NOT a comment)
//!   * line continuations (`\` at EOL) — joining them would change
//!     semantics for tools that read these files line-wise
//!
//! Real-world ratio on commented `nginx.conf` / `php.ini`: 25-45%.

/// Heuristic detector. Must satisfy ALL THREE:
///   * ≥1 `[section]` header, AND
///   * ≥3 lines that look like config (section, kv, or block-open),
///   * no HTML/XML hard-reject prefix.
///
/// Requiring an explicit `[section]` header is what separates "real
/// INI / `.ini` / `.properties` / systemd-unit / `php.ini`" from
/// nginx.conf-style hierarchical configs (which use `{ }` blocks,
/// no sections) and dig DNS output (which has `;` comments but no
/// brackets). Hierarchical confs without sections still get covered
/// by the generic logs compactor below.
pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    let mut considered = 0usize;
    // DISTINCT section names, not occurrences. A config names each section
    // once — `[core]`, `[user]`, `[remote "origin"]`. A build log repeats ONE
    // tag: measured 2026-08-26 on a 53 871-byte Maven reactor build, every
    // module prints a bare `[INFO]` separator, so sixty identical tags read as
    // sixty sections, `Kind::Ini` won, and its compactor — which only strips
    // comments — returned all 53 871 bytes. The template fold answers each of
    // that log's shapes in about 82 bytes and never got the chance.
    let mut section_names: std::collections::BTreeSet<&str> = Default::default();
    for line in crate::fences::outside_fences(text)
        .filter(|l| !l.trim().is_empty())
        .take(50)
    {
        considered += 1;
        let t = line.trim_start();
        // Hard reject: HTML / XML start.
        if t.starts_with("<!DOCTYPE") || t.starts_with("<?xml") || t.starts_with("<html") {
            return false;
        }
        // #547 — hard-reject tokenade's own banner lines. A line like
        // `[tokenade:cargo-test -X% tokens]` looks like a `[section]`
        // header to the rule below; with ≥2 such banners (chained
        // `tokenade wrap A && tokenade wrap B` pipelines), the
        // distinguishing threshold trips and the ini compactor runs
        // on already-compacted tokenade output. The PostToolUse hook
        // skips when argv[0] is `tokenade` (#389), but the auto
        // dispatcher can still reach here from other call paths, and
        // a regression of that skip would silently re-introduce the
        // bug. Defense in depth.
        if t.starts_with("[tokenade:") || t.starts_with("[tokenade-mcp:") {
            return false;
        }
        // Same class as the banner reject above, for the web reader's own
        // image marker: `[image: Microsoft]` closes its bracket at end of
        // line, so it is section-header-shaped. A LinkedIn company page is
        // mostly logos and cover photos, and it was still classified `ini`
        // after the markdown-link fix below.
        if t.starts_with("[image:") {
            return false;
        }
        // Comments — `;` or `#` or `//`. Skipped from kv/hit counts;
        // they don't contribute to detection either way.
        if t.starts_with('#') || t.starts_with(';') || t.starts_with("//") {
            continue;
        }
        // [section] header — the ONLY distinguishing signal in this
        // tightened detector. nginx-style `block {` is now treated
        // as a regular hit only; `}` likewise.
        if t.starts_with('[') && t.contains(']') {
            // A real `[section]` header CLOSES at end of line (a trailing
            // `;`/`#` comment is allowed). A markdown link — `[text](url)` —
            // also starts with `[` and contains `]`, and a distilled web page
            // is made of them: theguardian.com's homepage was classified
            // `api.rest.ini`, which then named the stash file `.ini`, telling
            // any downstream agent to parse a news front page as config.
            // `trim()`, not `trim_end()`: `[remote "origin"]   ; upstream` has
            // whitespace BEFORE the comment, and trimming only the tail left
            // the check looking at spaces and rejecting genuine INI.
            let after_close = t[t.find(']').unwrap_or(0) + 1..].trim();
            let closes_the_line = after_close.is_empty()
                || after_close.starts_with(';')
                || after_close.starts_with('#');
            if !closes_the_line {
                continue;
            }
            hits += 1;
            section_names.insert(t[..t.find(']').map(|i| i + 1).unwrap_or(t.len())].trim());
            continue;
        }
        if t.ends_with('{') || t == "}" {
            hits += 1;
            continue;
        }
        // `key = value` / `key : value` / `key value` (with at least
        // one identifier char before the separator). Tabs and equals
        // are common separators. Counts toward hits but NOT toward
        // the distinguishing signal — those need something stronger.
        if has_kv_shape(t) {
            hits += 1;
        }
    }
    // `distinguishing` is bumped only by `[section]` headers in
    // this final design — `;` alone (dig DNS!) is no longer enough.
    // The variable name is preserved for clarity even though it now
    // strictly means "section_count".
    //
    // #481: require ≥2 sections (was ≥1). Real INI files —
    // php.ini, .gitconfig, sshd_config, systemd units, mysql.cnf —
    // all carry multiple sections. Single-section `[main] k=v k=v`
    // fragments were claiming `auto:ini` in production (519 of 764
    // calls had <1% savings: nothing to strip → 0 work → dashboard
    // noise). Mirror of #391/#409/#410/#439 detector tightenings.
    hits >= 3 && section_names.len() >= 2 && considered > 0
}

fn has_kv_shape(line: &str) -> bool {
    // Find first =, :, space, or tab AFTER at least one ident char.
    let bytes = line.as_bytes();
    let mut saw_ident = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'=' || c == b':' {
            return saw_ident && i > 0;
        }
        if c == b' ' || c == b'\t' {
            if !saw_ident {
                return false;
            }
            // Space-separated kv counts only when followed by a value.
            let rest = line[i + 1..].trim();
            return !rest.is_empty();
        }
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.' {
            saw_ident = true;
        } else {
            return false;
        }
    }
    false
}

/// Strip comments + collapse blank runs + trim trailing ws.
pub fn compact(text: &str) -> String {
    crate::code_strip::declare_strip(text, strip_only(text), "ini", None)
}

/// The strip WITHOUT its declaration.
///
/// **Not an output path.** Everything a user or agent sees goes through
/// `compact`, which declares what this removed and returns the file whole when
/// the note would cost more than the strip saved. This is public only so the
/// per-language mechanics can be asserted directly: the lesson is that a
/// guard aimed at a wrapper which short-circuits on short input passes against
/// broken code underneath, and these fixtures are one line each on purpose.
pub fn strip_only(text: &str) -> String {
    crate::code_strip::strip_line_language(
        text,
        |t| t.starts_with('#') || t.starts_with(';') || t.starts_with("//"),
        strip_inline_comment,
    )
}

/// `Savings`-yielding variant for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// INI-family: `#` always starts a comment; `;` only when whitespace AND text
/// follow it. `value;` at end of line is an nginx statement terminator, not a
/// comment, and cutting there loses the value.
fn strip_inline_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"#;", b"\"'", b"\"", false, |c, i| {
        if c != b';' {
            return true;
        }
        let after = &line[i + 1..];
        !after.is_empty() && after.starts_with([' ', '\t']) && !after.trim_start().is_empty()
    })
}

#[cfg(test)]
mod tests {

    /// The loop this module used to carry, kept as the ORACLE.
    ///
    /// Same reason as `hcl`/`proto`/`sql_source` : a comment
    /// stripper that eats content fails silently, and this repo has already
    /// lost 45 % of a `.tf` that way. The generic must agree line for line.
    #[allow(dead_code)]
    fn reference_compact(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut blank_run = 0usize;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                blank_run += 1;
                if blank_run <= 1 {
                    out.push('\n');
                }
                continue;
            }
            let t = trimmed.trim_start();
            // Full-line comments.
            if t.starts_with('#') || t.starts_with(';') || t.starts_with("//") {
                continue;
            }
            // Inline comment tail (quote-aware).
            let stripped = strip_inline_comment(trimmed);
            if stripped.is_empty() {
                continue;
            }
            blank_run = 0;
            out.push_str(&stripped);
            out.push('\n');
        }
        out
    }

    #[test]
    fn delegation_agrees_with_the_loop_it_replaced() {
        for case in &[
            "",
            "FROM alpine:3.19",
            "# whole-line comment",
            "   # indented comment",
            "; ini comment",
            "// slash comment",
            "key = value  # trailing",
            "key = \"a # b\"",
            "RUN echo 'hello # world'",
            "",
            "",
            "",
            "trailing spaces   ",
            "\ttab indented",
            "[section]",
            "no-comment-at-all",
        ] {
            assert_eq!(
                super::strip_only(case),
                reference_compact(case),
                "diverges on {case:?}"
            );
        }
        let whole = &[
            "",
            "FROM alpine:3.19",
            "# whole-line comment",
            "   # indented comment",
            "; ini comment",
            "// slash comment",
            "key = value  # trailing",
            "key = \"a # b\"",
            "RUN echo 'hello # world'",
            "",
            "",
            "",
            "trailing spaces   ",
            "\ttab indented",
            "[section]",
            "no-comment-at-all",
        ]
        .join("\n");
        assert_eq!(super::strip_only(whole), reference_compact(whole));
    }
    use super::*;

    const NGINX_CONF: &str = r#"# Main server config
# Generated 2026-05-29

user nginx;
worker_processes auto;

events {
    worker_connections 1024;
}

http {
    # Standard MIME types
    include       mime.types;
    default_type  application/octet-stream;

    # Compression
    gzip  on;

    server {
        listen 80;
        server_name example.com;  # primary host

        location / {
            root   /usr/share/nginx/html;
            index  index.html;
        }
    }
}
"#;

    const PHP_INI: &str = r#"; PHP runtime config
[PHP]
engine = On

; max upload
upload_max_filesize = 10M
post_max_size = 12M

[Session]
session.save_path = "/var/lib/php/sessions"  ; on disk
session.cookie_lifetime = 0
"#;

    #[test]
    fn nginx_conf_not_detected_no_sections() {
        // nginx has block syntax but no [section] headers, so the
        // tightened detector should NOT claim it. Hierarchical confs
        // without `[…]` markers are out of scope (the generic logs
        // compactor handles them with smaller gain).
        assert!(!detect(NGINX_CONF));
    }

    #[test]
    fn detect_php_ini() {
        assert!(detect(PHP_INI));
    }

    #[test]
    fn detect_rejects_single_section_fragment() {
        // Regression for #481: a single-section [main] + a handful of
        // kv lines isn't worth routing to ini::compact — typically no
        // comments, no blank runs → zero savings, just dashboard
        // noise. Real INI files (php.ini, .gitconfig, systemd units)
        // carry multiple sections; a fragment should fall through to
        // TOML or generic.
        let fragment = "[main]\nkey1 = value1\nkey2 = value2\nkey3 = value3\n";
        assert!(
            !detect(fragment),
            "single-section fragment must NOT detect as INI"
        );
    }

    #[test]
    fn detect_two_section_minimum_still_works() {
        // Lower bound of the new threshold — 2 sections is enough to
        // catch a tiny systemd unit (`[Unit]` + `[Service]`).
        let tiny_unit = "[Unit]\nDescription=demo\n\n[Service]\nExecStart=/bin/true\n";
        assert!(detect(tiny_unit), "2-section systemd unit must detect");
    }

    #[test]
    fn detect_rejects_html() {
        assert!(!detect("<!DOCTYPE html>\n<html><body>x</body></html>"));
    }

    #[test]
    fn detect_rejects_prose() {
        assert!(!detect("hello world\nthis is just prose\nno config\n"));
    }

    #[test]
    fn compact_strips_hash_and_semicolon_comments() {
        let out = strip_only(PHP_INI);
        assert!(!out.contains("PHP runtime"));
        assert!(!out.contains("max upload"));
        assert!(!out.contains("on disk"));
        assert!(out.contains("upload_max_filesize"));
    }

    #[test]
    fn compact_preserves_quoted_hash() {
        let input = "url = \"https://example.com/p#fragment\"\n";
        let out = compact(input);
        assert!(out.contains("p#fragment"));
    }

    #[test]
    fn compact_preserves_quoted_semicolon() {
        let input = "secret = \"a;b\" ; trailing\n";
        let out = strip_only(input);
        assert!(out.contains("a;b"));
        assert!(!out.contains("trailing"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "[a]\n\n\n\nkey=1\n";
        let out = strip_only(input);
        assert_eq!(out, "[a]\n\nkey=1\n");
    }

    #[test]
    fn compact_function_still_works_on_nginx_input() {
        // compact() is content-only — it'll run on anything thrown
        // at it (including nginx). Detection just won't ROUTE here.
        let out = compact(NGINX_CONF);
        assert!(out.contains("events {"));
        assert!(out.contains("worker_connections 1024;"));
        assert!(!out.contains("primary host"));
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        // Measured on the STRIP, not on the declared output. The two are
        // different claims and were one number: this fixture is a few hundred
        // bytes, so the 81-byte declaration costs ~9 points here, while on the
        // real files these fixtures stand in for it costs 1.9-3.7 (measured
        // 2026-08-26 on four Dockerfiles of 2.2-4.4 KB, savings 10-50%).
        // Folding the note into the ratio would have read as the compactor
        // getting worse. What declaring costs is asserted once for every
        // language in `declaration_costs_at_most_one_line`, size-independently.

        let (_, sav) = crate::archetypes::savings_of(NGINX_CONF, strip_only);
        assert!(
            sav.percent() >= 20.0,
            "expected ≥20% on commented nginx.conf, got {:.1}%",
            sav.percent()
        );
    }

    /// #547 — the detector must NOT claim a tokenade-compacted output
    /// as INI even when the banner lines look like `[section]` headers.
    /// Production audit traced 196 historic auto:ini events to chained
    /// `tokenade wrap A && tokenade wrap B` pipelines whose two
    /// `[tokenade:…]` banners satisfied the ≥2-section threshold.
    #[test]
    fn rejects_tokenade_banner_output() {
        // Two-banner output from a chained wrap pipeline.
        let chained = "\
[tokenade:cargo-test −82% tokens]
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 9276 filtered out
[tokenade:grep −50% tokens]
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out
";
        assert!(
            !detect(chained),
            "tokenade banner lines must not satisfy the ini section threshold"
        );
        // Same shape from the MCP sibling.
        let mcp_chained = "\
[tokenade-mcp:semantic_search −60% tokens]
hit count: 3
[tokenade-mcp:symbol_find −40% tokens]
hit count: 1
";
        assert!(
            !detect(mcp_chained),
            "tokenade-mcp banner lines must not satisfy the ini section threshold"
        );
    }

    #[test]
    fn distilled_web_page_is_not_ini() {
        // theguardian.com's homepage was classified `api.rest.ini`, which then
        // named its stash file `.ini` — telling the next reader to parse a
        // news front page as config. Markdown links are section-shaped:
        // `[text](url)` starts with `[` and contains `]`.
        let md = "\
[## Climate crisis](https://www.theguardian.com/environment/climate-crisis)
[### Full report US launches 10th round of strikes](/world/2026/jul/20/us-strikes)
[### Explained Andy Burnham's first cabinet: who's in](/politics/2026/jul/20/cabinet)
[### Energy industry European gas prices surge](/business/2026/jul/20/gas-prices)
";
        assert!(
            !detect(md),
            "markdown links must not read as [section] headers"
        );
    }

    #[test]
    fn image_markers_are_not_ini_sections() {
        // The web reader's own `[image: alt]` marker DOES close its bracket at
        // end of line. A LinkedIn company page is mostly logos.
        let md = "\
[image: Microsoft's cover photo]
[image: Microsoft]
[image: Satya Nadella]
[image: Office campus]
";
        assert!(!detect(md), "our own image marker must not read as ini");
    }

    #[test]
    fn real_ini_still_detected() {
        // The fix must not cost us actual INI: sections close at end of line,
        // trailing comments allowed.
        let ini = "\
; database settings
[core]
repositoryformatversion = 0
filemode = true
[remote \"origin\"]   ; the upstream
url = https://example.com/repo.git
fetch = +refs/heads/*:refs/remotes/origin/*
";
        assert!(detect(ini), "genuine INI must still be detected");
    }
}

/// A quoted config is not the document's syntax.
#[cfg(test)]
mod quoted_config_is_not_ini {
    use super::detect;

    /// Trimmed from
    /// `tokensave/docs/plans/2026-02-26-codegraph-rust-implementation-plan.md`,
    /// a 96 KB markdown plan whose only INI-looking lines are the `Cargo.toml`
    /// it quotes. It was claimed as `Kind::Ini`, and that compactor strips `#`
    /// comments — **all 14 of its headings were deleted**.
    fn plan() -> String {
        let mut t = String::from(
            "# CodeGraph Rust Port — Implementation Plan\n\n\
             **Goal:** Port CodeGraph from TypeScript to Rust as a single binary.\n\n\
             ## Task 1: Project Scaffold\n\n\
             Create the crate and pin the dependencies:\n\n\
             ```toml\n[package]\nname = \"codegraph\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\nrusqlite = { version = \"0.31\", features = [\"bundled\"] }\n\n\
             [dev-dependencies]\ninsta = \"1\"\n```\n\n",
        );
        for i in 2..14 {
            t.push_str(&format!(
                "## Task {i}: Step {i}\n\nWalk the tree, emit the nodes, and write them to the \
                 store, which is what step {i} is about.\n\n"
            ));
        }
        t
    }

    #[test]
    fn a_markdown_plan_quoting_a_manifest_is_not_ini() {
        assert!(
            !detect(&plan()),
            "the three `[section]` lines are inside a fenced block — they are \
             quoted, not the document's own syntax"
        );
    }

    /// …and a real INI file still detects. The helper only skips FENCES, and a
    /// config has none.
    #[test]
    fn a_real_config_still_detects() {
        let cfg = "; user settings\n[core]\neditor = vim\n\n[user]\nname = z\nemail = z@example.com\n\n[remote \"origin\"]\nurl = git@example.com:z/x.git\n";
        assert!(detect(cfg), "a plain git config stopped detecting");
    }

    /// A config that happens to CONTAIN a fence keeps being read as a config:
    /// the sections outside it are still counted.
    #[test]
    fn a_config_with_a_fence_in_a_comment_still_detects() {
        let cfg = "[core]\neditor = vim\n\n[user]\nname = z\n\n[notes]\n; see the snippet below\n; ```\n; not a section\n; ```\nkey = 1\n";
        assert!(
            detect(cfg),
            "the real sections outside the fence must still count"
        );
    }
}
