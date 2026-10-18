//! **When no name answers: recognise the SHAPE.**
//!
//! The fallback does not guess the command, it reads the output — JSON, a
//! table, repeated lines — and picks the best candidate by MEASURING it rather
//! than betting on it. `Unclaimed` is the honest case: nothing was recognised,
//! and that gets said.

use super::*;

/// `systemctl list-units` (the default listing, also produced by bare
/// `systemctl` and `systemctl --type=… / --failed / --all`). One unit
/// per line:
///
/// ```text
///   UNIT                 LOAD   ACTIVE SUB     DESCRIPTION
///   sshd.service         loaded active running OpenSSH Daemon
///   foo.service          loaded failed failed  Some Broken Thing
/// ```
///
/// The signal an operator is after is the units in an ABNORMAL state —
/// `failed`, `not-found`, `inactive`/`dead`, `activating`, `masked`. The
/// noise is the long healthy run of `loaded active running` / `loaded
/// active exited` units. We fold the healthy lines into a count and keep
/// the header, every abnormal unit, and the trailing legend verbatim —
/// so a failing unit is never decapitated.
///
/// LOAD/ACTIVE/SUB are systemd enum *values* (`loaded`, `active`,
/// `running`, `exited`, `failed`, …) emitted in English regardless of
/// locale, so matching them by text is locale-safe (the localisable
/// DESCRIPTION column is never matched).
pub(super) fn systemctl_list_units(output: &str) -> String {
    // A healthy, foldable unit line: the contiguous columns `loaded
    // active running` or `loaded active exited`. Scanning a 3-token
    // window (rather than fixed indices) tolerates the leading status
    // dot systemctl prints for some units and any column padding.
    fn is_healthy(line: &str) -> bool {
        let cols: Vec<&str> = line.split_whitespace().collect();
        cols.windows(3)
            .any(|w| w[0] == "loaded" && w[1] == "active" && is_sub_value(w[2]))
    }

    /// A systemd SUB enum value, as opposed to the first word of the
    /// localisable DESCRIPTION column.
    ///
    /// The pair that decides health is LOAD=`loaded` + ACTIVE=`active`;
    /// ACTIVE is what says the unit is up, and every SUB under it is a
    /// healthy sub-state. Listing SUB values instead — this asked for
    /// `running` or `exited` — leaves out `plugged` (devices), `mounted`
    /// (mounts), `active` (targets, timers, slices), `listening` (sockets)
    /// and `waiting` (paths). Measured on a real 623-unit host: **one** line
    /// matched, so a 122 KB listing folded to 112 KB and the banner
    /// announced a run of one.
    ///
    /// The third column is still CHECKED, because dropping the check would
    /// let a DESCRIPTION reading "… loaded active configuration" fold a unit
    /// away. A SUB value is one short lower-case word.
    fn is_sub_value(tok: &str) -> bool {
        !tok.is_empty()
            && tok.len() <= 12
            && tok.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    }

    let total_healthy = output.lines().filter(|l| is_healthy(l)).count();
    // Nothing worth folding (short list, or a non-list-units output that
    // slipped through) — leave it untouched; the central guard would
    // strip our banner anyway.
    if total_healthy < 10 {
        return crate::archetypes::passthrough(output);
    }

    // **The names are the answer.** "Is X running?" is what `list-units` is
    // run for, and the healthy run folded to a bare count answered it for no
    // unit. Measured 2026-10-03 on a real host: 46 services, all healthy,
    // delivered as `(46 healthy units folded)` under a header and a legend.
    // Each run now names its units, grouped by SUB (the one column that still
    // varies), while the whole fits; the count is the fallback beyond.
    let named = fold_units(output, true);
    if tk_base::delivery::fits(&named) {
        return named;
    }
    fold_units(output, false)
}

/// `Legend: LOAD → …` and its two continuation lines: systemd prints the same
/// three sentences under every `list-units`.
fn is_legend_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("Legend: ")
        || ["ACTIVE → ", "SUB    → ", "SUB → "]
            .iter()
            .any(|p| t.starts_with(p))
}

fn fold_units(output: &str, named: bool) -> String {
    use std::fmt::Write;
    fn is_healthy(line: &str) -> bool {
        healthy_sub(line).is_some()
    }
    /// The SUB value of a `loaded active <sub>` line, and the unit name.
    fn healthy_sub(line: &str) -> Option<(&str, &str)> {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let at = cols.windows(3).position(|w| {
            w[0] == "loaded"
                && w[1] == "active"
                && !w[2].is_empty()
                && w[2].len() <= 12
                && w[2].bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        })?;
        Some((cols[at + 2], *cols.get(at.checked_sub(1)?)?))
    }
    let mut out = String::with_capacity(output.len());
    // A run of ONE is not a fold: it swaps a unit line for a banner line and
    // loses the unit's name to say so.
    let mut run: Vec<&str> = Vec::new();
    let flush = |out: &mut String, run: &mut Vec<&str>| {
        match run.len() {
            0 => {}
            1 => {
                out.push_str(run[0]);
                out.push('\n');
            }
            n if !named => {
                let _ = writeln!(out, "  ... ({n} healthy units folded — loaded/active) ...");
            }
            _ => {
                let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
                for (sub, name) in run.iter().filter_map(|l| healthy_sub(l)) {
                    match groups.iter_mut().find(|(s, _)| *s == sub) {
                        Some((_, names)) => names.push(name),
                        None => groups.push((sub, vec![name])),
                    }
                }
                for (sub, names) in groups {
                    let _ = writeln!(
                        out,
                        "  [loaded active {sub} — {}] {}",
                        names.len(),
                        names.join(" · ")
                    );
                }
            }
        }
        run.clear();
    };
    for line in output.lines() {
        if is_healthy(line) {
            run.push(line);
        } else {
            flush(&mut out, &mut run);
            if !is_legend_line(line) {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    flush(&mut out, &mut run);
    // The legend's removal leaves a blank pair behind.
    out.replace("\n\n\n", "\n\n").trim_end().to_string()
}

/// The size half of the pathological-reduction guard's condition, with the
/// tiny-output test INVERTED: a reduction the guard would have rejected if only
/// its flat 30-token ceiling had scaled with the input.
///
/// Named and separate so the boundary is assertable. The event it drives is
/// telemetry, not a verdict — see the call site for why the ceiling was left
/// alone rather than widened on a hypothesis.
pub(super) fn near_miss_above_flat_ceiling(before: usize, after: usize) -> bool {
    const MIN_INPUT: usize = 100;
    const FLAT_CEILING: usize = 30;
    const SUSPICIOUS_PCT: usize = 80;
    before >= MIN_INPUT
        && after > FLAT_CEILING
        && after < before
        && (before - after) * 100 / before.max(1) >= SUSPICIOUS_PCT
}

/// A compactor that declined (`None`) is not a failure: the output was short
/// enough to print whole, and the generic/auto pass decides what happens next.
/// Six arms of `dispatch_by_command` said that in four lines each; they say it
/// here once, so a new listing compactor is one line of routing.
pub(super) fn folded_or_fallthrough(
    name: &'static str,
    folded: Option<String>,
    output: &str,
    before: usize,
) -> Result<(&'static str, String), FilterResult> {
    folded.map(|c| (name, c)).ok_or_else(|| {
        // The refusal gets a name. `folded == None` here means the compactor
        // written FOR this program did not recognise this program's output —
        // not that there was nothing to do. See
        // `archetypes::DECLINED_NAMED`.
        crate::archetypes::mark_named_compactor_declined(name);
        auto_or_generic(output, before)
    })
}

pub(super) fn auto_or_generic(output: &str, before: usize) -> FilterResult {
    auto_or_generic_keeping(output, before, false)
}

/// [`auto_or_generic`], told by the caller that `output` is a DOCUMENT the
/// agent asked to read (see `intent::pipeline_only_slices_a_source_file`):
/// unclaimed, it is kept rather than stride-sampled by `generic`.
pub(super) fn auto_or_generic_keeping(output: &str, before: usize, document: bool) -> FilterResult {
    // A command's output, not a file's bytes: the escapes it sent a terminal
    // are not text. Every routed arm of `dispatch` strips them; this door —
    // a pipe, a program nobody routes, `cat` of a non-source file — did not,
    // and whenever the content route folded nothing the passthrough handed
    // them over. Measured 2026-10-02 on `pnpm run ci 2>&1 | tail -60`, a real
    // call: 11 colour pairs around `✔`/`✖` delivered as is. Cursor moves are
    // replayed FIRST — `replay_cursor_redraws` reads them.
    let cleaned;
    let output = if output.contains('\x1b') {
        cleaned = strip_ansi(&auto::normalise_redraws(output));
        cleaned.as_str()
    } else {
        output
    };
    let (kind, compacted, savings) = auto::compact(output);

    // `Kind::Challenge` is a SECURITY / CORRECTNESS signal, not a size
    // play: a Cloudflare interstitial page is small enough that the
    // stealth-remediation banner makes the output BIGGER, but the agent
    // absolutely needs to know it hit a bot wall (otherwise it'll keep
    // scraping HTML that contains no real content). Always surface the
    // detected challenge, even at negative savings.
    if matches!(kind, auto::Kind::Challenge) {
        return FilterResult {
            filter: format!("auto:{}", kind.as_str()),
            output: compacted,
            savings,
        };
    }

    // A contiguous line-number gutter (`nl`, `cat -n`, `… | sed -n 'a,bp'`) is
    // a FILE the agent asked to see, numbered — a document, not transient
    // command noise. Folded, `generic` stride-samples the middle: measured
    // 2026-10-02, `nl -ba dispatch.rs | sed -n '300,500p'` lost 51 of its 201
    // lines, one in two across the middle of the very window requested. Kept,
    // a large one still goes to `disclose` like any document.
    let unclaimed = if document || crate::line_gutter::split(output).is_some() {
        Unclaimed::Keep
    } else {
        Unclaimed::Fold
    };
    let (filter, output, savings) =
        structured_or_generic(output, before, kind, compacted, savings, unclaimed);
    FilterResult {
        filter,
        output,
        savings,
    }
}

/// "Is the structured pass worth keeping, or does generic do better?" — ONE
/// home for the whole decision, both paths.
///
/// It used to be spelled twice. The COMMAND path had all three stages here;
/// the CONTENT path (`tokenade read`, the Read hook, `cmd_auto`) had only the
/// first, and an `else` that returned the structured output UNFOLDED. So a
/// claimant that folded nothing was delivered whole, and the fallback that
/// exists precisely for that case never ran. The gate is the same on both
/// sides — `tokens_after < before` — which is what made the divergence
/// invisible: the missing clause was not in the condition, it was in the
/// branch nobody had written.
///
/// Measured 2026-08-26 on a 29 018-byte storybook debug log with no path hint,
/// which `Json` claims and does not fold: 29 019 bytes delivered through the
/// content path (it GREW by one) against 4 626 through the command path. Same
/// bytes, same detector, 6.3x, decided by which spelling ran. The extension
/// hint hides this on files — `.log` routes to `Logs` before any of it — so it
/// bites exactly where there is no filename: MCP tool results, stdin, and any
/// extension the hint table does not know.
///
/// Returns the ledger label, the output and its savings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unclaimed {
    /// Command output nobody claimed is transient noise: dedup and truncate it,
    /// with the `expand-ref` pointer for the verbatim bytes.
    Fold,
    /// A DOCUMENT nobody claimed is served whole. Measured: a
    /// `pytest --collect-only` listing detects as `Unknown` and the information
    /// IS the three thousand test names, so folding 335 of its lines away
    /// removes the answer. The two paths genuinely disagree here — declared
    /// once, as an argument, rather than spelled twice as two `else` branches.
    Keep,
}

/// The delivered answer, with one backstop our token ESTIMATE cannot provide.
///
/// Every comparison below weighs `tokenizer::estimate`, and the agent is billed
/// on someone else's tokenizer. MEASURED 2026-09-04 on a 6,579-byte YAML of 400
/// `keyN: valueN` lines: our estimate read 2,839 tokens in and 2,373 out — a
/// 466-token saving — while tiktoken, which is what the model actually pays,
/// read 2,399 in and **2,475 out**. The fold dropped 222 lines and the recovery
/// footer cost more than they did, and nothing could see it, because the only
/// number in scope was the one that was wrong.
///
/// Bytes are not an estimate. An answer LONGER than its input has compacted
/// nothing, whatever any tokenizer says, so it is not delivered.
///
/// LONGER, not "no shorter": an answer of the same length may still be worth
/// delivering, because a reshape can cost fewer tokens at equal bytes (a table
/// rebuilt as rows, a JSON document minified). The ceiling is against
/// inflation, not against equality.
pub fn structured_or_generic(
    output: &str,
    before: usize,
    kind: auto::Kind,
    compacted: String,
    savings: Savings,
    unclaimed: Unclaimed,
) -> (String, String, Savings) {
    let (label, out, sav) =
        structured_or_generic_inner(output, before, kind, compacted, savings, unclaimed);
    // `Challenge` is the one answer that may grow: a bot wall is small and the
    // remediation banner is the point (see `auto_or_generic`).
    if out.len() > output.len() && !matches!(kind, auto::Kind::Challenge) {
        return (
            "passthrough".into(),
            output.to_string(),
            Savings::new(before, before),
        );
    }
    (label, out, sav)
}

/// Is `out` the same text as `input`, differing only in bytes an artifact
/// depends on — CRLF against LF, or a leading byte-order mark?
///
/// A compactor that REBUILDS its input (markdown, csv) re-emits normalised
/// text, and when there was nothing to fold that rebuild is the entire
/// difference. MEASURED 2026-09-04: a 29-byte CRLF markdown came back as 26
/// bytes of the same words with every `\r\n` turned into `\n`, and a 15-byte
/// CRLF csv the same way; a BOM was dropped the same silent way. Three bytes
/// saved, and `tokenade read x.md > y.md` rewrites the file — which is the harm
/// `read`'s byte-fidelity rule already names, one layer up: an artifact whose
/// checksum no longer matches.
///
/// A file big enough to be RESHAPED is a view and may differ however it likes;
/// this only catches the case where the reshape produced the same text back.
///
/// A byte-order mark is NOT part of this rule, measured: it is removed by
/// `text_encoding::decode` before any compactor sees the text, so nothing here
/// could put it back. That is an encoding decision and it is left alone.
pub fn differs_only_in_line_endings(input: &str, out: &str) -> bool {
    if input == out {
        return false;
    }
    // The closing newline is normalised away too, because it is decided at the
    // PRINT site and not here: `wrap::needs_terminator` re-adds one to a view
    // and withholds it from a passthrough. Comparing with it in place made this
    // rule miss every case it was written for — the rebuild that loses the
    // trailing newline is the same rebuild that turns `\r\n` into `\n`.
    let norm = |s: &str| s.replace("\r\n", "\n").trim_end_matches('\n').to_string();
    norm(input) == norm(out)
}

/// Fallback path for commands we don't have a hand-tuned filter for:
/// run `auto::compact` (which dispatches by content to one of 48 format-
/// specific compactors) and only fall through to the plain `generic` pass
/// if `auto` detected nothing or made things worse.
pub(super) fn structured_or_generic_inner(
    output: &str,
    before: usize,
    kind: auto::Kind,
    compacted: String,
    savings: Savings,
    unclaimed: Unclaimed,
) -> (String, String, Savings) {
    // Detected a structured kind AND the compactor actually shrank it
    // → use the structured output, once every competing candidate has had its
    // turn.
    if kind != auto::Kind::Unknown && savings.tokens_after < before {
        let (suffix, out, sav) = best_auto_candidate(output, before, kind, compacted, savings);
        return (format!("auto:{}{suffix}", kind.as_str()), out, sav);
    }

    if kind == auto::Kind::Unknown && unclaimed == Unclaimed::Keep {
        return (
            "passthrough".into(),
            output.to_string(),
            Savings::new(before, before),
        );
    }

    // Either no kind detected, OR the structured compactor was a no-op
    // (e.g. an 8-line `cargo bench` block where there's nothing to
    // aggregate). Fall back to generic — dedup + truncate.
    let cleaned = generic(output);
    let after = tokenizer::estimate(&cleaned);

    // Label rule (post-L122 audit, was always `auto:<kind>` before):
    //   - If generic ALSO produced no saving (after >= before): this
    //     was a misclassification (e.g. `echo "=== locales ===" && ls
    //     i18n/` matched as `proto` but isn't protobuf). Labeling it
    //     `auto:proto` pollutes the dashboard / suggests the compactor
    //     was the right pick. Use `passthrough` instead and return
    //     the original bytes.
    //   - If generic DID shrink (after < before): keep the kind label
    //     so we can see which structured detector triggered the
    //     fallback path (useful when tuning the detector heuristics).
    if after >= before {
        return (
            "passthrough".into(),
            output.to_string(),
            Savings::new(before, before),
        );
    }
    let label = if kind != auto::Kind::Unknown {
        format!("auto:{}", kind.as_str())
    } else {
        "generic".into()
    };
    (label, cleaned, Savings::new(before, after))
}

/// The competing candidates a structured compaction must beat, and the guard it
/// must survive — shared by BOTH paths.
///
/// These lived inside `auto_or_generic`, which only the COMMAND path reaches.
/// The CONTENT path (`tokenade read`, the Read hook, `cmd_auto`) called
/// `auto::compact` bare and got none of them. Measured 2026-08-26 on a
/// 94 890-byte tar listing of 1 500 entries in 1 500 distinct top-level
/// directories: the archive compactor prints ONE LINE PER ENTRY (39 538 bytes,
/// −58%) while the template fold sees one shape and answers in about 1 600
/// (−98%). 24x, decided only by whether tokenade RAN the command or was handed
/// the bytes. The ledger had been saying so for weeks: the same 13 841-token
/// payload booked at 89 tokens through `auto:archive+fold` and at 10 187
/// through `auto-compact`, nine times out of ten the second.
///
/// Returns the label SUFFIX (`""`, `"+rejected"`, `"+fold"`, `"+cap"`), the
/// output, and its savings.
pub fn best_auto_candidate(
    output: &str,
    before: usize,
    kind: auto::Kind,
    compacted: String,
    savings: Savings,
) -> (&'static str, String, Savings) {
    // SAFETY: misclassification guard. The format detector can match
    // a non-target format when the content contains a couple of
    // distinctive keywords (e.g. SSH output of Docker BuildKit logs
    // mentioning `npm` / `apt` gets classified as `pkginstall`,
    // whose compactor then emits its empty summary banner — 816→19
    // tokens of pure data loss).
    //
    // Rule: if the compactor reduced > 80% on substantial input
    // (>= 100 tokens) AND the result is suspiciously tiny
    // (<= 30 tokens), the detector was almost certainly wrong.
    // Reject the structured pass and let the generic deduper try.
    // Verified against the auto:pkginstall-on-SSH leak observed
    // in production at 816→19t.
    // The three thresholds live on [`reduction_is_pathological`], which both
    // doors now ask.
    // NEAR MISS: everything the guard requires EXCEPT the flat 30-token
    // ceiling. That ceiling is absolute while the input is not,
    // so the guard cannot fire above 30 tokens of output however large the
    // input — and every rejection recorded so far
    // (`~/.tokenade/debug.log`, 2026-08-04: markdown 329→10, ndjson 362→24,
    // pkginstall 258→30, ndjson 481→20) is a SMALL input. That is not
    // evidence that large ones are safe; large ones were never examined.
    //
    // Deliberately telemetry and NOT a widened ceiling. The reason to
    // expect the flat cap to hold is that a misdetection banner is
    // FIXED-size (`pkginstall`'s empty summary is ~19 tokens whether it ate
    // 816 or 81 600), so it should stay under 30 at any input size. That is
    // a hypothesis, and one of these two things is true: it holds and this
    // line never fires, or it does not and this line hands over the case
    // that proves it. Widening the ceiling on a guess would trade a
    // measured leak for an unmeasured loss of legitimate savings.
    if near_miss_above_flat_ceiling(before, savings.tokens_after)
        && !crate::speaks_for::structured_still_speaks_for(output, &compacted)
    {
        crate::debug_log::log(
            "filter",
            "auto_compactor_near_miss_above_flat_ceiling",
            serde_json::json!({
                "kind": kind.as_str(),
                "before": before,
                "after_structured": savings.tokens_after,
                "reduction_pct": (before - savings.tokens_after) * 100 / before.max(1),
            }),
        );
    }
    if crate::speaks_for::reduction_is_pathological(
        output,
        &compacted,
        before,
        savings.tokens_after,
    ) {
        {
            let reduction = (before - savings.tokens_after) * 100 / before.max(1);
            crate::debug_log::log(
                "filter",
                "auto_compactor_rejected_pathological_reduction",
                serde_json::json!({
                    "kind": kind.as_str(),
                    "before": before,
                    "after_structured": savings.tokens_after,
                    "reduction_pct": reduction,
                }),
            );
            // Fall through to the generic deduper below instead of
            // returning the structured compactor's tiny result.
            let cleaned = generic(output);
            let after = tokenizer::estimate(&cleaned);
            return ("+rejected", cleaned, Savings::new(before, after));
        }
    }
    // A structured compactor summarises by FORMAT — how many rows, how
    // many of each level. When the payload ALSO repeats itself, the
    // template fold says as much for a fraction: 3 000 identical NDJSON
    // lines cost 139 tokens as a format summary and 25 as one template
    // line plus its counts.
    //
    // Strictly a competing candidate, never a replacement. It has to win
    // by a clear margin, and it has to still carry the input's own words
    // (same predicate the misclassification guard uses) — so a payload
    // whose rows genuinely differ produces many templates, a fat fold,
    // and this declines on its own. Verified: an NDJSON stream with a
    // scattered error level folds to 2 853 tokens against the summary's
    // 1 188, and the summary rightly keeps the round.
    //
    // Restricted to formats where the LINE is the unit of meaning. In a
    // document format — pretty-printed JSON, YAML, a diff, a manifest —
    // folding away lines produces something that still looks like the
    // format and no longer parses as it, which is worse than being long.
    const FOLD_MUST_BEAT_BY: usize = 2;
    const FOLD_WORTH_TRYING_TOKENS: usize = 50;
    let line_oriented = matches!(
        kind,
        auto::Kind::Ndjson
            | auto::Kind::Logs
            | auto::Kind::AccessLog
            | auto::Kind::Journal
            | auto::Kind::Csv
            | auto::Kind::Tsv
            | auto::Kind::PrometheusMetrics
            | auto::Kind::ArchiveListing
            | auto::Kind::Strace
            | auto::Kind::Dns
    );
    if line_oriented && savings.tokens_after >= FOLD_WORTH_TRYING_TOKENS {
        let folded = crate::logs::compact_generic_logs(output);
        let folded_tokens = tokenizer::estimate(&folded);
        if folded_tokens.saturating_mul(FOLD_MUST_BEAT_BY) < savings.tokens_after
            && crate::speaks_for::structured_still_speaks_for(output, &folded)
        {
            return ("+fold", folded, Savings::new(before, folded_tokens));
        }
    }
    // L46 — hard cap mirrors the L45 Read-interceptor net.
    // Apply head/tail truncation on top of the structured
    // compaction when the result is still huge (e.g. multi-MB
    // log with thousands of distinct templates).
    if let Some((truncated, trunc_after)) = apply_hard_cap(&compacted, savings.tokens_after, "read")
    {
        return ("+cap", truncated, Savings::new(before, trunc_after));
    }
    ("", compacted, savings)
}
