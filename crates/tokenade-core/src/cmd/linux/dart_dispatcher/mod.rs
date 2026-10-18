//! `flutter analyze` / `dart analyze` and `flutter pub get` / `dart pub get`
//! — what the Dart toolchain prints, read for what an agent acts on.
//!
//! Captured 2026-10-05 (Flutter 3.47.6, Dart 3.13.5) on a real app,
//! `flutter/gallery`, against today's SDK: 529 diagnostics, 121 KB.
//!
//! * The analyzer prints one line per diagnostic, in two layouts:
//!   `flutter analyze` → `  error • message • lib/a.dart:9:8 • rule`,
//!   `dart analyze` → `  error - lib/a.dart:9:8 - message - rule`. One
//!   cause repeats: 293 lines `Undefined name 'GalleryLocalizations'`, 90
//!   `Target of URI doesn't exist` — a missing generated file.
//! * Until now this went to the generic fold: the first ~25 diagnostics and
//!   an elision, so the agent saw neither how many of each there were nor
//!   which files they were in.
//! * `pub get` lists every dependency, changed or not: 60 of its 104 lines
//!   are `  name version (newer available)` — a package that did not change.
//!
//! Diagnostics are grouped by severity, rule and message — the message once,
//! the count, and where (the first [`LOCATIONS_SHOWN`]); errors first. Every
//! other line is kept in order.

/// Where one diagnostic was raised, at most this many listed; the count says
/// how many more.
const LOCATIONS_SHOWN: usize = 12;

struct Diag<'a> {
    severity: &'a str,
    loc: &'a str,
    message: &'a str,
    rule: &'a str,
}

fn diagnostic(line: &str) -> Option<Diag<'_>> {
    let t = line.trim();
    let severity = ["error", "warning", "info"].into_iter().find(|s| t.starts_with(s))?;
    let rest = &t[severity.len()..];
    let (loc, message, rule) = if let Some(rest) = rest.strip_prefix(" • ") {
        let (rest, rule) = rest.rsplit_once(" • ")?;
        let (message, loc) = rest.rsplit_once(" • ")?;
        (loc, message, rule)
    } else {
        let rest = rest.strip_prefix(" - ")?;
        let (loc, rest) = rest.split_once(" - ")?;
        let (message, rule) = rest.rsplit_once(" - ")?;
        (loc, message, rule)
    };
    let is_loc = loc.rsplit(':').take(2).all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let is_rule = !rule.is_empty() && rule.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    (is_loc && is_rule).then_some(Diag { severity, loc, message, rule })
}

pub fn compact_analyze(text: &str) -> Option<String> {
    let diags: Vec<Diag> = text.lines().filter_map(diagnostic).collect();
    if diags.len() < 2 {
        return None;
    }
    let mut groups: Vec<(&str, &str, &str, Vec<&str>)> = Vec::new();
    for sev in ["error", "warning", "info"] {
        for d in diags.iter().filter(|d| d.severity == sev) {
            match groups.iter_mut().find(|g| g.0 == sev && g.1 == d.rule && g.2 == d.message) {
                Some(g) => g.3.push(d.loc),
                None => groups.push((sev, d.rule, d.message, vec![d.loc])),
            }
        }
    }
    let mut block = String::new();
    for (sev, rule, message, locs) in &groups {
        let more = locs.len().saturating_sub(LOCATIONS_SHOWN);
        block.push_str(&format!(
            "{sev} {rule} ×{}: {message}\n  in {}{}\n",
            locs.len(),
            locs[..locs.len().min(LOCATIONS_SHOWN)].join(", "),
            if more > 0 { format!(", +{more} more not shown") } else { String::new() }
        ));
    }
    let mut out = String::new();
    let mut placed = false;
    for line in text.lines() {
        if diagnostic(line).is_some() {
            if !placed {
                out.push_str(&block);
                placed = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out.len() < text.len()).then_some(out)
}

/// `  name 1.2.3 (1.4.0 available)`: a dependency `pub get` left as it was.
/// Changed ones are marked `>` `<` `+` `-`; a discontinued one is kept.
fn unchanged_dependency(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("  ") else { return false };
    let mut words = rest.split_whitespace();
    let (Some(name), Some(version)) = (words.next(), words.next()) else { return false };
    name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && version.starts_with(|c: char| c.is_ascii_digit())
        && !rest.contains("discontinued")
}

pub fn compact_pub(text: &str) -> Option<String> {
    let unchanged = text.lines().filter(|l| unchanged_dependency(l)).count();
    if unchanged < 5 {
        return None;
    }
    let mut out = String::new();
    let mut said = false;
    for line in text.lines() {
        if unchanged_dependency(line) {
            if !said {
                out.push_str(&format!("  [{unchanged} dependencies unchanged — newer versions exist for some: `pub outdated`]\n"));
                said = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out.len() < text.len()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/linux/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// The real 529 diagnostics, in both layouts: each cause once, with its
    /// count and first locations; every rule still named; the total kept.
    #[test]
    fn an_analysis_names_each_cause_once_with_its_count() {
        for name in ["flutter_analyze.txt", "dart_analyze.txt"] {
            let text = fixture(name);
            let out = compact_analyze(&text).unwrap();
            assert!(out.contains("error undefined_identifier ×293: Undefined name 'GalleryLocalizations'."), "{name}:\n{out}");
            assert!(out.contains("lib/data/demos.dart:123:27"), "{name}:\n{out}");
            for rule in ["uri_does_not_exist", "undefined_class", "unreachable_switch_default", "deprecated_member_use", "non_constant_list_element"] {
                assert!(out.contains(&format!(" {rule} ×")), "{rule} missing in {name}:\n{out}");
            }
            assert!(out.contains("529 issues found."), "{name}:\n{out}");
            assert!(out.find("error ").unwrap() < out.find("info ").unwrap(), "{name}: errors first");
            assert!(out.len() * 4 < text.len(), "{name}: {} → {}", text.len(), out.len());
        }
    }

    /// Through the whole pipeline, each cause keeps its locations under it.
    /// They were first written `  at …`: the stack-trace reader took them for
    /// frames, pulled them under the first heading and dropped them from a
    /// short analysis (`dart analyze lib/main.dart`, real run).
    #[test]
    fn the_locations_stay_under_their_cause_through_the_pipeline() {
        let argv: Vec<String> = ["flutter", "analyze"].iter().map(|s| s.to_string()).collect();
        let out = crate::filter::filter_command(&argv, &fixture("flutter_analyze.txt")).output;
        assert!(out.contains("Try correcting the name to one that is defined, or defining the name\n  in lib/data/demos.dart:123:27,"), "{out}");
        let short = "Analyzing main.dart...\n\n  error - main.dart:9:8 - Target of URI doesn't exist: 'package:x/y.dart'. - uri_does_not_exist\n  error - main.dart:68:18 - Undefined name 'G'. - undefined_identifier\n  error - main.dart:72:31 - Undefined name 'G'. - undefined_identifier\n\n3 issues found.\n";
        let argv: Vec<String> = ["dart", "analyze", "main.dart"].iter().map(|s| s.to_string()).collect();
        let out = crate::filter::filter_command(&argv, short).output;
        assert!(out.contains("main.dart:68:18, main.dart:72:31"), "{out}");
    }

    #[test]
    fn a_line_that_is_not_a_diagnostic_is_left_alone() {
        assert!(diagnostic("Analyzing fgallery...").is_none());
        assert!(diagnostic("529 issues found. (ran in 12.6s)").is_none());
        assert!(diagnostic("info: something - else - here").is_none());
        let d = diagnostic("   info - test/a.dart:28:20 - 'x' is deprecated - y - deprecated_member_use").unwrap();
        assert_eq!((d.loc, d.message, d.rule), ("test/a.dart:28:20", "'x' is deprecated - y", "deprecated_member_use"));
    }

    /// `pub get`: unchanged dependencies counted, every change and the
    /// discontinued package kept.
    #[test]
    fn pub_get_keeps_what_changed() {
        let text = fixture("flutter_pub_get.txt");
        let out = compact_pub(&text).unwrap();
        assert!(out.contains("[60 dependencies unchanged"), "{out}");
        assert!(out.contains("> analyzer 13.3.0 (was 5.13.0)"), "{out}");
        assert!(out.contains("+ cli_config 0.2.0"), "{out}");
        assert!(out.contains("adaptive_breakpoints 0.1.7 (discontinued)"), "{out}");
        assert!(out.contains("Changed 26 dependencies!"), "{out}");
        assert!(!out.contains("args 2.4.2"), "{out}");
    }
}
