//! Generic TOML compactor — `Cargo.toml`, `pyproject.toml`, and any
//! hand-written `.toml` config files NOT already routed to the
//! `lockfile` compactor (which handles `[[package]]`-array lockfiles
//! like `Cargo.lock` / `poetry.lock` differently).
//!
//! What it strips:
//!   * full-line `#` comments
//!   * inline `#` comment tails (after non-quoted code)
//!   * runs of blank lines (kept as a single blank to preserve
//!     visual section boundaries)
//!   * trailing whitespace on every line
//!
//! What it preserves verbatim:
//!   * `[section]` and `[[array-of-tables]]` headers
//!   * `key = value` lines
//!   * any in-string content (a `#` inside `"..."` or `'...'` is NOT
//!     a comment)
//!   * multi-line strings and arrays
//!
//! Real-world ratio on `Cargo.toml` files heavy with explanatory
//! comments: 20-40%. The win compounds because every Rust agent
//! session re-reads these files repeatedly.

/// Cheap heuristic detector — looks for ≥3 lines among the first 40
/// non-blank lines that match TOML's distinctive shapes (`[section]`,
/// `[[arr]]`, or `key = value`).
///
/// #550 — also requires at least one actual `[section]` /
/// `[[array]]` header. Without that, bash test scripts and shell
/// snippets with several `KEY=val` / `export FOO=bar` lines tripped
/// the threshold. Real TOML files — `Cargo.toml`, `pyproject.toml`,
/// `.cargo/config.toml`, even the most minimal `[tool.foo]`
/// pyproject — always carry at least one header. Mirror of #481
/// for INI.
/// A TOML table header is the WHOLE line: `[name]` or `[[name]]`, with nothing
/// after the closing bracket but whitespace and an optional `#` comment.
///
/// The rule used to be `starts_with('[') && contains(']')`, which claims every
/// build log that tags its lines — `[INFO] Building svc-00`, `[ERROR] …`,
/// `[worker3] compiling …`, `[2026-08-26 10:00:00] …`. Measured 2026-08-26 on a
/// 53 871-byte Maven reactor build: every line begins `[INFO] `, so the
/// detector saw 100% table headers, `Kind::TomlConfig` won, and the TOML
/// compactor — which only strips comments — returned all 53 871 bytes. The
/// template fold answers the same content in 82 bytes per shape and never got
/// the chance. Same shape as the lockfile-before-TOML defect one branch up: a
/// detector that is too loose blocks every compactor behind it.
fn is_table_header(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('[') else {
        return false;
    };
    let rest = rest.strip_prefix('[').unwrap_or(rest);
    let Some(close) = rest.find(']') else {
        return false;
    };
    let name = &rest[..close];
    if name.is_empty() || name.contains('[') {
        return false;
    }
    let after = rest[close..].trim_start_matches(']').trim();
    after.is_empty() || after.starts_with('#')
}

pub fn detect(text: &str) -> bool {
    let mut section_or_kv = 0usize;
    let mut section_count = 0usize;
    let mut total = 0usize;
    // #286 — a comment used to be counted in `total` while never counting
    // towards `section_or_kv`, so the 50% ratio treated TOML's own comment
    // syntax as evidence AGAINST TOML. A heavily documented config — which is
    // what a hand-edited manifest is — was declined by its own detector.
    // Filtering comments out BEFORE `take(40)` matters too: a long licence
    // header would otherwise eat the whole window.
    for line in text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .take(40)
    {
        total += 1;
        let t = line.trim_start();
        if is_table_header(t) {
            section_or_kv += 1;
            section_count += 1;
            continue;
        }
        // `key = value` shape — alphanumeric/underscore/hyphen ident
        // followed by `=`. We don't require quote-matching here, just
        // an `=` early enough to look like an assignment.
        if let Some((ident, _)) = t.split_once('=') {
            if !ident.is_empty()
                && ident.chars().all(|c| {
                    c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ' ' || c == '"'
                })
            {
                section_or_kv += 1;
            }
        }
    }
    section_count >= 1 && section_or_kv >= 3 && (section_or_kv * 100) / total.max(1) >= 50
}

/// Strip comments + blank-run collapse. Preserves quoted `#`.
///
/// **Not while the file fits.** A TOML file is configuration, and its
/// comments say why each setting has its value (`# six threads: the e2e tests
/// need two cores`). Measured 2026-10-02 over a month of transcripts: three
/// reads of TOML — `.config/nextest.toml` (707 bytes), a platform file, a
/// 270-line slice — came back `comments stripped`, and the next call was
/// `expand-ref --raw` each time. Under the delivery threshold the file is
/// handed over as written; above it, the strip (declared) still pays.
pub fn compact(text: &str) -> String {
    if tk_base::delivery::fits(text) {
        return text.to_string();
    }
    crate::code_strip::declare_strip(text, strip_only(text), "toml", None)
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
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut in_multiline_string = false;
    let mut multiline_marker = ""; // `"""` or `'''`

    for line in text.lines() {
        // Quoted multi-line string passthrough — anything inside
        // `"""..."""` or `'''...'''` is opaque to us.
        if in_multiline_string {
            out.push_str(line);
            out.push('\n');
            if line.contains(multiline_marker) {
                in_multiline_string = false;
            }
            continue;
        }
        if line.contains(r#"""""#) {
            in_multiline_string = true;
            multiline_marker = r#"""""#;
            out.push_str(line);
            out.push('\n');
            // A line containing both opener AND closer stays in
            // single-line mode.
            if line.matches(r#"""""#).nth(1).is_some() {
                in_multiline_string = false;
            }
            continue;
        }
        if line.contains("'''") {
            in_multiline_string = true;
            multiline_marker = "'''";
            out.push_str(line);
            out.push('\n');
            if line.matches("'''").nth(1).is_some() {
                in_multiline_string = false;
            }
            continue;
        }

        let trimmed = strip_trailing_ws(line);
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        // Full-line comment — unless the toolchain reads it. Same predicate
        // every other stripper asks; see `code_strip::comment_is_a_directive`.
        if t.starts_with('#') {
            if crate::code_strip::comment_is_a_directive(t.trim_start_matches('#')) {
                out.push_str(&trimmed);
                out.push('\n');
            }
            continue;
        }
        // Inline comment tail — only when `#` is NOT inside a quote.
        let stripped = strip_inline_comment(&trimmed);
        if stripped.is_empty() {
            // Whole line was a comment after some leading whitespace.
            continue;
        }
        blank_run = 0;
        out.push_str(&stripped);
        out.push('\n');
    }
    out
}

/// Variant returning `Savings` for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

// ─── helpers ────────────────────────────────────────────────────────

fn strip_trailing_ws(s: &str) -> String {
    s.trim_end().to_string()
}

/// TOML: `#` comments. A basic string (`"…"`) processes backslash escapes; a
/// LITERAL string (`'…'`) does not, which is why `'C:\\path\\'` must keep its
/// closing quote.
fn strip_inline_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"#", b"\"'", b"\"", false, |_, _| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_cargo_toml() {
        let cargo = r#"
[package]
name = "foo"
version = "0.1.0"

[dependencies]
serde = "1"
        "#;
        assert!(detect(cargo));
    }

    #[test]
    fn detect_pyproject_toml() {
        let py = r#"
[project]
name = "foo"
version = "0.1"
dependencies = ["requests"]

[build-system]
requires = ["hatchling"]
        "#;
        assert!(detect(py));
    }

    #[test]
    fn rejects_yaml_and_prose() {
        assert!(!detect("hello world\nthis is just prose\n"));
        assert!(!detect("key: value\nnested:\n  inner: 1\n"));
    }

    #[test]
    fn strips_full_line_comments() {
        let input = "# top comment\n[a]\nkey = 1\n# another\n";
        let out = strip_only(input);
        assert!(!out.contains("top comment"));
        assert!(!out.contains("another"));
        assert!(out.contains("[a]"));
        assert!(out.contains("key = 1"));
    }

    #[test]
    fn strips_inline_comment_tails() {
        let input = "key = 1 # trailing comment\nfoo = \"bar\" # x\n";
        let out = strip_only(input);
        assert_eq!(out, "key = 1\nfoo = \"bar\"\n");
    }

    #[test]
    fn preserves_hash_inside_quotes() {
        let input = "url = \"https://example.com/page#fragment\"\n";
        let out = compact(input);
        assert!(out.contains("page#fragment"));
    }

    #[test]
    fn preserves_hash_inside_single_quotes() {
        let input = "literal = 'a # b'\n";
        let out = compact(input);
        assert!(out.contains("a # b"));
    }

    #[test]
    fn collapses_blank_run_to_one() {
        let input = "[a]\n\n\n\nkey = 1\n";
        let out = strip_only(input);
        // Exactly ONE blank line between [a] and key=.
        assert_eq!(out, "[a]\n\nkey = 1\n");
    }

    /// #550 — bash scripts with several `KEY=val` lines must NOT
    /// trigger TOML detection. Production audit found 11 auto:toml
    /// events on labels like `echo "..." TEST_REQ='...' EXPECTED_RESULT=...`.
    #[test]
    fn bash_kv_script_not_detected_as_toml() {
        let bash = "\
TEST_REQ='cargo test'
EXPECTED_RESULT=pass
TARGET=x86_64-unknown-linux-gnu
TIMEOUT_SECS=120
RUST_LOG=debug
";
        assert!(
            !detect(bash),
            "bash KEY=val script must not satisfy TOML detection (no [section] header)"
        );
        let bash_with_exports = "\
export FOO=bar
export BAZ=qux
export QUUX=quuz
HOME=/tmp
LANG=en_US.UTF-8
";
        assert!(
            !detect(bash_with_exports),
            "bash `export FOO=bar` script must not satisfy TOML detection"
        );
    }

    /// #550 — minimal TOML (one section + a few kv) still detects.
    /// The new section_count >= 1 requirement must not break real
    /// TOML files that legitimately have just one header.
    #[test]
    fn minimal_pyproject_toml_still_detects() {
        let pyproject = "\
[tool.poetry]
name = \"foo\"
version = \"0.1.0\"
description = \"a thing\"
";
        assert!(
            detect(pyproject),
            "minimal pyproject.toml with one [tool.poetry] section must still detect"
        );
    }

    #[test]
    fn preserves_multiline_string_verbatim() {
        let input =
            "doc = \"\"\"\n# this is part of the docstring\n\"\"\"\n[a]\n# real comment\nkey = 1\n";
        let out = strip_only(input);
        // The hash inside the triple-quoted string survives.
        assert!(out.contains("# this is part of the docstring"));
        // The hash outside (real TOML comment) is gone.
        assert!(!out.contains("# real comment"));
    }

    #[test]
    fn real_world_cargo_toml_compaction() {
        let input = r#"
# Workspace manifest.
[workspace]
resolver = "2"
members = [
    "crates/foo",
    # leave bar out for now
    "crates/bar",
]

# Dependencies shared across the workspace.
[workspace.dependencies]
# Snapshot testing — locks compactor outputs so refactors that mangle
# the output get caught.
insta = "1"
serde = { version = "1", features = ["derive"] } # serde with derive
        "#;
        // A manifest this size fits under the delivery threshold: its comments
        // are the WHY of each setting and it comes back as written.
        assert_eq!(compact(input), input);
        // The strip itself, which still runs on a manifest too large to deliver.
        let out = strip_only(input);
        assert!(out.len() * 10 <= input.len() * 9, "expected ≥10% stripped");
        assert!(!out.contains("Workspace manifest"));
        assert!(!out.contains("Snapshot testing"));
        assert!(!out.contains("# serde with derive"));
        assert!(out.contains("[workspace]"));
        assert!(out.contains("insta = \"1\""));
    }
}

#[cfg(test)]
mod a_comment_is_toml_too {
    use super::detect;

    /// The shape that cost the most: a hand-edited manifest that documents
    /// every key. Before #286 the comments counted in the ratio denominator
    /// and never in the numerator, so the file was declined and delivered
    /// whole by the markdown path — 21711 bytes in, 21711 out.
    const DOCUMENTED: &str = r#"# The hackernews manifest.
# Every key below is documented, because the next person to edit it
# will not have read the API docs.
[platform]
# The name the ledger files this under.
name = "hackernews"
# Whether a human or an agent is expected to read the output.
kind = "human"
# Seconds to wait between two writes.
pacing = 30
"#;

    #[test]
    fn a_documented_manifest_is_still_toml() {
        assert!(detect(DOCUMENTED));
    }

    #[test]
    fn a_long_licence_header_does_not_eat_the_window() {
        let mut t = String::new();
        for i in 0..60 {
            t.push_str(&format!("# licence line {i}\n"));
        }
        t.push_str("[package]\nname = \"x\"\nversion = \"1\"\nedition = \"2021\"\n");
        assert!(detect(&t));
    }

    #[test]
    fn prose_is_still_not_toml() {
        let prose = "# A heading\n\nSome ordinary prose about configuration files.\n\
It mentions a key = value pair in passing but is not one.\n\
And it goes on for several more sentences without a table header.\n";
        assert!(!detect(prose));
    }

    #[test]
    fn a_file_that_is_only_comments_is_not_toml() {
        let only = "# one\n# two\n# three\n# four\n";
        assert!(!detect(only));
    }
}
