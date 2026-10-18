//! **No claim without its quote, checked VERBATIM.**
//!
//! Every quote returned by the model is searched for as-is (modulo
//! whitespace) in the packed evidence. An invalid quote earns a single retry
//! with the error fed back; after that the scout ABSTAINS.
//!
//! Abstaining is the right answer, not a failure: an unverified claim costs
//! more than no answer, because it reads exactly like a real one.

use super::*;

// ---- verification -----------------------------------------------------------

/// Check every citation: (id, line) must name a real line of a packed snippet.
/// The harness recovers the exact line text itself — the model never copies
/// anything, so there is nothing to mis-copy. Returns (verified citations
/// enriched with file + line text, failure descriptions for retry feedback).
pub(crate) fn verify_citations(parsed: &Value, snippets: &[Snippet]) -> (Vec<Value>, Vec<String>) {
    let mut verified = Vec::new();
    let mut failed = Vec::new();
    let empty = Vec::new();
    for c in parsed["citations"].as_array().unwrap_or(&empty) {
        let id = c["id"].as_u64().unwrap_or(u64::MAX) as usize;
        let line = c["line"].as_u64().unwrap_or(0) as usize;
        // Models sometimes cross id and line between snippets. Line numbers are
        // globally unique in the pack (absolute file lines), so if the pair
        // mismatches but exactly one snippet contains that line, remap to it —
        // deterministic heal, no model round-trip.
        let in_range =
            |s: &Snippet| line >= s.start_line && line < s.start_line + s.text.lines().count();
        let snip = match snippets.get(id).filter(|s| in_range(s)) {
            Some(s) => s,
            None => {
                let mut owners = snippets.iter().filter(|s| in_range(s));
                match (owners.next(), owners.next()) {
                    (Some(s), None) => s,
                    _ => {
                        failed.push(format!("line {line} is not in snippet [{id}]"));
                        continue;
                    }
                }
            }
        };
        let text = snip
            .text
            .lines()
            .nth(line - snip.start_line)
            .unwrap_or_default();
        if text.trim().len() < 3 {
            failed.push(format!(
                "cited line {line} is blank — cite the line that states the fact"
            ));
            continue;
        }
        verified.push(json!({
            "file": snip.file.display().to_string(),
            "line": line,
            "text": text.trim(),
        }));
    }
    (verified, failed)
}

/// Anti-hallucination grounding: every "hard" token of the answer — numbers,
/// identifiers, paths, addresses — must literally exist in the evidence (or the
/// question). A small model can only get those wrong by inventing them; values
/// it read off the evidence pass by construction.
pub(super) fn ground_answer(answer: &str, evidence: &str, question: &str) -> Vec<String> {
    let ev = evidence.to_lowercase();
    let q = question.to_lowercase();
    let mut missing = Vec::new();
    for tok in answer.split(|c: char| c.is_whitespace() || ",;()`'\"".contains(c)) {
        let t = tok.trim_matches(|c: char| ".:!?".contains(c));
        if !is_hard_token(t) {
            continue;
        }
        let tl = t.to_lowercase();
        if !ev.contains(&tl) && !q.contains(&tl) && !missing.contains(&t.to_string()) {
            missing.push(t.to_string());
        }
    }
    missing
}

/// Tokens a model cannot legitimately produce without evidence: multi-digit
/// numbers, snake_case / SCREAMING_CASE / camelCase identifiers, dotted or
/// slashed paths, host:port shapes.
pub(crate) fn is_hard_token(t: &str) -> bool {
    if t.len() < 2 {
        return false;
    }
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    if digits >= 2 && t.chars().all(|c| c.is_ascii_digit() || ".:x".contains(c)) {
        return true; // 512, 8192, 127.0.0.1:8919, 0x1F
    }
    let alnum = t
        .chars()
        .all(|c| c.is_alphanumeric() || "_./:-".contains(c));
    if !alnum {
        return false;
    }
    t.contains('_')
        || t.contains("::")
        || (t.contains('/') && t.len() >= 5)
        || (t.contains('.') && t.len() >= 5 && !t.chars().next().is_some_and(|c| c.is_uppercase()))
        || (t.chars().next().is_some_and(|c| c.is_lowercase())
            && t.chars().any(|c| c.is_uppercase()))
}
