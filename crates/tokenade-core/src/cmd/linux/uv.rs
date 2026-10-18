//! `uv` — shared compactor for uv subcommand output.
//!
//! Handles the pattern produced by `uv sync`, `uv install`, etc.:
//!   - Summary lines: "Resolved N packages…", "Installed N…", "Uninstalled N…"
//!   - Delta lines: " + pkg==ver", " - pkg==ver", " ~ pkg==ver"
//!
//! Keeps all summary lines verbatim, retains a small sample of delta lines,
//! and folds the remainder with an announced counter.  Declines when the
//! summary+delta shape is absent (e.g. `uv venv`) or when there are too few
//! deltas to justify compaction.

use std::fmt::Write;
const DELTA_SAMPLE: usize = 5;

fn compact_inner(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();

    let mut summary: Vec<&str> = Vec::new();
    let mut deltas: Vec<&str> = Vec::new();
    let mut other: Vec<&str> = Vec::new();

    for line in &lines {
        let t = line.trim();
        if t.starts_with("Resolved ")
            || t.starts_with("Installed ")
            || t.starts_with("Uninstalled ")
            || t.starts_with("Audited ")
        {
            summary.push(line);
        } else if t.starts_with("+ ") || t.starts_with("- ") || t.starts_with("~ ") {
            deltas.push(line);
        } else if !t.is_empty() {
            other.push(line);
        }
    }

    if summary.is_empty() || deltas.len() <= DELTA_SAMPLE {
        return None;
    }

    let kept = DELTA_SAMPLE.min(deltas.len());
    let folded = deltas.len() - kept;

    let mut out = String::with_capacity(512);
    let _ = writeln!(out, "[tokenade:uv] {} line(s)", lines.len());
    for s in &summary { out.push_str(s); out.push('\n'); }
    for s in &other   { out.push_str(s); out.push('\n'); }
    for d in &deltas[..kept] { out.push_str(d); out.push('\n'); }
    if folded > 0 {
        let _ = write!(out, "  ... {folded} more package line(s)");
    }
    Some(out)
}

pub fn detect(text: &str) -> bool { !text.trim().is_empty() }

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    compact_inner(text)
}

pub fn compact(text: &str) -> String {
    compact_inner(text).unwrap_or_else(|| text.trim_end().to_string())
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}
