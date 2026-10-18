//! **`tokenade gain`: the ledger, read back and rendered.**
//!
//! The command that reads the most lines in the product. The delicate point is
//! the comparison BASELINE: a borrowed baseline (another model's, another
//! period's) gives a percentage that looks like a fact, and the command must
//! say which one it used.

use super::*;

/// Best-effort terminal width detection. Returns `None` outside a TTY or
/// when the ioctl is unavailable; the caller falls back to 80 columns.
pub(crate) fn term_width() -> Option<usize> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return None;
    }
    if let Ok(s) = std::env::var("COLUMNS") {
        if let Ok(n) = s.parse::<usize>() {
            return Some(n);
        }
    }
    None
}

pub(crate) fn cmd_audit_gains(rest: &[String]) -> i32 {
    if let Err(c) = crate::code_intel::refuse_all_positionals(
        rest,
        "audit-gains",
        &["--json", "--help", "-h"],
        &[],
    ) {
        return c;
    }
    let json = rest.iter().any(|a| a == "--json");
    let (checked, bad) = tokenade_core::gain::audit_gains();
    if json {
        println!(
            "{}",
            serde_json::json!({"rows_checked": checked, "violations": bad.len(), "detail": bad})
        );
        return i32::from(!bad.is_empty());
    }
    println!("Ledger reconciliation \u{2014} {checked} rows checked");
    if bad.is_empty() {
        println!("  \u{2713} no invariant violations (saved\u{2264}before-after, exact\u{2260}estimated, io/class sane)");
        0
    } else {
        println!("  \u{2717} {} violation(s):", bad.len());
        for b in &bad {
            println!("    \u{b7} {b}");
        }
        1
    }
}

/// `" (91% of it on 07-28)"` when ONE calendar day owns most of an op's window volume,
/// `""` otherwise. The percentage beside an op is a volume-weighted sum, so a
/// single busy day decides it; without this the reader takes a one-day figure
/// for a trend. Two guards keep it honest:
///
/// * **80 % dated.** Rows written before `ts` existed carry no day. If they are
///   more than a fifth of the volume, the concentration is not knowable and
///   nothing is claimed.
/// * **70 % share.** Below that no single day determines the number, and the
///   note would be noise on every row.
/// * **Five calls.** Fewer than that and `N×` has already told the reader.
///
/// Scoped to `gain`'s `by op:` list on purpose: that is where a percentage is
/// the headline. The dashboard's TOP COMPACTORS ranks by absolute tokens saved,
/// so nothing there invites reading a rate as a trend.
pub(crate) fn heaviest_day_note(
    days: Option<&std::collections::BTreeMap<i64, u64>>,
    op_before: u64,
    count: u64,
) -> String {
    // Under five calls the `N×` column already says this cannot be a trend,
    // and the note would fire on every one-shot op as pure noise.
    const MIN_CALLS: u64 = 5;
    if count < MIN_CALLS {
        return String::new();
    }
    let Some(days) = days else {
        return String::new();
    };
    let dated: u64 = days.values().sum();
    if dated == 0 || dated.saturating_mul(100) < op_before.saturating_mul(80) {
        return String::new();
    }
    let Some((&day, &top)) = days.iter().max_by_key(|(_, &v)| v) else {
        return String::new();
    };
    if top.saturating_mul(100) < dated.saturating_mul(70) {
        return String::new();
    }
    let (_, m, d) = tokenade_core::platform::civil_from_days(day);
    // `of it` and the parentheses, because the row already carries a
    // percentage and the two mean different things: `−91%` is a SAVING RATE,
    // this one is a SHARE OF VOLUME. Printed as `38× −91%  89% on 08-31` they
    // sit side by side in the same unit with nothing between them, and the
    // second reads as "saved 89% that day" — a misreading by the very note
    // written to prevent one.
    format!(
        "  ({:.0}% of it on {m:02}-{d:02})",
        100.0 * top as f64 / dated as f64
    )
}

/// A single rule, in core, next to the ratio it is about: see
/// [`tokenade_core::code_intel_heuristic::baseline_is_borrowed`]. The terminal
/// table was the only consumer that knew it; `gain --json` did not, and the
/// value ranking that reads this JSON to CHOOSE each iteration's target
/// therefore presented a borrowed rate amid fourteen measured rates.
/// "Measured, borrowed, or both?" for the savings of ONE aggregated row.
///
/// An `op` is not homogeneous: `skeleton` tokenizes the file it is handed
/// (measured) whereas `structure_map` has no input (borrowed), and one family
/// can carry both. Returning three states rather than a boolean is what lets
/// a consumer exclude the rows whose rate means nothing without throwing away
/// those that have one.
pub(crate) fn baseline_kind(borrowed_saved: u64, saved: u64) -> &'static str {
    if borrowed_saved == 0 {
        "measured"
    } else if borrowed_saved >= saved {
        "borrowed"
    } else {
        "mixed"
    }
}

pub(crate) fn baseline_is_borrowed(class: Option<&str>, op: &str, before: u64, after: u64) -> bool {
    tokenade_core::coefficients::baseline_is_borrowed(class, op, before, after)
}

/// Everything `tokenade gain` accumulates in one pass over the ledger.
///
/// Field names are the local names the printers below already use, so the
/// reporting code reads unchanged after the destructuring.
#[derive(Default)]
pub(super) struct GainTotals {
    total_before: u64,
    total_after: u64,
    estimated_saved: u64,
    /// How many CALLS the estimated rows stand for. `estimated_saved` lets a
    /// reader reconcile the token totals; nothing let them reconcile the call
    /// count, and the by-op table includes these rows while `operations:` does
    /// not.
    estimated_calls: u64,
    borrowed_ratio_saved: u64,
    /// Of the measured total, the part standing on a delivery-cap stand-in.
    /// The signed receipt calls the same number `tokens_saved_approximated`;
    /// that name is frozen by the signature and cannot be aligned.
    approximated_saved: u64,
    /// The same rows credited on the server's coarser dedup key. Never above
    /// the local figure: `credit_key` is `server_dedup_key` plus two fields,
    /// so it can only split groups the server keeps together.
    metered_count: u64,
    metered_before: u64,
    metered_after: u64,
    count: u64,
    per_op: std::collections::BTreeMap<String, (u64, u64, u64)>,
    /// Of `per_op`, the share of `saved` whose baseline is BORROWED from the
    /// published ratio. A separate map, like `per_op_day`, so as not to change
    /// the shape seven consumers already destructure.
    per_op_borrowed: std::collections::BTreeMap<String, u64>,
    /// Of `per_op`, what was actually delivered (`gain::delivered_of`):
    /// `after` minus the outputs that a stash or a dedup of the same call
    /// replaced. A separate map, like `per_op_borrowed`.
    per_op_delivered: std::collections::BTreeMap<String, u64>,
    per_source: std::collections::BTreeMap<String, (u64, u64, u64)>,
    per_op_day: std::collections::BTreeMap<String, std::collections::BTreeMap<i64, u64>>,
    /// The span the totals actually cover, from the ledger's own rows.
    ///
    /// `--recent` names its window in the header because "the number alone
    /// cannot say which it is" — and the DEFAULT, which is every row ever
    /// recorded, named nothing. So a lifetime average across months of use,
    /// several versions and a rotation reads as a rate that applies today.
    /// Measured on this install: 33,250 rows spanning three months behind a
    /// bare `saved: 93.2%`. Rows with no `ts` predate the field and are not
    /// accounted here, for the same reason they are not bucketed by day.
    first_ts: u64,
    last_ts: u64,
}

/// One pass over the gain ledger, crediting each distinct event once.
///
/// What is shared with every other reader of this ledger is `gain::decode_row`;
/// what stays here is what only `gain` reports — the credit-once set, the
/// per-source bucket and the borrowed-ratio share.
///
/// `history` also makes the pass PRINT each row as it goes, which is why the
/// two output flags come in rather than the caller filtering afterwards: the
/// `--history` listing is the scan, in order.
pub(super) fn scan_gain_ledger(
    content: &str,
    cutoff: u64,
    history: bool,
    json_mode: bool,
) -> GainTotals {
    use tokenade_core::code_intel_heuristic::is_code_intel as is_ci;
    let mut total_before = 0u64;
    let mut total_after = 0u64;
    let mut estimated_saved = 0u64;
    let mut estimated_calls = 0u64;
    // The part of `estimated_saved` whose BASELINE is the published
    // third-party ratio in `code_intel_heuristic`, not something measured
    // here. Measured on this install: 27 % of the bucket. The other 73 % is
    // `skeleton`, where the file really was read and tokenised. One line
    // calling both "inferred baseline, not measured" was wrong about the
    // larger half, and it was the confident half.
    let mut borrowed_ratio_saved = 0u64;
    // The slice of the MEASURED total that rests on a stand-in rather than
    // exact arithmetic. `LedgerScan` has tracked this since it was added and
    // the signed receipt prints it; this scanner never computed it, so
    // `tokenade gain` printed the total in silence while disclosing two
    // SMALLER estimate categories right beneath it (827 982 and 127 321
    // against 4 736 778 on this install). Same rule, one function, both
    // scanners.
    let mut approximated_saved = 0u64;
    // The same rows credited on the COARSER key the server's ingest uses
    // (`gain::server_dedup_key`). `credit_key` is that key plus the label and
    // the session, so this can only be lower — and the gap is the one number a
    // user needs when their receipt and their account disagree. Three comments
    // asserted the gap was zero; it stopped being zero the day the local key
    // gained those two fields, and no surface said so.
    let mut metered_credited: std::collections::HashSet<(u64, String, u64, u64)> =
        std::collections::HashSet::new();
    let mut metered_count = 0u64;
    let mut metered_before = 0u64;
    let mut metered_after = 0u64;
    let mut count = 0u64;
    let mut per_op: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
    let mut per_op_borrowed: std::collections::BTreeMap<String, u64> = Default::default();
    let mut per_op_delivered: std::collections::BTreeMap<String, u64> = Default::default();
    let mut per_source: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
    // op → (days-since-epoch → `before`). A window figure is a SUM, so one
    // heavy day owns it. Measured on this install's 7-day window: 91 % of
    // `html-to-markdown`'s volume came from a single day (07-28, −56 %), while
    // the two most recent days scored −90 % and −93 %. The header said −57 %
    // and read as "how it is doing now"; it was how one day went, and that day
    // was the OLDEST in the window. `structure_map` was 96 % one day,
    // `read_disclose` and `sysinfo` 100 %. See [`heaviest_day_note`].
    let mut first_ts: u64 = 0;
    let mut last_ts: u64 = 0;
    let mut per_op_day: std::collections::BTreeMap<String, std::collections::BTreeMap<i64, u64>> =
        Default::default();
    // `--recent`: the last seven days only.
    //
    // The ledger is months deep and the report sums all of it, so a compactor
    // that was rebuilt last week still shows the number it scored in June. On
    // this machine `a11y-compact` reads `3× −7%` — three events from 2026-07-20,
    // against 41 % on the same content today. A figure that averages a fix with
    // the defect it replaced answers a question nobody asked.
    //
    // A window, not a filter on version: the ledger records no version, and the
    // honest proxy for "how is it doing NOW" is "what has it done lately".
    let mut credited: std::collections::HashSet<(u64, String, u64, u64, u64)> =
        std::collections::HashSet::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !credited.insert(tokenade_core::gain::credit_key(&v)) {
            continue;
        }
        // A row with no timestamp predates the field; under `--recent` it is
        // by definition not recent.
        let row_ts = v["ts"].as_u64().unwrap_or(0);
        if cutoff > 0 && row_ts < cutoff {
            continue;
        }
        // Row decoding is shared with every other surface that reads this
        // ledger (`gain::decode_row`): rollup cardinality, the `filter`
        // fallback for `op`, and the #544 baseline expansion for a code-intel
        // row that recorded no delta. What stays local to `gain` is what only
        // `gain` reports: the credit-once set above, the per-source bucket,
        // and the borrowed-ratio share below.
        let row = tokenade_core::gain::decode_row(&v);
        let (b, a, is_code_intel) = (row.before, row.after, row.is_code_intel);
        let f = row.op.clone();
        let src = v["source"].as_str().unwrap_or("cli").to_string();
        // A row whose baseline came out of the borrowed ratio rather than out
        // of anything this machine read. TWO shapes: the writer already
        // expanded it and said so (`class: "ci_heuristic"`), or it arrived
        // unmeasurable (`before == after`) and `decode_row` applied the
        // multiplier. Everything else in the code-intel bucket had a REAL
        // input — `skeleton` tokenises the file it was handed; only the
        // alternative ("you would have read it whole") is assumed.
        let from_borrowed_ratio = baseline_is_borrowed(
            v["class"].as_str(),
            &f,
            v["before"].as_u64().unwrap_or(0),
            v["after"].as_u64().unwrap_or(0),
        );
        let saved = b.saturating_sub(a);
        if is_code_intel {
            estimated_saved += saved;
            // How MANY calls those estimates stand for. `saved:` has
            // `estimated_saved` beside it to reconcile with, and
            // `operations:` had nothing: the by-op rows below are filled for
            // every row, so the table's `N×` column summed past the headline
            // with no number anywhere that closed the gap.
            estimated_calls += row.calls;
            if from_borrowed_ratio {
                borrowed_ratio_saved += saved;
            }
        } else {
            // A rollup row stands for N original calls; counting it as one
            // would make rotated history look quieter than it was.
            count += row.calls;
            total_before += b;
            total_after += a;
            if metered_credited.insert(tokenade_core::gain::server_dedup_key(&v)) {
                metered_count += row.calls;
                metered_before += b;
                metered_after += a;
            }
            approximated_saved =
                approximated_saved.saturating_add(tokenade_core::coefficients::approximated_slice(
                    v["agent"].as_str().unwrap_or(""),
                    &src,
                    &f,
                    b,
                    a,
                ));
        }
        // Same `b` the percentage is computed from, bucketed by calendar day.
        // Rows with no `ts` predate the field and are simply not accounted —
        // better to say nothing than to attribute them to a day.
        if row_ts > 0 {
            first_ts = if first_ts == 0 {
                row_ts
            } else {
                first_ts.min(row_ts)
            };
            last_ts = last_ts.max(row_ts);
            *per_op_day
                .entry(f.clone())
                .or_default()
                .entry((row_ts / 86_400) as i64)
                .or_insert(0) += b;
        }
        // Same argument as `count += row.calls` above, and it was made there
        // and not here: a rollup row stands for N original calls, so counting
        // it as one makes rotated history look quieter than it was. The
        // breakdown was doing exactly what the total's own comment forbids —
        // measured 2026-09-02 on this install, `operations:` read 72 064 while
        // the `N×` column under it summed to 1 041, the same word 69× apart in
        // one screen. `before`/`after` were already summed from the rollup, so
        // only the call count was wrong: a row reading `auto:makefile 11×` was
        // announcing eleven calls for a saving accumulated over hundreds.
        if from_borrowed_ratio {
            *per_op_borrowed.entry(f.clone()).or_insert(0) += saved;
        }
        *per_op_delivered.entry(f.clone()).or_insert(0) += if v.get("delivered").is_some() {
            tokenade_core::gain::delivered_of(&v)
        } else {
            a
        };
        let e = per_op.entry(f).or_insert((0, 0, 0));
        e.0 += b;
        e.1 += a;
        e.2 += row.calls;
        let s = per_source.entry(src).or_insert((0, 0, 0));
        s.0 += b;
        s.1 += a;
        s.2 += row.calls;
        if history && !json_mode {
            println!(
                "{:>6}→{:<6}  −{:>3.0}%  {}",
                b,
                a,
                Savings::new(b as usize, a as usize).percent(),
                v["label"].as_str().unwrap_or("")
            );
        }
    }
    GainTotals {
        total_before,
        total_after,
        estimated_saved,
        estimated_calls,
        borrowed_ratio_saved,
        approximated_saved,
        metered_count,
        metered_before,
        metered_after,
        count,
        per_op,
        per_op_borrowed,
        per_op_delivered,
        per_source,
        per_op_day,
        first_ts,
        last_ts,
    }
}

pub(crate) fn cmd_gain(rest: &[String]) -> i32 {
    // Silent-accept sweep (#813-#815 follow-up): reject unknown flags.
    if let Err(code) = validate_simple_bool_flags(
        rest,
        "gain",
        &["--history", "--by-source", "--json", "--recent"],
    ) {
        return code;
    }
    // Same rule as `map --json -o paths`: two shapes, only one wins. An
    // earlier comment said `--json` "suppresses" the `--history` dump;
    // suppressing what was asked for without saying so is exactly what this
    // repository refuses elsewhere.
    if let Err(code) = crate::install_cli::reject_conflicting_shape_flags(
        rest,
        "gain",
        "--history",
        "--json",
        "--json emits one aggregate object; --history emits one row per ledger entry.",
    ) {
        return code;
    }
    use tokenade_core::code_intel_heuristic::is_code_intel as is_ci;
    let history = rest.iter().any(|a| a == "--history");
    let by_source = rest.iter().any(|a| a == "--by-source");
    // #616 — JSON output for CI/dashboards (mirror of #613's
    // `healthcheck --json`). Suppresses both the human text AND the
    // per-line `--history` dump; emits a single structured object.
    let json_mode = rest.iter().any(|a| a == "--json");
    let path = global_gain_path();
    // #1045 — ledger UNION rollup, like every other surface. Reading the live
    // file alone meant `tokenade gain` reported only what had not yet been
    // rotated: the same defect the statusline had, found by the test that
    // makes the four surfaces answer with one number.
    let content = tokenade_core::gain::ledger_with_rollup(&path);
    let recent = rest.iter().any(|a| a == "--recent");
    let cutoff = if recent {
        tokenade_core::platform::now_unix_secs().saturating_sub(7 * 86_400)
    } else {
        0
    };
    // PO #37 (2026-08-04): credit each identical event ONCE. A machine loop
    // repeating one command books its full counterfactual on every pass, and
    // that counterfactual assumes a model turn that never happened — measured,
    // a `cat` loop at ~37 identical events/second put 89 M tokens into a single
    // day, 28 % of this install's lifetime figure, for ~20 s of work. The key
    // is `gain::credit_key`, which is `gain::server_dedup_key` plus the label
    // and the session — FINER than what the server's ingest dedups on, so this
    // headline can sit above the metered number (measured: 0.24 %). It is not
    // the same key, and this comment said it was for a day. Rows are never
    // dropped from the ledger; only the credit is collapsed.
    let GainTotals {
        total_before,
        total_after,
        estimated_saved,
        estimated_calls,
        borrowed_ratio_saved,
        approximated_saved,
        metered_count,
        metered_before,
        metered_after,
        count,
        per_op,
        per_op_borrowed,
        per_op_delivered,
        per_source,
        per_op_day,
        first_ts,
        last_ts,
    } = scan_gain_ledger(&content, cutoff, history, json_mode);
    if json_mode {
        // Emit even on empty ledger — consumers prefer a stable
        // shape over a one-off "no savings yet" string.
        let per_op_arr: Vec<serde_json::Value> = per_op
            .iter()
            .map(|(op, (b, a, n))| {
                serde_json::json!({
                    "op": op,
                    "before": b,
                    "after": a,
                    "delivered": per_op_delivered.get(op.as_str()).copied().unwrap_or(*a),
                    "count": n,
                    "saved": b.saturating_sub(*a),
                    "percent": Savings::new(*b as usize, *a as usize).percent(),
                    // What share of this op's volume came from its single
                    // busiest day, 0.0 when not knowable. A consumer plotting
                    // `percent` as a trend needs this to know when it isn't one.
                    // What does this row compare its rate against?
                    // `measured`: an input this machine read.
                    // `borrowed`: the baseline is the published ratio of
                    // `code_intel_heuristic`, so `percent` equals that ratio
                    // and nothing was folded. `mixed`: both.
                    // The terminal table has always partitioned the
                    // code-intel ops; this table did not, and it is the one
                    // automated consumers read.
                    "baseline": baseline_kind(per_op_borrowed.get(op.as_str()).copied().unwrap_or(0), b.saturating_sub(*a)),
                    "borrowed_saved": per_op_borrowed.get(op.as_str()).copied().unwrap_or(0),
                    "heaviest_day_share": per_op_day
                        .get(op.as_str())
                        .map(|d| {
                            let tot: u64 = d.values().sum();
                            let top = d.values().max().copied().unwrap_or(0);
                            if tot == 0 { 0.0 } else { top as f64 / tot as f64 }
                        })
                        .unwrap_or(0.0),
                })
            })
            .collect();
        let per_source_arr: Vec<serde_json::Value> = per_source
            .iter()
            .map(|(src, (b, a, n))| {
                serde_json::json!({
                    "source": src,
                    "before": b,
                    "after": a,
                    "count": n,
                    "saved": b.saturating_sub(*a),
                    "percent": Savings::new(*b as usize, *a as usize).percent(),
                })
            })
            .collect();
        let root = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "operations": count,
            "before": total_before,
            "after": total_after,
            "saved": total_before.saturating_sub(total_after),
            "percent": Savings::new(total_before as usize, total_after as usize).percent(),
            // #741 — separate heuristic estimate (code-intelligence
            // tools) from measured savings, mirroring how
            // dashboard --json exposes estimated_tokens_saved. The
            // CodeGraph 0.9.4 baseline is conservative; clients that
            // want the full picture sum saved + estimated_saved.
            "estimated_saved": estimated_saved,
            // The same reconciliation for the CALL count. Without it a
            // consumer summing `by_op[].count` lands past `operations` with
            // no field that closes the gap — the prose line says 740, and a
            // machine reader cannot read prose.
            "estimated_calls": estimated_calls,
            // Of `estimated_saved`, the part whose baseline is the published
            // third-party ratio rather than a measured input. A consumer
            // quoting the total needs to know how much of it we made up.
            "estimated_from_borrowed_ratio": borrowed_ratio_saved,
            // Of `saved` — INSIDE it, unlike the two estimate fields above —
            // the part resting on a delivery-cap stand-in rather than exact
            // arithmetic. The signed receipt calls the same number
            // `tokens_saved_approximated`; that name is frozen by the
            // signature, so the two differ in spelling only.
            "approximated_saved": approximated_saved,
            // The same rows credited on the COARSER key the server's ingest
            // dedups on. `credit_key` is that key plus the label and the
            // session, so `saved` can sit above this; the gap is what a
            // receipt and an account will disagree by.
            "operations_as_metered": metered_count,
            "saved_as_metered": metered_before.saturating_sub(metered_after),
            // Honest, prompt-cache-aware framing is the DEFAULT, not an
            // opt-in: net savings land on the *fresh* (uncacheable) fraction
            // of folded output; a cached re-read bills ~flat regardless of
            // size. Surfaced here so dashboards don't overstate the win.
            "net_savings_note":
                tokenade_core::cache_frame::footer(total_before.saturating_sub(total_after)),
            // What the totals above actually cover. A count with no time
            // basis is read as a rate that applies now, and these are every
            // row ever recorded — months of use, several versions, and a
            // rotation. `null` when no row carries a timestamp rather than a
            // guessed span.
            "span": if first_ts > 0 {
                serde_json::json!({
                    "first_ts": first_ts,
                    "last_ts": last_ts,
                    "first_iso": tokenade_core::platform::epoch_to_iso8601_z(first_ts),
                    "last_iso": tokenade_core::platform::epoch_to_iso8601_z(last_ts),
                    "days": (last_ts.saturating_sub(first_ts) / 86_400) + 1,
                })
            } else {
                serde_json::Value::Null
            },
            "by_op": per_op_arr,
            "by_source": per_source_arr,
        });
        println!("{}", serde_json::to_string_pretty(&root).unwrap());
        return 0;
    }
    if count == 0 {
        // A zero has at least two causes and they call for opposite
        // actions: nothing is wired for this agent (use `wrap`), or
        // everything is wired and simply nothing has run yet (do nothing).
        // The line used to give the `wrap` advice unconditionally, which
        // reads as "not installed" to a user whose hooks are working —
        // the exact confusion this empty state should be resolving.
        use tokenade_core::detect::Coverage;
        let agent = tokenade_core::detect::detect(&root());
        match agent.coverage() {
            // An unrecognised agent is NOT an uncovered one: telling the user
            // "nothing covers unknown" names no agent and suggests no fix.
            Coverage::AgentUnknown => println!(
                "no savings recorded yet — no agent recognised here, so coverage \
                 cannot be reported; `tokenade wrap '<cmd>'` compacts a command by hand"
            ),
            Coverage::Uncovered => println!(
                "no savings recorded yet — nothing automatic covers {}; \
                 run commands via `tokenade wrap '<cmd>'`",
                agent.as_str()
            ),
            // Explicit, because the catch-all below says "is wired" — which
            // for this state is false. It is WIRABLE, and the difference is
            // the one command that closes it.
            Coverage::McpWrappable => println!(
                "no savings recorded yet — {} has no hook or plugin, but its MCP \
                 servers can be wrapped: run `tokenade mcp-wrap install`",
                agent.as_str()
            ),
            // Explicit for the same reason as `McpWrappable` above: the
            // catch-all says "is wired", and for this state that is only HALF
            // true. The shell channel is covered and the agent's own file
            // reads are not, so a zero here has two possible causes and the
            // user is told both instead of being reassured.
            Coverage::ShellProxy => println!(
                "no savings recorded yet — {} has no hook or plugin; its shell commands \
                 are compacted by the universal shell proxy, but anything it reads \
                 WITHOUT a shell is not covered",
                agent.as_str()
            ),
            c => println!(
                "no savings recorded yet — {} is wired ({}), so this just means \
                 nothing has run through it yet",
                agent.as_str(),
                c.as_str()
            ),
        }
        return 0;
    }
    let overall = Savings::new(total_before as usize, total_after as usize);
    // Name the window in the header. A reader who sees 63 % where they saw
    // 88 % yesterday must be able to tell a regression from a different
    // question, and the number alone cannot say which it is.
    if recent {
        println!("─── tokenade gain (last 7 days) ───");
    } else if first_ts > 0 {
        // The `--recent` branch has named its window since it existed, for a
        // reason written right here: "a reader who sees 63 % where they saw
        // 88 % yesterday must be able to tell a regression from a different
        // question, and the number alone cannot say which it is". The default
        // branch is the one with the widest window and it named nothing, so a
        // lifetime average across months read as today's rate.
        let days = (last_ts.saturating_sub(first_ts) / 86_400) + 1;
        let since = tokenade_core::platform::epoch_to_iso8601_z(first_ts);
        let since = since.split('T').next().unwrap_or(&since).to_string();
        println!(
            "─── tokenade gain (everything recorded: {days} day{}, since {since}) ───",
            if days == 1 { "" } else { "s" }
        );
    } else {
        println!("─── tokenade gain ───");
    }
    println!("operations: {count}");
    println!("tokens: {total_before} → {total_after}");
    println!(
        "saved:   {} ({:.1}%)",
        total_before.saturating_sub(total_after),
        overall.percent()
    );
    // The server's ingest dedups more coarsely than this scan does, so the
    // figure above can sit over what the account meters. Printed only when the
    // two actually differ: a caveat that is always there stops being read.
    let metered_saved = metered_before.saturating_sub(metered_after);
    let local_saved = total_before.saturating_sub(total_after);
    if local_saved > metered_saved {
        println!(
            "metered: {metered_saved} ({metered_count} events) is what the server's ingest \
             counts — it dedups more coarsely, so `saved:` above can exceed it"
        );
    }
    // `saved:` is a measured total, but not all of it is exact arithmetic:
    // where the agent would have REFUSED an oversized payload rather than
    // truncating it, crediting the delivery cap is a deliberate stand-in. That
    // slice is kept IN the total by decision — and it was kept there in
    // silence. Only the signed receipt disclosed it, while these very lines
    // disclosed two SMALLER estimate categories underneath. Labelling is not
    // subtracting: the number does not move, the reader is simply told what
    // part of it stands on a stand-in.
    if approximated_saved > 0 {
        let share = if total_before > total_after {
            100.0 * approximated_saved as f64 / total_before.saturating_sub(total_after) as f64
        } else {
            0.0
        };
        println!(
            "of which:{approximated_saved} ({share:.1}%) credits a delivery cap rather than exact \
             arithmetic (the agent would have refused the payload, not truncated it)"
        );
    }
    // Code-intel ops (`skeleton`, `query`, `symbol_find`, `map`, …) are held
    // out of the counters above on purpose: their saving is not measured, it
    // is inferred from a baseline, so mixing it into a measured total would
    // launder an estimate as a fact.
    //
    // But until 2026-08-04 this number was computed here and then DROPPED —
    // only `--json` ever showed it. Two things were wrong with that. It
    // under-reported the product's own work (4.6 M tokens on this install),
    // the same defect as the `evict-stale` total fixed the same day. And it
    // made this very output self-contradictory: `per_op` below is filled for
    // every row, so the reader sees `skeleton 331×` in the by-op table while
    // `operations:` above counts none of them. One line explains both.
    if estimated_saved > 0 {
        // Two confidences, and they were being printed as one. `skeleton`
        // tokenises the file it was handed, so only the ALTERNATIVE is assumed;
        // `symbol_find` and friends have no input to measure and their baseline
        // is a published third-party ratio. Both are excluded from `saved:`
        // above, and only one of them is a number we made up.
        let measured_input = estimated_saved.saturating_sub(borrowed_ratio_saved);
        if measured_input > 0 {
            println!(
                "est.:    {measured_input} more where the input WAS measured but the \
                 alternative is assumed (skeleton/query/map — you would have read the \
                 file whole)"
            );
        }
        if borrowed_ratio_saved > 0 {
            println!(
                "         {borrowed_ratio_saved} more from a published third-party ratio, \
                 not our own measurement (symbol_find/impact_of/…)"
            );
        }
        // Name BOTH totals they are missing from. The comment above says
        // "one line explains both" and this line explained one: a reader
        // summing the `N×` column below lands past `operations:` too, and
        // until now nothing said why. Same doctrine as the two-stage fold
        // banner — a number is honest when a reader who does arithmetic on it
        // is not misled.
        if estimated_calls > 0 {
            println!(
                "         both excluded from `saved:` and from `operations:` above \
                 ({estimated_calls} call(s)) — the `by op:` rows below DO include them, \
                 so that column sums past both"
            );
        } else {
            println!("         both excluded from `saved:` above");
        }
    }
    // "The best options by default": the honest prompt-cache-aware
    // caveat is shown unconditionally, not gated behind `cache-note`. Net
    // savings accrue on the fresh, uncacheable fraction of folded output.
    println!(
        "note:    {}",
        tokenade_core::cache_frame::footer(total_before.saturating_sub(total_after))
    );
    println!("by op:");
    // #785 — pre-fix the format was hardcoded `{:<22}` which let names
    // like `auto:pkginstall+rejected` (24 chars) overflow → count column
    // pushed right, breaking visual alignment. Compute the width from
    // the actual rows; floor at 22 to preserve the previous default look.
    let op_w = per_op
        .keys()
        .map(|f| f.chars().count())
        .max()
        .unwrap_or(22)
        .max(22);
    for (f, (b, a, n)) in per_op {
        println!(
            "  {:<op_w$} {:>5}× −{:.0}%{}",
            f,
            n,
            Savings::new(b as usize, a as usize).percent(),
            heaviest_day_note(per_op_day.get(&f), b, n)
        );
    }
    if by_source {
        println!("by source:");
        for (src, (b, a, n)) in per_source {
            println!(
                "  {:<10} {:>5}× −{:.0}%",
                src,
                n,
                Savings::new(b as usize, a as usize).percent()
            );
        }
    }
    0
}
