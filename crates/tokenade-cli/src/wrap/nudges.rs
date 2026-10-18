//! What the wrapper says to the agent besides the output itself.
//!
//! A recovery pointer when bytes were dropped, an advisory when a head-slice
//! would have been cheaper, a nudge when no compactor covers the command.
//! None of them ever change the exit code or the bytes.

use super::*;

/// Remove the "expand-ref <hash>, printed in the banner" half of a fold footer
/// when no banner will follow it.
///
/// Deliberately surgical: it edits the one clause that became false and leaves
/// the rest of the footer — including `tokenade raw <cmd>`, which is a real way
/// back — exactly as written.
pub(crate) fn strip_unkept_hash_promise(display: &str) -> String {
    let mut out = display.to_string();
    for (from, to) in [
        // Read from tokenade-core, never re-typed: the emitter in
        // `filter::render_kept` and this rewrite must match byte for byte or
        // the net silently stops firing. Before 2026-08-04 both sides — and
        // this function's test — carried three separate copies of the
        // sentence.
        (
            tokenade_core::elision::FOLD_FOOTER_HASH_PROMISE,
            tokenade_core::elision::FOLD_FOOTER_NO_HASH,
        ),
        (
            "`tokenade expand-ref <hash>` (hash printed in this output's banner) for the verbatim document",
            "re-run the source command for the verbatim document",
        ),
    ] {
        out = out.replace(from, to);
    }
    out
}

/// Point the fold footer at the stash banner that is about to follow it.
///
/// The footer promises "the `full output:` banner below" and, failing that,
/// `tokenade raw <cmd>` — a RE-RUN. When [`attach_drop_recovery`] appends its
/// banner, neither is true: no banner says `full output:`, and the bytes are
/// already stored under the hash it names. Measured 2026-10-03 on a real
/// `pacman -Q` (2 263 packages): two recovery lines, the first sending the
/// reader to look for a banner that was not there.
pub(crate) fn point_footer_at_stash(display: &str, hash: &str) -> String {
    display.replace(
        &tokenade_core::elision::fold_footer_promise(),
        &format!("every folded line is in the stash `{hash}` below"),
    )
}

/// **A cut VALUE is a loss whatever it weighs.** The gate above measures
/// tokens, and a JSON whose long strings were cut (`…[+153 chars]`) can drop
/// little while losing a lot: measured 2026-10-02 on `cat dm-historique.json`
/// (12 440 bytes → 8 091), 30 of its 63 strings came back amputated, ~1 100
/// tokens dropped of ~3 100 — under both thresholds — and the output named no
/// way back to the rest of any of them. The marker is the one the value
/// truncators write (`tk-formats` json/yaml/notebook/transcript_view).
pub(crate) fn cuts_values(display: &str) -> bool {
    display.match_indices("…[+").any(|(at, m)| {
        let rest = &display[at + m.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(" chars]")
    })
}

/// Name the way back when the fold dropped a lot and the SOURCE IS A FILE.
///
/// Reading a file needs no stash — the path the agent typed still holds every
/// byte, and writing a second copy of a 4 MB log to the cache buys nothing.
/// That decision is right and this does not change it. What was missing is the
/// sentence: the call site justified skipping the pointer with "the disclose
/// footer already says to grep or re-read it", and measured 2026-09-04 across
/// 44 KB, 454 KB and **4.6 MB** of log, no footer of any kind appears on this
/// route. 4,608,890 bytes came back as **154** — one shaped line, correct and
/// complete about what it folded — naming nothing an agent could do next,
/// while the installed guidance tells it to run `expand-ref` with "the hash
/// printed in the compactor's banner". There is no banner and no hash here,
/// and the honest answer is not to write one: it is to name the file.
///
/// Same materiality gate as [`attach_drop_recovery`], and deliberately NOT
/// booked as `recovery:offered` — nothing was stashed, so counting it would
/// put in the denominator an offer `expand-ref` can never satisfy.
pub(crate) fn attach_path_recovery(
    display: String,
    path: &str,
    dropped_tokens: usize,
    before_tokens: usize,
    producer: &str,
) -> String {
    if !crate::wrap::fold_loss_earns_recovery(dropped_tokens, before_tokens)
        && !cuts_values(&display)
    {
        return display;
    }
    let sep = if display.ends_with('\n') { "" } else { "\n" };
    format!(
        "{display}{sep}{head} — the file itself is the \
         complete copy, so nothing was stashed: `tokenade read {path} --prompt \"q1, q2\"` for \
         what you need from it, or grep it.]",
        head = fold_banner_head(producer, dropped_tokens)
    )
}

/// **The head of the fold banner, and the name of the compactor that produced it.**
///
/// Written once for the three banners of this file, which each composed it on
/// their own. The name is the CONSOLIDATION and the fix in the same move:
/// measured over 758 real transcripts, **99.9 % of the 3,596 folds** came out
/// under one of three generic labels — `[tokenade: N tokens
/// folded away …]`, `[stashed: …]`, `[tokenade:elided …]` — none of which says
/// WHO folded. The question "which fold sends the agent looking for its bytes"
/// was therefore unanswerable from our own output, because the product did
/// not write it.
///
/// The name is the ledger's (`auto:grep`, `auto:html`, `read_dedup`,
/// `skeleton`…), not a second vocabulary: two names for the same thing, and
/// the two measurements would stop cross-referencing.
/// **An empty name yields exactly the historical banner**, without any branch
/// deciding it: `[tokenade:` + `""` + space gives `[tokenade: `, the form every
/// transcript carries. The explicit guard that used to be written here produced
/// the same string as the normal path — a mutant showed it by surviving, and
/// that was the right answer. The test says so in its place.
fn fold_banner_head(producer: &str, dropped_tokens: usize) -> String {
    format!(
        "[tokenade:{name} {dropped_tokens} tokens folded away",
        name = producer.trim()
    )
}

/// Append a recovery pointer for the bytes a compactor folded away.
///
/// No-op when a pointer is already present (the disclose path stashed the
/// visible output and printed its own), when the fold was small, or when the
/// stash declines — a declined stash must strand nobody with a dead hash.
pub(crate) fn attach_drop_recovery(
    display: String,
    raw: &str,
    dropped_tokens: usize,
    before_tokens: usize,
    hint: &tokenade_core::disclose::SourceHint,
    producer: &str,
) -> String {
    attach_drop_recovery_noting(
        display,
        raw,
        dropped_tokens,
        before_tokens,
        hint,
        producer,
        true,
    )
}

/// [`attach_drop_recovery`], with the `offered` recovery row left to the
/// caller when `note_offer` is false — the LLM proxy folds the same block
/// again on every call and books the offer once, on the block's first fold.
pub(crate) fn attach_drop_recovery_noting(
    display: String,
    raw: &str,
    dropped_tokens: usize,
    before_tokens: usize,
    hint: &tokenade_core::disclose::SourceHint,
    producer: &str,
    note_offer: bool,
) -> String {
    // **A refusal that does not explain itself gets rightly disobeyed.** This
    // path decides whether there is a way back to the folded bytes, and it
    // made its decision silently: a measurement showed that a SMALLER fold made
    // the pointer disappear, which is impossible if the gate judges itself on
    // `before - after` — and a real gain had to be withdrawn for lack of a way
    // to settle it. A trace here costs the agent zero bytes and makes the
    // decision re-readable, like `self_probe_reason`.
    let earned = crate::wrap::fold_loss_earns_recovery(dropped_tokens, before_tokens)
        || cuts_values(&display);
    tokenade_core::debug_log::log(
        "wrap",
        "drop_recovery_decision",
        serde_json::json!({
            "earned": earned,
            "dropped_tokens": dropped_tokens,
            "before_tokens": before_tokens,
            "display_bytes": display.len(),
            "raw_bytes": raw.len(),
        }),
    );
    if !earned {
        return display;
    }
    // A pointer already sitting in the view used to end this function. It
    // cannot, because the two pointers do not lead to the same bytes:
    // `maybe_stash` stashes what is about to be SHOWN, this stashes the RAW.
    // When the fold is deep enough for `maybe_stash` to fire on the remainder,
    // its pointer is the only one the agent gets — and it names a stash the
    // fold has already emptied.
    //
    // MEASURED end to end: a 252-line command output came back as a 3-line
    // stash, and `expand-ref <hash> --grep 'unique line 137'` — a string that
    // WAS in the output — answered `0 matching line(s) of 4 total`. A false
    // negative on the recovery path is worse than no pointer: the agent
    // concludes the string was never printed.
    //
    // So the question is not "is there a pointer" but "does a pointer lead to
    // THESE bytes". Stash first, then ask; the stash is content-addressed and
    // idempotent, so when the live pointer already names this hash the write
    // is a touch and nothing is appended.
    let Some((hash, _)) = tokenade_core::disclose::stash_for_reference(raw, hint) else {
        // No hash could be produced — the stash is unwritable (a full disk, a
        // read-only cache directory, a path that is not a directory). The fold
        // footer is still sitting there telling the reader to run
        // `expand-ref <hash>` with "the hash printed in this output's banner",
        // and there is no banner and no hash. OBSERVED end to end with the
        // stash path replaced by a regular file: 2 322 lines folded away and
        // the only named recovery pointing at nothing.
        //
        // `tokenade raw <cmd>` still works and is named in the same sentence,
        // so the fix is to stop promising the half that does not.
        return strip_unkept_hash_promise(&display);
    };
    if display.contains(&hash) {
        // The live pointer names these exact bytes — one hash, complete
        // output, nothing to add.
        return display;
    }
    // The offer is MADE here, so it is counted here — the same rule
    // `disclose::maybe_stash` states for its own site ("the exact moment
    // `tokens` stop being delivered"). The pair is the whole point: `offered`
    // is the denominator of `expanded`, and this path was missing from it.
    //
    // Measured 2026-08-27 on this machine's estimated ledger:
    // `recovery:offered` 54 events / 1 058 719 tokens against
    // `recovery:expanded` 13 / **15 077 869** — fourteen times more recovered
    // than was ever offered, which cannot happen. `maybe_stash` books what the
    // POINTER REPLACED; this books what the FOLD DROPPED, and only the second
    // is the big one (a `grep -r` folds 14 000 matches into a couple of hundred
    // lines). Both are expandable by hash, so `expand-ref` counted both while
    // the denominator counted one.
    if note_offer {
        tokenade_core::gain::record_recovery("offered", dropped_tokens);
    }
    let display = point_footer_at_stash(&display, &hash);
    let sep = if display.ends_with('\n') { "" } else { "\n" };
    // Say NOW what a bare expand would only say a turn later. The stashed bytes
    // and their size are both in hand here, and the gate that refuses a whole
    // dump is a pure function of that size — so pointing at
    // `tokenade expand-ref <hash>` (and advertising `--raw`) for a blob the
    // gate refuses buys the caller nothing but a round trip. Measured
    // 2026-09-06: a fold of 1 225 395 tokens did exactly that, and the answer
    // was 333 bytes of "too large to dump whole, ask instead".
    let stashed_tokens = tokenade_core::tokenizer::estimate(raw);
    if crate::code_intel::expand_ref::bare_dump_refused(stashed_tokens) {
        let advice = crate::code_intel::expand_ref::too_large_advice(stashed_tokens, &hash, raw);
        return format!(
            // Two DIFFERENT quantities, one sentence apart: what the fold
            // dropped, and how big the whole stored output is. Named, because
            // two bare numbers side by side read as one of them being wrong.
            "{display}{sep}{head}; the stash holds the \
             COMPLETE output — {advice} Any other hash above covers only what was shown.]",
            head = fold_banner_head(producer, dropped_tokens)
        );
    }
    // The three remedies, not two: this banner kept quiet about `--prompt`,
    // the cheapest option and the first the product recommends on prose.
    // The order comes from the blob's shape, through the same declarer as the
    // long banner — see `expand_ref::recovery_menu`.
    format!(
        "{display}{sep}{head} — the COMPLETE output is \
         stashed; {menu} Any other hash above covers only what was shown.]",
        head = fold_banner_head(producer, dropped_tokens),
        menu = crate::code_intel::expand_ref::recovery_menu(&hash, raw)
    )
}

/// Once a command whose output tokenade can only compact GENERICALLY (or not at
/// all) has recurred several times, nudge the agent — on STDERR, so stdout
/// stays byte-identical (no dedup/cache disturbance, mirror of
/// [`emit_head_slice_advisory`]) — to author a command-specific compactor. That
/// is a one-time cost that compacts every future run. Fires at most once per
/// command per ~day. Silence with `TOKENADE_NO_COMPACTOR_NUDGE=1`.
/// Is a hand-written compactor actually worth suggesting for a run this size?
///
/// The question that matters is not "was the output big?" but "did the generic
/// pass leave anything worth taking?" — and those differ sharply. Measured over
/// 31 330 real runs: the generic pass already removes 89–96% of `web`, HTML,
/// `ps` and `skeleton` output, and asking someone to hand-write a compactor for
/// those is asking for work with nothing behind it. It removes 13% of a
/// `cargo` run and still hands ~1 100 tokens to the model — that is a real gap.
///
/// So: fire only when a substantial payload STILL reaches the model and the
/// generic pass took less than half. A nudge that fires without checking
/// trains the reader to ignore it, including on the runs where it is right.
///
/// Inflation (`compacted > raw`) counts as worth it: the generic pass made
/// things worse there, which is exactly where a specific one earns its keep.
pub(crate) fn nudge_worth_suggesting(raw_tokens: usize, compacted_tokens: usize) -> bool {
    /// Tokens still reaching the model after the generic pass.
    const MIN_RESIDUE_TOKENS: usize = 800;
    /// Fraction the generic pass must fall short of to leave room for a
    /// hand-written one.
    const MAX_GENERIC_RATIO: f64 = 0.5;
    if compacted_tokens < MIN_RESIDUE_TOKENS || raw_tokens == 0 {
        return false;
    }
    let saved = 1.0 - (compacted_tokens as f64 / raw_tokens as f64);
    saved < MAX_GENERIC_RATIO
}

pub(crate) fn emit_uncovered_compactor_nudge(
    route: Option<&str>,
    cmd_specific: bool,
    raw_tokens: usize,
    compacted_tokens: usize,
) {
    if std::env::var("TOKENADE_NO_COMPACTOR_NUDGE").is_ok() {
        return;
    }
    // `None` means the route this run took already has a compactor — it just
    // didn't win this time (piped, chained, or out-compacted by the generic
    // pass). Telling someone to write a `git` compactor because a `git status`
    // routed generically is advice for work that is already done.
    let Some(cmd_name) = route else {
        return;
    };
    // A real per-command compactor already fired (or there's nothing to key on):
    // nothing to suggest.
    if cmd_specific || cmd_name.is_empty() || cmd_name == "tokenade" {
        return;
    }
    // Worth suggesting only when the generic pass left something behind. See
    // [`nudge_worth_suggesting`] — this used to ask merely whether the RAW
    // output was large, which fired on commands the generic pass had already
    // reduced to almost nothing.
    if !nudge_worth_suggesting(raw_tokens, compacted_tokens) {
        return;
    }
    const NUDGE_AT: u32 = 5;
    const TTL_SECS: u64 = 24 * 3600;
    // The route can now carry a subcommand ("git ls-files"). Fold the space
    // into the key rather than dropping it, so `git ls-files` and `gitls
    // files` cannot land on the same counter file.
    let safe: String = cmd_name
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    if safe.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("cmdstats");
    let _ = std::fs::create_dir_all(&dir);
    // Opportunistic GC: drop counter/marker files past the window so the dir
    // can't grow without bound over months of varied commands.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let stale = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() > TTL_SECS)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let count_path = dir.join(format!("{safe}.count"));
    let nudged_path = dir.join(format!("{safe}.nudged"));
    let fresh = |p: &std::path::Path| {
        std::fs::metadata(p)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs() < TTL_SECS)
            .unwrap_or(false)
    };
    // Count uncovered runs within the rolling window (stale counter resets).
    let prev = if fresh(&count_path) {
        std::fs::read_to_string(&count_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let count = prev + 1;
    let _ = std::fs::write(&count_path, count.to_string());
    if count < NUDGE_AT || fresh(&nudged_path) {
        return; // not yet / already nudged this window
    }
    let _ = std::fs::write(&nudged_path, b"1");
    tokenade_core::gain::record_nudge(
        "compactor-gap",
        "fire",
        tokenade_core::gain::NudgeMetric::Occurrence(count as usize),
    );
    eprintln!(
        "[tokenade] '{cmd_name}' has run {count}× recently with only generic \
         compaction. A command-specific compactor would cut its output on every \
         future run — see `tokenade add-compactor --help` for the TOML format, \
         then install it with `tokenade add-compactor --file <file>.toml`."
    );
}

// Truncation-aware advisory (flow analysis 2026-06). `<search> | head -N`
// is the single most common search shape (41% of all search commands in
// the transcript corpus) and it slices BLIND: if the hunted value sits
// past line N the agent never sees it and re-runs the slice over and
// over. We can't compact here (pipes passthrough), and we deliberately
// do NOT re-execute the search (no side effects, no double cost). But
// when the slice returned EXACTLY N lines, `head` almost certainly cut
// some off — so emit a one-line nudge on STDERR (leaves stdout byte-
// identical for any programmatic consumer) telling the agent to re-run
// without the slice (tokenade auto-dedupes repeats, so it stays small).
//
// Shared by the live compactor path AND the fail-open passthrough: this is
// an autonomous note with NO MCP pointer, so it stays valid on both.
pub(crate) fn emit_head_slice_advisory(shell_cmd: &str, stdout: &[u8]) {
    // Measurement gate (not a shipped opt-in): lets an A/B bench isolate
    // the net effect of this advisory by suppressing it. Default behaviour
    // is unchanged when the env var is absent.
    if std::env::var_os("TOKENADE_NO_HEAD_ADVISORY").is_some() {
        return;
    }
    if let Some(n) = filter::search_head_slice_n(shell_cmd) {
        let stdout_lines = String::from_utf8_lossy(stdout).lines().count();
        // Fleet forensics: this advisory fired 14+ times VERBATIM across two
        // sessions, including `| head -1` on by-construction-unique patterns
        // — pure spam past the first occurrences. n==1 = deliberate single
        // answer; cap at 2 firings per session after that.
        if n == 1 {
            return;
        }
        // Cap only within a REAL session: the "nosess" bucket (manual CLI,
        // tests) never resets and would silence the advisory forever after
        // two firings — ever (caught by e2e_wrap_warns_on_blind_head_truncation).
        if let Some(skey) = tokenade_core::session::ambient_session_key() {
            // Cap state lives under HOME (session-scoped, test-isolated) —
            // system temp persisted across sessions and test runs (C12).
            let cap = tokenade_core::platform::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".tokenade")
                .join("sessions")
                .join(format!(
                    "headadv-{}",
                    tokenade_core::dedup_ref::hash_content(skey.as_bytes())
                ));
            tokenade_core::platform::ensure_parent_dir(&cap);
            let fired: u32 = std::fs::read_to_string(&cap)
                .ok()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0);
            if fired >= 2 {
                return;
            }
            let _ = std::fs::write(&cap, (fired + 1).to_string());
        }
        if n > 0 && stdout_lines == n {
            eprintln!(
                "[tokenade] `head/tail -{n}` returned exactly {n} lines, so this slice MAY \
                 have hidden later matches. If your answer is already in the {n} lines above, \
                 ignore this. ONLY if it isn't, re-run WITHOUT the `| head/tail` slice \
                 (tokenade folds repeated lines, so full output stays compact) or tighten the \
                 pattern — don't re-query if you already have what you need."
            );
        }
    }
}
