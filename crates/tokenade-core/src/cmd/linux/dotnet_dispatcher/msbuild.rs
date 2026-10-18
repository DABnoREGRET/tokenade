//! `dotnet build` / `dotnet test` — what MSBuild and VSTest print, read for
//! what an agent acts on.
//!
//! Captured 2026-10-05 on a real solution (SDK 10.0.112, a class library with
//! 122 warnings, an xUnit project with 3 failing tests of 65), in English and
//! French:
//!
//! * MSBuild prints every diagnostic TWICE — where it occurs, then again in
//!   the summary after `Build succeeded.` / `Build FAILED.`: 244 lines, 122
//!   distinct. The keyword (`warning CS0219`, `error CS8954`) is the same in
//!   every UI language; the sentences around it are not.
//! * Until now everything went to the `cargo build` reader, which does not know
//!   this shape: the generic fold kept a count per pattern and one example
//!   (`[×80 lines · distinct N=40 · e.g. N=9]`), so the agent could not tell
//!   WHICH files warned.
//! * VSTest names each failure twice (`[xUnit.net …] X [FAIL]`, then
//!   `Failed X [< 1 ms]`) and ends each stack trace in reflection frames
//!   (`at System.Reflection.MethodBaseInvoker…`) that point at no code of
//!   the project.
//!
//! Errors are kept one per line, de-duplicated. Warnings are grouped by code:
//! the first message, how many, and where (the first [`LOCATIONS_SHOWN`]). Every
//! other line is kept in order, the summary's repeated diagnostics dropped.

/// Where a warning code was raised, at most this many listed; the count says
/// how many more.
const LOCATIONS_SHOWN: usize = 12;

struct Diag<'a> {
    line: &'a str,
    loc: &'a str,
    severity: &'a str,
    code: &'a str,
    message: &'a str,
}

/// `path(4,26): warning CS0219: message [project]`
fn diagnostic(line: &str) -> Option<Diag<'_>> {
    let t = line.trim();
    let (sev_at, severity) = [": warning ", ": error "]
        .iter()
        .filter_map(|k| t.find(k).map(|i| (i, k.trim_matches([':', ' ']))))
        .min_by_key(|(i, _)| *i)?;
    let loc = &t[..sev_at];
    let rest = &t[sev_at + severity.len() + 3..];
    let (code, message) = rest.split_once(": ")?;
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) || !code.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let message = match message.rfind(" [") {
        Some(i) if message.ends_with(']') => &message[..i],
        _ => message,
    };
    Some(Diag { line: t, loc, severity, code, message })
}

pub fn compact_build(text: &str) -> Option<String> {
    let diags: Vec<Diag> = text.lines().filter_map(diagnostic).collect();
    if diags.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&Diag> = diags.iter().filter(|d| seen.insert(d.line)).collect();
    let prefix = common_dir(unique.iter().map(|d| d.loc));
    let short = |loc: &str| loc.strip_prefix(prefix.as_str()).unwrap_or(loc).to_string();

    let mut block = String::new();
    if !prefix.is_empty() {
        block.push_str(&format!("(paths below are under {prefix})\n"));
    }
    for d in unique.iter().filter(|d| d.severity == "error") {
        block.push_str(&format!("{}: error {}: {}\n", short(d.loc), d.code, d.message));
    }
    let mut codes: Vec<&str> = Vec::new();
    for d in unique.iter().filter(|d| d.severity == "warning") {
        if !codes.contains(&d.code) {
            codes.push(d.code);
        }
    }
    for code in codes {
        let of: Vec<&&Diag> = unique.iter().filter(|d| d.severity == "warning" && d.code == code).collect();
        let shown: Vec<String> = of.iter().take(LOCATIONS_SHOWN).map(|d| short(d.loc)).collect();
        let more = of.len().saturating_sub(LOCATIONS_SHOWN);
        block.push_str(&format!(
            "warning {code} ×{}: {}\n  in {}{}\n",
            of.len(),
            of[0].message,
            shown.join(", "),
            if more > 0 { format!(", +{more} more not shown") } else { String::new() }
        ));
    }

    let mut out = String::new();
    let mut placed = false;
    let mut blank = false;
    for line in text.lines() {
        if diagnostic(line).is_some() {
            if !placed {
                out.push_str(&block);
                placed = true;
            }
            continue;
        }
        if line.trim().is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
            continue;
        }
        blank = false;
        out.push_str(line);
        out.push('\n');
    }
    (out.len() < text.len()).then_some(out)
}

/// The directory every location shares, with its trailing separator.
fn common_dir<'a>(mut locs: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = locs.next() else { return String::new() };
    let sep = if first.contains('\\') && !first.contains('/') { '\\' } else { '/' };
    let mut common = match first.rfind(sep) {
        Some(i) => &first[..=i],
        None => return String::new(),
    };
    for l in locs {
        while !l.starts_with(common) {
            match common[..common.len() - 1].rfind(sep) {
                Some(i) => common = &common[..=i],
                None => return String::new(),
            }
        }
    }
    common.to_string()
}

/// A stack frame of the runtime or the test framework, not of the project.
fn framework_frame(line: &str) -> bool {
    let t = line.trim_start();
    let Some(frame) = t.strip_prefix("at ") else { return false };
    ["System.", "Microsoft.", "Xunit.", "NUnit.", "InvokeStub_"].iter().any(|p| frame.starts_with(p))
}

pub fn compact_test(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut dropped = 0usize;
    let build = compact_build(text);
    let source: Vec<&str> = match &build {
        Some(b) => b.lines().collect(),
        None => lines,
    };
    for (i, line) in source.iter().enumerate() {
        // `[xUnit.net 00:00:00.12]     X [FAIL]` announces the `Failed X`
        // block that follows it.
        let t = line.trim_start();
        if t.starts_with("[xUnit.net ") && t.ends_with("[FAIL]") && source[i + 1..].iter().take(3).any(|n| n.trim_start().starts_with("Failed ")) {
            dropped += 1;
            continue;
        }
        if framework_frame(line) {
            dropped += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if dropped == 0 && build.is_none() {
        return None;
    }
    if dropped > 0 {
        out.push_str(&format!("[tokenade] {dropped} line(s) left out: runtime / test-framework stack frames and the duplicate `[FAIL]` announcement of each failure.\n"));
    }
    (out.len() < text.len()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/linux/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// 122 warnings printed twice: each code once, with every location
    /// counted and the first ones named; the verdict and counts kept.
    #[test]
    fn a_build_with_warnings_names_where_each_code_was_raised() {
        let text = fixture("dotnet_build_warnings.txt");
        let out = compact_build(&text).unwrap();
        assert!(out.contains("warning CS0219 ×41:"), "{out}");
        assert!(out.contains("warning CS8603 ×40:"), "{out}");
        assert!(out.contains("W1.cs(4,26)"), "{out}");
        assert!(out.contains("+29 more"), "{out}");
        assert!(out.contains("Build succeeded."), "{out}");
        assert!(out.contains("122 Warning(s)"), "{out}");
        assert_eq!(out.matches("warning CS8600").count(), 1, "{out}");
        assert!(out.len() * 10 < text.len(), "{} → {}", text.len(), out.len());
    }

    /// Errors are what the agent fixes: every one kept, once.
    #[test]
    fn every_error_is_kept_once() {
        let text = fixture("dotnet_build_errors.txt");
        let out = compact_build(&text).unwrap();
        for i in 1..=40 {
            assert_eq!(out.matches(&format!("W{i}.cs(7,11): error CS8954")).count(), 1, "W{i}:\n{out}");
        }
        assert!(out.contains("Build FAILED."), "{out}");
    }

    #[test]
    fn a_failing_test_run_keeps_each_failure_and_the_project_frames() {
        let text = fixture("dotnet_test_failures.txt");
        let out = compact_test(&text).unwrap();
        for name in ["Div_zero", "Sub_wrong", "Add_theory(a: 2, b: 2, e: 5)"] {
            assert_eq!(out.matches(&format!("Failed Core.Tests.CalcTests.{name}")).count(), 1, "{name}:\n{out}");
        }
        assert!(!out.contains("[xUnit.net"), "{out}");
        assert!(!out.contains("at System.Reflection"), "{out}");
        assert!(out.contains("at Core.Calc.Div(Int32 a, Int32 b)"), "{out}");
        assert!(out.contains("Expected: 5"), "{out}");
        assert!(out.contains("Failed:     3, Passed:    62"), "{out}");
    }

    #[test]
    fn a_line_that_is_not_a_diagnostic_is_left_alone() {
        assert!(diagnostic("  Core -> /src/Core/bin/Debug/net10.0/Core.dll").is_none());
        assert!(diagnostic("    2 Warning(s)").is_none());
        let d = diagnostic("/s/A.cs(4,40): warning CS0219: The variable 'u' is assigned [/s/A.csproj]").unwrap();
        assert_eq!((d.loc, d.code, d.message), ("/s/A.cs(4,40)", "CS0219", "The variable 'u' is assigned"));
    }
}
