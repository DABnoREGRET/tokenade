//! **What happens outside tokenade.**
//!
//! The usage reported by the agent, the functions it performs without going
//! through us, the wrapped MCP servers. These numbers come from ANOTHER
//! source than the ledger; mixing them with ours would give a total that nothing
//! supports.

use super::*;

pub(super) fn render_claude_usage(s: &mut String, d: &DashboardData, theme: &Theme, p: &Pal<'_>) {
    if d.claude_usage.is_empty() {
        return;
    }
    // Breakdown line is ~40 cols of leading indent + up to ~75 cols of
    // tool chunks ("tokenade 99 · Read 9999 · Bash 9999 · ..."). On
    // sub-100-col terminals it wraps and looks like garbage. Hide it
    // there — the main row still shows the total + session count.
    let show_breakdown = theme.width >= 100;
    // #682 — section retitled per user request ("outside of tokenade
    // - Projects") + verbose explanatory text removed (#672/#678
    // framing now lives in CHANGELOG; the new title is
    // self-documenting). Displayed value = bypass (tool_result_tokens
    // - tokens that went through tokenade compactors), not the
    // cumulative API total — matches the "outside" framing.
    render_section_heading(s, "AGENT TOKENS (outside of tokenade) - Projects", p);
    // #682 — precompute bypass (= tool_result_tokens that didn't go
    // through tokenade compactors) per row. THIS is what the section
    // displays now — "tokens outside tokenade" is the value, not the
    // cumulative API total. Rows with zero bypass are skipped (nothing
    // to report).
    let rows_with_bypass: Vec<(&ClaudeUsageRow, u64, u64)> = d
        .claude_usage
        .iter()
        .map(|row| {
            let tokenade_after: u64 = d
                .by_project
                .iter()
                .find(|(path, _, _, _)| path == &row.cwd)
                .map(|(_, _, after, _)| *after)
                .unwrap_or(0);
            let bypass = row.tool_result_tokens.saturating_sub(tokenade_after);
            (row, tokenade_after, bypass)
        })
        .filter(|(_, _, bypass)| *bypass > 0)
        .collect();
    let max_bypass = rows_with_bypass
        .iter()
        .map(|(_, _, b)| *b)
        .max()
        .unwrap_or(1)
        .max(1);
    // #760 — `_tokenade_after` is the per-project compaction total
    // computed in the closure above. It was consumed by the "↳ of X
    // compactable, Y through tokenade" sub-line until #749 removed it
    // as misleading. The triplet shape is kept (rather than re-mapping
    // to a pair) because (a) the closure already returns the triple,
    // and (b) a future per-row metric (e.g. coverage colour-coding)
    // would want this field back without rewriting the iter chain.
    for (row, _tokenade_after, bypass) in rows_with_bypass.iter().take(6) {
        let bar_w = ((24.0 * (*bypass) as f64) / max_bypass as f64).round() as usize;
        let bar_w = bar_w.clamp(1, 24);
        let bar = format!(
            "{}{}{}{}{}",
            p.yellow(), // yellow = "this is the gap, not the wins"
            "█".repeat(bar_w),
            p.fade(),
            "░".repeat(24 - bar_w),
            p.rst()
        );
        let agent_tag = if row.agent_label.is_empty() {
            String::new()
        } else {
            format!("[{}]", row.agent_label)
        };
        let display = truncate_tail(&row.display_cwd, 28);
        let _ = writeln!(
            s,
            "   {}{:<8}{} {}{:<28}{}  {}  {}{:>4} sess{}  {}{:>8}{} bypass",
            p.yellow(),
            agent_tag,
            p.rst(),
            p.cyan(),
            display,
            p.rst(),
            bar,
            p.dim(),
            row.sessions,
            p.rst(),
            p.yellow(),
            human_tokens(*bypass),
            p.rst(),
        );
        // #749 — user-removed: the "↳ of X compactable, Y through
        // tokenade (Z%)" sub-line was confusing AND inconsistent.
        // Two issues:
        //   1. Cognitive — saying "X compactable, Y compacted" begs
        //      the question "why not more?" which this surface
        //      doesn't (and shouldn't) answer.
        //   2. Numeric — `tokenade_after` here was pulled from
        //      `by_project` (gain.jsonl rows keyed by cwd), while
        //      the WRAPPED MCPs section computes its own per-project
        //      totals from a different code path. The two numbers
        //      disagreed for the same project (user reported 1.6k
        //      here vs 32k elsewhere), which made the ratio claim
        //      unreliable. Real-numbers reconciliation should happen
        //      in the underlying ledger, not in display prose.
        // The headline bypass number above is the only thing this
        // section claims to measure; per-tool breakdown follows.
        // Per-row tool breakdown: shown only if at least one
        // bypass-eligible call was logged AND the terminal is wide
        // enough to fit it without wrapping. Highlights the buckets
        // that have a Tokenade alternative; muted tokenade count
        // last so green ≈ good, dim labels ≈ where you can save more.
        if show_breakdown && (row.bypass_eligible() > 0 || row.tokenade_calls > 0) {
            let pairs: Vec<(&str, u64, &str)> = vec![
                ("tokenade", row.tokenade_calls, "→ keep going"),
                ("Read", row.read_calls, "→ skeleton"),
                ("Bash", row.bash_calls, "→ exec_compact"),
                ("Grep", row.grep_calls, "→ semantic_search"),
                ("Glob", row.glob_calls, "→ structure_map"),
                ("Web*", row.web_calls, "→ web_html_to_markdown"),
            ];
            let mut chunks = Vec::new();
            for (label, n, _hint) in pairs.iter().filter(|(_, n, _)| *n > 0) {
                let colour = if *label == "tokenade" {
                    p.green()
                } else {
                    p.yellow()
                };
                chunks.push(format!("{}{} {}{}", colour, label, n, p.rst()));
            }
            let sep = format!("{} · ", p.dim());
            let _ = writeln!(
                s,
                "   {:<39}{}↳ {}{}",
                "",
                p.dim(),
                chunks.join(&sep),
                p.rst()
            );
            // Suggestion line: pick the heaviest bypass-eligible bucket
            // and surface its tokenade equivalent. Only when there are
            // 5+ such calls — below that the noise/signal isn't worth
            // a recommendation line.
            let heaviest = pairs
                .iter()
                .filter(|(label, _, _)| *label != "tokenade")
                .max_by_key(|(_, n, _)| *n);
            if let Some((label, n, hint)) = heaviest {
                if *n >= 5 {
                    let _ = writeln!(
                        s,
                        "   {:<39}{}{}× {} {} {}",
                        "",
                        p.dim(),
                        n,
                        label,
                        hint,
                        p.rst()
                    );
                }
            }
        }
    }
    // Agents whose logs are partial or absent: say so explicitly,
    // otherwise their absence from the rows above reads as "zero
    // usage" when it actually means "not measurable".
    for (agents, en, fr) in crate::agent_transcripts::coverage_notes() {
        let note = if d.lang_fr { fr } else { en };
        let _ = writeln!(s, "   {}· {agents}: {note}{}", p.dim(), p.rst());
    }
    let _ = writeln!(s);
}

/// #683 — companion to render_claude_usage. Where the Projects
/// section answers "which CWD spent tokens outside tokenade?",
/// this one answers "which tool?". `mcp__tokenade__*` calls are
/// filtered out (those IS tokenade, not bypass). Top N shown with
/// yellow gap-bars matching the Projects section's visual idiom.
pub(super) fn render_outside_tokenade_functions(
    s: &mut String,
    d: &DashboardData,
    _theme: &Theme,
    p: &Pal<'_>,
) {
    if d.bypass_by_tool.is_empty() {
        return;
    }
    // #684 — for each tool, subtract the post-compaction tokens that
    // gain.jsonl attributes to that tool's route. This is what makes
    // the "outside of tokenade" framing honest: Bash and Read go
    // through tokenade hooks for most invocations, so their raw
    // tool_result_tokens overstates true bypass. The CLI did the
    // attribution; here we apply it.
    let filtered: Vec<(String, u64)> = d
        .bypass_by_tool
        .iter()
        // #685 — match the full set of namespaces that `ToolBreakdown::bump`
        // classifies as tokenade: the direct install (`mcp__tokenade__*`),
        // the Claude Code plugin install (`mcp__plugin_tokenade_tokenade__*`),
        // and the legacy pre-rename installs (`mcp__supertokensaver__*` /
        // `mcp__sts__*`). Calls to ANY of these route through tokenade and
        // must not appear in the "outside" section. Before this fix, plugin
        // users saw their entire tokenade traffic mis-attributed as bypass.
        .filter(|(name, _)| {
            !(name.starts_with("mcp__tokenade__")
                || name.starts_with("mcp__plugin_tokenade_tokenade__")
                || name.starts_with("mcp__supertokensaver__")
                || name.starts_with("mcp__sts__")
                // #742 — drop the "unknown" bucket from display. The
                // claudecode_transcript parser assigns this label when
                // a tool_result block has a tool_use_id that doesn't
                // match any prior tool_use in the same session file
                // (orphan — partial-write truncation or a system-
                // injected result). The bytes are still in the
                // aggregate tool_result_tokens (so per-project bypass
                // is unchanged), but "unknown" in the per-tool
                // breakdown is noise — we can't action it, and it
                // misleadingly suggests a real tool we failed to
                // identify. Common cause: 1 orphan in a long-running
                // session file is normal.
                || *name == "unknown")
        })
        .map(|(name, total)| {
            let processed = d.tokenade_after_by_tool.get(name).copied().unwrap_or(0);
            let bypass = total.saturating_sub(processed);
            (name.clone(), bypass)
        })
        .filter(|(_, b)| *b > 0)
        .collect();
    if filtered.is_empty() {
        return;
    }
    render_section_heading(s, "AGENT TOKENS (outside of tokenade) - Functions", p);
    let max_tok = filtered.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    for (name, tok) in filtered.iter().take(10) {
        let bar_w = ((24.0 * (*tok) as f64) / max_tok as f64).round() as usize;
        let bar_w = bar_w.clamp(1, 24);
        let bar = format!(
            "{}{}{}{}{}",
            p.yellow(), // yellow = gap, matches Projects section
            "█".repeat(bar_w),
            p.fade(),
            "░".repeat(24 - bar_w),
            p.rst()
        );
        let display = truncate_tail(name, 38);
        let _ = writeln!(
            s,
            "   {}{:<38}{}  {}  {}{:>8}{} bypass",
            p.cyan(),
            display,
            p.rst(),
            bar,
            p.yellow(),
            human_tokens(*tok),
            p.rst(),
        );
    }
    let _ = writeln!(s);
}

pub(super) fn render_code_intel(s: &mut String, d: &DashboardData, _theme: &Theme, p: &Pal<'_>) {
    if d.code_intel.is_empty() {
        return;
    }
    render_section_heading(s, "CODE INTELLIGENCE (heuristic estimate)", p);
    if let Some(note) = &d.code_intel_disclaimer {
        let _ = writeln!(s, "   {}{}{}", p.dim(), note, p.rst());
    }
    for (op, calls, est_saved) in d.code_intel.iter() {
        let _ = writeln!(
            s,
            "   {}{:<24}{}  {}{:>4}×{}   est. saved  {}{:>7}{}",
            p.cyan(),
            op.chars().take(24).collect::<String>(),
            p.rst(),
            p.dim(),
            calls,
            p.rst(),
            p.green(),
            human_tokens(*est_saved),
            p.rst(),
        );
    }
    let _ = writeln!(s);
}

pub(super) fn render_cache_and_index(
    s: &mut String,
    d: &DashboardData,
    _theme: &Theme,
    p: &Pal<'_>,
) {
    let mb = d.cache_bytes_served as f64 / (1024.0 * 1024.0);
    let total = d.cache_hits + d.cache_misses;
    let rate = if total > 0 {
        100.0 * d.cache_hits as f64 / total as f64
    } else {
        0.0
    };
    if d.cache_hits + d.cache_misses + d.semantic_docs > 0 {
        render_section_heading(s, "INDEX & CACHE", p);
        if total > 0 {
            let _ = writeln!(
                s,
                "   {}cache         {} hits   {} misses   {}{:.1}%{} hit-rate   {}{:.1} MB{} served",
                p.label(),
                d.cache_hits,
                d.cache_misses,
                p.green(),
                rate,
                p.rst(),
                p.cyan(),
                mb,
                p.rst()
            );
        }
        if d.semantic_docs > 0 {
            let _ = writeln!(
                s,
                "   {}semantic     {} doc(s)   {} term(s) indexed",
                p.label(),
                d.semantic_docs,
                d.semantic_terms,
            );
        }
        let _ = writeln!(s);
    }
}

pub(super) fn render_footer(s: &mut String, d: &DashboardData, p: &Pal<'_>) {
    let line = "─".repeat(72);
    let _ = writeln!(s, "  {}{}{}", p.gray(), line, p.rst());
    let _ = writeln!(
        s,
        "  {}tokenade · {}tokenade.net{}   ·   ledger: {}",
        p.dim(),
        p.cyan(),
        p.rst(),
        d.ledger_path.chars().take(40).collect::<String>(),
    );
    // Honest prompt-cache-aware framing, shown by default (not opt-in):
    // net savings land on the fresh, uncacheable fraction of folded output.
    let saved = d.tokens_before.saturating_sub(d.tokens_after);
    let _ = writeln!(
        s,
        "  {}{}{}",
        p.dim(),
        crate::cache_frame::footer(saved),
        p.rst(),
    );
    let _ = writeln!(s);
}
