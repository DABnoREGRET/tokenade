//! GNU Make `Makefile` compactor.
//!
//! Covers `Makefile`, `makefile`, `GNUmakefile`, `*.mk` includes.
//! Heavily-commented Makefiles (Apache/MIT preambles + section
//! banners + recipe-explanation lines) typically lose 30-50% of
//! their tokens through this pass.
//!
//! What it strips:
//!   * full-line `#` comments (including license headers)
//!   * inline `#` tails (Make's comment delimiter — `;` is NOT a
//!     comment, it's a recipe separator)
//!   * blank-line runs → collapse to one
//!   * trailing whitespace per line
//!
//! What it preserves verbatim:
//!   * `target: deps` rule headers
//!   * tab-indented recipe bodies (every byte after `\t` matters —
//!     bash sees it untouched)
//!   * variable assignments (`VAR = …`, `VAR := …`, `VAR ?= …`,
//!     `VAR += …`, `export VAR=…`)
//!   * `.PHONY`, `.SUFFIXES`, `.SECONDARY`, `include`, `ifeq`,
//!     `ifneq`, `ifdef`, `ifndef`, `else`, `endif`, `define`,
//!     `endef` directives
//!   * line continuations (`\` at EOL) — joining them changes Make
//!     semantics for variable definitions

/// GNU Make directives we recognise as structural lines. Stripping
/// or compacting these would change build behaviour, so we never
/// touch them.
const DIRECTIVES: &[&str] = &[
    ".PHONY",
    ".SUFFIXES",
    ".SECONDARY",
    ".DEFAULT",
    ".PRECIOUS",
    ".INTERMEDIATE",
    ".DELETE_ON_ERROR",
    ".IGNORE",
    ".SILENT",
    ".EXPORT_ALL_VARIABLES",
    ".NOTPARALLEL",
    ".POSIX",
    "include",
    "-include",
    "sinclude",
    "ifeq",
    "ifneq",
    "ifdef",
    "ifndef",
    "else",
    "endif",
    "define",
    "endef",
    "override",
    "export",
    "unexport",
    "vpath",
    "VPATH",
];

/// Cheap heuristic detector: must have at least one `tab`-indented
/// recipe line AND at least one `target: deps` shape, OR ≥3 lines
/// matching the variable-assignment shape. Either signal is
/// reasonably specific to Make.
pub fn detect(text: &str) -> bool {
    // Split per #441: real tab-indented recipes vs Justfile-style
    // 4-space recipes. Rust / Python / TS source code has tons of
    // 4-space-indented lines following `fn name(arg: type) {` (lhs
    // matches target_shape because parens + spaces are permitted)
    // — the old single counter mistook every code file for a
    // Justfile. The strict path now requires a real `\t` recipe.
    let mut tab_recipes = 0usize; // real GNU Make (\t indent)
    let mut space_recipes = 0usize; // Justfile fallback (4-space)
    let mut targets = 0usize;
    let mut considered = 0usize;
    // `in_rule` stays TRUE across consecutive indented recipe lines
    // — real Justfile rules have 2-5 recipes each, so we must keep
    // counting them as recipes (not just the FIRST after a target).
    // Blank or non-indented non-comment lines exit the rule.
    let mut in_rule = false;
    for line in text.lines().take(100) {
        if line.is_empty() {
            in_rule = false;
            continue;
        }
        considered += 1;
        // Hard reject — these are unmistakably NOT a Makefile.
        if line.starts_with("<!DOCTYPE") || line.starts_with("<?xml") {
            return false;
        }
        // Recipe — line starting with a literal tab byte. GNU Make
        // requires tab. Justfile allows 4-space; we count those
        // separately so the acceptance gate can demand stronger
        // signal for the looser shape.
        if line.starts_with('\t') {
            tab_recipes += 1;
            in_rule = true;
            continue;
        }
        if in_rule && line.starts_with("    ") {
            space_recipes += 1;
            continue;
        }
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        // Directive lines never count as a target — short-circuit
        // before the `:` check (`include foo:bar.mk` shouldn't be
        // misread as a target).
        let first_token = t.split_whitespace().next().unwrap_or("");
        if DIRECTIVES.contains(&first_token) {
            continue;
        }
        // `target: deps` — colon NOT followed by `=` (which is
        // `:=`, an assignment). Bare `:` is the rule separator.
        if let Some((lhs_raw, after)) = t.split_once(':') {
            // Make sure it's not `://` (URL) or `key := value`.
            let is_assignment = after.starts_with('=');
            let is_url = after.starts_with("//");
            if !is_assignment && !is_url && !t.starts_with(':') {
                // The bit before `:` should look like a target name
                // (identifier / wildcards / path), not free text.
                let lhs = lhs_raw.trim();
                // Reject lhs that starts with a language keyword that
                // mimics target shape: `def name(args: type):` (Python),
                // `fn name(...) ->` is caught by `{` not `:`, but
                // `async fn`, `pub fn`, `class`, `for x in`, `if cond`,
                // `while`, `match`, `case`, `let`, `var`, `function`,
                // `func` (Go), etc. all have `:` somewhere later.
                // Strip leading visibility / async modifiers, then test
                // the first identifier-like word.
                let stripped = lhs
                    .trim_start_matches("pub ")
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub(super) ")
                    .trim_start_matches("async ")
                    .trim_start_matches("export ")
                    .trim_start_matches("public ")
                    .trim_start_matches("private ")
                    .trim_start_matches("protected ")
                    .trim_start_matches("static ");
                let first_word = stripped.split_whitespace().next().unwrap_or("");
                let is_code_keyword = matches!(
                    first_word,
                    "fn" | "def"
                        | "class"
                        | "function"
                        | "func"
                        | "let"
                        | "var"
                        | "const"
                        | "if"
                        | "for"
                        | "while"
                        | "match"
                        | "case"
                        | "switch"
                        | "elif"
                        | "else"
                        | "try"
                        | "except"
                        | "finally"
                        | "with"
                        | "return"
                        | "yield"
                        | "raise"
                        | "throw"
                        | "throws"
                        | "import"
                        | "from"
                        | "use"
                        | "impl"
                        | "trait"
                        | "interface"
                        | "struct"
                        | "enum"
                        | "type"
                        | "where"
                );
                if is_code_keyword {
                    in_rule = false;
                    continue;
                }
                let target_shape = !lhs.is_empty()
                    && lhs.chars().all(|c| {
                        c.is_alphanumeric()
                            || c == '_'
                            || c == '-'
                            || c == '.'
                            || c == '/'
                            || c == '%'
                            || c == '$'
                            || c == '('
                            || c == ')'
                            || c == ' '
                            || c == '*'
                    });
                if target_shape {
                    targets += 1;
                    in_rule = true;
                    continue;
                }
            }
        }
        // Anything we didn't match exits the rule.
        in_rule = false;
        // (Variable-assignment counting was removed in L82 along
        // with the `assignments >= 3` acceptance path that
        // mis-classified .env-style shell output as Makefile.)
    }
    // Acceptance gate (post-#441): TAB-only. The heuristic detector
    // ONLY accepts canonical GNU Makefile shape — at least 1 real
    // `\t` recipe + 1 target. Real Justfile files (4-space recipes)
    // ship with a `justfile` filename and get dispatched via the
    // extension table in auto.rs before this detector runs; the
    // heuristic path triggers on piped/stdin content with no
    // filename hint, where false-positives on Rust / Python / TS
    // source far outweigh the rare piped-Justfile case.
    //
    // History:
    //   - Pre-#410: accepted assignments-only (.env)
    //   - #410: required tab_recipes + targets together (but
    //     counted 4-space recipes alongside tab, so Rust source
    //     with `fn(arg: type) {` still false-matched)
    //   - #441: split counters, dropped the Justfile heuristic
    //     entirely; tab-only is unambiguous
    let _ = space_recipes; // counted for future diagnostics; not in gate
    if considered == 0 {
        return false;
    }
    tab_recipes >= 1 && targets >= 1
}

/// Strip comments + blank-run collapse + trailing-ws trim.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.lines() {
        // Tab-indented recipe — DO NOT modify. Strip trailing
        // whitespace is FORBIDDEN in recipes because trailing
        // whitespace can be significant inside multi-line shell
        // commands. Pass through verbatim plus the newline.
        if line.starts_with('\t') {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        // Full-line `#` comment.
        if t.starts_with('#') {
            continue;
        }
        // Inline `#` tail. NOTE: Make does NOT treat `;` as a
        // comment (unlike INI). The `#` rule is also conditional:
        // inside `$(shell …)` or within a variable assignment that
        // uses `$()`, a `#` is still a comment. We use a simple
        // detector that won't break correct files: only strip `#`
        // when preceded by whitespace AND the `#` is NOT escaped
        // (`\#`) AND not inside parentheses we can count.
        let stripped = strip_inline_make_comment(trimmed);
        if stripped.is_empty() {
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

/// Strip a `#` comment tail when conservative.
///
/// Rules:
///   * `\#` is escaped — keep
///   * `#` inside balanced `(...)` — keep (could be `$(shell …)`)
///   * `#` after whitespace at depth 0 — strip from there to EOL
fn strip_inline_make_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut paren_depth = 0i32;
    let mut cut: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if c == b'(' || c == b'{' {
            paren_depth += 1;
        } else if c == b')' || c == b'}' {
            paren_depth = (paren_depth - 1).max(0);
        } else if c == b'#' && paren_depth == 0 {
            // Require leading whitespace so `target:dep#1.0` (rare
            // but legal) doesn't get truncated.
            let prev_is_ws = i == 0 || matches!(bytes[i - 1], b' ' | b'\t');
            if prev_is_ws {
                cut = Some(i);
                break;
            }
        }
        i += 1;
    }
    match cut {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APACHE_MAKEFILE: &str = "#\n# Jake JavaScript build tool\n# Copyright 2112 Matthew Eernisse\n#\n# Licensed under the Apache License, Version 2.0\n#\n.PHONY: all build install clean\nPREFIX=/usr/local\nDESTDIR=\nVERSION := 0.1.0\n\n# default target\nall: build\n\nbuild:\n\t@echo \"Compiling sources\"\n\t@npm run build\n\ntest: build\n\t@npm test\n\nclean:\n\trm -rf dist/\n";

    const JUSTFILE: &str = "# Generate code and run lints and tests\ntest-local: gen lint test\n    @just test --features const-generics\n# Produce generated code\ngen:\n    cargo run --package generate\n    cargo fmt\n# Update lockfiles\nup:\n    nix flake update\n    cargo update\n";

    #[test]
    fn detect_justfile_space_indented_via_heuristic_dropped() {
        // Post-#441: the heuristic detector NO LONGER matches Justfile
        // shape (4-space recipes) because the same shape is too easily
        // produced by Rust/Python source. Real Justfiles still get
        // routed to this compactor via the `justfile` filename
        // extension dispatch in auto.rs. The heuristic path triggers
        // on piped/stdin content without a filename hint — there,
        // false-positives on code source far outweigh the rare
        // piped-Justfile case.
        assert!(!detect(JUSTFILE));
    }

    #[test]
    fn detect_classic_makefile() {
        assert!(detect(APACHE_MAKEFILE));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello world\nthis is just text\n"));
        assert!(!detect("<!DOCTYPE html>\n"));
        assert!(!detect("[section]\nkey = value\nkey2 = value\n"));
    }

    #[test]
    fn detect_rejects_assignment_only_envfile() {
        // L82 — tightened: assignment-only files (.env, shell output
        // with KEY=VALUE lines) MUST be rejected by the Makefile
        // detector. The env_dump detector (which runs FIRST in the
        // auto chain) catches the legitimate env case. Previous
        // behaviour: `detect == true` for `.env` shape, which then
        // ran the Makefile compactor → 63 production false positives
        // / 2.7% saved on non-Makefile shell output.
        let m = "FOO = bar\nBAZ ?= qux\nQUUX := 1\nOUTPUT += a b c\n";
        assert!(!detect(m), "assignment-only must not match makefile");
    }

    #[test]
    fn detect_rejects_rust_source_with_function_signatures() {
        // Regression for #441: grep -A output of Rust code was being
        // tagged as Makefile because `fn name(arg: type) {` matches
        // target_shape (lhs has parens + spaces, both in allowed set)
        // and 4-space-indented body lines counted as Justfile recipes.
        // Production: 21 events / 24h at sav_pct=4% — almost pure
        // wasted compaction.
        let rust = r#"pub fn detect(text: &str) -> bool {
    let mut tab_recipes = 0usize;
    let mut targets = 0usize;
    let mut considered = 0usize;
    for line in text.lines().take(100) {
        if line.is_empty() {
            continue;
        }
    }
    targets >= 1
}

pub fn other(arg: &str) -> bool {
    let x = 1;
    x > 0
}
"#;
        assert!(
            !detect(rust),
            "Rust source with function sigs + 4-space bodies must NOT be classified as Makefile"
        );
    }

    #[test]
    fn detect_rejects_python_source_with_def_signatures() {
        // Same class: `def foo(arg: type):` is target-shaped + 4-space body.
        let py = r#"def detect(text: str) -> bool:
    lines = text.splitlines()
    targets = 0
    for line in lines:
        if line.startswith("target:"):
            targets += 1
    return targets > 0

def other(arg: int) -> bool:
    return arg > 0
"#;
        assert!(
            !detect(py),
            "Python source must NOT be classified as Makefile"
        );
    }

    #[test]
    fn compact_strips_license_header() {
        let out = compact(APACHE_MAKEFILE);
        assert!(!out.contains("Jake JavaScript"));
        assert!(!out.contains("Apache License"));
        assert!(!out.contains("default target"));
    }

    #[test]
    fn compact_preserves_targets_and_recipes() {
        let out = compact(APACHE_MAKEFILE);
        assert!(out.contains(".PHONY: all build install clean"));
        assert!(out.contains("PREFIX=/usr/local"));
        assert!(out.contains("VERSION := 0.1.0"));
        assert!(out.contains("all: build"));
        assert!(out.contains("\t@echo \"Compiling sources\""));
        assert!(out.contains("\t@npm run build"));
        assert!(out.contains("test: build"));
        assert!(out.contains("\trm -rf dist/"));
    }

    #[test]
    fn compact_never_touches_recipe_lines() {
        // Trailing whitespace in a recipe shouldn't be stripped — it
        // could be significant inside a heredoc-style continuation.
        let input = "all:\n\t@cmd   \t\n";
        let out = compact(input);
        assert!(
            out.contains("\t@cmd   \t\n"),
            "recipe must round-trip verbatim, got: {out:?}"
        );
    }

    #[test]
    fn compact_handles_inline_comment_outside_paren() {
        let input = "VAR = 1   # this is the version\n";
        let out = compact(input);
        assert_eq!(out, "VAR = 1\n");
    }

    #[test]
    fn compact_preserves_hash_inside_shell_subst() {
        // `$(shell uname -a)` has no `#`, but `# fragment` inside
        // `$(…)` should survive. (We don't have such a real Make
        // pattern, but the strip_inline rule must respect the
        // parenthesis depth.)
        let input = "WHOAMI = $(shell id -u # not a comment here)\n";
        let out = compact(input);
        assert!(out.contains("not a comment here"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "all:\n\n\n\n\tcmd\n";
        let out = compact(input);
        assert_eq!(out, "all:\n\n\tcmd\n");
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        let (_, sav) = compact_with_savings(APACHE_MAKEFILE);
        assert!(
            sav.percent() >= 25.0,
            "expected ≥25% on commented Makefile, got {:.1}%",
            sav.percent()
        );
    }
}
