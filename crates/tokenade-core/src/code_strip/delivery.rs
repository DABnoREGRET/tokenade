//! **What is actually DELIVERED, and what it cost.**
//!
//! The strip is not free: it removes material, so it must say
//! how much. `Delivery` carries the answer, and it is what makes it possible to refuse
//! a strip whose rendering is bigger than the input.

use super::*;

/// The outcome of [`strip_for_delivery`]: which mode won, and what it cost.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub text: String,
    /// `"comments"` or `"lite"` — the label the banner prints.
    pub mode: &'static str,
    pub before: usize,
    pub after: usize,
    /// Whole percent saved, `(before - after) * 100 / before`.
    pub pct: usize,
}

/// Two-stage strip selection, shared by every surface that DELIVERS source to
/// an agent.
///
/// 1. `Comments` at a 10 % bar — drops ordinary comments, keeps docstrings.
///    It wins big on heavily-commented files and reads better than preserving
///    noise.
/// 2. `Lite` at a 5 % bar — only collapses blank-line runs, so line numbers
///    barely move. The catch-all for sparsely-commented code (tokenade's own
///    `.rs` files strip 0.4 % in Comments mode).
///
/// `None` when neither clears its bar, or when the winner is still too big to
/// deliver — see the ceiling below. The caller then falls through to its own
/// path, which for a file this size is the disclose/stash one.
///
/// **A rung that clears its bar can still be the wrong answer.** Measured
/// 2026-08-27 on a 404 669-byte Rust file: `Comments` saved 27 %, cleared the
/// 10 % bar, and delivered **293 904 bytes**. Declining instead — the same
/// call with `TOKENADE_NO_CODE_STRIP=1` — handed the file to the disclose path
/// and delivered **1 224**. The gentlest rung was pre-empting the one that
/// actually fits, and it did so BECAUSE it succeeded. So a rung is accepted
/// only if what it produces is deliverable: at or under
/// [`crate::disclose::read_threshold_tokens`], the documented point where
/// "the agent should really have paginated instead" begins.
///
/// The rule lived only inside the Read hook until 2026-08-26, which is why
/// `tokenade read src/foo.go` returned the file whole while reading the same
/// path through the hook stripped it. Measured over this tree: Go source is
/// 0.800 of its bytes once comments go, C 0.847, Ruby 0.836, Python 0.902,
/// Rust 0.906 — so the content path was handing back a fifth more bytes than
/// the hook did, for the same file.
pub fn strip_for_delivery(text: &str, language: Language) -> Option<Delivery> {
    let before = crate::tokenizer::estimate(text);
    let pct_of = |after: usize| {
        before
            .saturating_sub(after)
            .checked_mul(100)
            .and_then(|n| n.checked_div(before))
            .unwrap_or(0)
    };
    let ceiling = crate::disclose::read_threshold_tokens();
    let comments = strip(text, language, StripMode::Comments);
    let after = crate::tokenizer::estimate(&comments);
    let pct = pct_of(after);
    if pct >= 10 && after <= ceiling {
        return Some(delivered(comments, "comments", before));
    }
    let lite = strip(text, language, StripMode::Lite);
    let after = crate::tokenizer::estimate(&lite);
    let pct = pct_of(after);
    if pct >= 5 && after <= ceiling {
        return Some(delivered(lite, "lite", before));
    }
    None
}

/// The ONE place a `Delivery` is made, and therefore the one place its bytes
/// are scrubbed.
///
/// Every other surface that shows a model something scrubs at its own print
/// site — there are forty-six of them — and this path returns BEFORE the one
/// in `cmd_auto`, because it prints and exits on its own. Measured 2026-08-27
/// by writing the output to a file and testing membership there (printing the
/// check redacts the check): a 19 937-byte Python file carrying
/// `AWS_SECRET_ACCESS_KEY = "wJal…"` came back through `tokenade read` with the
/// key **in clear and no `<redacted>` marker at all**, while the same key in a
/// 89-byte file and in a `.txt` came back scrubbed. Size alone decided whether
/// a secret was shown.
///
/// Scrubbing HERE rather than at the caller is deliberate: a print site can
/// be forgotten by the next surface that delivers source, and this function's
/// own contract already says it is "shared by every surface that DELIVERS
/// source to an agent".
///
/// `after` and `pct` are recomputed on the scrubbed text, because redaction
/// routinely makes text LONGER (`API_KEY=hunter2` → `API_KEY=<redacted>`) and a
/// banner that quotes the pre-scrub figure would be describing bytes nobody
/// received.
pub(crate) fn delivered(text: String, mode: &'static str, before: usize) -> Delivery {
    let text = crate::redact::redact_for_model(&text);
    let after = crate::tokenizer::estimate(&text);
    let pct = before
        .saturating_sub(after)
        .checked_mul(100)
        .and_then(|n| n.checked_div(before))
        .unwrap_or(0);
    Delivery {
        text,
        mode,
        before,
        after,
        pct,
    }
}
