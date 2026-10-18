//! Protobuf `.proto` schema compactor (proto2 + proto3).
//!
//! Covers `*.proto` files holding messages, services, enums, RPC
//! definitions. Public protobuf libraries (Google's
//! `descriptor.proto`, gRPC schemas, federated gateway specs) often
//! ship with extensive doc comments on every field plus standard
//! license preambles — typically 30-50% drop after this pass.
//!
//! What it strips:
//!   * full-line `//` comments (single-line)
//!   * full-line and multi-line `/* … */` block comments
//!   * inline `//` and `/* … */` comment tails (quote-aware so a
//!     `"//"` inside a `string` literal default value survives)
//!   * blank-line runs → one max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * `syntax`, `package`, `import`, `option` declarations
//!   * every `message`, `enum`, `service`, `oneof`, `extend`,
//!     `reserved` block
//!   * field definitions and their tag numbers (`int32 foo = 7;`)
//!   * `rpc Method(Request) returns (Response) { … };` RPCs
//!   * field options (`[deprecated = true, json_name = "x"]`)
//!   * map types (`map<string, int32>`)
//!   * indentation (preserves nested-message readability)

/// Top-level proto keywords used to identify a `.proto` file.
const PROTO_KEYWORDS: &[&str] = &[
    "syntax",
    "package",
    "import",
    "option",
    "message",
    "enum",
    "service",
    "oneof",
    "extend",
    "reserved",
    "extensions",
    "rpc",
];

/// Cheap heuristic: among the first 80 non-blank, non-comment
/// lines, ≥3 must start with a known proto keyword AND ≥1 of
/// those must be a "distinctive" proto-only keyword.
///
/// #598 — the generic-3-hit version was too loose: `package`,
/// `import`, and `option` are also Java/Kotlin/Go/Scala keywords.
/// A Java file with `package com.foo; import a.b; import c.d` at
/// the top hit all three and got mis-detected as proto. The
/// distinctive set excludes those generic tokens; `syntax`,
/// `message`, `enum`, `service`, `oneof`, `extend`, `reserved`,
/// `extensions`, `rpc` are proto-only keywords no other source
/// language uses at the top of a file.
pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    let mut distinctive = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(80) {
        considered += 1;
        let t = line.trim_start();
        if t.starts_with("<!DOCTYPE") || t.starts_with("<?xml") {
            return false;
        }
        if t.starts_with("//") || t.starts_with("/*") {
            continue;
        }
        let first: String = t
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ';' && *c != '=' && *c != '{')
            .collect();
        if PROTO_KEYWORDS.contains(&first.as_str()) {
            hits += 1;
            // Proto-only tokens. `enum`, `extend` and `reserved` USED to be in
            // this set and are not proto-only at all: `enum` is a keyword in
            // Rust, Swift, Java, C++ and TypeScript. Measured 2026-08-26 over
            // this corpus — 8 real `.proto` files detected, **not one** of them
            // carried by those three alone, against **106 of the 354 source
            // files** claimed that had nothing else.
            //
            // And a proto declaration is `keyword Name {`, never
            // `keyword = value`: `message = format!(…)` is an ordinary
            // assignment, and the token scan stops at `=` so it read as the
            // declaration. `syntax` and `option` are excluded from that check
            // because in proto they ARE assignments (`syntax = "proto3";`).
            let stopped_on_equals = t[first.len()..].trim_start().starts_with('=');
            let proto_only = matches!(
                first.as_str(),
                "syntax" | "message" | "service" | "oneof" | "extensions" | "rpc"
            ) && !(stopped_on_equals && first != "syntax");
            if proto_only {
                distinctive += 1;
            }
        }
    }
    hits >= 3 && distinctive >= 1 && considered > 0
}

/// Strip comments + collapse blank-runs + trim trailing ws.
pub fn compact(text: &str) -> String {
    // A wire-compat note ("do NOT renumber") is the kind of line a .proto keeps
    // in a comment and nowhere else. See `code_strip::declare_strip`.
    crate::code_strip::declare_strip(text, strip_only(text), "proto", None)
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
    crate::code_strip::strip_block_language(
        text,
        "//",
        strip_block_comment,
        strip_inline_line_comment,
    )
}

/// `Savings`-yielding variant.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn strip_block_comment(line: &str, in_block: bool) -> (String, bool) {
    crate::code_strip::strip_block_comment_line(line, in_block, b"\"'")
}

/// Cut the line at its first comment marker that is not inside a string:
/// `//`, in strings quoted with `"` or `'`.
///
/// This was a 40-line hand-rolled scanner, and `hcl.rs`, `proto.rs` and
/// `sql_source.rs` each had one. They differed by their quote set and their
/// marker and by NOTHING else — same string tracking, same backslash skip,
/// same `line[..i].trim_end()` tail. `code_strip::strip_inline_comment_line`
/// already expresses all three; the two-byte markers go through its `confirm`
/// hook, which is what `ini.rs` uses it for.
fn strip_inline_line_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"/", b"\"'", b"\"'", true, |_, i| {
        line.as_bytes().get(i + 1) == Some(&b'/')
    })
}

#[cfg(test)]
mod tests {

    /// The scanner this module used to carry, kept as the ORACLE.
    ///
    /// The delegation to `code_strip` must agree with it on every line, not
    /// just on the cases someone thought to write down — a comment stripper
    /// that silently eats code is the failure mode this repo has already paid
    /// for once (45 % of a `.tf` lost with no marker, 2026-08-04).
    #[allow(dead_code)]
    fn reference_strip(line: &str) -> String {
        let bytes = line.as_bytes();
        let mut in_string: Option<u8> = None;
        let mut cut: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            match in_string {
                Some(q) => {
                    if c == q {
                        in_string = None;
                    }
                }
                None => match c {
                    b'"' | b'\'' => in_string = Some(c),
                    b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                        cut = Some(i);
                        break;
                    }
                    _ => {}
                },
            }
            i += 1;
        }
        match cut {
            Some(i) => line[..i].trim_end().to_string(),
            None => line.to_string(),
        }
    }

    #[test]
    fn delegation_agrees_with_the_scanner_it_replaced() {
        for line in &[
            "",
            "plain line",
            "a # b",
            "a // b",
            "a -- b",
            "a / b",
            "a - b",
            "key = \"a # b\"",
            "key = \"a // b\"",
            "key = \"a -- b\"",
            "key = 'a # b'",
            "key = \"a\\\" # still string\"",
            "key = \"unterminated # b",
            "key = `a -- b`",
            "  indented = 1  // trailing",
            "\\# escaped at column zero",
            "x = 1 ## double",
            "x = 1 /// triple",
            "x = 1 --- triple",
        ] {
            assert_eq!(
                super::strip_inline_line_comment(line),
                reference_strip(line),
                "diverge sur {line:?}"
            );
        }
    }
    use super::*;

    const REAL_PROTO: &str = "// Copyright 2026 Example Corp.\n// SPDX-License-Identifier: Apache-2.0\n\nsyntax = \"proto3\";\npackage example.api;\n\nimport \"google/protobuf/timestamp.proto\";\n\n/* The User entity\n   carries identity + profile */\nmessage User {\n  // Stable id\n  string id = 1;\n  // Display name\n  string name = 2;  // utf-8, max 256\n  google.protobuf.Timestamp created_at = 3;\n  reserved 4, 5, \"email\";\n}\n\nservice UserService {\n  // Fetch by id\n  rpc Get(GetRequest) returns (User);\n  rpc List(ListRequest) returns (ListResponse) {\n    option (google.api.http) = { get: \"/v1/users\" };\n  }\n}\n";

    #[test]
    fn detect_real_proto() {
        assert!(detect(REAL_PROTO));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello\nworld\n"));
        assert!(!detect("<!DOCTYPE html>\n"));
    }

    #[test]
    fn detect_rejects_java_with_only_generic_keywords() {
        // #598 — a Java file with `package` + 2 `import` hits 3
        // PROTO_KEYWORDS but no proto-distinctive token (no
        // syntax/message/enum/service/rpc/etc.). Must reject.
        let java = "package com.foo.bar;\n\
                    \n\
                    import java.util.List;\n\
                    import java.util.Map;\n\
                    import com.foo.bar.Util;\n\
                    \n\
                    public class MyService {\n  \
                        private final List<String> items;\n  \
                        public MyService(List<String> items) {\n    \
                            this.items = items;\n  \
                        }\n  \
                        public void process() {\n    \
                            items.forEach(System.out::println);\n  \
                        }\n\
                    }\n";
        assert!(
            !detect(java),
            "Java with only package+import hits must NOT be detected as proto"
        );
    }

    #[test]
    fn detect_rejects_go_with_only_generic_keywords() {
        // Same false-positive class for Go.
        let go = "package main\n\n\
                  import (\n  \
                      \"fmt\"\n  \
                      \"strings\"\n  \
                      \"os\"\n\
                  )\n\n\
                  func main() {\n  \
                      fmt.Println(\"hi\")\n  \
                      _ = strings.ToUpper(\"x\")\n  \
                      _ = os.Args\n\
                  }\n";
        assert!(
            !detect(go),
            "Go with only package+import hits must NOT be detected as proto"
        );
    }

    #[test]
    fn detect_accepts_minimal_proto_with_one_distinctive() {
        // Sanity: the minimum-viable proto has `syntax` + `message`
        // + a field decl. That's enough distinctive presence.
        let mini = "syntax = \"proto3\";\n\
                    package foo;\n\
                    message Bar {\n  \
                        string id = 1;\n\
                    }\n";
        assert!(detect(mini));
    }

    #[test]
    /// The `SPDX-License` line moved from "must be dropped" to "must be kept":
    /// it is the machine-readable licence of the file, read by scanners, and
    /// the copyright PROSE above it is what is actually noise.
    fn compact_strips_full_line_slash_comments() {
        let out = compact(REAL_PROTO);
        assert!(!out.contains("Copyright"));
        assert!(!out.contains("Stable id"));
        assert!(!out.contains("Display name"));
        assert!(!out.contains("Fetch by id"));
        assert!(
            out.contains("SPDX-License"),
            "the licence identifier is not prose"
        );
    }

    #[test]
    fn compact_strips_block_comments() {
        let out = compact(REAL_PROTO);
        assert!(!out.contains("User entity"));
        assert!(!out.contains("identity + profile"));
    }

    #[test]
    fn compact_strips_inline_comment_tails() {
        let out = compact(REAL_PROTO);
        assert!(!out.contains("utf-8, max 256"));
        assert!(out.contains("string name = 2;"));
    }

    #[test]
    fn compact_preserves_all_definitions() {
        let out = compact(REAL_PROTO);
        assert!(out.contains("syntax = \"proto3\""));
        assert!(out.contains("package example.api"));
        assert!(out.contains("import \"google/protobuf/timestamp.proto\""));
        assert!(out.contains("message User"));
        assert!(out.contains("string id = 1;"));
        assert!(out.contains("google.protobuf.Timestamp created_at = 3;"));
        assert!(out.contains("reserved 4, 5, \"email\""));
        assert!(out.contains("service UserService"));
        assert!(out.contains("rpc Get(GetRequest) returns (User)"));
        assert!(out.contains("rpc List(ListRequest) returns (ListResponse)"));
        assert!(out.contains("option (google.api.http)"));
    }

    #[test]
    fn compact_preserves_double_slash_inside_string() {
        let input = "string default_url = 1 [default = \"https://example.com\"];\n";
        let out = compact(input);
        assert!(out.contains("https://example.com"));
    }

    #[test]
    fn compact_preserves_block_marker_inside_string() {
        let input = "string regex = 1 [default = \"/* not a comment */\"];\n";
        let out = compact(input);
        assert!(out.contains("/* not a comment */"));
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

        let (_, sav) = crate::archetypes::savings_of(REAL_PROTO, strip_only);
        assert!(
            sav.percent() >= 20.0,
            "expected ≥20% on commented .proto, got {:.1}%",
            sav.percent()
        );
    }
}

#[cfg(test)]
mod other_languages_are_not_proto {
    //! `enum` was in the "proto-only" set, and it is a keyword in Rust, Swift,
    //! Java, C++ and TypeScript. `message` was too, and `message = …` is an
    //! ordinary assignment in every one of them — the token scan stops at the
    //! `=`, so it read as a declaration.

    #[test]
    fn a_rust_file_with_an_enum_is_not_proto() {
        let rs = "use std::fmt;\n\nenum Mode {\n    Fast,\n    Slow,\n}\n\nenum Kind {\n    A,\n    B,\n}\n\nenum Level {\n    Low,\n    High,\n}\n\nfn main() {}\n";
        assert!(
            !super::detect(rs),
            "three Rust enums are not three proto messages"
        );
    }

    #[test]
    fn python_keyword_arguments_are_not_declarations() {
        // The real shape, taken from a router in the corpus: a call whose
        // named arguments each sit on their own indented line, so the token
        // scan — which stops at `=` — reads `message`, `service` and `rpc`
        // as proto declarations. 157 of the 248 files still claimed after
        // the keyword-set fix were this and nothing else.
        let py = "def handle(self, resp):\n    self.raise_error(\n        message=\"Response output was blocked.\",\n        service=ServiceTypes.ROUTER,\n    )\n\n\ndef log(self):\n    self.emit(\n        message=\"done\",\n        service=ServiceTypes.ROUTER,\n        rpc=self.name,\n    )\n";
        assert!(!super::detect(py), "`message=…` is a keyword argument");
    }

    #[test]
    fn a_real_proto_file_still_detects() {
        let proto = "syntax = \"proto3\";\n\npackage demo.v1;\n\nimport \"google/protobuf/timestamp.proto\";\n\nmessage User {\n  string id = 1;\n  string name = 2;\n}\n\nservice Users {\n  rpc GetUser(GetUserRequest) returns (User);\n}\n";
        assert!(super::detect(proto));
    }

    #[test]
    fn a_proto_enum_still_counts_through_its_neighbours() {
        // `enum` no longer carries a file on its own; a real `.proto` that has
        // one also has a `message` or a `service`, and that is what detects.
        let proto = "syntax = \"proto3\";\n\nenum Status {\n  UNKNOWN = 0;\n  OK = 1;\n}\n\nmessage Result {\n  Status status = 1;\n}\n";
        assert!(super::detect(proto));
    }
}
