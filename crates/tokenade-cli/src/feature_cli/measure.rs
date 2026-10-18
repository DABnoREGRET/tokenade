//! `tokenade measure` — audit of the files loaded when an agent starts.
#![allow(unused_imports)]
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;

/// `tokenade measure` — scan project for CLAUDE.md and other agent-loaded files,
/// estimate startup-token waste, and recommend a minimal layout
/// (nadimtuhin/claude-token-optimizer pattern).
///
/// Reads (no writes): `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursorrules`,
/// `.windsurfrules`, `.clinerules`, plus any `.md` files in `.claude/` that
/// look auto-loaded (anything not under `.claude/sessions/` or
/// `.claude/completions/`).
///
/// Output: per-file token estimate + total + suggested target (~800 tokens
/// per nadimtuhin's 4-file pattern) + the potential delta.
pub(crate) fn cmd_measure(rest: &[String]) -> i32 {
    // Silent-accept sweep (#813/#814 follow-up): reject unknown flags
    // + extra positionals (cmd_measure takes no positionals).
    if let Err(code) = validate_simple_bool_flags(rest, "measure", &["--json"]) {
        return code;
    }
    // #620 — `--json` mirrors #613/#616/#617/#619. Useful for CI that
    // wants to fail-fast when CLAUDE.md drifts above the per-session
    // token budget — the human text path doesn't give them anything
    // to grep cheaply.
    let json_mode = rest.iter().any(|a| a == "--json");
    let r = root();
    // The candidate files an agent typically auto-loads at session start.
    // Order matters only for display.
    let candidates: &[&str] = &[
        "CLAUDE.md",
        "AGENTS.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".github/copilot-instructions.md",
    ];

    // #975 — separate agent-specific scaffold files (each loaded by
    // exactly one agent: Claude→CLAUDE.md, Cursor→.cursorrules, etc.)
    // from `.claude/*.md` (Claude-only). Per-session cost = MAX of the
    // first group + SUM of the second, NOT the sum of everything. The
    // old SUM mis-reported a 4-agent scaffold (4×643 = 2572) as the
    // per-session cost when reality is 643 for any single agent.
    let mut hits: Vec<(String, usize)> = Vec::new();
    let mut agent_file_max: usize = 0;
    let mut claude_extras_sum: usize = 0;
    for rel in candidates {
        let p = r.join(rel);
        if let Ok(s) = fs::read_to_string(&p) {
            let t = tokenizer::estimate(&s);
            if t > agent_file_max {
                agent_file_max = t;
            }
            hits.push((rel.to_string(), t));
        }
    }

    // Auto-scan .claude/ for additional always-loaded markdown (excluding
    // session/completion archives which the agent treats as lazy).
    let claude_dir = r.join(".claude");
    if claude_dir.is_dir() {
        if let Ok(rd) = fs::read_dir(&claude_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Heuristic: any *.md at the top of .claude/ that isn't
                // settings.json is likely auto-loaded.
                if !name.ends_with(".md") {
                    continue;
                }
                if let Ok(s) = fs::read_to_string(&path) {
                    let t = tokenizer::estimate(&s);
                    claude_extras_sum += t;
                    let rel = format!(".claude/{name}");
                    hits.push((rel, t));
                }
            }
        }
    }
    // Worst-case per-session cost: any one agent loads its own file
    // (the largest is the worst case) plus, for Claude only, the
    // .claude/*.md extras. For non-Claude agents the `.claude/` sum
    // is zero in practice (the dir doesn't exist), so the formula
    // collapses to MAX agent-file token count — the right answer.
    let total_tokens: usize = agent_file_max + claude_extras_sum;

    let fw = framework::detect(&r);

    // #620 — JSON renderer. Computed BEFORE the human banner so we
    // never partially emit text + JSON mixed. Same data the text
    // path uses: per-file table, overall total, the 800-tok target,
    // the cost-estimate model, and `already_scaffolded` so CI can
    // distinguish "needs trimming" from "needs scaffold install".
    if json_mode {
        let target: u64 = 800;
        let total_u64 = total_tokens as u64;
        let waste = total_u64.saturating_sub(target);
        let waste_pct = if total_u64 > 0 {
            100.0 * waste as f64 / total_u64 as f64
        } else {
            0.0
        };
        use tokenade_core::pricing;
        let model = pricing::default_model();
        let monthly_usd = pricing::cost_for(model, waste * 50);
        let already_scaffolded = hits.iter().any(|(rel, _)| {
            let path = r.join(rel);
            std::fs::read_to_string(&path)
                .map(|s| s.contains(tokenade_core::project_scaffold::SCAFFOLD_MARKER))
                .unwrap_or(false)
        });
        let files: Vec<serde_json::Value> = hits
            .iter()
            .map(|(rel, t)| serde_json::json!({"path": rel, "tokens": t}))
            .collect();
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "root": r.display().to_string(),
            "framework": fw.as_str(),
            "files": files,
            // #975 — `total_tokens` is the worst-case PER-SESSION cost
            // (max agent file + .claude/*.md sum), NOT the sum across
            // every agent scaffold. The old SUM over-counted by 4×
            // when the standard 4-agent scaffold was installed.
            "total_tokens": total_tokens,
            "agent_file_max_tokens": agent_file_max,
            "claude_extras_sum_tokens": claude_extras_sum,
            "target_tokens": target,
            "waste_tokens": waste,
            "waste_pct": waste_pct,
            "monthly_usd_at_50_sessions": monthly_usd,
            "model_label": model.label,
            "over_budget": total_u64 > target,
            "already_scaffolded": already_scaffolded,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }

    println!("─── tokenade measure ───");
    println!("root:      {}", r.display());
    println!("framework: {}", fw.as_str());
    println!();

    if hits.is_empty() {
        println!("No agent-loaded files detected. Startup cost: 0 tokens.");
        println!();
        println!("Recommended minimal layout (~800 tokens / session start):");
        println!("  CLAUDE.md                  ~450 tok  rules + tech stack");
        println!("  .claude/COMMON_MISTAKES.md ~350 tok  top 5 bugs in this repo");
        println!("  .claude/QUICK_START.md     ~100 tok  common commands");
        println!("  .claude/ARCHITECTURE_MAP.md ~150 tok where things live");
        println!();
        let fw_label = if matches!(fw, framework::Framework::Unknown) {
            "generic"
        } else {
            fw.as_str()
        };
        println!("Run `tokenade install --with-claude-md` to scaffold a {fw_label} template.");
        return 0;
    }

    // Per-file table.
    println!("{:<42} {:>10}", "file", "tokens");
    println!("{:-<42} {:->10}", "", "");
    for (rel, t) in &hits {
        println!("{rel:<42} {t:>10}");
    }
    println!("{:-<42} {:->10}", "", "");
    // #975 — was "TOTAL (every session start)" + raw sum, which
    // over-counted by N× for N-agent scaffolds (a Cursor session
    // never loads CLAUDE.md and vice-versa). The honest figure is
    // MAX(agent files) + sum(.claude/*.md) — the worst single-agent
    // session. Show both lines when an installed agent dragged in
    // .claude/*.md extras, so the math is auditable.
    if claude_extras_sum > 0 {
        println!(
            "{:<42} {:>10}",
            "PER SESSION (max agent file + .claude/*.md)", total_tokens
        );
        println!(
            "{:<42} {:>10}",
            "  ↳ largest single agent scaffold", agent_file_max
        );
        println!(
            "{:<42} {:>10}",
            "  ↳ + .claude/*.md (Claude-only extras)", claude_extras_sum
        );
    } else {
        println!(
            "{:<42} {:>10}",
            "PER SESSION (one agent loads one file)", total_tokens
        );
    }
    println!();

    // Compare against nadimtuhin's ~800 target.
    let target: usize = 800;
    if total_tokens > target {
        let waste = total_tokens - target;
        let pct = 100.0 * waste as f64 / total_tokens as f64;
        // Translate to USD at the default model's rate assuming ~50 sessions/month.
        use tokenade_core::pricing;
        let monthly = pricing::cost_for(pricing::default_model(), waste as u64 * 50);
        println!(
            "Above the {target}-token target: {waste} tokens above baseline per session ({pct:.0}%)."
        );
        println!(
            "At ~50 sessions/month on {}: ~${monthly:.2}/month in startup overhead.",
            pricing::default_model().label
        );
        let fw_label = if matches!(fw, framework::Framework::Unknown) {
            "generic"
        } else {
            fw.as_str()
        };
        // Context-aware suggestion: if any hit file already contains
        // the tokenade scaffold marker, the install-with-claude-md
        // pitch is moot (re-running install would no-op the rules
        // block; the bloat is in the agent-specific/project text
        // ABOVE the marker, which only the user can prune). Mirror
        // of the discover-suggestion fix (#455).
        let already_scaffolded = hits.iter().any(|(rel, _)| {
            let path = r.join(rel);
            std::fs::read_to_string(&path)
                .map(|s| s.contains(tokenade_core::project_scaffold::SCAFFOLD_MARKER))
                .unwrap_or(false)
        });
        println!();
        if already_scaffolded {
            println!(
                "Files already contain the tokenade scaffold block — re-running install \
                 won't shrink them. The bloat lives in your project-specific text ABOVE \
                 the `<!-- tokenade-scaffold -->` marker."
            );
            println!();
            println!("To trim further:");
            println!(
                "  • prune the prose above the marker in CLAUDE.md / AGENTS.md / .cursorrules"
            );
            println!(
                "  • move framework-specific details into `.claude/ARCHITECTURE_MAP.md` (loaded on-demand)"
            );
            println!(
                "  • run `tokenade install --with-brevity-skill` for a terser response-style preamble"
            );
        } else {
            println!(
                "Suggestion: run `tokenade install --with-claude-md` to replace the auto-loaded"
            );
            println!("files with a {fw_label}-targeted minimal scaffold.");
        }
    } else {
        println!("Startup cost is below the ~{target}-token target. Nothing to do.");
    }
    0
}
