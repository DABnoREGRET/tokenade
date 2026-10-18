//! `tokenade judge` — the "no quality loss" proof on ONE artifact.
#![allow(unused_imports)]
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;

/// `tokenade judge <file>|- -q "question" [--expect a,b] [--compacted <file>] [--json]`
///
/// On-demand "no quality loss" proof for ONE artifact (sigmap-style answer
/// groundedness). Reads the raw output, compacts it (tokenade's own
/// `auto::compact`, or an external `--compacted <file>`), then asks a model the
/// question against each side and reports whether the answer survived. With
/// `--expect a,b` it uses the exact corpus span gate; without, a referee model
/// judges answer equivalence. Needs `ANTHROPIC_API_KEY`; without it the verdict
/// is "inconclusive" and exit is 0 (an unrunnable judge never fails on its own,
/// matching the live bench). Exit 1 only on a *measured* quality loss.
pub(crate) fn cmd_judge(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade judge <file>|- -q \"question\" [--expect a,b] [--compacted <file>] [--json]\n\
             \n\
             Proves a single compaction kept the answer. Compacts the input with\n\
             tokenade's auto compactor (or reads --compacted), then asks a model the\n\
             question against raw vs compacted and reports whether the answer survived.\n\
             \n\
             --expect a,b   ground-truth spans that MUST survive (re:… = regex). Without\n\
             this, a referee model judges raw-vs-compacted answer equivalence.\n\
             --compacted F  judge an external compacted file instead of auto-compacting.\n\
             --json         machine-readable verdict.\n\
             \n\
             Needs ANTHROPIC_API_KEY (model: $TOKENADE_JUDGE_MODEL / $TOKENADE_BENCH_MODEL\n\
             / claude-haiku-4-5). Without a key the verdict is inconclusive (exit 0)."
        );
        return 0;
    }
    let json_mode = rest.iter().any(|a| a == "--json");
    let question = match get_flag(rest, "--question").or_else(|| get_flag(rest, "-q")) {
        Some(q) => q.to_string(),
        None => {
            eprintln!(
                "usage: tokenade judge <file>|- -q \"question\" [--expect a,b] [--compacted <file>] [--json]"
            );
            return 2;
        }
    };
    let expect: Vec<String> = get_flag(rest, "--expect")
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Resolve the single positional raw input, skipping value-flag pairs.
    let value_flags = ["--question", "-q", "--expect", "--compacted"];
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, a) in rest.iter().enumerate() {
        if value_flags.contains(&a.as_str()) {
            consumed.insert(i);
            consumed.insert(i + 1);
        }
    }
    let mut positionals: Vec<&String> = Vec::new();
    for (i, a) in rest.iter().enumerate() {
        if consumed.contains(&i) || a == "--json" {
            continue;
        }
        if a.starts_with("--") || (a.starts_with('-') && a.as_str() != "-") {
            eprintln!("tokenade judge: unknown flag `{a}`");
            return 2;
        }
        positionals.push(a);
    }

    let read_stdin = || -> Result<String, i32> {
        let mut s = String::new();
        if crate::bounded_input::read_stdin_lossy(&mut s).is_err() {
            return Err(1);
        }
        Ok(s)
    };
    let raw = match positionals.len() {
        0 => {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "usage: tokenade judge <file>|- -q \"question\" [--expect a,b] [--compacted <file>] [--json]"
                );
                return 2;
            }
            match read_stdin() {
                Ok(s) => s,
                Err(c) => return c,
            }
        }
        1 => {
            let p = positionals[0];
            if p == "-" {
                match read_stdin() {
                    Ok(s) => s,
                    Err(c) => return c,
                }
            } else {
                match fs::read_to_string(p) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tokenade judge: cannot read `{p}`: {e}");
                        return 1;
                    }
                }
            }
        }
        n => {
            eprintln!("tokenade judge: expected a single input file (or `-`), got {n}");
            return 2;
        }
    };

    let compacted = if let Some(cf) = get_flag(rest, "--compacted") {
        match fs::read_to_string(cf) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade judge: cannot read --compacted `{cf}`: {e}");
                return 1;
            }
        }
    } else {
        auto::compact(&raw).1
    };

    let v = judge::judge(&question, &raw, &compacted, &expect);
    let mode_str = match v.mode {
        judge::Mode::Spans => "spans",
        judge::Mode::Referee => "referee",
    };

    if json_mode {
        let out = serde_json::json!({
            "question": v.question,
            "mode": mode_str,
            "tokens_raw": v.tokens_raw,
            "tokens_compacted": v.tokens_compacted,
            "savings_pct": (v.savings_pct * 10.0).round() / 10.0,
            "raw_answer": v.raw.answer,
            "compacted_answer": v.compacted.answer,
            "raw_grounded": v.raw.grounded,
            "compacted_grounded": v.compacted.grounded,
            "preserved": v.preserved,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string())
        );
        return i32::from(v.preserved == Some(false));
    }

    let first_line = |s: &str| s.lines().next().unwrap_or("").trim().to_string();
    println!("tokenade judge — {mode_str} mode");
    println!(
        "  tokens: {} → {}  ({:.1}% saved)",
        v.tokens_raw, v.tokens_compacted, v.savings_pct
    );
    match &v.raw.answer {
        Some(a) => println!("  raw answer:       {}", first_line(a)),
        None => {
            println!("  raw answer:       (inconclusive — no ANTHROPIC_API_KEY or call failed)")
        }
    }
    match &v.compacted.answer {
        Some(a) => println!("  compacted answer: {}", first_line(a)),
        None => {
            println!("  compacted answer: (inconclusive — no ANTHROPIC_API_KEY or call failed)")
        }
    }
    if v.mode == judge::Mode::Spans {
        println!(
            "  spans grounded:   raw={}  compacted={}",
            v.raw.grounded, v.compacted.grounded
        );
    }
    match v.preserved {
        Some(true) => println!("  verdict: ✓ no quality loss — the answer survived compaction"),
        Some(false) => {
            println!("  verdict: ✗ QUALITY LOSS — the compacted output dropped the answer")
        }
        None => println!("  verdict: — inconclusive (set ANTHROPIC_API_KEY to run the judge)"),
    }
    i32::from(v.preserved == Some(false))
}
