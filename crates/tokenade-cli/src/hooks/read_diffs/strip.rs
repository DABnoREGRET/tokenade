//! **Serve less than the file, when less is enough.**
//!
//! Comments stripped, lite mode, skeleton: three views of one file, and the
//! choice between them does NOT live here. It lives in
//! `tokenade_core::read_view`, because the bench REPLAYS it to price what it
//! costs — spelled in both places, it would price the old rule the day a bar
//! moves, with nothing saying so.
//!
//! Automatic compaction is the same idea applied to what is not code: it only
//! wins if the rendered view is smaller than the input, which is not a given.

use super::*;

/// L22 source-code strip (extracted phase of `handle_read_pretool`). For
/// full-file Reads (offset==limit==0, size ≥ 3 KB) of a known language not
/// guarded as editable source, try Comments → Lite → skeleton compaction and
/// serve the winning view. `editable_source_guard` is computed once by the
/// caller (it is reused by the later auto-compact / disclose phases). Returns
/// `Some(0)` when a stripped view is emitted; `None` otherwise.
/// The decision "which view does this file get" lives in
/// [`tokenade_core::read_view`], because `read_view_headroom` REPLAYS it to
/// price what it costs and pick the next iteration's target. It used to be
/// spelled here AND there, "reproduced identically" — so the day a bar moves
/// here, the bench prices the old rule with nothing saying so.
use tokenade_core::read_view::{
    a_skeleton_that_wins_by_far, the_better_ordinary_mode, SKELETON_MIN_SIZE,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_code_strip(
    req: &ReadRequest<'_>,
    path: &std::path::Path,
    size: u64,
    editable_source_guard: bool,
) -> Option<i32> {
    let ReadRequest {
        parsed,
        file_path,
        hash,
        sliced_current,
        offset,
        limit,
        started,
        ..
    } = *req;
    // Guard clauses rather than three levels of `if` wrapped around the whole
    // body: the conditions say WHEN this pass does not apply, and reading them
    // first is what makes the 250 lines below about one thing.
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_CODE_STRIP")
        || offset != 0
        || limit != 0
        || size < 3_000
        || editable_source_guard
    {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str())?;
    let lang = tokenade_core::code_strip::language_for_extension(ext)?;
    // L73 — two-stage strip:
    //   1. Comments mode: drop ordinary comments, keep
    //      docstrings. Threshold 10% — drops comments only
    //      when they're a meaningful fraction of the file.
    //   2. Lite-mode FALLBACK: when Comments doesn't reach
    //      the bar (sparsely-commented code like tokenade's
    //      own .rs files: 467 misses / 2 hits = 0.4%), try
    //      Lite-mode at a 5% threshold. Lite only collapses
    //      blank-line runs — no comments touched, line
    //      numbers shift much less than Comments mode.
    //
    // Why two stages instead of going straight to Lite:
    // Comments wins MORE on heavily-commented files (where
    // it'd save 20-40%) and the agent's experience is
    // better when we strip noise vs preserving it. Lite is
    // the catch-all that ensures something always shrinks.
    let stripped = tokenade_core::code_strip::strip(
        sliced_current,
        lang,
        tokenade_core::code_strip::StripMode::Comments,
    );
    let before = tokenade_core::tokenizer::estimate(sliced_current);
    let after = tokenade_core::tokenizer::estimate(&stripped);
    let pct = before
        .saturating_sub(after)
        .checked_mul(100)
        .and_then(|n| n.checked_div(before))
        .unwrap_or(0);
    // D3 was a SKELETON view guarded behind `TOKENADE_READ_CODE_VIEW=1`,
    // marked "bench pending" and off by default. The bench settled it:
    // `read_view_headroom` measures, over 50 043 real code files of more than
    // 6 kB, that the ladder delivered 246 364 851 tokens of them out of
    // 270 123 474 (−8 %) and that 14 526 of them returned more than twice
    // what the skeleton would have left. So the view no longer has to be
    // asked for: it goes first when it wins by far, under the SINGLE gate
    // below. Two skeleton builds, two gates, two notes and two credit labels
    // have become one.
    // The two modes — comments stripped, blank lines folded — differ only by
    // the delivered text, its figures and the footer note. The DELIVERY,
    // though, is the same: banner, emission, credit (`read_code_strip_lite`)
    // and trace. It used to carry two copies of it, like the six that gave
    // `register_view_and_credit` just above.
    let lang_label = format!("{lang:?}").to_lowercase();
    // The skeleton, computed ONCE. It used to be computed in two places — the
    // guarded view at the top and the fallback at the bottom — each with its
    // gate, its label and its note, so the question "does this file deserve a
    // skeleton?" had two possible answers depending on the path taken.
    let skeleton: Option<(String, usize)> = (size >= SKELETON_MIN_SIZE).then(|| {
        let ext_l = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        let sk = tokenade_core::skeleton::skeleton_ext_budget(
            sliced_current,
            ext_l.as_deref(),
            usize::MAX,
        );
        let t = tokenade_core::tokenizer::estimate(&sk);
        (sk, t)
    });
    let deliver = |mode: &str, body: &str, after: usize, pct: usize, note: &str| -> Option<i32> {
        let reason = format!(
            "[tokenade:read-code-strip lang={lang_label} mode={mode}] {file_path} ({before}→{after} tokens, -{pct}%)\n\n\
             {body}\n\n\
             ({note} \
             Editing? Re-Read with offset/limit for exact line anchoring. \
             Persistent opt-out: `TOKENADE_NO_CODE_STRIP=1`.)",
        );
        // L64 — stash huge views; deliver a full view via the
        // cache-stable temp-redirect, a stashed marker via head-slice.
        let net_sav = emit_view_compaction(parsed, file_path, offset, limit, reason);
        // Register raw under metadata + content hashes
        // so future identical Reads dedup AND a Bash
        // `cat` of the same file cross-hits.
        register_view_and_credit(req, net_sav, "read_code_strip_lite");
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_code_strip_hit",
            serde_json::json!({
                "file": file_path,
                "lang": lang_label,
                "before": before,
                "after": after,
                "pct": pct,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "mode": mode,
            }),
        );
        Some(0)
    };

    // What the skeleton view says about itself. Written once: the fallback at
    // the bottom carried its own copy, one word apart.
    let skeleton_note = format!(
        "Structural skeleton: kept every declaration line (signatures, types, \
         imports, braces) and its identifiers; replaced function/method BODIES \
         with `// … N lines …` markers. License/boilerplate comments were dropped \
         FIRST — no symbol the agent searches for is hidden behind head/tail \
         truncation. LINE NUMBERS HAVE SHIFTED. \
         Need a body? `tokenade expand-ref {hash} --grep <name>` returns it and \
         `--raw` the whole file, or `tokenade query \"<name>\"` to locate a declaration, \
         or re-Read with offset/limit."
    );
    // **The skeleton goes first when it wins by far.** Without this, a mode
    // that has just cleared its bar returns and the skeleton is never reached.
    let skeleton_beats = |chosen: usize| -> Option<(&String, usize)> {
        let (sk, sk_tokens) = skeleton.as_ref()?;
        a_skeleton_that_wins_by_far(*sk_tokens, chosen).then_some((sk, *sk_tokens))
    };
    let lite_tokens = {
        let lite = tokenade_core::code_strip::strip(
            sliced_current,
            lang,
            tokenade_core::code_strip::StripMode::Lite,
        );
        tokenade_core::tokenizer::estimate(&lite)
    };
    if let Some((sk, sk_tokens)) = skeleton_beats(the_better_ordinary_mode(after, lite_tokens)) {
        let sk_pct = before
            .saturating_sub(sk_tokens)
            .checked_mul(100)
            .and_then(|n| n.checked_div(before))
            .unwrap_or(0);
        return deliver("skeleton", sk, sk_tokens, sk_pct, &skeleton_note);
    }
    if pct >= 10 {
        return deliver(
            "comments",
            &stripped,
            after,
            pct,
            &format!(
                "Dropped ordinary comments (//, /* */, #); kept doc-comments (///, /** */, \"\"\"…\"\"\"). \
                 LINE NUMBERS HAVE SHIFTED. \
                 Need a comment? `tokenade expand-ref {hash} --grep <pattern>` returns the raw line, \
                 `--raw` the whole file."
            ),
        );
    }
    // L73 — Lite-mode fallback. Comments didn't reach 10%,
    // so try collapsing blank-line runs only (no comments
    // touched). Threshold 5% — anything less isn't worth
    // the small line-shift in inter-block whitespace.
    let lite_stripped = tokenade_core::code_strip::strip(
        sliced_current,
        lang,
        tokenade_core::code_strip::StripMode::Lite,
    );
    let lite_after = tokenade_core::tokenizer::estimate(&lite_stripped);
    let lite_pct = before
        .saturating_sub(lite_after)
        .checked_mul(100)
        .and_then(|n| n.checked_div(before))
        .unwrap_or(0);
    if lite_pct >= 5 {
        return deliver(
            "lite",
            &lite_stripped,
            lite_after,
            lite_pct,
            &format!(
                "Collapsed blank-line runs to single blanks; comments preserved. \
                 Inter-block line numbers MAY shift slightly. \
                 Need the original spacing? `tokenade expand-ref {hash} --raw` returns the raw bytes instantly."
            ),
        );
    }
    // #2 (0.4.4 findings — Cobra hallucination) — structural
    // skeleton fallback. Comments/Lite both missed the bar on a
    // big source file, so WITHOUT this it falls through to the
    // L44 `filter::generic` head/tail net. Head/tail on code is
    // the exact value-destroyer the bench caught: it keeps the
    // file HEAD (license banner + imports) and TAIL, eliding the
    // declarations in the MIDDLE — so the agent greps for a symbol
    // that was folded away and hallucinates. `skeleton` inverts
    // that: it elides bodies/boilerplate FIRST and preserves every
    // declaration line (`fn`/`func`/`class`/…) and its identifier,
    // so a symbol the agent looks for is still there to find.
    // Gated to big exploratory reads (size ≥ SKELETON_MIN_SIZE)
    // where the body detail isn't what's wanted.
    if let Some((sk, sk_tokens)) = skeleton.as_ref() {
        let sk_pct = before
            .saturating_sub(*sk_tokens)
            .checked_mul(100)
            .and_then(|n| n.checked_div(before))
            .unwrap_or(0);
        if sk_pct >= 10 {
            return deliver("skeleton", sk, *sk_tokens, sk_pct, &skeleton_note);
        }
    }
    None
}

/// L16 format-specific lossless auto-compact (extracted phase of
/// `handle_read_pretool`). Detects the file's structured kind and, when its
/// compaction is lossless and wins ≥10%, serves the compacted view via the
/// cache-stable temp-redirect. Returns `(early_exit, read_kind_folds_values)`:
/// the bool is computed unconditionally (the terminal disclose phase consumes
/// it) and `Some(0)` is set only when a compacted view is emitted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_auto_compact(
    req: &ReadRequest<'_>,
    editable_source_guard: bool,
) -> (Option<i32>, bool) {
    let ReadRequest {
        parsed,
        file_path,
        hash,
        content_hash,
        sliced_current,
        offset,
        limit,
        started,
        ..
    } = *req;
    let read_kind_folds_values;
    {
        // L40 — pass file_path so the extension hint short-circuits
        // the 30-detector chain when the extension unambiguously
        // pins a kind (`.json` → Json, `Cargo.toml` → TomlConfig,
        // `BUILD` → Bazel, …). Faster AND more accurate on borderline
        // content-only matches.
        // L60 — `_indexed` variant: for Kind::Logs also persists fold
        // records keyed by `content_hash`, so the agent can drill into
        // a folded run via `tokenade expand-fold <template_id>`. We already
        // have the hash from earlier in the pipeline (identity dedup
        // computed it). For all other kinds, behaviour is identical
        // to `compact_with_path_hint`.
        let (kind, mut compact, mut sav) = tokenade_core::auto::compact_with_path_hint_indexed(
            sliced_current,
            file_path,
            content_hash,
        );
        read_kind_folds_values = kind_folds_values(&kind);
        // L44 — huge-Unknown safety net. When auto::compact can't
        // recognise the format AND the file is REALLY big, fall
        // back to a head/tail truncation. Without this, a 200 KB
        // plain-text log (no format markers) passes through with
        // 0% savings — the agent eats every byte.
        //
        // Threshold: ≥4000 tokens raw AND kind is Unknown. Below
        // that the truncation banner overhead isn't worth the
        // savings. The agent can re-Read with offset+limit to get
        // the elided middle if they need it.
        if matches!(kind, tokenade_core::auto::Kind::Unknown) && sav.tokens_before >= 4000 {
            // #26 — structure-preserving fallback instead of positional
            // head/tail. Head/tail throws away the MIDDLE of the file, which
            // on a doc or an unrecognised source file is exactly where the
            // substance is (a buried section, a declaration past the license
            // header). `structural_truncate` serves an outline (docs) or a
            // skeleton (code) and only falls back to head/tail for genuinely
            // flat data — see skeleton::structural_truncate.
            let (structured, s_sav) =
                tokenade_core::skeleton::structural_truncate(sliced_current, file_path);
            if s_sav.tokens_after < sav.tokens_before {
                compact = structured;
                sav = s_sav;
            }
        }
        // L45 — hard cap on compacted output. If the format-specific
        // compactor still leaves us with >50k tokens (e.g. a 10 MB
        // JSON file minifies to 2.5 M tokens — still catastrophic
        // for the agent's context window), layer head/tail truncation
        // on top so the agent sees a bounded slice plus an explicit
        // marker. Agent can `tokenade expand-ref <hash>` to fetch
        // raw bytes or re-Read with offset/limit for the middle.
        //
        // The cap is intentionally large — 50 k tokens is roughly
        // 5–8 % of a fresh context window, big enough to convey real
        // structure while small enough to be survivable.
        if let Some((truncated, trunc_tokens)) =
            tokenade_core::filter::apply_hard_cap(&compact, sav.tokens_after, "read-hook")
        {
            compact = truncated;
            sav = tokenade_core::Savings::new(sav.tokens_before, trunc_tokens);
        }
        let pct = sav.percent() as u64;
        // Coarse pre-filter only. The authoritative net-cost guard (which
        // accounts for the head-slice + additionalContext injection, not just
        // the compacted body) lives centrally in `emit_read_compaction`.
        //
        // `kind_lossless_read_view` is the load-bearing gate: only structured
        // formats whose compaction is lossless get the substituted view. Lossy
        // kinds (logs, html, …) fall through to a TRUE passthrough — see the
        // bench evidence on `kind_lossless_read_view`.
        if pct >= 10 && !editable_source_guard && kind_lossless_read_view(&kind) {
            let kind_label = format!("{kind:?}").to_lowercase();
            // Value-folding compactors (the log family) show the shape but drop
            // specific one-off values; steer the agent at the original file on
            // disk for a precise needle instead of re-reading the whole thing.
            let search_hint = if kind_folds_values(&kind) {
                read_search_steer(file_path, sliced_current.lines().count())
            } else {
                String::new()
            };
            let reason = format!(
                "[tokenade:read-auto-compact kind={kind_label}] {file_path} ({before}→{after} tokens, -{pct}%)\n\n\
                 {compact}\n\n\
                 (Compacted by tokenade's `{kind_label}` compactor at Read time. \
                 Use `tokenade expand-ref {hash} --raw` to fetch raw bytes.){search_hint}",
                before = sav.tokens_before,
                after = sav.tokens_after,
            );
            // Specific compactor (json/csv/yaml/…): serve the FULL compacted
            // view directly via a temp-file redirect (cache-stable, see
            // emit_read_compaction). No pre-stash — the whole view goes to the
            // temp file and Claude Code's native Read truncation bounds it if
            // large; raw bytes stay recoverable via `expand-ref` (registered
            // just below).
            let net_sav = emit_read_compaction(parsed, file_path, offset, limit, reason, true);
            // Still register raw bytes + path pointer so expand_ref
            // works AND a future identical Read hits the §ref§ cache.
            // ALSO register the content hash (+ trimmed form) — P5
            // 2026-06: this early-exit path skipped them, so a file
            // served via auto-compact never cross-hit afterwards
            // (touch without change, Bash `cat` of the same bytes,
            // another path to the same file) → full re-ship.
            register_view_and_credit(req, net_sav, "read_auto_compact");
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_auto_compact_hit",
                serde_json::json!({
                    "file": file_path,
                    "kind": kind_label,
                    "before": sav.tokens_before,
                    "after": sav.tokens_after,
                    "pct": pct,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return (Some(0), read_kind_folds_values);
        }
    }
    (None, read_kind_folds_values)
}
