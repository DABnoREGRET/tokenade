//! **Recognise the shape before choosing the fold.**
//!
//! A table, a key-value sequence, a form: each one folds
//! differently, and applying the wrong fold costs more than it returns. These
//! predicates fold nothing — they say which one to call.

use super::*;

/// Any non-blank output at all. The commonest detector in the command tree:
/// 75 modules spelled it out, so "what counts as output worth folding" could
/// only be changed in 75 places.
pub fn any_output(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Any line carrying more than a couple of characters — output that is not
/// just punctuation or a prompt. 35 modules spelled this one out.
pub fn has_substantive_line(text: &str) -> bool {
    text.lines().any(|l| l.trim().len() > 2)
}

/// Worth folding when it is a table, or when it is simply longer than the
/// module's own head budget. 54 modules spelled this one out.
pub fn table_or_over_budget(text: &str, head: usize) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    looks_like_table(text) || text.lines().filter(|l| !l.trim().is_empty()).count() > head
}

// Flat parameters on purpose: the callers are per-command modules whose
// own signatures are flat, and building a context struct at each site
// would cost more lines than the block this replaces.
#[allow(clippy::too_many_arguments)]
/// The form-aware cascade: columnar table first (surfaces problem rows), then
/// key/value records (never cut mid-record), else head+tail (the final outcome
/// line is never decapitated).
///
/// This lived as a verbatim 11-line copy in 133 command modules. Every one of
/// them passed the SAME label and noun to all three stages, so the copies were
/// duplication with no variation to justify them — and a cascade that lives in
/// 133 places is a cascade that gets fixed in one of them.
///
/// The table fold comes from [`crate::table_fold`], not from `cmd/`. It lived
/// under `cmd/linux/docker_table.rs`, which forced THIS function — a
/// primitive the whole crate calls — to climb up into the commands
/// layer to reach a fold that knows no command.
pub fn form_aware(text: &str, head: usize, tail: usize, label: &str, noun: &str) -> Option<String> {
    if looks_like_table(text) {
        if let Some(o) = crate::table_fold::fold_table_priority(text, head, label, noun) {
            return Some(o);
        }
    }
    if looks_like_kv(text) {
        if let Some(o) = kv_records(text, head, label, noun) {
            return Some(o);
        }
    }
    head_tail(text, head, tail, label, noun)
}

/// The two-branch cousin of [`form_aware`], for commands whose output is
/// either a table or plain lines: table when it is one, head+tail otherwise.
/// 54 command modules carried this shape verbatim.
pub fn table_or_head_tail(
    text: &str,
    head: usize,
    tail: usize,
    label: &str,
    noun: &str,
) -> Option<String> {
    if looks_like_table(text) {
        crate::table_fold::fold_table_priority(text, head, label, noun)
    } else {
        head_tail(text, head, tail, label, noun)
    }
}

pub fn looks_like_table(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 {
        return false;
    }
    let col_runs = |l: &str| l.matches("  ").count();
    let header_cols = col_runs(lines[0]);
    if header_cols < 1 {
        return false;
    }
    // Majority of data rows share the header's multi-space column shape.
    let aligned = lines[1..].iter().filter(|l| col_runs(l) >= 1).count();
    aligned * 2 >= lines.len() - 1
}

/// Returns true when `text` looks like key/value record output:
/// a majority of non-blank lines match `key: value` or `key = value`
/// with a short alphanumeric key.
pub fn looks_like_kv(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 {
        return false;
    }
    let kv = lines
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            if let Some(idx) = t.find([':', '=']) {
                idx > 0
                    && idx <= 40
                    && t[..idx]
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
            } else {
                false
            }
        })
        .count();
    kv * 2 >= lines.len()
}

/// Estimate tokens before / after to compute Savings.
pub fn savings_of<F: FnOnce(&str) -> String>(text: &str, f: F) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let out = f(text);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}
