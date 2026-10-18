//! Field-qualified query parser for `semantic_search`.
//!
//! ## Written, tested, and deliberately not wired yet
//!
//! No command exposes these filters today. That is a RESERVE, not an
//! oversight, and the difference used to be invisible: a compiler sweep found
//! eight modules in this crate with no caller anywhere in the workspace, and
//! nothing distinguished "kept on purpose" from "forgotten". Five were
//! deleted. This one survived because `kind:`/`name:`/`path:` filters on
//! `query` and `semantic` are a real capability, and deleting a written and
//! tested one destroys value that git alone does not give back cheaply.
//!
//! Exposing it is a feature with its own cycle: a flag, a help entry, and a
//! decision about what an unmatched filter does. Until then, if you are here
//! because nothing calls it — that is expected, and this note is the answer.
//!
//! Ported from codegraph's `search/query-parser.ts`. The agent (or a
//! user) types:
//!
//! ```text
//! kind:function name:auth path:src/api describe the flow
//! ```
//!
//! and we split that into structured filters + free-text. The free text
//! drives BM25 + embedding ranking; the filters narrow the candidate set
//! before scoring. Without this, the user has to dump all qualifiers
//! into the free text and hope BM25 picks the right files.
//!
//! ## What we support
//!
//! - **`kind:<value>`** — restrict to symbols of a given kind
//!   (`function`, `class`, `type`, `method`, `enum`, `interface`, etc.).
//!   Multiple `kind:` clauses are OR'd.
//! - **`name:<value>`** — restrict to symbols whose name **contains**
//!   the value (case-insensitive). Multiple `name:` clauses are OR'd.
//! - **`path:<value>`** — restrict to files whose path **contains** the
//!   value (case-insensitive). Multiple `path:` clauses are OR'd.
//! - **Quoted strings** — `path:"my dir/foo"` keeps the space inside.
//! - **Unknown fields** — passed through as plain text. So `TODO:fix`
//!   doesn't error; it just searches for the literal.

#[derive(Debug, Clone, Default)]
pub struct ParsedQuery {
    /// The free-text portion (with all `field:value` clauses stripped).
    /// Drives BM25 + embedding scoring.
    pub text: String,
    /// `kind:` filters. Empty → no restriction.
    pub kinds: Vec<String>,
    /// `name:` filters. Empty → no restriction.
    pub names: Vec<String>,
    /// `path:` filters. Empty → no restriction.
    pub paths: Vec<String>,
}

/// Parse a raw query string into structured filters + free text.
///
/// Robust on malformed input: unknown fields fall through as plain
/// text (so `TODO:fix` stays in the text), unterminated quotes get
/// closed at end-of-input, and an empty input yields an empty parse.
pub fn parse(query: &str) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    let mut text_parts: Vec<String> = Vec::new();

    let mut chars = query.chars().peekable();
    let mut buf = String::new();
    let flush = |buf: &mut String, text_parts: &mut Vec<String>, out: &mut ParsedQuery| {
        if buf.is_empty() {
            return;
        }
        if let Some((field, value_raw)) = buf.split_once(':') {
            let value = value_raw.trim_matches('"');
            match field.to_ascii_lowercase().as_str() {
                "kind" if !value.is_empty() => out.kinds.push(value.to_string()),
                "name" if !value.is_empty() => out.names.push(value.to_ascii_lowercase()),
                "path" if !value.is_empty() => out.paths.push(value.to_ascii_lowercase()),
                _ => text_parts.push(buf.clone()),
            }
        } else {
            text_parts.push(buf.clone());
        }
        buf.clear();
    };

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            flush(&mut buf, &mut text_parts, &mut out);
            continue;
        }
        if c == '"' {
            // Quoted span — consume until the matching quote.
            for inner in chars.by_ref() {
                if inner == '"' {
                    break;
                }
                buf.push(inner);
            }
            continue;
        }
        buf.push(c);
    }
    flush(&mut buf, &mut text_parts, &mut out);

    out.text = text_parts.join(" ").trim().to_string();
    out
}

/// True when `meta` (kind, name, path) passes the query's filters.
/// Empty filter lists mean "no restriction" for that dimension. All
/// three dimensions are AND'd together; within a dimension OR is used.
pub fn matches_filters(q: &ParsedQuery, kind: &str, name: &str, path: &str) -> bool {
    if !q.kinds.is_empty() && !q.kinds.iter().any(|k| k.eq_ignore_ascii_case(kind)) {
        return false;
    }
    if !q.names.is_empty() {
        let name_l = name.to_ascii_lowercase();
        if !q.names.iter().any(|n| name_l.contains(n)) {
            return false;
        }
    }
    if !q.paths.is_empty() {
        let path_l = path.to_ascii_lowercase();
        if !q.paths.iter().any(|p| path_l.contains(p)) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_yields_empty_parse() {
        let q = parse("");
        assert!(q.text.is_empty());
        assert!(q.kinds.is_empty());
        assert!(q.names.is_empty());
        assert!(q.paths.is_empty());
    }

    #[test]
    fn plain_text_passes_through() {
        let q = parse("describe the flow");
        assert_eq!(q.text, "describe the flow");
        assert!(q.kinds.is_empty());
    }

    #[test]
    fn single_field_extracted() {
        let q = parse("kind:function explain");
        assert_eq!(q.kinds, vec!["function"]);
        assert_eq!(q.text, "explain");
    }

    #[test]
    fn multiple_fields() {
        let q = parse("kind:function name:auth path:src/api explain");
        assert_eq!(q.kinds, vec!["function"]);
        assert_eq!(q.names, vec!["auth"]);
        assert_eq!(q.paths, vec!["src/api"]);
        assert_eq!(q.text, "explain");
    }

    #[test]
    fn quoted_value_keeps_spaces() {
        let q = parse(r#"path:"my dir/foo" text"#);
        assert_eq!(q.paths, vec!["my dir/foo"]);
        assert_eq!(q.text, "text");
    }

    #[test]
    fn unknown_field_falls_through_as_text() {
        // `TODO:fix` shouldn't be parsed as a field — `TODO` isn't one of
        // ours, so it becomes part of the free text.
        let q = parse("TODO:fix this");
        assert!(q.kinds.is_empty());
        assert!(q.text.contains("TODO:fix"));
    }

    #[test]
    fn matches_filters_basic() {
        let q = parse("kind:function name:auth path:src");
        assert!(matches_filters(
            &q,
            "function",
            "authenticate",
            "src/auth.rs"
        ));
        assert!(!matches_filters(&q, "type", "authenticate", "src/auth.rs"));
        assert!(!matches_filters(&q, "function", "main", "src/main.rs"));
        assert!(!matches_filters(
            &q,
            "function",
            "authenticate",
            "tests/auth_test.rs"
        ));
    }

    #[test]
    fn matches_filters_empty_means_unrestricted() {
        let q = ParsedQuery::default();
        assert!(matches_filters(&q, "anything", "anything", "anywhere"));
    }
}
