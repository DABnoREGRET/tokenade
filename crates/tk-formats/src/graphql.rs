//! GraphQL Schema Definition Language (SDL) compaction.
//!
//! `.graphql` / `.gql` files (and `IntrospectionQuery` SDL responses) routinely
//! span 10 KB – 1 MB on real APIs (GitHub, Shopify, Stripe). 90%+ of the bytes
//! are field bodies, argument defaults, and docstrings the agent doesn't need
//! to integrate against — what they want is the *list* of types and the *API
//! surface* (queries, mutations, subscriptions with their signatures).
//!
//! [`compact`] keeps:
//!   * one-line summary: N types / Q queries / M mutations / S subscriptions
//!   * flat name lists for `type` / `interface` / `enum` / `union` / `scalar` /
//!     `input` / `directive` (capped per category)
//!   * full signatures for every `Query` / `Mutation` / `Subscription` field
//!     (including `extend type Query` blocks — common in federated schemas)
//!
//! Drops:
//!   * `"""…"""` triple-quoted docstrings (often >50% of bytes on real schemas)
//!   * `#`-prefixed line comments
//!   * field bodies inside `type` / `interface` / `input` definitions (the
//!     agent can fetch the raw schema if it needs argument-level detail)
//!
//! Detection is conservative: we require either a *strong* SDL token
//! (`directive @`, `schema {`, `extend type Query/Mutation/Subscription`) or
//! at least two block-style declarations on different lines. This is enough
//! to distinguish a real `.graphql` file from a Rust/TS file that happens to
//! contain `type Foo = …`.

use std::fmt::Write;

/// Per-category list caps. Past this many entries, the listing is folded.
pub const NAMES_CAP: usize = 200;
pub const FIELDS_CAP: usize = 300;
/// Names sharing a CamelCase suffix before it is said once.
const SUFFIX_FAMILY_MIN: usize = 5;

/// `CreateIssuePayload` → `Payload`; a name that is ONE word (`Query`) has no
/// tail to factor out, and neither does an all-caps one (`URI`).
fn camel_tail(name: &str) -> &str {
    match name
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_uppercase())
    {
        Some((i, _))
            if i > 0
                && name[i..].len() > 1
                && name[i + 1..].chars().all(|c| c.is_ascii_lowercase()) =>
        {
            &name[i..]
        }
        _ => "",
    }
}

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);

    // Strong, unambiguous SDL signals — each ANCHORED at the start of its
    // line. They were `contains` over the whole 8 KB head until 2026-08-26,
    // and `schema {` is an ordinary substring of ordinary Rust: the line
    // `if self.minify_schema {` carries it, and so does the format string
    // `"unsupported manual selection schema {}"`. Both files below detected
    // as SDL on that alone, and the veto underneath never got to run because
    // this block returns first. In real SDL these are top-level block
    // headers, so requiring the line to START with the token costs nothing
    // and takes an identifier suffix out of the running.
    if head.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("directive @")
            || ((t.starts_with("schema {") || t.starts_with("schema{"))
                && t.trim_end().ends_with('{'))
            || t.starts_with("extend type Query")
            || t.starts_with("extend type Mutation")
            || t.starts_with("extend type Subscription")
    }) {
        return true;
    }

    // Source-code veto: TS `interface Props {` / Go `type Foo struct {`
    // satisfy the weak block-declaration signal below, and the SDL
    // compactor then rebuilds NAME LISTS ONLY — a `cat file.ts` lost its
    // imports and function bodies. Real SDL contains none of these.
    // Rust was missing from this list, and Rust is what got wiped: `use`,
    // `fn`, `pub`, `impl`, `mod`, `crate::`, `->` and `match` are as
    // unmistakably not-SDL as `func` or `export`. Measured 2026-08-26,
    // `lean-ctx/…/calibrator/selection.rs` (38 063 bytes) and
    // `llmtrim/…/stages/tools.rs` (56 952) both cleared the old veto because
    // their first 8 KB is doc comments and `use` lines — no `let `, no
    // ` struct ` — and the SDL rebuild returned the EMPTY STRING for each.
    // #298 — the membership is shared with every other detector that asks this
    // question; only the POLICY is local. Two things make it local: the veto
    // must see `crate::` in the MIDDLE of a line, and SDL shares `type`,
    // `interface`, `enum` and `input` with TypeScript, so those four must not
    // be read as source here. That exclusion is exactly why the three callers
    // keep their own matching instead of one shared predicate.
    const SDL_AMBIGUOUS: &[&str] = &["type ", "interface ", "enum ", "input "];
    let source_line = head.lines().any(|l| {
        let t = l.trim_start();
        !t.is_empty()
            && crate::source_signals::LINE_OPENERS
                .iter()
                .filter(|k| !SDL_AMBIGUOUS.contains(k))
                .any(|k| t.starts_with(k))
    });
    let source_marker = crate::source_signals::INLINE_MARKERS
        .iter()
        .any(|k| head.contains(k));
    // `export ` and `package ` are already in `LINE_OPENERS`, checked anchored
    // just above; repeating them loosely here was residue from the list this
    // replaced. ` struct ` is a genuine MID-LINE marker (`type Foo struct {`)
    // and moved to `INLINE_MARKERS`, where mid-line markers live.
    if source_line || source_marker {
        return false;
    }

    // Weak signal: at least two block-style declarations (with `{`).
    head.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.contains('{')
                && ["type ", "interface ", "input ", "enum "]
                    .iter()
                    .any(|kw| t.starts_with(kw))
        })
        .nth(1)
        .is_some()
}

/// Strip `"""…"""` triple-quoted docstrings while preserving newlines so any
/// later line-based pass stays in sync. Single-quoted descriptions and string
/// argument defaults inside `(...)` are left alone — they could carry useful
/// signal (default values) and rarely contain SDL keywords.
fn strip_docstrings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find("\"\"\"") {
            Some(start) => {
                out.push_str(&rest[..start]);
                let after = &rest[start + 3..];
                match after.find("\"\"\"") {
                    Some(end) => {
                        // Keep newlines from the skipped region.
                        for c in after[..end].chars() {
                            if c == '\n' {
                                out.push('\n');
                            }
                        }
                        rest = &after[end + 3..];
                    }
                    None => {
                        // Unterminated docstring; bail out.
                        break;
                    }
                }
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// Return the identifier that follows `kw` (e.g. `type` → `Foo`), or `None`.
fn parse_name(line: &str, kw: &str) -> Option<String> {
    let prefix = format!("{kw} ");
    let rest = line.strip_prefix(&prefix)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    // Pre-clean: remove docstrings so they don't confuse the line scanner.
    let cleaned = strip_docstrings(text);

    let mut types: Vec<String> = Vec::new();
    let mut interfaces: Vec<String> = Vec::new();
    let mut enums: Vec<String> = Vec::new();
    let mut unions: Vec<String> = Vec::new();
    let mut scalars: Vec<String> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut directives: Vec<String> = Vec::new();
    let mut query_fields: Vec<String> = Vec::new();
    let mut mutation_fields: Vec<String> = Vec::new();
    let mut subscription_fields: Vec<String> = Vec::new();

    // Brace-depth tracker + which root-level field bucket (if any) we're in.
    enum Bucket {
        Query,
        Mutation,
        Subscription,
    }
    let mut depth: i32 = 0;
    let mut collector: Option<Bucket> = None;
    let mut field_buf = String::new();
    // Every line this scanner does not claim is DESTROYED, and nothing
    // said so. The rebuild is a name list: the fields of every ordinary type,
    // and any top-level line that is not a declaration, simply vanish. The
    // router's own fuzzer plants a needle line and demands it either survive
    // or be disclosed; SDL failed that on five seeds out of four hundred, and
    // a 244-byte schema came back as 67 bytes of counts with no mention of
    // what was gone. Same class as the empty-render rule below.
    let mut dropped = 0usize;

    for raw_line in cleaned.lines() {
        // Strip `#` line comments.
        let line_no_comment = match raw_line.find('#') {
            Some(idx) => &raw_line[..idx],
            None => raw_line,
        };
        let line = line_no_comment.trim();
        if line.is_empty() {
            continue;
        }
        // Braces alone carry no content, so they are not a loss.
        let punctuation_only = line.chars().all(|c| c == '{' || c == '}');

        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;

        if depth == 0 {
            // Look for top-level declarations.
            if let Some(name) = parse_name(line, "type") {
                match name.as_str() {
                    "Query" => collector = Some(Bucket::Query),
                    "Mutation" => collector = Some(Bucket::Mutation),
                    "Subscription" => collector = Some(Bucket::Subscription),
                    _ => types.push(name),
                }
            } else if let Some(name) = parse_name(line, "interface") {
                interfaces.push(name);
            } else if let Some(name) = parse_name(line, "enum") {
                enums.push(name);
            } else if let Some(name) = parse_name(line, "input") {
                inputs.push(name);
            } else if let Some(rest) = line.strip_prefix("scalar ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    scalars.push(name);
                }
            } else if let Some(rest) = line.strip_prefix("union ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    unions.push(name);
                }
            } else if let Some(rest) = line.strip_prefix("directive @") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    directives.push(name);
                }
            } else if line.starts_with("extend type Query") {
                collector = Some(Bucket::Query);
            } else if line.starts_with("extend type Mutation") {
                collector = Some(Bucket::Mutation);
            } else if line.starts_with("extend type Subscription") {
                collector = Some(Bucket::Subscription);
            } else if !punctuation_only {
                dropped += 1;
            }
        } else if depth >= 1 && collector.is_some() && !line.starts_with('}') {
            // Inside Query/Mutation/Subscription — accumulate field signature.
            if !field_buf.is_empty() {
                field_buf.push(' ');
            }
            // Strip trailing `{` if a directive arg block opens here (rare).
            let cleaned_chunk = line.trim_end_matches('{').trim();
            field_buf.push_str(cleaned_chunk);

            // Field is complete when parens balance and we've seen a return-type colon.
            let open_p = field_buf.matches('(').count();
            let close_p = field_buf.matches(')').count();
            let has_return = field_buf.contains(':');
            if open_p == close_p && has_return {
                let sig = collapse_ws(&field_buf);
                if !sig.is_empty() {
                    match collector.as_ref().unwrap() {
                        Bucket::Query => query_fields.push(sig),
                        Bucket::Mutation => mutation_fields.push(sig),
                        Bucket::Subscription => subscription_fields.push(sig),
                    }
                }
                field_buf.clear();
            }
        } else if !punctuation_only {
            // Inside an ordinary type's braces: its fields are the content,
            // and the rebuild keeps only the type's NAME.
            dropped += 1;
        }

        depth += opens - closes;
        if depth < 0 {
            depth = 0;
        }
        if depth == 0 {
            collector = None;
            field_buf.clear();
        }
    }

    // ---- Render ----
    let mut out = String::with_capacity(2048);
    let mut summary_parts: Vec<String> = Vec::new();
    for (n, label) in [
        (types.len(), "types"),
        (interfaces.len(), "interfaces"),
        (enums.len(), "enums"),
        (unions.len(), "unions"),
        (scalars.len(), "scalars"),
        (inputs.len(), "inputs"),
        (directives.len(), "directives"),
        (query_fields.len(), "queries"),
        (mutation_fields.len(), "mutations"),
        (subscription_fields.len(), "subscriptions"),
    ] {
        if n > 0 {
            summary_parts.push(format!("{n} {label}"));
        }
    }
    if !summary_parts.is_empty() {
        out.push_str(&summary_parts.join(", "));
        out.push('\n');
    }

    let names = |out: &mut String, label: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        let n = items.len();
        let _ = write!(out, "\n{label} ({n}): ");
        if n <= NAMES_CAP {
            out.push_str(&items.join(", "));
            out.push('\n');
            return;
        }
        // Past the cap, every name stays — a type past the 200th was simply
        // gone — but a generated family (`CreateIssuePayload`, `…Edge`) says
        // its suffix once: `*Payload (120): CreateIssue, …`.
        let mut by_suffix: Vec<(&str, Vec<&str>)> = Vec::new();
        for name in items {
            let suffix = camel_tail(name);
            match by_suffix.iter_mut().find(|(s, _)| *s == suffix) {
                Some((_, v)) => v.push(name),
                None => by_suffix.push((suffix, vec![name])),
            }
        }
        let mut plain: Vec<&str> = Vec::new();
        let mut families = String::new();
        for (suffix, names) in &by_suffix {
            if names.len() >= SUFFIX_FAMILY_MIN && !suffix.is_empty() {
                let stems: Vec<&str> = names.iter().map(|n| &n[..n.len() - suffix.len()]).collect();
                let _ = writeln!(
                    families,
                    "  *{suffix} ({}): {}",
                    names.len(),
                    stems.join(", ")
                );
            } else {
                plain.extend(names);
            }
        }
        out.push_str(&plain.join(", "));
        out.push('\n');
        out.push_str(&families);
    };
    names(&mut out, "types", &types);
    names(&mut out, "interfaces", &interfaces);
    names(&mut out, "enums", &enums);
    names(&mut out, "unions", &unions);
    names(&mut out, "scalars", &scalars);
    names(&mut out, "inputs", &inputs);
    names(&mut out, "directives", &directives);

    let fields = |out: &mut String, label: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        let _ = write!(out, "\n{} ({}):\n", label, items.len());
        for f in items.iter().take(FIELDS_CAP) {
            out.push_str("  ");
            out.push_str(f);
            out.push('\n');
        }
        // Past the cap the SIGNATURES go, not the operations: a 400-mutation
        // schema lost its last 100 mutations unnamed.
        if items.len() > FIELDS_CAP {
            let rest: Vec<&str> = items[FIELDS_CAP..]
                .iter()
                .map(|f| f.split(['(', ':']).next().unwrap_or(f).trim())
                .collect();
            let _ = writeln!(
                out,
                "  … {} more, names only: {}",
                rest.len(),
                rest.join(", ")
            );
        }
    };
    fields(&mut out, "queries", &query_fields);
    fields(&mut out, "mutations", &mutation_fields);
    fields(&mut out, "subscriptions", &subscription_fields);

    // The empty-render lesson, applied: a disclosure is an ADDITION and never the answer.
    // Posted unconditionally it would make `out` never empty, and the empty
    // render is exactly what tells the caller upstream that SDL claimed a
    // document it could not read — the floor that returns the input verbatim.
    if dropped > 0 && !out.is_empty() {
        let _ = writeln!(out, "\n… {dropped} other line(s) elided …");
    }

    out.trim_end().to_string()
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_ws = true;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
                last_was_ws = true;
            }
        } else {
            out.push(c);
            last_was_ws = false;
        }
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
"""
Top-level docstring with type Sneaky { fake } inside.
"""
schema {
  query: Query
  mutation: Mutation
}

scalar DateTime
scalar JSON

directive @auth(role: Role!) on FIELD_DEFINITION
directive @deprecated(reason: String) on FIELD_DEFINITION

enum Role {
  ADMIN
  USER
}

interface Node {
  id: ID!
}

union SearchResult = User | Post

input CreateUserInput {
  name: String!
  email: String!
}

type User implements Node {
  id: ID!
  name: String!
  posts: [Post!]!
}

type Post implements Node {
  id: ID!
  title: String!
  author: User!
}

type Query {
  # Get a single user by id
  user(id: ID!): User
  users(
    filter: String,
    limit: Int = 10
  ): [User!]!
}

type Mutation {
  createUser(input: CreateUserInput!): User!
  deleteUser(id: ID!): Boolean!
}

type Subscription {
  userUpdated(id: ID!): User!
}

extend type Query {
  search(q: String!): SearchResult
}
"#;

    #[test]
    fn detects_real_schema() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_with_only_strong_directive() {
        assert!(detect("directive @auth on FIELD_DEFINITION\n"));
    }

    #[test]
    fn rejects_rust_type_alias() {
        assert!(!detect(
            "pub type Foo = std::collections::HashMap<String, i32>;\n"
        ));
    }

    #[test]
    fn rejects_typescript_interfaces_without_blocks() {
        // Only declarations, no opening braces on same line.
        assert!(!detect(
            "interface Foo extends Bar\ninterface Baz extends Qux\n"
        ));
    }

    #[test]
    fn extracts_categories_correctly() {
        let out = compact(SAMPLE);
        // Summary.
        /* L122: banner removed; assertion deleted */
        // Type names captured (Query/Mutation/Subscription excluded by design).
        assert!(out.contains("types (2): "));
        assert!(out.contains("User"));
        assert!(out.contains("Post"));
        assert!(out.contains("interfaces (1): Node"));
        assert!(out.contains("enums (1): Role"));
        assert!(out.contains("unions (1): SearchResult"));
        assert!(out.contains("scalars (2): DateTime, JSON"));
        assert!(out.contains("inputs (1): CreateUserInput"));
        assert!(out.contains("directives (2): auth, deprecated"));
        // Fields with full signatures (multi-line `users` collapsed).
        assert!(out.contains("queries (3):"));
        assert!(out.contains("user(id: ID!): User"));
        assert!(out.contains("users( filter: String, limit: Int = 10 ): [User!]!"));
        assert!(out.contains("search(q: String!): SearchResult"));
        assert!(out.contains("mutations (2):"));
        assert!(out.contains("createUser(input: CreateUserInput!): User!"));
        assert!(out.contains("subscriptions (1):"));
        assert!(out.contains("userUpdated(id: ID!): User!"));
        // Docstring leakage check — `Sneaky` should NOT appear (it's inside """ ... """).
        assert!(!out.contains("Sneaky"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is just prose";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_large_schema() {
        // Build a 500-type schema with verbose field bodies & docstrings.
        let mut s = String::from("schema { query: Query }\n");
        s.push_str("\"\"\"\nIntro docstring\n\"\"\"\n");
        s.push_str("type Query {\n");
        for i in 0..100 {
            let _ = writeln!(s, "  field{i}(arg{i}: String): Type{i}");
        }
        s.push_str("}\n");
        for i in 0..500 {
            let _ = write!(
                s,
                "\"\"\"\nDocstring for Type{i} — lots of words to inflate the size\n\"\"\"\n"
            );
            let _ = write!(
                s,
                "type Type{i} {{\n  id: ID!\n  name: String!\n  description: String\n  related: [Type{i}!]!\n}}\n"
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("500 types"));
        assert!(sav.percent() > 70.0, "got {sav:?}");
    }

    #[test]
    fn past_the_field_cap_every_mutation_is_still_named() {
        let mut s = String::from(
            "schema { query: Query mutation: Mutation }\ntype Query { x: Int }\ntype Mutation {\n",
        );
        for i in 0..(FIELDS_CAP + 40) {
            let _ = writeln!(
                s,
                "  doThing{i}(input: DoThing{i}Input!, dryRun: Boolean): DoThing{i}Payload"
            );
        }
        s.push_str("  lastOne: Boolean\n}\n");
        let out = compact(&s);
        assert!(out.contains("doThing0(input: DoThing0Input!"), "{out}");
        let tail = out
            .lines()
            .find(|l| l.contains("names only"))
            .unwrap_or_else(|| panic!("{out}"));
        assert!(tail.contains("41 more"), "{tail}");
        assert!(
            tail.ends_with(&format!("doThing{}, lastOne", FIELDS_CAP + 39)),
            "{tail}"
        );
        assert!(tail.contains(&format!("doThing{}, ", FIELDS_CAP)), "{tail}");
        assert!(
            !tail.contains("Payload") && !tail.contains("Input") && !tail.contains("Boolean"),
            "{tail}"
        );
    }

    #[test]
    fn past_the_name_cap_every_type_stays_and_families_share_their_suffix() {
        let mut s = String::from("schema { query: Query }\ntype Query { x: Int }\n");
        for i in 0..(NAMES_CAP + 50) {
            let _ = writeln!(s, "type T{i} {{ x: Int }}");
        }
        for w in [
            "CreateIssue",
            "CloseIssue",
            "AddStar",
            "RemoveStar",
            "MergePr",
        ] {
            let _ = writeln!(s, "type {w}Payload {{ x: Int }}");
        }
        s.push_str("type ClientMutationPayloadEdge { x: Int }\ntype URI { x: Int }\n");
        // A version tag is not a family word.
        for w in ["User", "Repo", "Team", "Org", "Gist"] {
            let _ = writeln!(s, "type {w}V2 {{ x: Int }}");
        }
        let out = compact(&s);
        assert!(
            out.contains(&format!("types ({}):", NAMES_CAP + 62)),
            "{out}"
        );
        assert!(out.contains(", UserV2, ") && !out.contains("*V2"), "{out}");
        assert!(!out.contains("more elided"), "{out}");
        assert!(out.contains(&format!("T{}, ", NAMES_CAP + 49)), "{out}");
        assert!(
            out.contains("  *Payload (5): CreateIssue, CloseIssue, AddStar, RemoveStar, MergePr"),
            "{out}"
        );
        assert!(out.contains(", URI, "), "{out}");
        assert!(out.contains("ClientMutationPayloadEdge"), "{out}");
    }
}

#[cfg(test)]
mod strong_signals_are_line_anchored {
    //! `schema {` is an ordinary substring of ordinary source. Two real Rust
    //! files carried it — one as an identifier suffix, one inside a format
    //! string — and the unanchored `head.contains` claimed both as SDL before
    //! the source-code veto underneath could run.

    #[test]
    fn an_identifier_ending_in_schema_is_not_a_schema_block() {
        let rust = "use std::fmt;\n\npub fn go(&self) {\n    if self.minify_schema {\n        return;\n    }\n}\n";
        assert!(!super::detect(rust), "`if self.minify_schema {{` is Rust");
    }

    #[test]
    fn schema_inside_a_string_literal_is_not_a_schema_block() {
        let rust = "pub fn err() -> String {\n    format!(\"unsupported manual selection schema {}\", n)\n}\n";
        assert!(!super::detect(rust), "a format string is not SDL");
    }

    #[test]
    fn a_real_schema_block_header_still_detects() {
        let sdl = "schema {\n  query: Query\n  mutation: Mutation\n}\n";
        assert!(super::detect(sdl), "top-level `schema {{` is SDL");
    }

    #[test]
    fn an_indented_schema_block_header_still_detects() {
        let sdl = "  schema {\n    query: Query\n  }\n";
        assert!(
            super::detect(sdl),
            "leading whitespace is not a disqualifier"
        );
    }

    #[test]
    fn a_directive_declaration_still_detects() {
        let sdl = "directive @auth(requires: Role!) on FIELD_DEFINITION\n";
        assert!(super::detect(sdl), "`directive @` at line start is SDL");
    }

    /// The rebuild is a NAME LIST: the fields of every ordinary type and
    /// any top-level line that is not a declaration are destroyed. Nothing said
    /// so — 244 bytes came back as 67 bytes of counts with no mention of what
    /// was gone, and the router's fuzzer caught it the moment SDL got a
    /// generator (five seeds in four hundred, plus one where the SDL section
    /// sat inside a mixed document).
    #[test]
    fn a_line_the_rebuild_does_not_claim_is_disclosed() {
        let doc = "directive @auth(requires: Role = ADMIN) on OBJECT\n\
                   schema {\n  query: Query\n  mutation: Mutation\n}\n\n\
                   type Alpha {\n  one: String!\n  two: [Int!]!\n}\n\n\
                   NEEDLE-0123456789abcdef unique diagnostic marker\n\n\
                   type Beta {\n  three: String!\n}\n";
        let out = super::compact(doc);
        assert!(
            out.contains("types (2): Alpha, Beta"),
            "still a real fold: {out}"
        );
        // The exact count proves BOTH counters: the top-level fallthrough
        // (`schema {`, the needle) and the body of an ordinary type
        // (`one`, `two`, `three`, and the two root fields under `schema {`).
        assert!(
            out.contains("… 7 other line(s) elided …"),
            "the loss is not disclosed: {out}"
        );
    }

    /// The empty-render lesson, pinned. The disclosure is an ADDITION: posted
    /// unconditionally it makes the render never empty, and the EMPTY render is
    /// exactly what tells the caller upstream that SDL claimed a document it
    /// could not read, so that the input comes back verbatim. Replacing a
    /// document with one line of elision is worse than the loss it reports.
    #[test]
    fn the_disclosure_never_replaces_unreadable_input() {
        // Detected as SDL on the anchored `directive @` line, and then nothing
        // the scanner recognises — no type, no interface, no root field.
        let doc = "directive @\n\
                   une ligne de prose ordinaire\n\
                   une deuxieme ligne\n\
                   une troisieme ligne\n";
        assert!(
            super::detect(doc),
            "fixture no longer reaches the SDL rebuild"
        );
        let out = super::compact(doc);
        assert!(
            out.trim().is_empty(),
            "an empty rebuild must stay empty so the caller can return the input: {out:?}"
        );
    }
}
