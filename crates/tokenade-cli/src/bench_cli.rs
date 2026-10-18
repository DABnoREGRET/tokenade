//! `tokenade bench` — in-session A/B benchmark that replays prompts TWICE
//! (WITH Tokenade vs WITHOUT) on the user's OWN repo and prints a tokens +
//! real-dollar savings report, so a user can prove the economy for themselves.
//!
//! Design (why it's shaped this way):
//!   * The prompt executor is behind the [`BenchRunner`] trait so the whole
//!     pipeline — prompt parsing, plan, 2-variant orchestration, aggregation,
//!     reporting, pricing — is unit-tested with a deterministic mock and makes
//!     ZERO billed LLM calls. The production executor is
//!     [`ExternalAgentRunner`], which shells out to a user-supplied agent
//!     command and toggles `TOKENADE_HOOK_DISABLED=1` for the control side.
//!   * Cost is derived from the SAME live Helicone price table the dashboard
//!     uses (`tokenade_core::model_costs::cost_for_model`) — not re-invented.
//!   * SAFE-BY-DEFAULT: with no `--run`, the command only prints the PLAN
//!     (prompts × 2 variants) and exits; a real (billed) run needs an explicit
//!     `--run` AND a wired `--agent-cmd`. Aligns with the repo bench policy
//!     ("never bench without an explicit order").
//!
//! Integration point for a real run: measuring a real agent's token usage
//! requires the agent to REPORT it. `ExternalAgentRunner` runs the agent once
//! per (prompt, variant) with `TOKENADE_BENCH_USAGE_FILE` pointing at a temp
//! file the agent (or a wrapper) must write as `{"input_tokens":N,
//! "output_tokens":N}`. That is the one seam left to wire to a concrete agent;
//! everything else is complete and tested.

use std::path::{Path, PathBuf};

/// Which side of the A/B comparison a single task run belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Tokenade active — hooks/compaction ON (the default hook path).
    WithTokenade,
    /// Control — `TOKENADE_HOOK_DISABLED=1`, no compaction.
    WithoutTokenade,
}

impl Variant {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Variant::WithTokenade => "with-tokenade",
            Variant::WithoutTokenade => "without-tokenade",
        }
    }
}

/// Token usage measured for one prompt run under one variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Measurement {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Measurement {
    fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Pluggable executor for one prompt under one variant. Abstracted so the
/// orchestration + reporting are testable with a deterministic mock and no
/// real (billed) LLM calls. Production impl: [`ExternalAgentRunner`].
pub(crate) trait BenchRunner {
    /// Run `prompt` under `variant` against `repo`; return measured usage.
    /// Pricing uses the plan's model, so the runner reports raw usage only.
    fn run(&self, prompt: &str, variant: Variant, repo: &Path) -> Result<Measurement, String>;
}

/// A fully-resolved benchmark plan: what will be run, where, priced as which
/// model. Built once, then either printed (dry-run) or executed.
pub(crate) struct BenchPlan {
    pub prompts: Vec<String>,
    pub repo: PathBuf,
    pub model: String,
}

/// One prompt's A vs B measurements.
pub(crate) struct PromptResult {
    pub prompt: String,
    pub with: Measurement,
    pub without: Measurement,
}

/// The full A/B result set for a plan.
pub(crate) struct Report {
    pub model: String,
    pub results: Vec<PromptResult>,
}

/// Percent reduction of `after` relative to `before` (0 when `before` is 0).
fn pct(before: u64, after: u64) -> f64 {
    if before == 0 {
        0.0
    } else {
        (before as f64 - after as f64) / before as f64 * 100.0
    }
}

/// $ for a measurement under `model`, or `None` when the Helicone price table
/// has no entry / is unreachable (fail-open — we still report tokens).
fn usd_for(model: &str, m: &Measurement) -> Option<f64> {
    tokenade_core::model_costs::cost_for_model(model)
        .map(|c| c.usd(m.input_tokens, 0, 0, m.output_tokens))
}

fn fmt_usd(v: Option<f64>) -> String {
    match v {
        Some(d) => format!("${d:.4}"),
        None => "$   n/a".to_string(),
    }
}

/// Run every prompt through BOTH variants. Control (without) first, then
/// with — order is irrelevant to the measured deltas. Never calls an LLM
/// itself; delegates to the injected `runner`.
pub(crate) fn orchestrate(plan: &BenchPlan, runner: &dyn BenchRunner) -> Result<Report, String> {
    let mut results = Vec::with_capacity(plan.prompts.len());
    for p in &plan.prompts {
        let without = runner.run(p, Variant::WithoutTokenade, &plan.repo)?;
        let with = runner.run(p, Variant::WithTokenade, &plan.repo)?;
        results.push(PromptResult {
            prompt: p.clone(),
            with,
            without,
        });
    }
    Ok(Report {
        model: plan.model.clone(),
        results,
    })
}

fn truncate(s: &str, n: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        let head: String = one_line.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Human-readable A/B report.
pub(crate) fn render_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("tokenade bench — A/B token & cost report\n");
    out.push_str(&format!("model: {}\n\n", report.model));

    let (mut sum_with_in, mut sum_with_out) = (0u64, 0u64);
    let (mut sum_wo_in, mut sum_wo_out) = (0u64, 0u64);

    for (i, r) in report.results.iter().enumerate() {
        let wo = r.without.total();
        let w = r.with.total();
        sum_with_in += r.with.input_tokens;
        sum_with_out += r.with.output_tokens;
        sum_wo_in += r.without.input_tokens;
        sum_wo_out += r.without.output_tokens;

        out.push_str(&format!("[{}] {}\n", i + 1, truncate(&r.prompt, 70)));
        out.push_str(&format!(
            "    without tokenade: {wo:>8} tok   {}\n",
            fmt_usd(usd_for(&report.model, &r.without))
        ));
        out.push_str(&format!(
            "    with    tokenade: {w:>8} tok   {}\n",
            fmt_usd(usd_for(&report.model, &r.with))
        ));
        let saved_tok = wo.saturating_sub(w);
        let saved_usd = match (
            usd_for(&report.model, &r.without),
            usd_for(&report.model, &r.with),
        ) {
            (Some(a), Some(b)) => Some(a - b),
            _ => None,
        };
        out.push_str(&format!(
            "    saved:            {saved_tok:>8} tok ({:.1}%)   {}\n\n",
            pct(wo, w),
            fmt_usd(saved_usd)
        ));
    }

    let with = Measurement {
        input_tokens: sum_with_in,
        output_tokens: sum_with_out,
    };
    let without = Measurement {
        input_tokens: sum_wo_in,
        output_tokens: sum_wo_out,
    };
    let saved_usd = match (
        usd_for(&report.model, &without),
        usd_for(&report.model, &with),
    ) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    out.push_str("aggregate:\n");
    out.push_str(&format!(
        "    without tokenade: {:>8} tok   {}\n",
        without.total(),
        fmt_usd(usd_for(&report.model, &without))
    ));
    out.push_str(&format!(
        "    with    tokenade: {:>8} tok   {}\n",
        with.total(),
        fmt_usd(usd_for(&report.model, &with))
    ));
    out.push_str(&format!(
        "    saved:            {:>8} tok ({:.1}%)   {}\n",
        without.total().saturating_sub(with.total()),
        pct(without.total(), with.total()),
        fmt_usd(saved_usd)
    ));
    out
}

/// Machine-readable A/B report.
pub(crate) fn render_json(report: &Report) -> String {
    let per: Vec<serde_json::Value> = report
        .results
        .iter()
        .map(|r| {
            serde_json::json!({
                "prompt": r.prompt,
                "without_tokenade": {
                    "input_tokens": r.without.input_tokens,
                    "output_tokens": r.without.output_tokens,
                    "total_tokens": r.without.total(),
                    "usd": usd_for(&report.model, &r.without),
                },
                "with_tokenade": {
                    "input_tokens": r.with.input_tokens,
                    "output_tokens": r.with.output_tokens,
                    "total_tokens": r.with.total(),
                    "usd": usd_for(&report.model, &r.with),
                },
                "saved_tokens": r.without.total().saturating_sub(r.with.total()),
                "saved_pct": pct(r.without.total(), r.with.total()),
            })
        })
        .collect();
    let tot_with: u64 = report.results.iter().map(|r| r.with.total()).sum();
    let tot_wo: u64 = report.results.iter().map(|r| r.without.total()).sum();
    serde_json::to_string_pretty(&serde_json::json!({
        "model": report.model,
        "prompts": per,
        "aggregate": {
            "with_total_tokens": tot_with,
            "without_total_tokens": tot_wo,
            "saved_tokens": tot_wo.saturating_sub(tot_with),
            "saved_pct": pct(tot_wo, tot_with),
        }
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// Prompt sources
// ---------------------------------------------------------------------------

/// Small representative default set used when the user supplies no prompts.
const DEFAULT_PROMPTS: &[&str] = &[
    "Summarize what this repository does and its main entry points.",
    "Find where errors are logged and list the files involved.",
    "Explain the build and test workflow for this project.",
];

const DEFAULT_MODEL: &str = "claude-3-5-sonnet";

/// Parse a prompts file: one prompt per non-empty line. Supports a plain-text
/// list, a simple YAML sequence (`- prompt`), and `#` comments. Deliberately
/// tolerant rather than a full YAML parser — the format is a flat prompt list.
fn parse_prompts_file(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let t = t.strip_prefix("- ").unwrap_or(t);
        let t = t.trim().trim_matches('"').trim_matches('\'').trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

/// Collect every `--prompt <text>` occurrence (repeatable flag).
fn collect_prompt_flags(rest: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--prompt" {
            if let Some(v) = rest.get(i + 1) {
                if !v.starts_with("--") {
                    out.push(v.clone());
                    i += 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Resolve the effective prompt list from CLI args: `--prompt` occurrences and
/// `--prompts-file` are merged; if none supplied, the embedded default set is
/// used. Returns `Err` when `--prompts-file` is given but unreadable/empty.
fn resolve_prompts(rest: &[String]) -> Result<Vec<String>, String> {
    let mut prompts = collect_prompt_flags(rest);
    if let Some(path) = crate::get_flag(rest, "--prompts-file") {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("--prompts-file {path}: {e}"))?;
        let parsed = parse_prompts_file(&text);
        if parsed.is_empty() {
            return Err(format!("--prompts-file {path}: no prompts found"));
        }
        prompts.extend(parsed);
    }
    if prompts.is_empty() {
        prompts = DEFAULT_PROMPTS.iter().map(|s| s.to_string()).collect();
    }
    Ok(prompts)
}

// ---------------------------------------------------------------------------
// Production runner (the one integration seam left to wire)
// ---------------------------------------------------------------------------

/// Runs a user-supplied agent command once per (prompt, variant). The control
/// side is produced by exporting `TOKENADE_HOOK_DISABLED=1` (Tokenade's own
/// bypass switch) into the child's environment; the tokenade side runs with
/// hooks live. Real token usage is read back from a temp file the agent writes
/// (contract: `{"input_tokens":N,"output_tokens":N}` at
/// `TOKENADE_BENCH_USAGE_FILE`) — the one seam a concrete agent must satisfy.
pub(crate) struct ExternalAgentRunner {
    /// Shell command template; `{prompt}` is substituted, else prompt is
    /// appended as a final argument. Run via `sh -c`.
    pub agent_cmd: String,
}

impl BenchRunner for ExternalAgentRunner {
    fn run(&self, prompt: &str, variant: Variant, repo: &Path) -> Result<Measurement, String> {
        use std::process::Command;
        let usage_file = std::env::temp_dir().join(format!(
            "tokenade-bench-usage-{}-{}.json",
            std::process::id(),
            fastish_id(prompt, variant)
        ));
        let _ = std::fs::remove_file(&usage_file);

        let cmd = if self.agent_cmd.contains("{prompt}") {
            self.agent_cmd.replace("{prompt}", &shell_quote(prompt))
        } else {
            format!("{} {}", self.agent_cmd, shell_quote(prompt))
        };

        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(&cmd)
            .current_dir(repo)
            .env("TOKENADE_BENCH_USAGE_FILE", &usage_file);
        if variant == Variant::WithoutTokenade {
            c.env("TOKENADE_HOOK_DISABLED", "1");
        }
        let status = c
            .status()
            .map_err(|e| format!("spawn agent ({}): {e}", variant.label()))?;
        if !status.success() {
            return Err(format!(
                "agent exited {} on {} variant",
                status.code().unwrap_or(-1),
                variant.label()
            ));
        }
        let raw = std::fs::read_to_string(&usage_file).map_err(|_| {
            format!(
                "no usage report at {} — the agent must write \
                 {{\"input_tokens\":N,\"output_tokens\":N}} there \
                 (this is the runner integration seam; see bench_cli.rs docs)",
                usage_file.display()
            )
        })?;
        let _ = std::fs::remove_file(&usage_file);
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("usage report parse error: {e}"))?;
        let get = |k: &str| v.get(k).and_then(|x| x.as_u64());
        match (get("input_tokens"), get("output_tokens")) {
            (Some(input_tokens), Some(output_tokens)) => Ok(Measurement {
                input_tokens,
                output_tokens,
            }),
            _ => Err("usage report missing input_tokens/output_tokens".to_string()),
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn fastish_id(prompt: &str, variant: Variant) -> u64 {
    // Cheap non-crypto hash for a unique temp filename per (prompt, variant).
    let mut h: u64 = 1469598103934665603;
    for b in prompt.bytes().chain(std::iter::once(variant as u8)) {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

const BENCH_HELP: &str = "\
usage: tokenade bench [--prompt <text> ...] [--prompts-file <path>]
                      [--repo <path>] [--model <id>] [--run] [--json]
                      [--agent-cmd <shell-cmd>]

Replay prompts TWICE — WITH Tokenade (hooks/compaction ON) and WITHOUT
(TOKENADE_HOOK_DISABLED=1) — on YOUR repo, and report tokens in/out + real
dollar cost per variant so you can prove the savings for yourself.

Prompts (merged; embedded default set if none given):
  --prompt <text>       Add one prompt. Repeatable.
  --prompts-file <path> One prompt per line (plain, `- yaml`, `# comments`).

Scope & pricing:
  --repo <path>         Repo to run against (default: current directory).
  --model <id>          Model id for $ pricing via the live Helicone table
                        (default: claude-3-5-sonnet).

Cost guard (SAFE BY DEFAULT):
  (no flag)             DRY-RUN: print the plan (prompts × 2 variants) and the
                        billed-run warning. Runs NOTHING. This is the default.
  --run                 Actually execute — makes REAL, BILLED agent calls.
                        Requires --agent-cmd.
  --agent-cmd <cmd>     Shell command that runs one prompt via your agent. The
                        prompt substitutes for `{prompt}` (else appended). The
                        agent MUST write its token usage as
                        {\"input_tokens\":N,\"output_tokens\":N} to the file at
                        $TOKENADE_BENCH_USAGE_FILE. The control variant is run
                        with TOKENADE_HOOK_DISABLED=1 exported automatically.

  --json                Emit the report (or plan) as JSON.
  --help, -h            This help.";

/// `tokenade bench` — see [`BENCH_HELP`]. Dev/bench tooling: functional and
/// documented via `--help`, but kept out of the catalogue + README docs guard
/// (like `calibrate`).
pub(crate) fn cmd_bench(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("{BENCH_HELP}");
        return 0;
    }

    let prompts = match resolve_prompts(rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tokenade bench: {e}");
            return 2;
        }
    };
    let repo = match crate::get_flag(rest, "--repo") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let model = crate::get_flag(rest, "--model")
        .unwrap_or(DEFAULT_MODEL)
        .to_string();
    let json_mode = rest.iter().any(|a| a == "--json");
    let do_run = rest.iter().any(|a| a == "--run");

    let plan = BenchPlan {
        prompts,
        repo,
        model,
    };

    if !do_run {
        print_dry_run(&plan, json_mode);
        return 0;
    }

    // Real run: needs a wired agent. We NEVER call an LLM ourselves.
    let Some(agent_cmd) = crate::get_flag(rest, "--agent-cmd") else {
        eprintln!(
            "tokenade bench --run needs --agent-cmd <shell-cmd> — the command \
             that runs one prompt via your agent and writes its token usage to \
             $TOKENADE_BENCH_USAGE_FILE. See `tokenade bench --help`."
        );
        return 2;
    };
    eprintln!(
        "tokenade bench: executing {} REAL (billed) runs ({} prompts × 2 variants)…",
        plan.prompts.len() * 2,
        plan.prompts.len()
    );
    let runner = ExternalAgentRunner {
        agent_cmd: agent_cmd.to_string(),
    };
    match orchestrate(&plan, &runner) {
        Ok(report) => {
            if json_mode {
                println!("{}", render_json(&report));
            } else {
                print!("{}", render_text(&report));
            }
            0
        }
        Err(e) => {
            eprintln!("tokenade bench: {e}");
            1
        }
    }
}

fn print_dry_run(plan: &BenchPlan, json_mode: bool) {
    if json_mode {
        let plan_json = serde_json::json!({
            "dry_run": true,
            "model": plan.model,
            "repo": plan.repo.display().to_string(),
            "prompts": plan.prompts,
            "variants": ["without-tokenade", "with-tokenade"],
            "total_runs_if_executed": plan.prompts.len() * 2,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&plan_json).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    println!("tokenade bench — PLAN (dry-run, nothing executed)");
    println!("model: {}   repo: {}", plan.model, plan.repo.display());
    println!(
        "{} prompt(s) × 2 variants = {} run(s) if executed:",
        plan.prompts.len(),
        plan.prompts.len() * 2
    );
    for (i, p) in plan.prompts.iter().enumerate() {
        println!("  [{}] {}", i + 1, truncate(p, 76));
        println!("        → without-tokenade (TOKENADE_HOOK_DISABLED=1)");
        println!("        → with-tokenade");
    }
    println!();
    println!("⚠ A real run makes REAL, BILLED agent/LLM calls. To execute, re-run with:");
    println!("    tokenade bench … --run --agent-cmd '<your agent command>'");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic runner: the control side "costs" a fixed multiple of the
    /// tokenade side, so orchestration/report math is asserted without any LLM.
    struct MockRunner;
    impl BenchRunner for MockRunner {
        fn run(&self, prompt: &str, variant: Variant, _repo: &Path) -> Result<Measurement, String> {
            let base = prompt.len() as u64 + 10;
            match variant {
                // with tokenade: leaner input (compaction), same output.
                Variant::WithTokenade => Ok(Measurement {
                    input_tokens: base,
                    output_tokens: base / 2,
                }),
                // control: 2× the input, same output → 50% input savings.
                Variant::WithoutTokenade => Ok(Measurement {
                    input_tokens: base * 2,
                    output_tokens: base / 2,
                }),
            }
        }
    }

    fn plan(prompts: &[&str]) -> BenchPlan {
        BenchPlan {
            prompts: prompts.iter().map(|s| s.to_string()).collect(),
            repo: PathBuf::from("."),
            model: "claude-3-5-sonnet".to_string(),
        }
    }

    #[test]
    fn orchestrate_measures_both_variants_with_savings() {
        let r = MockRunner;
        let report = orchestrate(&plan(&["hello world", "second prompt"]), &r).unwrap();
        assert_eq!(report.results.len(), 2);
        for pr in &report.results {
            assert!(
                pr.with.total() < pr.without.total(),
                "tokenade side must be leaner"
            );
        }
    }

    #[test]
    fn pct_math() {
        assert!((pct(100, 50) - 50.0).abs() < 1e-9);
        assert_eq!(pct(0, 0), 0.0);
        assert!((pct(1000, 250) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn text_report_contains_savings_and_aggregate() {
        let r = MockRunner;
        let report = orchestrate(&plan(&["one", "two"]), &r).unwrap();
        let txt = render_text(&report);
        assert!(txt.contains("without tokenade"));
        assert!(txt.contains("with    tokenade"));
        assert!(txt.contains("saved:"));
        assert!(txt.contains("aggregate:"));
        // Deterministic mock: input halves, output equal → total savings > 0.
        let saved_line = txt.lines().rev().find(|l| l.contains("saved:")).unwrap();
        assert!(saved_line.contains('%'));
    }

    #[test]
    fn json_report_is_valid_and_has_aggregate() {
        let r = MockRunner;
        let report = orchestrate(&plan(&["alpha"]), &r).unwrap();
        let js = render_json(&report);
        let v: serde_json::Value = serde_json::from_str(&js).unwrap();
        assert_eq!(v["model"], "claude-3-5-sonnet");
        assert!(v["aggregate"]["saved_tokens"].as_u64().unwrap() > 0);
        assert_eq!(v["prompts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parse_prompts_file_handles_plain_yaml_and_comments() {
        let text = "\
# a comment
Summarize the repo
- \"quoted yaml item\"
- plain yaml item

  indented line  ";
        let got = parse_prompts_file(text);
        assert_eq!(
            got,
            vec![
                "Summarize the repo",
                "quoted yaml item",
                "plain yaml item",
                "indented line",
            ]
        );
    }

    #[test]
    fn collect_prompt_flags_is_repeatable() {
        let rest = vec![
            "--prompt".to_string(),
            "first".to_string(),
            "--repo".to_string(),
            "/x".to_string(),
            "--prompt".to_string(),
            "second".to_string(),
        ];
        assert_eq!(collect_prompt_flags(&rest), vec!["first", "second"]);
    }

    #[test]
    fn resolve_prompts_falls_back_to_default_set() {
        let got = resolve_prompts(&[]).unwrap();
        assert_eq!(got.len(), DEFAULT_PROMPTS.len());
        assert_eq!(got[0], DEFAULT_PROMPTS[0]);
    }

    #[test]
    fn resolve_prompts_prefers_explicit_over_default() {
        let rest = vec!["--prompt".to_string(), "only this".to_string()];
        assert_eq!(resolve_prompts(&rest).unwrap(), vec!["only this"]);
    }
}
