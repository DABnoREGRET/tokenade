//! **Writing a filter from an example.**
//!
//! An agent that has just seen verbose output can derive a filter from it —
//! but it must write it as valid TOML, and guess the right `match_command`.
//!
//! The draft is therefore conservative: it anchors on the program and its
//! subcommand rather than on the whole line, because a filter that is too
//! broad applies to commands nobody ever looked at.

use super::*;

// ─── Scaffold-from-transcript ─────────────────────────────────────────────
//
// `tokenade add-compactor --from <sample>` turns a REAL captured command output
// into a starter filter the author then trims. Everything here is CONTENT-
// AGNOSTIC: the match pattern is derived from the command line (author input),
// and every inferred transform is STRUCTURAL (strip ANSI, drop blank lines, cap
// line count) — never keyed on the sample's bytes. A generated filter therefore
// cannot encode a secret from the sample. The sample appears only as an inline
// test `input`/`expect_equals` pair for review; `draft = true` blocks install
// until the author signs off.

/// Suggest `max_lines` once the sample exceeds this many lines …
pub(crate) const DRAFT_MANY_LINES: usize = 40;
/// … capping to this (leaves useful head context, mirrors the built-ins).
pub(crate) const DRAFT_MAX_LINES: usize = 40;

/// Program name of a command: basename of its first whitespace token
/// (`/usr/bin/git status` → `git`).
pub(crate) fn draft_program(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or("");
    first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first)
        .to_string()
}

/// A stable first subcommand — a bare lowercase word, not a flag/path — for a
/// tighter match, or `None`.
pub(crate) fn draft_subcommand(command: &str) -> Option<String> {
    static BARE: OnceLock<Regex> = OnceLock::new();
    let re = BARE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]*$").unwrap());
    command
        .split_whitespace()
        .nth(1)
        .filter(|s| re.is_match(s))
        .map(|s| s.to_string())
}

/// Conservative, regex-escaped `match_command`: `^<prog>\b`, narrowed to
/// `^<prog> <sub>\b` when a stable subcommand exists.
pub(crate) fn draft_match_command(prog: &str, command: &str) -> String {
    let p = regex::escape(prog);
    match draft_subcommand(command) {
        Some(sub) => format!(r"^{p} {}\b", regex::escape(&sub)),
        None => format!(r"^{p}\b"),
    }
}

/// Deterministic filter name / filename: `git status` → `git-status`.
pub(crate) fn draft_filter_name(prog: &str, command: &str) -> String {
    let base = match draft_subcommand(command) {
        Some(sub) => format!("{prog}-{sub}"),
        None => prog.to_string(),
    };
    let s: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let t = s.trim_matches('-').to_string();
    if t.is_empty() {
        "filter".to_string()
    } else {
        t
    }
}

/// Whether the sample is a single complete JSON document. Such output must get
/// NO line-truncating transform (that would reintroduce mid-JSON corruption).
pub(crate) fn looks_like_complete_json(sample: &str) -> bool {
    let t = sample.trim();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// Quote `s` as a TOML basic string (double-quoted, fully escaped). Used
/// instead of a TOML serializer because the `toml` dependency is compiled with
/// `parse`-only features. Correct for arbitrary sample bytes (newlines, quotes,
/// backslashes, control chars).
pub(crate) fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build a STARTER filter TOML from a real command line and a captured sample
/// of its output. See the section header for the content-agnostic guarantee.
/// The returned document carries `draft = true` (install-blocking) and an inline
/// self-test seeded with the sample and the filter's ACTUAL output on it.
pub fn draft_filter_toml(command_line: &str, sample: &str) -> Result<String, String> {
    let prog = draft_program(command_line);
    if prog.is_empty() {
        return Err("cannot infer program: empty command".into());
    }
    let name = draft_filter_name(&prog, command_line);
    let match_command = draft_match_command(&prog, command_line);

    // Content-agnostic transform inference.
    let mut strip_lines: Vec<String> = Vec::new();
    let mut max_lines: Option<usize> = None;
    if !looks_like_complete_json(sample) {
        let lines: Vec<&str> = sample.trim_end_matches('\n').split('\n').collect();
        if lines.iter().any(|l| l.trim().is_empty()) {
            strip_lines.push(r"^\s*$".to_string());
        }
        if lines.len() > DRAFT_MANY_LINES {
            max_lines = Some(DRAFT_MAX_LINES);
        }
    }

    // The fixture is bounded BEFORE anything is derived from it. MEASURED
    // 2026-08-04: a 3,16 MB sample drafted a 3,22 MB TOML — the whole sample
    // embedded verbatim as `input`. That fails twice over. The draft goes to
    // stdout, so an agent that runs this takes ~800 000 tokens of log into its
    // context; and the header asks the author to REVIEW the file, which nobody
    // does at three megabytes.
    //
    // A contiguous HEAD, not a head+tail with a gap: whatever is embedded here
    // is a test fixture, and a fixture with an elision marker spliced into it
    // is no longer output the command could produce. Truncating first also
    // keeps `expect_equals` honest — it is computed from exactly the bytes the
    // test will feed back in.
    const DRAFT_FIXTURE_MAX_LINES: usize = 200;
    const DRAFT_FIXTURE_MAX_BYTES: usize = 20_000;
    let total_lines = sample.lines().count();
    let mut fixture = String::new();
    let mut kept = 0usize;
    for l in sample.lines().take(DRAFT_FIXTURE_MAX_LINES) {
        if fixture.len() + l.len() + 1 > DRAFT_FIXTURE_MAX_BYTES {
            break;
        }
        fixture.push_str(l);
        fixture.push('\n');
        kept += 1;
    }
    let sample: &str = if kept < total_lines { &fixture } else { sample };
    let trimmed_note = (kept < total_lines).then(|| {
        format!(
            "#  - the sample was trimmed to its first {kept} of {total_lines} line(s) so this \
             file stays reviewable; replace the test's `input` with your own if the part that \
             matters is further down\n"
        )
    });

    // Seed the test's `expect_equals` with the inferred filter's real output, so
    // a generated draft self-validates (author then trims it to intent).
    let raw = UserFilter {
        name: name.clone(),
        match_command: match_command.clone(),
        strip_ansi: true,
        strip_lines_matching: strip_lines.clone(),
        max_lines,
        ..Default::default()
    };
    let expected = raw
        .compile()
        .map(|f| f.apply(sample))
        .unwrap_or_else(|_| sample.to_string());

    let mut b = String::new();
    b.push_str("# Drafted by `tokenade add-compactor --from`. REVIEW before installing:\n");
    b.push_str("#  - tighten `match_command` if the pattern is too broad\n");
    b.push_str("#  - trim the test's `expect_equals` down to the intended compact output\n");
    b.push_str("#  - delete the `draft = true` line, then install with\n");
    b.push_str("#      tokenade add-compactor --file <this>.toml\n");
    if let Some(n) = &trimmed_note {
        b.push_str(n);
    }
    b.push_str("# (install is REFUSED while `draft = true`.)\n\n");
    b.push_str(&format!("name = {}\n", toml_basic_string(&name)));
    b.push_str(&format!(
        "match_command = {}\n",
        toml_basic_string(&match_command)
    ));
    b.push_str("draft = true\n");
    b.push_str("strip_ansi = true\n");
    if !strip_lines.is_empty() {
        let parts: Vec<String> = strip_lines.iter().map(|s| toml_basic_string(s)).collect();
        b.push_str(&format!("strip_lines_matching = [{}]\n", parts.join(", ")));
    }
    if let Some(m) = max_lines {
        b.push_str(&format!("max_lines = {m}\n"));
    }
    b.push_str("\n[[tests]]\n");
    b.push_str(&format!("input = {}\n", toml_basic_string(sample)));
    b.push_str(&format!(
        "expect_equals = {}\n",
        toml_basic_string(&expected)
    ));
    Ok(b)
}
