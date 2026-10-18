//! Light-weight prose filler stripping.
//!
//! Removes sentence-leading pleasantries (`Please`, `Of course,`) from
//! plain-text while preserving fenced/inline code, URLs, and file paths
//! verbatim. Used by `turn_summary`.
//!
//! **Nothing INSIDE a sentence is removed.** Hedges and degree words were:
//! `Maybe the deploy failed` came back `the deploy failed`, `not very risky`
//! came back `not risky`, `appears to be fixed` came back `be fixed` — a
//! guess delivered as a fact, under a summarizer that promises never to
//! invent (2026-10-03). Every word on those lists changes what a sentence
//! claims in some context (`not really`, `just two files`), so none goes.

use regex::Regex;
use std::fmt::Write;
use std::sync::OnceLock;

// ─── protected-segment patterns ──────────────────────────────────────────────
// Applied in priority order: fenced code > inline code > URL > file path.
// Each matched segment is replaced with a null-byte sentinel `\x00Pn\x00`
// that survives the filler-removal pass, then restored afterwards.

crate::regexes::lazy_regex!(fn re_fenced_code() = r"(?s)```.*?```");

/// Inline code spans, CommonMark style: a run of N backticks closes on the
/// next run of EXACTLY N. A single-backtick regex split ``grep `really` x``
/// into two spans and left `really` bare for the filler pass to delete — the
/// command came back changed (caveman-compress, 2026-10-03).
pub(crate) fn inline_code_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let b = text.as_bytes();
    let run_at = |i: usize| b[i..].iter().take_while(|&&c| c == b'`').count();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let n = run_at(i);
        let mut j = i + n;
        let mut close = None;
        while j < b.len() && b[j] != b'\n' {
            if b[j] == b'`' {
                let m = run_at(j);
                if m == n {
                    close = Some(j + m);
                    break;
                }
                j += m;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                spans.push(i..end);
                i = end;
            }
            None => i += n,
        }
    }
    spans
}

crate::regexes::lazy_regex!(fn re_url() = r"https?://\S+");

fn re_filepath() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Unix/Windows absolute paths and ./relative paths
    RE.get_or_init(|| Regex::new(r"(?:(?:[A-Za-z]:)?[/\\]|\./|\.\\)\S+").unwrap())
}

// ─── filler patterns ─────────────────────────────────────────────────────────

crate::regexes::lazy_regex!(fn re_pleasantries() = concat!(
    r"(?im)^(?:please |kindly |of course[,!]?\s*|",
    r"sure[,!]?\s*|certainly[,!]?\s*|absolutely[,!]?\s*)"
));

crate::regexes::lazy_regex!(fn re_multi_space() = r" {2,}");

// ─── sentinel helpers ─────────────────────────────────────────────────────────

fn protect(text: &str, sentinels: &mut Vec<String>) -> String {
    let regex_spans = |re: fn() -> &'static Regex| {
        move |t: &str| re().find_iter(t).map(|m| m.range()).collect::<Vec<_>>()
    };
    let finders: [&dyn Fn(&str) -> Vec<std::ops::Range<usize>>; 4] = [
        &regex_spans(re_fenced_code),
        &inline_code_spans,
        &regex_spans(re_url),
        &regex_spans(re_filepath),
    ];
    let mut out = text.to_string();
    for find in finders {
        let mut new_out = String::new();
        let mut last = 0;
        for r in find(&out) {
            new_out.push_str(&out[last..r.start]);
            let idx = sentinels.len();
            sentinels.push(out[r.clone()].to_string());
            let _ = write!(new_out, "\x00P{idx}\x00");
            last = r.end;
        }
        new_out.push_str(&out[last..]);
        out = new_out;
    }
    out
}

fn restore(text: &str, sentinels: &[String]) -> String {
    let mut out = text.to_string();
    for (i, original) in sentinels.iter().enumerate() {
        out = out.replace(&format!("\x00P{i}\x00"), original);
    }
    out
}

// ─── public API ───────────────────────────────────────────────────────────────

/// Strip sentence-leading pleasantries from plain-text prose.
///
/// Protected segments (fenced/inline code, URLs, file paths) are preserved
/// verbatim.  Returns the original string unchanged when the input is < 200
/// bytes (not worth processing).
pub fn strip(text: &str) -> String {
    if text.len() < 50 {
        return text.to_string();
    }

    let mut sentinels = Vec::new();
    let protected = protect(text, &mut sentinels);

    let s = re_pleasantries().replace_all(&protected, "").into_owned();
    let s = re_multi_space().replace_all(&s, " ").into_owned();

    restore(&s, &sentinels)
}

// Compiled matcher for constraint lines (see [`is_constraint`]).
crate::regexes::lazy_regex!(fn re_constraint() = concat!(
    // Negative imperatives + strong requirements (prescriptive language).
    r"(?i)\b(do not|don't|never|must not|mustn't|must|shall|always|",
    r"required|requires?|only use|ensure that|make sure|forbidden|prohibited)\b",
    // Destructive command literals — losing one of these from a folded
    // turn can let the model re-run something irreversible.
    r"|(?i)(rm\s+-rf|sudo\s|mkfs\b|drop\s+table|delete\s+from|",
    r"truncate\s+table|git\s+reset\s+--hard|dd\s+(if|of)=)",
));

/// True when a line carries a CONSTRAINT a lossy fold must not silently drop:
/// a negative imperative (`do not`, `never`, `must not`), a strong requirement
/// (`must`, `always`, `required`, `only use`), or a destructive command literal
/// (`rm -rf`, `DROP TABLE`, `git reset --hard`, …).
///
/// Reusable preservation floor for any extractive / summarizing compactor: such
/// lines are low-salience (few distinct content words) so a pure salience score
/// prunes them, after which the model can violate a now-invisible rule — a
/// silent quality failure. Callers pin matching lines verbatim BEFORE salience
/// selection (see `turn_summary::summarize`), so they can fold more aggressively
/// without that tail risk.
pub fn is_constraint(line: &str) -> bool {
    re_constraint().is_match(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A double-backtick span holds single backticks; its words are code.
    #[test]
    fn a_double_backtick_span_is_protected_whole() {
        let input = "To check this, run ``grep -c `a  b` notes.txt`` and it is  fine to do so.";
        let out = strip(input);
        assert!(out.contains("``grep -c `a  b` notes.txt``"), "{out}");
        assert!(
            out.contains("is fine"),
            "prose spacing still collapses: {out}"
        );
    }

    #[test]
    fn is_constraint_flags_rules_and_destructive_literals() {
        assert!(is_constraint("Never force-push to the main branch."));
        assert!(is_constraint("You must use the v2 API for new endpoints."));
        assert!(is_constraint("Do not delete the production config."));
        assert!(is_constraint("Always run the migration before deploying."));
        assert!(is_constraint("Clean with rm -rf build/ first."));
        assert!(is_constraint("This will DROP TABLE users on rollback."));
        // Plain descriptive prose is not a constraint.
        assert!(!is_constraint(
            "The service reads configuration from consul."
        ));
        assert!(!is_constraint(
            "We refactored the helper modules for clarity."
        ));
    }

    /// A hedge or a degree word is part of the claim: it stays.
    #[test]
    fn keeps_hedges_and_degree_words() {
        let input = "Maybe the deploy failed. The test appears to be fixed. The fix is not very risky, not really.";
        assert_eq!(strip(input), input);
    }

    #[test]
    fn strips_pleasantries_at_line_start() {
        let out = strip("Please run the installer first.\nOf course, you can skip this.");
        assert!(
            !out.starts_with("Please"),
            "leading pleasantry must go: {out}"
        );
    }

    #[test]
    fn preserves_fenced_code() {
        let input = "Here is how:\n```\nthe_value = just_a_variable\n```\nJust run it.";
        let out = strip(input);
        assert!(
            out.contains("the_value = just_a_variable"),
            "fenced code content must be preserved: {out}"
        );
    }

    #[test]
    fn preserves_inline_code() {
        let input = "Call `just_do_it()` to trigger the process. Just remember to export first.";
        let out = strip(input);
        assert!(
            out.contains("`just_do_it()`"),
            "inline code must be preserved: {out}"
        );
    }

    #[test]
    fn preserves_urls() {
        let input = "Visit https://example.com/the-really-long-path for details. Just click.";
        let out = strip(input);
        assert!(
            out.contains("https://example.com/the-really-long-path"),
            "URL must be preserved: {out}"
        );
    }

    #[test]
    fn preserves_file_paths() {
        let input = "The file /home/user/just-a-file.txt should be edited. Just open it.";
        let out = strip(input);
        assert!(
            out.contains("/home/user/just-a-file.txt"),
            "file path must be preserved: {out}"
        );
    }

    #[test]
    fn short_input_passthrough() {
        let short = "Just a short message.";
        assert_eq!(
            strip(short),
            short,
            "short inputs must pass through unchanged"
        );
    }

    #[test]
    fn no_double_spaces_after_strip() {
        let out = strip("This is  a test sentence for   checking double spaces.");
        assert!(
            !out.contains("  "),
            "double spaces must be collapsed: {out}"
        );
    }
}
