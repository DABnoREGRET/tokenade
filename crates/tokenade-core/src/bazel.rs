//! Bazel `BUILD` / `BUILD.bazel` / `WORKSPACE` / `*.bzl` compactor.
//!
//! Starlark (a Python dialect) syntax. Real monorepo BUILD files
//! often have 200+ lines with extensive copyright headers, rule
//! documentation, and conditional `select({...})` clauses — typical
//! 20-35% drop after this pass.
//!
//! What it strips:
//!   * full-line `#` comments
//!   * inline `# …` comment tails (quote-aware so `# fragment`
//!     inside `"…"` URLs survives)
//!   * blank-line runs → one max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every Bazel rule call (`cc_library`, `py_binary`,
//!     `java_test`, `go_library`, `genrule`, `filegroup`, …)
//!   * `load(…)` statements (Starlark imports)
//!   * `select({…})` ladders
//!   * `glob(…)` calls
//!   * `package(…)` / `package_group(…)` / `licenses(…)` directives
//!   * indentation (Starlark is whitespace-sensitive)

/// Bazel rule names we recognise as proof of a BUILD file. Includes
/// the core built-in rules plus very common community providers
/// (rules_go, rules_rust, rules_python, rules_nodejs).
const BAZEL_RULES: &[&str] = &[
    // Built-in & rules_cc.
    "cc_library",
    "cc_binary",
    "cc_test",
    "cc_toolchain",
    "cc_proto_library",
    // rules_java / rules_kotlin.
    "java_library",
    "java_binary",
    "java_test",
    "java_plugin",
    "java_proto_library",
    "kt_jvm_library",
    "kt_jvm_binary",
    "kt_jvm_test",
    // rules_python.
    "py_library",
    "py_binary",
    "py_test",
    "py_proto_library",
    "pip_install",
    "py_runtime",
    // rules_go / gazelle.
    "go_library",
    "go_binary",
    "go_test",
    "go_proto_library",
    "go_repository",
    "gazelle",
    // rules_rust.
    "rust_library",
    "rust_binary",
    "rust_test",
    "rust_proc_macro",
    // rules_nodejs / rules_js.
    "nodejs_binary",
    "nodejs_test",
    "ts_library",
    "ts_project",
    "js_library",
    "js_binary",
    "esbuild",
    // rules_proto / gRPC.
    "proto_library",
    "grpc_library",
    "cpp_grpc_library",
    "py_grpc_library",
    // Generic / built-in.
    "filegroup",
    "genrule",
    "alias",
    "config_setting",
    "package_group",
    "exports_files",
    "bind",
    "test_suite",
    // Imports.
    "load",
    "package",
    "licenses",
    "compatibility_proto",
    // rules_pkg / containers.
    "container_image",
    "container_push",
    "docker_build",
    "pkg_tar",
    // Toolchain.
    "toolchain",
    "platform",
    "constraint_setting",
    "constraint_value",
];

/// Cheap heuristic detector. Among the first 80 non-blank, non-`#`
/// lines, ≥3 must start with a known Bazel rule name (case-sensitive
/// first token before `(`).
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
        if t.starts_with('#') {
            continue;
        }
        let first: String = t
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '(')
            .collect();
        if BAZEL_RULES.contains(&first.as_str()) {
            hits += 1;
            // #599 — distinctive: `_`-bearing multi-word rule names
            // (cc_library, py_binary, java_test, target_link_libraries
            // pattern, …). These are unique to Bazel/Starlark; no
            // other source language has top-level tokens with the same
            // shape. Without this gate, a Java/Go/Kotlin file with
            // `package` + 2 of bind/licenses/load matches the
            // generic-only path. Matches the cmake #589/#597 + proto
            // #598 pattern.
            if first.contains('_') {
                distinctive += 1;
            }
        }
    }
    hits >= 3 && distinctive >= 1 && considered > 0
}

/// Strip `#` comments + collapse blank runs + trim trailing ws.
/// Indentation is preserved BYTE-FOR-BYTE because Starlark uses
/// indentation for block scoping.
pub fn compact(text: &str) -> String {
    crate::code_strip::declare_strip(text, strip_only(text), "bazel", None)
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
    crate::code_strip::strip_line_language(text, |t| t.starts_with('#'), strip_inline_comment)
}

/// `Savings`-yielding variant.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Starlark: `#` comments, both quote styles, backslashes escape in both.
fn strip_inline_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"#", b"\"'", b"\"'", true, |_, _| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_BUILD: &str = "# Copyright 2026 Example Corp\n# Licensed under Apache 2.0\nload(\"@rules_cc//cc:defs.bzl\", \"cc_binary\", \"cc_library\")\n\nconfig_setting(\n    name = \"windows\",\n    constraint_values = [\"@platforms//os:windows\"],\n)\n\n# Main library with warning suppressions\ncc_library(\n    name = \"my_lib\",\n    srcs = [\"src/core.cc\"],\n    hdrs = glob([\"include/*.h\"]),\n    copts = select({\n        \":windows\": [\"/W4\"],\n        \"//conditions:default\": [\"-Wall\"],\n    }),\n)\n\ncc_binary(\n    name = \"my_bin\",  # binary target\n    srcs = [\"src/main.cc\"],\n    deps = [\":my_lib\"],\n)\n";

    #[test]
    fn detect_real_build() {
        assert!(detect(REAL_BUILD));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello\nworld\n"));
        assert!(!detect("import os\nprint('hi')\n"));
    }

    #[test]
    fn detect_rejects_generic_only_hits() {
        // #599 — load/package/licenses/bind are generic Bazel tokens
        // that overlap with Java/Go/Kotlin source. Without the
        // distinctive `_`-bearing rule requirement, a file with
        // these alone (no cc_library/py_binary/etc.) would match.
        // Construct an adversarial fixture that hits 3 generic
        // BAZEL_RULES tokens but has no `_`-bearing call.
        let fake = "load = something\n\
                    package = \"foo\"\n\
                    licenses = [\"Apache-2.0\"]\n\
                    bind = lookup_value\n";
        assert!(
            !detect(fake),
            "generic-only-token text must NOT be detected as bazel; got: {:?}",
            fake
        );
    }

    #[test]
    fn detect_accepts_real_build_via_cc_library_distinctive() {
        // Sanity: the REAL_BUILD fixture above passes via cc_library
        // (distinctive) + load + config_setting + cc_binary. Triple-
        // covered. Add an explicit minimal case for documentation.
        let minimal = "load(\"@rules_cc//cc:defs.bzl\", \"cc_binary\")\n\
                       package(default_visibility = [\"//visibility:public\"])\n\
                       cc_binary(\n  name = \"x\",\n  srcs = [\"x.cc\"],\n)\n";
        assert!(detect(minimal));
    }

    #[test]
    fn compact_strips_full_line_comments() {
        let out = compact(REAL_BUILD);
        assert!(!out.contains("Copyright"));
        assert!(!out.contains("Apache 2.0"));
        assert!(!out.contains("Main library"));
    }

    #[test]
    fn compact_strips_inline_comment_tails() {
        let out = compact(REAL_BUILD);
        assert!(!out.contains("binary target"));
        assert!(out.contains("name = \"my_bin\""));
    }

    #[test]
    fn compact_preserves_all_rules_and_calls() {
        let out = compact(REAL_BUILD);
        assert!(out.contains("load(\"@rules_cc//cc:defs.bzl\""));
        assert!(out.contains("config_setting("));
        assert!(out.contains("cc_library("));
        assert!(out.contains("cc_binary("));
        assert!(out.contains("glob([\"include/*.h\"])"));
        assert!(out.contains("select({"));
    }

    #[test]
    fn compact_preserves_hash_inside_string() {
        let input = "name = \"my#tag\"\n";
        let out = compact(input);
        assert!(out.contains("my#tag"));
    }

    #[test]
    fn compact_preserves_indentation_byte_for_byte() {
        // Starlark uses indentation for scoping — every byte matters.
        let input = "cc_library(\n    name = \"x\",\n        srcs = [],\n)\n";
        let out = compact(input);
        assert!(out.contains("    name = \"x\","));
        assert!(out.contains("        srcs = []"));
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

        let (_, sav) = crate::archetypes::savings_of(REAL_BUILD, strip_only);
        assert!(
            sav.percent() >= 15.0,
            "expected ≥15% on commented BUILD, got {:.1}%",
            sav.percent()
        );
    }
}
