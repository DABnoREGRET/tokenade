//! `mistakes` — a tiny persistent "bad direction" memory (the `engram` idea).
//!
//! When a change is *reverted*, that revert is a signal: the direction it
//! undid was a mistake worth remembering, so the agent doesn't cheerfully
//! re-propose it next session. This module records such mistakes as a JSONL
//! ledger next to the gain ledger, and reads them back.
//!
//! The **pure** parts — the record shape, JSONL (de)serialization, and the
//! revert-summary extraction — are unit-tested in isolation. The thin IO
//! (append/load against the on-disk ledger) reuses `gain`'s root convention
//! so a single `TOKENADE_GAIN_DIR`/`set_root` controls both.

use std::io::Write;
use std::path::PathBuf;

/// One remembered mistake.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mistake {
    /// Unix seconds when recorded (passed in by the caller so the pure core
    /// stays clock-free and testable).
    pub ts: u64,
    /// Coarse category, e.g. `"revert"` or `"manual"`.
    pub kind: String,
    /// Human-readable description of the bad direction.
    pub summary: String,
    /// Optional git ref / commit the mistake relates to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl Mistake {
    pub fn new(ts: u64, kind: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            ts,
            kind: kind.into(),
            summary: summary.into(),
            reference: None,
        }
    }

    pub fn with_reference(mut self, reference: Option<String>) -> Self {
        self.reference = reference.filter(|r| !r.is_empty());
        self
    }

    /// One JSONL row (no trailing newline).
    pub fn to_jsonl(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Parse a JSONL ledger body into mistakes, skipping blank/garbled lines so a
/// single corrupt row never sinks the whole history.
pub fn parse_lines(text: &str) -> Vec<Mistake> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<Mistake>(l).ok())
        .collect()
}

/// Extract a short summary of what a `git revert` undid, given the command's
/// output. Returns `None` when the text doesn't look like a revert (so the
/// caller records nothing). Pure: the revert *detection* is delegated to the
/// existing `git_revert` compactor by the caller; here we just pull a subject.
///
/// git revert prints lines like `[main 1a2b3c4] Revert "add broken cache"`.
/// We surface the quoted original subject when present, else the whole line.
pub fn revert_summary(git_output: &str) -> Option<String> {
    for line in git_output.lines() {
        let line = line.trim();
        if !line.contains("Revert") {
            continue;
        }
        // Prefer the quoted original subject: Revert "<subject>".
        if let Some(start) = line.find("Revert \"") {
            let rest = &line[start + "Revert \"".len()..];
            if let Some(end) = rest.find('"') {
                let subj = &rest[..end];
                if !subj.is_empty() {
                    return Some(format!("reverted: {subj}"));
                }
            }
        }
        // Fall back to the whole revert line, trimmed of a leading `[ref]`.
        let cleaned = line.split_once(']').map(|(_, r)| r.trim()).unwrap_or(line);
        return Some(format!("reverted: {cleaned}"));
    }
    None
}

/// Path to the mistakes ledger — sibling of the gain ledger so both share a
/// root via `gain::set_root` / the `TOKENADE_GAIN_DIR` env knob.
pub fn ledger_path() -> PathBuf {
    let gain = crate::gain::current_ledger_path();
    match gain.parent() {
        Some(dir) => dir.join("mistakes.jsonl"),
        None => PathBuf::from("mistakes.jsonl"),
    }
}

/// Append one mistake to the on-disk ledger (creating it if needed).
pub fn record(m: &Mistake) -> std::io::Result<()> {
    let path = ledger_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        // Single-write append: see debug_log.rs on cross-process interleaving.
        f.write_all(format!("{}\n", m.to_jsonl()).as_bytes())?;
    }
    // Bound the ledger like its sibling gain.jsonl: drop rows older than the
    // 30-day window and rotate if oversized, so a long-lived install can't
    // let this append-only file grow without limit. Best-effort.
    let _ = crate::gain::rotate_if_oversized(&path, 30 * 86_400);
    Ok(())
}

/// Load all recorded mistakes (most-recent last). Missing ledger → empty.
pub fn load() -> Vec<Mistake> {
    match std::fs::read_to_string(ledger_path()) {
        Ok(body) => parse_lines(&body),
        Err(_) => Vec::new(),
    }
}

/// How many recorded mistakes a session reminder names before it falls back to
/// a count. Small on purpose: this text is paid for on every session boot.
pub const REMINDER_SHOWN: usize = 3;

/// Longest summary a reminder repeats verbatim.
pub const REMINDER_SUMMARY_CHARS: usize = 120;

/// The one-line reminder a session boot injects, or `None` when there is
/// nothing recorded.
///
/// The module opens by saying the point is that the agent "doesn't cheerfully
/// re-propose it next session" — and nothing surfaced these anywhere. `load`
/// had exactly one caller, the `tokenade mistake list` subcommand a human has
/// to type. Written and never read where it mattered.
///
/// `None` on an empty ledger is the property that makes wiring this safe at
/// all: the session-boot channel is measured, and an earlier block was REMOVED
/// from it for costing 560-710 cache-creation tokens per session with no
/// behavioural gain. A user who has recorded nothing pays nothing.
///
/// Newest first, because a dead end found today is likelier to be re-proposed
/// than one from three weeks ago, and the ledger is written oldest-first.
///
/// The count names the TOTAL, not the shown subset: a capped list that says
/// nothing reads as the complete set.
pub fn session_reminder(items: &[Mistake], max: usize) -> Option<String> {
    if items.is_empty() || max == 0 {
        return None;
    }
    let shown: Vec<String> = items
        .iter()
        .rev()
        .take(max)
        .map(|m| {
            let s = m.summary.trim();
            let cut = s.char_indices().nth(REMINDER_SUMMARY_CHARS).map(|(i, _)| i);
            match cut {
                Some(i) => format!("\"{}…\"", &s[..i]),
                None => format!("\"{s}\""),
            }
        })
        .collect();
    let total = items.len();
    let head = if total == 1 {
        "1 dead end recorded on this machine".to_string()
    } else {
        format!("{total} dead ends recorded on this machine")
    };
    let tail = if total > shown.len() {
        format!(
            " ({} shown; `tokenade mistake list` for the rest)",
            shown.len()
        )
    } else {
        String::new()
    };
    Some(format!(
        "[tokenade:mistakes] {head}{tail} — do not re-propose: {}",
        shown.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ts: u64, summary: &str) -> Mistake {
        Mistake::new(ts, "revert", summary)
    }

    /// The property that makes wiring this into session boot safe: nothing
    /// recorded, nothing emitted, no tokens spent.
    #[test]
    fn an_empty_ledger_says_nothing_at_all() {
        assert!(session_reminder(&[], REMINDER_SHOWN).is_none());
        assert!(session_reminder(&[m(1, "x")], 0).is_none());
    }

    #[test]
    fn the_reminder_is_newest_first_capped_and_counts_the_rest() {
        let items: Vec<Mistake> = (1..=5)
            .map(|i| m(i, &format!("dead end number {i}")))
            .collect();
        let line = session_reminder(&items, 3).expect("five recorded, one line");
        // Newest first: the ledger is appended oldest-first, and a dead end
        // found today is likelier to be re-proposed than one from weeks ago.
        let p5 = line.find("number 5").expect("newest must be named");
        let p3 = line.find("number 3").expect("third-newest must be named");
        assert!(p5 < p3, "not newest-first: {line}");
        assert!(!line.contains("number 2"), "the cap did not hold: {line}");
        // The count names the TOTAL, not the shown subset.
        assert!(line.contains("5 dead ends"), "{line}");
        assert!(line.contains("3 shown"), "{line}");
        assert!(line.contains("tokenade mistake list"), "{line}");
    }

    #[test]
    fn nothing_is_hidden_when_everything_fits() {
        let line = session_reminder(&[m(1, "only one")], 3).unwrap();
        assert!(line.contains("1 dead end recorded"), "{line}");
        assert!(
            !line.contains("shown"),
            "no cap notice when nothing is cut: {line}"
        );
    }

    /// A summary is somebody's free text; the session-boot channel is not the
    /// place to repeat an essay.
    #[test]
    fn a_long_summary_is_cut() {
        let long = "z".repeat(400);
        let line = session_reminder(&[m(1, &long)], 3).unwrap();
        assert!(
            line.len() < 400,
            "the summary was not cut: {} chars",
            line.len()
        );
        assert!(line.contains('…'), "a cut must say so: {line}");
    }

    /// Multi-byte input must not be cut mid-character.
    #[test]
    fn a_long_multibyte_summary_is_cut_on_a_character_boundary() {
        let long = "é".repeat(400);
        let line = session_reminder(&[m(1, &long)], 3).unwrap();
        assert!(line.contains('…'), "{line}");
        assert!(line.chars().filter(|c| *c == 'é').count() <= REMINDER_SUMMARY_CHARS);
    }

    #[test]
    fn jsonl_roundtrips() {
        let m = Mistake::new(123, "revert", "reverted: add broken cache")
            .with_reference(Some("abc123".into()));
        let line = m.to_jsonl();
        let back = parse_lines(&line);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], m);
    }

    #[test]
    fn reference_omitted_when_empty() {
        let m = Mistake::new(1, "manual", "bad idea").with_reference(Some(String::new()));
        assert!(m.reference.is_none());
        assert!(!m.to_jsonl().contains("reference"));
    }

    #[test]
    fn parse_skips_garbage_lines() {
        let body = "{\"ts\":1,\"kind\":\"revert\",\"summary\":\"x\"}\nnot json\n\n";
        let got = parse_lines(body);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].summary, "x");
    }

    #[test]
    fn revert_summary_prefers_quoted_subject() {
        let out = "[main 1a2b3c4] Revert \"add broken cache layer\"\n 1 file changed";
        assert_eq!(
            revert_summary(out).as_deref(),
            Some("reverted: add broken cache layer")
        );
    }

    #[test]
    fn revert_summary_falls_back_to_line() {
        let out = "[main deadbee] Revert of the flaky migration";
        assert_eq!(
            revert_summary(out).as_deref(),
            Some("reverted: Revert of the flaky migration")
        );
    }

    #[test]
    fn revert_summary_none_on_unrelated() {
        assert!(revert_summary("Already up to date.\nnothing to commit").is_none());
    }
}
