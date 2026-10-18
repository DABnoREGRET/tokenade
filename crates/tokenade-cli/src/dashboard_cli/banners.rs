//! **The banners: say why a number is what it is.**
//!
//! A number without its frame is misread. Is coverage partial? Is the
//! accounting estimated rather than measured? These banners answer before
//! someone draws a conclusion from a total.

use super::*;

pub(crate) fn cmd_dashboard_text(window_days: u32) -> i32 {
    use std::io::IsTerminal;
    use tokenade_core::dashboard::{render, Theme};
    let (data, window_cutoff) = collect_dashboard_data(window_days);

    // Colour only on real TTYs and when NO_COLOR is unset (de-facto standard
    // since 2018 — see https://no-color.org). Piping to a file or `less`
    // gets a clean mono render.
    let color = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TOKENADE_NO_COLOR").is_none();
    let theme = Theme {
        color,
        width: term_width().unwrap_or(80),
    };

    print_coverage_banner(color, &data);

    print!("{}", render(&data, &theme));
    // #window — feed the same window + the windowed measured saved so the
    // accounting block matches the hero instead of showing lifetime totals.
    print_estimated_accounting(
        window_cutoff,
        data.tokens_before.saturating_sub(data.tokens_after),
    );
    0
}

/// Why is this dashboard empty?
///
/// An agent no channel
/// covers, where the numbers are empty because nothing is wired, not because
/// something failed. Both read identically on screen, and the second is the
/// one a user reports as "tokenade doesn't work".
///
/// Silent whenever the window shows any measured saving — a user who IS
/// saving does not need to be told their agent is unusual, and a banner that
/// fires on a working install is noise that trains people to skip banners.
pub(super) fn print_coverage_banner(color: bool, data: &tokenade_core::dashboard::DashboardData) {
    if data.tokens_before > 0 {
        return;
    }
    let agent = tokenade_core::detect::detect(&root());
    // Two states share this banner because both mean "your zero is not a
    // failure". They must NOT share its text: `Uncovered` has no channel to
    // offer, `McpWrappable` has one that a single command wires. Telling an
    // MCP-only user to compact by hand, when `tokenade mcp-wrap install`
    // would wire a whole channel, is a channel that exists and is not offered.
    let coverage = agent.coverage();
    use tokenade_core::detect::Coverage;
    if !matches!(coverage, Coverage::Uncovered | Coverage::McpWrappable) {
        return;
    }
    let (dim, reset) = if color {
        ("\x1b[2m", "\x1b[0m")
    } else {
        ("", "")
    };
    // Same language as the gate banner two lines above: they sit side by
    // side on the same screen and answer the same question.
    let fr = user_lang_is_fr();
    let body = match (coverage, fr) {
        (Coverage::McpWrappable, true) => format!(
            "  ni hook ni plugin pour {} — mais ses serveurs MCP peuvent passer par le \
             proxy compactant.\n  Lancez `tokenade mcp-wrap install` pour câbler ce \
             canal, ou `tokenade detect` pour le détail.",
            agent.as_str()
        ),
        (Coverage::McpWrappable, false) => format!(
            "  no hook and no plugin for {} — but its MCP servers can be routed through \
             the compacting proxy.\n  Run `tokenade mcp-wrap install` to wire that \
             channel, or `tokenade detect` for the detail.",
            agent.as_str()
        ),
        (_, true) => format!(
            "  aucun canal automatique ne couvre {} — ni hook ni plugin pour cet agent.\n  \
             Compactez une commande à la main avec `tokenade wrap '<cmd>'`, ou lancez \
             `tokenade detect` pour le détail.",
            agent.as_str()
        ),
        (_, false) => format!(
            "  nothing automatic covers {} — no hook and no plugin ship for it.\n  \
             Compact a command by hand with `tokenade wrap '<cmd>'`, or run \
             `tokenade detect` for the detail.",
            agent.as_str()
        ),
    };
    println!("{dim}{body}{reset}");
}

/// 0.6.4 — print the savings-accounting block beneath the dashboard:
/// MEASURED (exact compaction + dedup) plus the ESTIMATED buckets
/// (output-lean, batching, compounding) minus the scaffold OVERHEAD, and the
/// net. The estimated buckets are bench-calibrated approximations, shown on
/// their own lines so the measured hero above stays honest. Silent when
/// nothing estimated has accrued yet.
pub(crate) fn print_estimated_accounting(window_cutoff: u64, windowed_measured: u64) {
    let est = if window_cutoff > 0 {
        tokenade_core::gain::estimated_summary_windowed(window_cutoff)
    } else {
        tokenade_core::gain::estimated_summary()
    };
    // #15 — output-lean estimated for OTHER agents too (from their readable
    // token logs); Claude Code is excluded here (credited live in `est`).
    let cross: Vec<(&'static str, u64)> = tokenade_core::platform::home_dir()
        .map(|h| tokenade_core::agent_transcripts::estimated_output_lean_by_agent(&h))
        .unwrap_or_default();
    let cross_total: u64 = cross.iter().map(|(_, s)| *s).sum();
    if est.saved() == 0 && est.overhead == 0 && cross_total == 0 {
        return;
    }
    // #window — in windowed mode the measured figure is the windowed `saved`
    // computed by the caller (so this block matches the hero); lifetime mode
    // keeps the cumulative ledger total. The realistic harness-cap variant is
    // only computed for lifetime (no windowed realistic accumulator yet) — we
    // suppress the realistic sub-line under a window rather than show a
    // lifetime figure that contradicts the windowed measured.
    let measured = if window_cutoff > 0 {
        windowed_measured
    } else {
        tokenade_core::gain::lifetime_saved().unwrap_or(0)
    };
    // Realistic variant: huge-file compactions are bounded by what the harness
    // would actually have delivered (it truncates a 1.2 MB Read/Bash output),
    // so `raw − compacted` overstates them. `lifetime_saved_realistic` caps the
    // direct part and credits only a fraction of the truncated overflow.
    let measured_realistic = if window_cutoff > 0 {
        measured
    } else {
        tokenade_core::gain::lifetime_saved_realistic().unwrap_or(measured)
    };
    let net = tokenade_core::gain::net_tokens(measured, cross_total, &est);
    let net_realistic = tokenade_core::gain::net_tokens(measured_realistic, cross_total, &est);
    // group thousands with a thin separator for readability (1,234,567).
    let k = |n: u64| {
        let s = n.to_string();
        let b = s.as_bytes();
        let mut out = String::new();
        for (i, c) in b.iter().enumerate() {
            if i > 0 && (b.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(*c as char);
        }
        out
    };
    println!();
    let scope = if window_cutoff > 0 {
        // Derive the day count back from the cutoff for the label.
        let now = tokenade_core::platform::now_unix_secs();
        let days = now.saturating_sub(window_cutoff) / 86_400;
        format!(" — last {days} days")
    } else {
        " — lifetime".to_string()
    };
    println!("  ── Savings accounting (tokens){scope} ──");
    println!("    measured (compaction + dedup)   {:>14}", k(measured));
    if measured_realistic != measured {
        println!(
            "      └ realistic (harness-capped)  {:>14}",
            k(measured_realistic)
        );
    }
    println!("    + est. output-lean (Claude)     {:>14}", k(est.output));
    println!(
        "    + est. batching                 {:>14}",
        k(est.batching)
    );
    println!(
        "    + est. compounding (cache_read) {:>14}",
        k(est.compounding)
    );
    if cross_total > 0 {
        println!(
            "    + est. output-lean (other agents) {:>12}",
            k(cross_total)
        );
    }
    println!(
        "    − injected overhead             {:>14}",
        k(est.overhead)
    );
    if est.overhead_cache > 0 {
        println!(
            "    − overhead re-billed (cache)    {:>14}",
            k(est.overhead_cache)
        );
    }
    println!("    ══════════════════════════════════════════════");
    println!("    = net                           {:>14}", k(net));
    if net_realistic != net {
        println!(
            "    = net (realistic)               {:>14}",
            k(net_realistic)
        );
    }
    // Headline unit: plain "tokens", weighted at real price ratios so the
    // number means what it costs (user decision 2026-07-04). Legend is
    // mandatory on every surface that shows it.
    let weighted = tokenade_core::gain::weighted_from_parts(measured_realistic, cross_total, &est);
    println!("    = tokens                        {:>14}", k(weighted));
    println!("      [2m{}[0m", tokenade_core::gain::WEIGHTED_LEGEND_EN);
    // Direction split + $ — output tokens cost ~5x input, so a correct $
    // figure requires pricing the directions separately — the buckets the
    // reporter uploads, so this matches the server's costing.
    let (in_saved, cache_saved, out_saved) = est.by_io(measured_realistic, cross_total);
    let (p_in, p_cache, p_out) = (
        tokenade_core::coefficients::price_input_per_mtok(),
        tokenade_core::coefficients::price_cache_per_mtok(),
        tokenade_core::coefficients::price_output_per_mtok(),
    );
    let usd = in_saved as f64 / 1e6 * p_in
        + cache_saved as f64 / 1e6 * p_cache
        + out_saved as f64 / 1e6 * p_out;
    println!("    input saved                     {:>14}", k(in_saved));
    let cache_shown = if cache_saved < 0 {
        format!("−{}", k(cache_saved.unsigned_abs()))
    } else {
        k(cache_saved as u64)
    };
    println!("    cache-read saved (re-reads)     {:>14}", cache_shown);
    println!("    output saved                    {:>14}", k(out_saved));
    println!(
        "    ≈ ${usd:.2} API-cost equivalent (in ${p_in} · cache ${p_cache} · out ${p_out} /MTok)"
    );
    if cross_total > 0 {
        let parts: Vec<String> = cross
            .iter()
            .map(|(a, s)| format!("{a} {}", k(*s)))
            .collect();
        println!("      other-agent output-lean: {}", parts.join(" · "));
    }
    println!(
        "    (est. buckets are bench-calibrated approximations; `tokenade calibrate` refreshes them)"
    );
    println!(
        "    note: output-lean is estimated for any agent whose token logs are readable; batching &\n          compounding stay Claude Code only (need per-turn attribution). Agents with no readable\n          logs show MEASURED savings (compaction + dedup via the shell proxy) only — not estimated."
    );
}
