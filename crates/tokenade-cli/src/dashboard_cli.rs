//! Dashboard / gain / statusline CLI commands (`tokenade dashboard|gain|
//! statusline` + webdash payload). Extracted from main.rs (refactor).
#![allow(unused_imports)]
mod statusline;
mod webdash_payload;
use crate::wrap::*;
use crate::*;
pub(crate) use statusline::*;
pub(crate) use webdash_payload::*;
mod banners;
mod collect;
mod gain;
pub(crate) use banners::*;
pub(crate) use collect::*;
pub(crate) use gain::*;

pub(crate) fn cmd_dashboard(rest: &[String]) -> i32 {
    // Silent-accept sweep (#813/#814 follow-up): reject unknown flags +
    // extra positionals. `--html` is special — it takes an optional
    // value (the next non-flag arg), so consume it as a value-flag
    // only when the next arg doesn't start with `-`.
    const DASH_BOOL_FLAGS: &[&str] = &["--json", "--no-open", "--text"];
    {
        let mut i = 0;
        while i < rest.len() {
            let a = &rest[i];
            if a == "--html" || a == "--watch" || a == "--days" {
                // Optional value: `--html <path>` / `--watch <secs>`. Skip the
                // value if the next arg doesn't look like a flag, else fall
                // through (bare `--html` / `--watch` are both valid).
                if rest.get(i + 1).is_some_and(|n| !n.starts_with('-')) {
                    i += 2;
                    continue;
                }
            } else if a.starts_with("--") || (a.starts_with('-') && a.len() > 1 && a != "-") {
                if !DASH_BOOL_FLAGS.contains(&a.as_str()) {
                    eprintln!(
                        "tokenade dashboard: unknown flag '{a}'. Try `tokenade dashboard --help`."
                    );
                    return 2;
                }
            } else {
                eprintln!(
                    "tokenade dashboard: unexpected positional '{a}' (this command takes no positionals; use `--html <path>` for an explicit HTML target)."
                );
                return 2;
            }
            i += 1;
        }
    }
    // The two surfaces that are LIFETIME by design, and refuse to pretend
    // otherwise. `--days` reaches neither: `cmd_dashboard_json` takes no window
    // at all and `cmd_dashboard_web` takes none either, so both accepted the
    // flag and answered with the whole history. Measured 2026-09-04 on a ledger
    // holding three rows 60 days old: the TEXT dashboard answered 65.7k over 5
    // ops at `--days 7` and 92.7k over 8 at `--days 90`, while `--json --days
    // 7` and `--days 90` returned the same 92.7k over 8 — the very divergence
    // the JSON renderer's own doc says it was wired to close.
    //
    // Refused rather than implemented, because the reason these two are
    // lifetime is written where the payload is built and it is a real one:
    // they read the ledger UNION its durable rollup, and rollup rows do not all
    // carry a `ts`, so filtering by date would quietly shrink the number this
    // surface exists to preserve. A flag that cannot mean anything here should
    // say so instead of being folded in (#813's rule, and #639's).
    const LIFETIME_ONLY_SURFACES: &[&str] = &["--json", "--html"];
    if rest.iter().any(|a| a == "--days") {
        if let Some(surface) = LIFETIME_ONLY_SURFACES
            .iter()
            .find(|f| rest.iter().any(|a| a == **f))
        {
            eprintln!(
                "tokenade dashboard: `--days` does not apply to `{surface}` — that surface is the \
                 LIFETIME view on purpose (it reads the ledger union its rollup, and rollup rows \
                 carry no date). Drop `--days`, or use the text dashboard, which windows and says \
                 which window it used."
            );
            return 2;
        }
    }
    // #window — time window for ALL displayed figures (terminal + JSON). The
    // ledger + agent-usage are filtered to the last N days so the headline
    // numbers aren't diluted by the whole install history (a beta-tester saw
    // a misleadingly-low "% saved" because it spanned months). `--days 0` =
    // lifetime (the historical default); default 30 matches the HTML range.
    let window_days: u32 = get_flag(rest, "--days")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(30);
    // Live mode (conflux-inspired, added 2026-06-09): `--watch [secs]`
    // re-renders the text dashboard on an interval, clearing the screen
    // each pass, so the user can leave it open and watch savings tick up
    // while an agent works. Default cadence 2s; clamped to [1, 3600].
    // Incompatible with --html/--json (those are one-shot exports).
    if rest.iter().any(|a| a == "--watch") {
        let secs: u64 = get_flag(rest, "--watch")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(2)
            .clamp(1, 3600);
        return cmd_dashboard_watch(secs, window_days);
    }
    // HTML mode emits a self-contained file the user can open in a browser.
    // #510 — `--html <path>` extracts the path arg if present, falls back
    // to the default `<root>/.tokenade/dashboard.html` otherwise. Previously
    // any `<path>` arg was silently ignored — including the form documented
    // by the #508 help text. Caught by smoke-testing the help text.
    if let Some(idx) = rest.iter().position(|a| a == "--html") {
        let explicit_path = rest
            .get(idx + 1)
            .filter(|p| !p.starts_with('-'))
            .map(std::path::PathBuf::from);
        // One-shot export: write the file, never auto-open (scripted /
        // CI usage; the default no-flag mode is the interactive path).
        return cmd_dashboard_web(explicit_path, false);
    }
    // #617 — JSON output for CI / scripting consumers. Same data the
    // HTML payload carries (operations, totals, per_op, cache stats),
    // surfaced through the gain ledger without the ASCII / HTML
    // rendering layer. Mirrors `gain --json` (#616) and
    // `healthcheck --json` (#613).
    if rest.iter().any(|a| a == "--json") {
        return cmd_dashboard_json();
    }
    // Default mode = the interactive web dashboard, and nothing else.
    //
    // It used to be the terminal report FIRST and the page after, so every
    // invocation rendered the same numbers twice and the browser tab landed
    // under a screenful of text nobody asked for. The terminal report is now
    // the FALLBACK, for the two cases where the page cannot be put in front of
    // the user: a session with no browser (headless / SSH), and a launcher
    // that refused. `--text` asks for it on purpose; `--watch` is a terminal
    // thing by nature and still is one.
    //
    // The page is a static local file — the stats deliberately never leave the
    // machine — so "launching" it is writing the file and asking the OS to
    // open it.
    if rest.iter().any(|a| a == "--text") {
        return cmd_dashboard_text(window_days);
    }
    // `--days` windows the TERMINAL report. The page carries its own period
    // selector and ignores it — so accepting it here would be a flag that
    // parses, prints nothing, and changes nothing. It is refused with the two
    // things that do work.
    if rest.iter().any(|a| a == "--days") {
        eprintln!(
            "tokenade dashboard: `--days` windows the terminal report, and the default run opens \
             the web dashboard — which has its own period selector (7d / 30d / 90d / All) driving \
             every panel. Use `tokenade dashboard --text --days N`, or pick the range on the page."
        );
        return 2;
    }
    // `--no-open` is the scripted path: write the file, print where, say
    // nothing else.
    if rest.iter().any(|a| a == "--no-open") {
        return cmd_dashboard_web(None, false);
    }
    if is_headless() {
        // On stdout, and still naming a "headless session": the e2e harness
        // asserts this line to prove no test run can spawn a browser that
        // outlives it.
        println!(
            "  (headless session — no browser to open; terminal report below. `tokenade dashboard --html [path]` exports the page anyway)\n"
        );
        return cmd_dashboard_text(window_days);
    }
    let code = cmd_dashboard_web(None, true);
    if code != 0 {
        return code;
    }
    if !LAST_OPEN_SUCCEEDED.load(std::sync::atomic::Ordering::Relaxed) {
        println!("  (couldn't hand the page to a browser — terminal report below)\n");
        return cmd_dashboard_text(window_days);
    }
    0
}

/// Whether the last [`cmd_dashboard_web`] actually got the page in front of
/// the user. `cmd_dashboard` falls back to the terminal report when it did
/// not: a printed `file://` URL on a machine whose launcher just failed is not
/// a dashboard, it is homework.
pub(crate) static LAST_OPEN_SUCCEEDED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Live `--watch` loop for the text dashboard (added 2026-06-09). Clears
/// the screen and re-renders every `secs` seconds so the user can leave it
/// open while an agent works and watch the savings figures move. Runs until
/// interrupted (Ctrl-C). Each pass re-reads the gain ledger, so it reflects
/// new writes from the hook / MCP without a restart.
pub(crate) fn cmd_dashboard_watch(secs: u64, window_days: u32) -> i32 {
    use std::io::Write;
    loop {
        // Clear scrollback + move cursor home (ANSI: 2J clears the screen,
        // 3J clears scrollback, H homes the cursor). Keeps the view pinned
        // to the top instead of letting it scroll endlessly.
        print!("\x1b[2J\x1b[3J\x1b[H");
        let _ = std::io::stdout().flush();
        let code = cmd_dashboard_text(window_days);
        if code != 0 {
            return code;
        }
        println!("\n  \x1b[2m↻ live · refreshing every {secs}s · Ctrl-C to exit\x1b[0m");
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}

/// #617 — JSON renderer for `tokenade dashboard --json`. Reads from
/// the same gain ledger + cache stats sources as the text/HTML
/// modes, emits a compact pretty-printed JSON object.
/// This surface is the LIFETIME view, and takes no window: it reads the ledger
/// UNION its durable rollup so the figure survives rotation, and rollup rows do
/// not all carry a `ts` (see the `scope` field, where that decision is argued).
/// `--days` is therefore REFUSED alongside `--json` rather than accepted and
/// dropped — this comment used to claim the opposite, and the code never took
/// the parameter it described.
///
/// MEASURED 2026-08-04: the two surfaces reported nets 2 381 849 tokens apart
/// (0,26 %) after their formula was shared, purely because one was windowed to
/// 30 days and the other was not. Neither number was wrong; nothing said which
/// was which.
pub(crate) fn cmd_dashboard_json() -> i32 {
    use tokenade_core::cache::{Cache, Stats};
    use tokenade_core::code_intel_heuristic::is_code_intel as is_ci;
    let r = root();
    let gain_path = global_gain_path();
    // #1042 — fold any pre-rollup `.bak` files in once, then read the
    // ledger UNION its durable rollup so the lifetime figure survives
    // rotation (reading gain.jsonl alone collapsed it to the 30-day
    // window). rollup rows carry `rollup_calls` for collapsed cardinality.
    tokenade_core::gain::migrate_legacy_backups_into_rollup();
    let content = tokenade_core::gain::ledger_with_rollup(&gain_path);
    // ONE aggregator, shared with every other surface. This block used to be
    // its own copy of the rules; see `gain::scan_ledger` for what that cost.
    let scan = tokenade_core::gain::scan_ledger(&content, 0);
    let total_before = scan.measured_before;
    let total_after = scan.measured_after;
    let estimated_saved = scan.code_intel_saved;
    let count = scan.calls;
    let per_op = scan.per_op.clone();
    let saved = total_before.saturating_sub(total_after);
    let pct = if total_before > 0 {
        100.0 * saved as f64 / total_before as f64
    } else {
        0.0
    };
    let cache = Cache::open(&r);
    let s: Stats = cache.stats();
    // Conflux-inspired BY LAYER rollup — same bucketing the text/HTML
    // dashboards render, exposed to JSON consumers. Measured ops only
    // (code-intel excluded, mirroring the text path), grouped via the
    // shared pure helper so all surfaces agree.
    let by_layer = {
        let measured: Vec<(String, u64, u64, u64)> = per_op
            .iter()
            .filter(|(op, _)| !is_ci(op))
            .map(|(op, (b, a, n))| (op.clone(), *b, *a, *n))
            .collect();
        tokenade_core::dashboard::per_layer_rollup(&measured)
    };
    let est6 = tokenade_core::gain::estimated_summary();
    let est6_cross: u64 = tokenade_core::platform::home_dir()
        .map(|h| {
            tokenade_core::agent_transcripts::estimated_output_lean_by_agent(&h)
                .iter()
                .map(|(_, s)| *s)
                .sum()
        })
        .unwrap_or(0);
    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": r.display().to_string(),
        "ledger_path": gain_path.display().to_string(),
        "operations": count,
        "tokens_before": total_before,
        "tokens_after": total_after,
        "tokens_saved": saved,
        "savings_pct": pct,
        // #735 — surface the code-intelligence heuristic figure
        // separately, matching how the text dashboard renders it as
        // "+ N estimated (code-intelligence heuristic)" below the
        // hero. Consumers that only want measured can use
        // tokens_saved; consumers that want the full picture sum
        // both. Previously the JSON conflated the two.
        "estimated_tokens_saved": estimated_saved,
        // The CALL count those estimates stand for, for the same reason and in
        // the same place. `per_op` below is built from every row, so its `n`
        // column sums past `operations` by exactly this much;
        // `estimated_tokens_saved` already closed the token half of that gap
        // and nothing closed the call half. Measured on this install: `n`
        // summed to 1359 under an `operations: 1226`.
        "estimated_calls": scan.code_intel.values().map(|v| v.0).sum::<u64>(),
        // Of `tokens_saved` — inside it — the part resting on a delivery-cap
        // stand-in. Same number the signed receipt publishes as
        // `tokens_saved_approximated`.
        "approximated_saved": scan.approximated_saved,
        // Secrets masked before the output reached a model (lifetime —
        // the JSON surface is unwindowed, like the totals above).
        "secrets_redacted": tokenade_core::redact::redactions_since(0),
        // Which RULE caught them. A count alone cannot tell a busy week from a
        // rule that started firing wrongly, and one of them did.
        "secrets_redacted_by_rule": tokenade_core::redact::redactions_by_rule(0),
        "cache_hits": s.hits,
        "cache_misses": s.misses,
        "cache_hit_rate": s.hit_rate(),
        "by_layer": by_layer.iter().map(|(layer,b,a,n)| serde_json::json!({
            "layer": layer,
            "before": b,
            "after": a,
            "n": n,
            "saved": b.saturating_sub(*a),
            "pct": if *b > 0 { 100.0 * (*b as f64 - *a as f64) / *b as f64 } else { 0.0 }
        })).collect::<Vec<_>>(),
        "per_op": per_op.iter().map(|(k,(b,a,n))| serde_json::json!({
            "op": k,
            "before": b,
            "after": a,
            "n": n,
            // #492-class guard: f64 cast BEFORE the subtraction so an
            // inflation case (a > b) reports as a negative percent
            // instead of a u64-underflow.
            "pct": if *b > 0 { 100.0 * (*b as f64 - *a as f64) / *b as f64 } else { 0.0 }
        })).collect::<Vec<_>>(),
        // 0.6.4 — approximate savings ledger (gain_estimated.jsonl) bucketed
        // by class, plus the net (measured + estimated − overhead). Separate
        // from the measured `tokens_saved` so consumers opt in explicitly.
        "estimated_breakdown": {
            "output_lean": est6.output,
            "batching": est6.batching,
            "compounding_cache_read": est6.compounding,
            "output_lean_other_agents": est6_cross,
            "scaffold_overhead": est6.overhead,
            "estimated_saved": est6.saved().saturating_add(est6_cross),
            // Was its OWN copy of the formula, and it omitted `overhead_cache`
            // — 51 million tokens of disagreement with the human view.
            "overhead_rebilled_cache": est6.overhead_cache,
            "net": tokenade_core::gain::net_tokens(saved, est6_cross, &est6),
            // The same net over the REALISTIC measured figure — each huge
            // output bounded by what the harness would have delivered, the
            // text block's `realistic` line. `net` above keeps the naive
            // `before − after` of `tokens_saved`, its own basis; measured
            // 2026-10-05 on this machine: 624.5 M naive, 492 M realistic.
            "net_realistic": tokenade_core::gain::net_tokens(
                tokenade_core::gain::lifetime_saved_realistic().unwrap_or(saved),
                est6_cross,
                &est6,
            ),
        },
        // Price-weighted headline unit ("tokens" on human surfaces): raw
        // counts aren't comparable across kinds. Legend text ships with the
        // number so downstream consumers can't display it unexplained.
        "weighted_tokens_saved": tokenade_core::gain::weighted_from_parts(saved, est6_cross, &est6),
        "weighted_legend": tokenade_core::gain::WEIGHTED_LEGEND_EN,
        // Which slice of history every figure above covers. Absent, a consumer
        // could not tell this apart from the text dashboard's windowed view.
        // This surface reads the ledger UNION its durable rollup, so its
        // figures are CUMULATIVE — the text dashboard's default is a 30-day
        // window. MEASURED 2026-08-04: the two nets differ by 2 381 849 tokens
        // (0,26 %) for that reason alone, and neither said which it was.
        //
        // Labelled, not silently windowed: rollup rows exist precisely so the
        // cumulative figure survives ledger rotation, and they do not all carry
        // a `ts` — filtering by date would quietly shrink the number this
        // surface is for. A consumer that wants the windowed view reads the
        // text dashboard, which states its own window.
        "scope": "lifetime",
    });
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    0
}
