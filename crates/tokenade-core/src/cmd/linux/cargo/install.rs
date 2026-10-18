//! Extracted from `filter.rs` in the L94/L92 refactor.
//! Body preserved verbatim — only the home changed.

use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut installed: Vec<String> = Vec::new();
    let mut destinations: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut already_installed: Vec<String> = Vec::new();
    let mut unclaimed = 0usize;

    for raw in s.lines() {
        let t = raw.trim_start();
        if let Some(rest) = t.strip_prefix("Installed package `") {
            // `Installed package `foo v1.0.0 (registry+…)` (executable `foo`)`
            installed.push(rest.split('`').next().unwrap_or(rest).to_string());
        } else if let Some(rest) = t.strip_prefix("Installing ") {
            // `Installing /home/u/.cargo/bin/foo`
            destinations.push(rest.to_string());
        } else if t.starts_with("Compiling ")
            || t.starts_with("Checking ")
            || t.starts_with("Downloaded ")
            || t.starts_with("Downloading ")
        {
            // Build/network chatter — intentionally dropped, and COUNTED: the
            // note promises "compile/download progress and anything
            // unrecognised", so leaving these out of the tally would make the
            // banner understate what is missing. Counting only the surprises
            // reported 4 where 6 lines were absent.
            unclaimed += 1;
        } else if let Some(rest) = t.strip_prefix("Ignored package `") {
            // `Ignored package `foo v1.0.0` is already installed`
            already_installed.push(rest.split('`').next().unwrap_or(rest).to_string());
        } else if t.starts_with("error") || t.starts_with("error:") {
            errors.push(raw.to_string());
        } else if t.starts_with("warning") || t.starts_with("warning:") {
            warnings.push(raw.to_string());
        } else if !t.is_empty() {
            // Everything else. The five buckets above are a WHITELIST, so a
            // line nobody claimed is simply gone — and that is not only build
            // chatter: `note: the linker \`cc\` failed with signal 9 (out of
            // memory)` starts with none of the recognised prefixes, and it is
            // the line that explains the failure. MEASURED on a realistic
            // 9-line install, 6 lines fell here.
            //
            // The right precedent is three lines up: `warnings (N)` states the
            // TRUE total even when the list it shows is capped at 20. Say the
            // same thing about what was dropped.
            unclaimed += 1;
        }
    }

    let mut out = String::with_capacity(512);
    // #469: each section used to prefix with `\n` which left the
    // compacted output starting with a blank line. Build sections
    // without leading newlines and use `join_section` to insert
    // separators only BETWEEN populated sections.
    let mut push_section = |label: String, items: Vec<String>| {
        if items.is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&label);
        for it in items {
            out.push_str("  ");
            out.push_str(&it);
            out.push('\n');
        }
    };
    push_section("installed:\n".to_string(), installed.clone());
    push_section("already installed:\n".to_string(), already_installed.clone());
    push_section("destinations:\n".to_string(), destinations.clone());
    let warn_capped: Vec<String> = warnings.iter().take(20).cloned().collect();
    push_section(format!("warnings ({}):\n", warnings.len()), warn_capped);
    push_section(format!("errors ({}):\n", errors.len()), errors.clone());
    if installed.is_empty() && already_installed.is_empty() && errors.is_empty() && warnings.is_empty() {
        // #548/#549 — no recognised cargo-install signal. Return the
        // ANSI-stripped input verbatim rather than routing through
        // generic (which would dedup/truncate and could lose useful
        // context). Same fix shape as cargo::build #548. The filter
        // label stays `cargo-install`; if savings are tiny the
        // dashboard noise filter (#540/#541) drops the entry.
        if !s.trim().is_empty() {
            return s;
        }
    }
    let mut out = out.trim_end().to_string();
    if unclaimed > 0 && !out.is_empty() {
        out.push_str(&format!(
            "\n[tokenade:cargo-install] {unclaimed} other line(s) not shown \
             (compile/download progress and anything unrecognised) — \
             `tokenade expand-ref <hash>` from the banner below, or re-run \
             through `tokenade raw`"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;
    /// What no bucket claimed is COUNTED, not simply gone.
    ///
    /// The five buckets are a whitelist, so `note: the linker `cc` failed with
    /// signal 9` — which starts with none of the recognised prefixes — used to
    /// vanish beside a successful `Installed package`. The count follows the
    /// precedent this same function already sets with `warnings (N)`, whose
    /// label states the true total even when the list is capped at 20.
    #[test]
    fn what_no_bucket_claimed_is_counted() {
        let input = "\
    Updating crates.io index
  Installing ripgrep v14.1.0
   Compiling libc v0.2.155
   Compiling memchr v2.7.4
Blocking waiting for file lock on package cache
    Finished `release` profile [optimized] target(s) in 41.2s
  Installing /home/u/.cargo/bin/rg
   Installed package `ripgrep v14.1.0` (executable `rg`)
note: the linker `cc` failed with signal 9 (out of memory)
";
        let out = compact(input);
        assert!(
            out.contains("[tokenade:cargo-install]"),
            "the drop must announce itself: {out}"
        );
        assert!(
            out.contains("6 other line(s) not shown"),
            "the count must be the real one: {out}"
        );
        // Anti-vacuity, both directions: what IS recognised still shows, and a
        // run with nothing unrecognised carries no note at all.
        assert!(out.contains("ripgrep v14.1.0"), "the install line is kept: {out}");
        let clean = "  Installing /home/u/.cargo/bin/rg\n   Installed package `ripgrep v14.1.0` (executable `rg`)\n";
        let out2 = compact(clean);
        assert!(
            !out2.contains("[tokenade:cargo-install]"),
            "nothing was dropped, so nothing may be claimed: {out2}"
        );
    }


    #[test]
    fn fresh_install_summary_no_leading_newline() {
        // Regression for #469: output used to begin with "\ninstalled:\n…"
        // which produced an extra blank line in cmd_wrap output.
        let input = "  Downloading foo v1.0.0\n   Compiling bar v0.1.0\n   Compiling foo v1.0.0\n    Finished `release` profile [optimized] target(s) in 12.3s\n  Installing /home/u/.cargo/bin/foo\n   Installed package `foo v1.0.0 (registry+...)` (executable `foo`)\n";
        let out = compact(input);
        assert!(!out.starts_with('\n'), "no leading newline; got: {out:?}");
        assert!(out.contains("installed:"), "missing installed section");
        assert!(out.contains("foo v1.0.0"), "missing pkg name");
        assert!(out.contains("destinations:"), "missing destinations section");
        assert!(out.contains("/home/u/.cargo/bin/foo"), "missing destination path");
        // Build chatter (Downloading, Compiling) must be dropped.
        assert!(!out.contains("Compiling"), "Compiling chatter leaked");
        assert!(!out.contains("Downloading"), "Downloading chatter leaked");
    }

    #[test]
    fn already_installed_routed_to_its_own_section() {
        let input = "    Updating crates.io index\n     Ignored package `foo v1.0.0` is already installed, use --force to override\n";
        let out = compact(input);
        assert!(out.contains("already installed:"), "missing section: {out}");
        assert!(out.contains("foo v1.0.0"), "missing package name");
        // The `installed:` (without `already`) section is for FRESH installs.
        // Match a literal line — `^installed:` at line-start — not a substring,
        // since "already installed:" contains "installed:" as a substring.
        assert!(
            !out.lines().any(|l| l == "installed:"),
            "must NOT have a fresh 'installed:' section: {out}"
        );
    }

    #[test]
    fn warnings_section_includes_count_and_caps_at_20() {
        let mut input = String::from("   Installed package `bigcrate v0.1.0`\n");
        for i in 0..25 {
            let _ = writeln!(input, "warning: deprecated thing {i}");
        }
        let out = compact(&input);
        assert!(out.contains("warnings (25):"), "must show full count: {out}");
        // 20 warnings + the header line = 21 lines starting with "warning" or "warnings"
        let warning_body_lines = out.lines().filter(|l| l.trim_start().starts_with("warning") && !l.trim_start().starts_with("warnings (")).count();
        assert_eq!(warning_body_lines, 20, "cap at 20 warning lines, got {warning_body_lines}");
    }

    #[test]
    fn errors_section_shows_every_error() {
        // Errors are actionable signal — keep ALL of them (no cap).
        let mut input = String::new();
        for i in 0..3 {
            let _ = writeln!(input, "error[E0001]: failure {i}");
        }
        let out = compact(&input);
        assert!(out.contains("errors (3):"));
        for i in 0..3 {
            assert!(out.contains(&format!("failure {i}")), "error {i} missing");
        }
    }

    #[test]
    fn no_signal_falls_back_to_ansi_stripped_input() {
        // #548/#549 — when nothing matches any heuristic, return the
        // ANSI-stripped input verbatim (was: generic which dedup/
        // truncated and could lose context). Same fix shape as
        // cargo::build.
        let input = "\x1b[32mtotally arbitrary cargo install output that matches nothing\x1b[0m";
        let out = compact(input);
        assert!(!out.is_empty(), "must not return empty on no-signal input");
        assert!(
            out.contains("totally arbitrary"),
            "fallback must preserve every line; got: {out:?}"
        );
        assert!(
            !out.contains("\x1b["),
            "ANSI escapes must still be stripped in fallback; got: {out:?}"
        );
    }

    #[test]
    fn output_trim_end_no_trailing_newline() {
        let input = "   Installed package `foo v1.0.0` (executable `foo`)\n";
        let out = compact(input);
        assert!(!out.ends_with('\n'), "must not end with newline: {out:?}");
    }
}


