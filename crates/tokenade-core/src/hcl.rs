//! HashiCorp Configuration Language (HCL) source compactor.
//!
//! Covers Terraform `*.tf` / `*.tfvars`, Packer `*.pkr.hcl`,
//! Consul / Nomad / Vault HCL configs. NOT to be confused with
//! `terraform_plan` output — that's a separate compactor for
//! `terraform plan` execution output.
//!
//! Heavy infra projects have hundreds of `resource "type" "name" {…}`
//! blocks, each with verbose `description = "…"` fields and tag
//! ladders. Plus `# what this resource does` banners on most.
//! 20-35% drop typical.
//!
//! What it strips:
//!   * full-line `#`, `//`, and `/* … */` block comments
//!   * inline `#` / `//` comment tails (quote-aware)
//!   * blank-line runs → one max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every `resource`, `variable`, `output`, `module`, `provider`,
//!     `data`, `locals`, `terraform`, `backend`, `provisioner` block
//!   * block-attribute assignments (`key = expr`, `key { … }`)
//!   * `for_each` / `count` / `depends_on` meta-arguments
//!   * `${…}` / `for … in …` template expressions

/// HCL block keywords (case-sensitive). At least one of these as a
/// top-level token proves the file is HCL.
const HCL_BLOCKS: &[&str] = &[
    "resource",
    "variable",
    "output",
    "module",
    "provider",
    "data",
    "locals",
    "terraform",
    "backend",
    "provisioner",
    "connection",
    "lifecycle",
    "dynamic",
    "moved",
    // `import` is DELIBERATELY absent. It is a Terraform 1.5+ block that
    // barely appears in the wild, and it is the first word of every ES module
    // import ever written. An earlier fix closed the single-line form by requiring the
    // header to end on its brace; the MULTI-LINE form walks straight through
    // it, because `import {` alone on a line ends on the brace exactly like
    // `locals {` does:
    //
    //     import {
    //       Tabs,
    //     } from "@astrojs/starlight/components";
    //
    // Measured 2026-08-26 over this corpus: 25 real Terraform/HCL files, of
    // which **not one uses the `import` keyword at all**, against **1 003 of
    // 45 487 JS/TS files claimed as HCL**. The keyword buys nothing here and
    // costs a thousand misroutes, on a compactor that strips `#` comments —
    // which is how those documents lost every heading.
    "removed",
    "check",
    "ephemeral",
    // Packer.
    "source",
    "build",
    "post-processor",
    // Vault / Nomad common.
    "path",
    "job",
    "group",
    "task",
    "service",
    "network",
    "vault",
    "storage",
    "listener",
    "telemetry",
    "ui",
    "seal",
];

/// Cheap heuristic: ≥3 of the first 80 non-blank, non-`#`/`//`
/// lines must start with an HCL block keyword (followed by `"` for
/// resource/data/etc OR `{`).
pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(80) {
        considered += 1;
        let t = line.trim_start();
        if t.starts_with("<!DOCTYPE") || t.starts_with("<?xml") {
            return false;
        }
        if t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        let first: String = t.chars().take_while(|c| !c.is_whitespace()).collect();
        if HCL_BLOCKS.contains(&first.as_str()) {
            // Must be followed by either a quoted label (`resource
            // "type"`) or an opening brace (`locals {`) …
            let after = t[first.len()..].trim_start();
            // … and the line must END on the block's opening brace.
            //
            // An HCL block header always does: `locals {`,
            // `resource "aws_s3_bucket" "b" {`. An ES module import does NOT —
            // `import { Tabs, TabItem } from "@astrojs/starlight/components";`
            // has the whole specifier after the brace, and `import` is an HCL
            // block keyword, so every MDX file that pulls in a component
            // counted three of them and was claimed as HCL.
            //
            // That is not a cosmetic misroute: the HCL compactor strips `#`
            // comments, and a markdown heading IS `#`. Measured 2026-08-26 on
            // `html-to-markdown-rs/docs-site/.../tables.mdx`, 6 headings became
            // 0; on `errors.mdx`, 4 became 0. The same mechanism one format
            // over, where SQL's `--` ate the header of every diff hunk.
            //
            // Scope, measured over 26 683 markdown/MDX files and 21 real
            // HCL/Terraform files in the corpus: **265 documents claimed as HCL
            // before, 1 after**. The price is one HCL file of the 21 losing
            // detection — it writes its labels and its brace on separate lines —
            // and that file merely falls back to the generic fold. Nothing is
            // corrupted on that side; 265 documents stop losing every heading on
            // this one.
            if after.starts_with(['"', '{']) && t.trim_end().ends_with('{') {
                hits += 1;
            }
        }
    }
    hits >= 3 && considered > 0
}

/// Strip comments + collapse blank runs + trim trailing ws.
///
/// **Says so when it does.** This compactor used to return the stripped text
/// with no marker at all — MEASURED 2026-08-04 on a 323-byte `.tf` file:
/// 45 % removed, including `# pinned by policy` and `# default chosen for
/// latency`, with nothing in the output to say a comment had ever existed.
/// A Terraform comment is where the *why* of a resource lives, and an agent
/// reading a silently stripped file believes it has the whole file.
///
/// Its sibling on the code path already discloses
/// (`[tokenade:read-code-strip lang=… mode=comments]`); this one is the same
/// operation and owed the same note.
pub fn compact(text: &str) -> String {
    let mut removed = 0usize;
    let out = compact_inner(text, &mut removed);
    if removed == 0 {
        return out;
    }
    // One declarer for every comment stripper — this wording was here alone
    // for three weeks while seven other languages stripped in silence.
    crate::code_strip::declare_strip(text, out, "hcl", Some(removed))
}

fn compact_inner(text: &str, removed: &mut usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut in_block_comment = false;
    for line in text.lines() {
        // Block-comment `/* … */` pass. We rebuild the line by
        // dropping comment regions.
        let (line_clean, still_in) = strip_block_comment(line, in_block_comment);
        in_block_comment = still_in;
        // A `/* … */` region eaten here never reaches the `#`/`//` branches
        // below, so it has to be counted where it happens.
        let block_ate_something = line_clean.len() < line.len();
        let trimmed = line_clean.trim_end();
        if trimmed.is_empty() {
            if block_ate_something {
                *removed += 1;
            }
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        if block_ate_something {
            *removed += 1;
        }
        let t = trimmed.trim_start();
        if t.starts_with('#') || t.starts_with("//") {
            *removed += 1;
            continue;
        }
        let stripped = strip_inline_line_comment(trimmed);
        let stripped_t = stripped.trim_end();
        if stripped_t.is_empty() {
            *removed += 1;
            continue;
        }
        if stripped_t.len() < trimmed.len() {
            // An inline tail (`x = 1  # why`) — the line survives, the reason
            // does not. Counted, or a file whose comments are all inline would
            // report zero and print no note at all.
            *removed += 1;
        }
        blank_run = 0;
        out.push_str(stripped_t);
        out.push('\n');
    }
    out
}

/// `Savings`-yielding variant.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn strip_block_comment(line: &str, in_block: bool) -> (String, bool) {
    crate::code_strip::strip_block_comment_line(line, in_block, b"\"")
}

/// Cut the line at its first comment marker that is not inside a string:
/// `#` and `//`, in strings quoted with `"` or `'`.
///
/// This was a 40-line hand-rolled scanner, and `hcl.rs`, `proto.rs` and
/// `sql_source.rs` each had one. They differed by their quote set and their
/// marker and by NOTHING else — same string tracking, same backslash skip,
/// same `line[..i].trim_end()` tail. `code_strip::strip_inline_comment_line`
/// already expresses all three; the two-byte markers go through its `confirm`
/// hook, which is what `ini.rs` uses it for.
fn strip_inline_line_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"#/", b"\"'", b"\"'", true, |c, i| {
        c == b'#' || line.as_bytes().get(i + 1) == Some(&b'/')
    })
}

#[cfg(test)]
mod string_awareness_tests {

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
                    b'#' => {
                        cut = Some(i);
                        break;
                    }
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
    /// `/*` inside a STRING is not a comment. This module's block-comment
    /// stripper was a third copy of the one in `proto` and `sql_source`, but
    /// the degraded version: no string tracking at all, so a value containing
    /// `/*` swallowed everything up to the next `*/` — or to the end of the
    /// file when there was none. Centralising the three copies is what surfaced
    /// it.
    #[test]
    fn a_block_comment_marker_inside_a_string_is_not_a_comment() {
        let src = "resource \"aws_s3_bucket\" \"b\" {\n  \
                   bucket = \"logs/*/archive\"\n  \
                   prefix = \"a/*keep*/b\"\n  \
                   region = \"eu-west-1\"\n}\n";
        let out = super::compact(src);
        assert!(out.contains("logs/*/archive"), "value truncated:\n{out}");
        assert!(out.contains("a/*keep*/b"), "value truncated:\n{out}");
        assert!(
            out.contains("eu-west-1"),
            "lost everything after the marker:\n{out}"
        );
    }

    /// And a real block comment is still removed.
    #[test]
    fn a_real_block_comment_is_still_stripped() {
        // Repeated past the disclosure note's own cost: below it, `compact`
        // returns the input verbatim by design, and this test would be
        // asserting about a file that was deliberately left alone.
        let src = "/* header */\nresource \"aws_s3_bucket\" \"b\" {\n  \
                   bucket = \"data\" /* trailing note */\n}\n"
            .repeat(4);
        let out = super::compact(&src);
        assert!(!out.contains("trailing note"), "comment survived:\n{out}");
        assert!(!out.contains("header"), "comment survived:\n{out}");
        assert!(out.contains("data"), "lost the value:\n{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_TF: &str = "# Provider block\nterraform {\n  required_version = \">= 1.5\"\n  required_providers {\n    aws = {\n      source = \"hashicorp/aws\"\n      version = \"~> 5.0\"\n    }\n  }\n}\n\nprovider \"aws\" {\n  region = \"us-east-1\"\n}\n\n/* The S3 bucket holding\n   per-tenant uploads */\nresource \"aws_s3_bucket\" \"uploads\" {\n  bucket = \"my-uploads\"  # global namespace\n  tags = {\n    Owner = \"team-x\"  // owner tag\n  }\n}\n\nvariable \"env\" {\n  type    = string\n  default = \"prod\"\n}\n";

    #[test]
    fn detect_real_terraform() {
        assert!(detect(REAL_TF));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello world\n"));
        assert!(!detect("[section]\nkey = value\n"));
    }

    #[test]
    fn compact_strips_full_line_comments() {
        let out = compact(REAL_TF);
        assert!(!out.contains("Provider block"));
        assert!(!out.contains("per-tenant uploads"));
    }

    #[test]
    fn compact_strips_inline_hash_and_slashes() {
        let out = compact(REAL_TF);
        assert!(!out.contains("global namespace"));
        assert!(!out.contains("owner tag"));
        assert!(out.contains("bucket = \"my-uploads\""));
        assert!(out.contains("Owner = \"team-x\""));
    }

    #[test]
    fn compact_strips_block_comments() {
        // Padded past the note's own cost, or `compact` rightly returns the
        // input verbatim and there is nothing to assert about stripping.
        let input = "/* one liner */\nlocals {\n  /* multi\n     line */\n  x = 1\n}\n".repeat(6);
        let out = compact(&input);
        assert!(!out.contains("one liner"));
        assert!(!out.contains("multi"));
        // NOT `!out.contains("line")` — the disclosure note says "comment
        // line(s)", and an assertion that fires on the note instead of the
        // content proves nothing about the content.
        assert!(!out.contains("line */"));
        assert!(out.contains("locals {"));
        assert!(out.contains("x = 1"));
    }

    #[test]
    fn compact_preserves_all_blocks() {
        let out = compact(REAL_TF);
        assert!(out.contains("terraform {"));
        assert!(out.contains("required_version = \">= 1.5\""));
        assert!(out.contains("provider \"aws\" {"));
        assert!(out.contains("resource \"aws_s3_bucket\" \"uploads\" {"));
        assert!(out.contains("variable \"env\" {"));
    }

    #[test]
    fn compact_preserves_hash_inside_strings() {
        let input = "url = \"https://x.com/page#fragment\"\n";
        let out = compact(input);
        assert!(out.contains("page#fragment"));
    }

    #[test]
    fn a_strip_that_is_not_declared_did_not_happen() {
        // The defect this guards: 45 % of a `.tf` file removed — `# pinned by
        // policy`, `# default chosen for latency` — with NOTHING in the output
        // saying a comment had ever been there. A Terraform comment carries the
        // *why* of a resource, and silence reads as "this is the whole file".
        let out = compact(&REAL_TF.repeat(10));
        assert!(
            out.starts_with("[tokenade:hcl] "),
            "comments were stripped without saying so:\n{out}"
        );
        assert!(
            out.lines()
                .next()
                .unwrap()
                .contains("comment line(s) stripped"),
            "the note must say WHAT was removed: {:?}",
            out.lines().next()
        );
        // Untouched input earns no note — a banner on a file nothing happened
        // to is noise, and would teach the reader to ignore the banner.
        let clean = "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"data\"\n}\n".repeat(10);
        assert!(!compact(&clean).contains("[tokenade:hcl]"));
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        // A realistic file, not a 380-byte scrap: the fixed cost of the
        // disclosure note dominates a fixture too small to be representative,
        // and weakening the floor to accommodate that would have hidden the
        // real ratio instead of measuring it.
        let (_, sav) = compact_with_savings(&REAL_TF.repeat(10));
        assert!(
            sav.percent() >= 10.0,
            "expected ≥10% on commented .tf, got {:.1}%",
            sav.percent()
        );
    }
}

/// An ES module import is not an HCL `import` block.
#[cfg(test)]
mod es_import_is_not_hcl {
    use super::{compact, detect};

    /// Straight from `html-to-markdown-rs/docs-site/src/content/docs/tables.mdx`,
    /// trimmed to the three imports that were enough to reach `hits >= 3`.
    const MDX: &str = "---\ntitle: Tables\n---\n\nimport { Tabs, TabItem } from \"@astrojs/starlight/components\";\nimport { Content as Snip_rust } from \"../../snippets/rust.md\";\nimport { Content as Snip_python } from \"../../snippets/python.md\";\n\n# Extracting tables\n\nThe extractor walks the DOM and emits one row per `<tr>`.\n\n## Basic extraction\n\nPass the HTML in and read the rows out.\n\n## Nested tables\n\nA nested table becomes a nested list.\n";

    #[test]
    fn a_document_of_es_imports_is_not_claimed() {
        assert!(
            !detect(&full_article()),
            "an MDX file that imports components was read as HCL"
        );
    }

    /// The harm, stated as the thing a reader loses: the HCL compactor strips
    /// `#` comments, and a markdown heading IS `#`.
    /// Big enough to clear the strip's cost gate: on a short document
    /// `declare_strip` returns the whole file because the note would cost more
    /// than the strip saved, so a small fixture proves nothing here.
    fn full_article() -> String {
        let mut t = String::from(MDX);
        for i in 0..40 {
            t.push_str(&format!(
                "\n## Section {i}\n\nThe extractor walks the DOM and emits one row per table \
                 row, keeping cell order and dropping presentational wrappers, which is what \
                 section {i} is about.\n"
            ));
        }
        t
    }

    #[test]
    fn the_headings_would_have_been_deleted() {
        let article = full_article();
        let out = compact(&article);
        for h in ["# Extracting tables", "## Section 7", "## Section 39"] {
            assert!(
                !out.contains(h),
                "this test only means something if the HCL compactor really \
                 deletes headings — it kept {h:?}"
            );
        }
    }

    /// Real HCL still detects. A block header ends on its opening brace.
    #[test]
    fn real_hcl_still_detects() {
        let tf = "terraform {\n  required_version = \">= 1.5\"\n}\n\nprovider \"aws\" {\n  region = \"eu-west-1\"\n}\n\nresource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\"\n}\n";
        assert!(detect(tf), "a plain Terraform file stopped detecting");
    }

    /// And the rule is the brace at END of line, not merely a brace present.
    #[test]
    fn a_block_keyword_with_a_trailing_specifier_does_not_count() {
        let js = "import { a } from \"m\";\nimport { b } from \"n\";\nimport { c } from \"o\";\nimport { d } from \"p\";\n";
        assert!(!detect(js), "ES imports counted as HCL block headers");
    }
}

#[cfg(test)]
mod a_multiline_es_import_is_not_hcl {
    //! An earlier fix closed `import { X } from "y"` by requiring an HCL block header to
    //! END on its brace. The multi-line form ends on its brace too, so it
    //! walked straight through — 1 003 of 45 487 JS/TS files in this corpus
    //! were still claimed as HCL, on a compactor that strips `#` comments.

    #[test]
    fn a_multiline_import_block_does_not_count() {
        let ts = "import {\n  Tabs,\n  TabItem,\n} from \"@astrojs/starlight/components\";\n\nimport {\n  a,\n} from \"./a\";\n\nimport {\n  b,\n} from \"./b\";\n\nexport const x = 1;\n";
        assert!(
            !super::detect(ts),
            "three ES imports are not three HCL blocks"
        );
    }

    #[test]
    fn a_document_that_opens_with_multiline_imports_keeps_its_headings() {
        let mdx = "import {\n  Tabs,\n} from \"@astrojs/starlight/components\";\n\nimport {\n  Card,\n} from \"./card\";\n\nimport {\n  Note,\n} from \"./note\";\n\n# Title\n\nSome prose.\n\n## Section\n\nMore prose.\n\n### Detail\n\nEven more prose here.\n";
        assert!(!super::detect(mdx));
        let out = crate::auto::compact(mdx).1;
        for h in ["# Title", "## Section", "### Detail"] {
            assert!(out.contains(h), "heading {h:?} was dropped: {out}");
        }
    }

    #[test]
    fn real_hcl_still_detects() {
        let tf = "terraform {\n  required_version = \">= 1.5\"\n}\n\nprovider \"aws\" {\n  region = \"eu-west-1\"\n}\n\nresource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\"\n}\n\nlocals {\n  a = 1\n}\n";
        assert!(super::detect(tf));
    }

    #[test]
    fn the_other_block_keywords_are_untouched() {
        let tf = "moved {\n  from = a\n}\n\nremoved {\n  from = b\n}\n\ncheck \"c\" {\n  assert {\n    condition = true\n  }\n}\n";
        assert!(super::detect(tf), "only `import` was dropped from the list");
    }
}
