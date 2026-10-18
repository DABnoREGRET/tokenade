//! **How many REAL Claude tokens is one estimated token worth?**
//!
//! ```text
//! tokenizer_drift            # this machine's Claude Code transcripts
//! tokenizer_drift <dir>      # another transcripts directory
//! tokenizer_drift --codex    # Codex sessions (~/.codex/sessions), OpenAI models
//! ```
//!
//! `tokenizer::estimate` is calibrated on cl100k (OpenAI's tokenizer), and
//! the whole gains ledger is written in that unit. The model reading the
//! output counts with ITS OWN. A transcript gives that real count without a
//! tokenizer: between two calls of the same tool round, the context grows by
//! the output of the first and the tool results it received —
//! `ctx(B) − ctx(A) − output(A)` — up to framing and injected reminders.
//! The ratio to `estimate(results)` is measured per model and per tool; the
//! median over results of at least 2,000 estimated tokens makes the framing
//! noise negligible.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
struct Msg {
    ctx: u64,
    out: u64,
    model: String,
    tools: Vec<(String, String)>,
    /// Visible text of a text-only message (no thinking, no call): its
    /// `output_tokens` is then a direct, independent real count.
    text: Option<String>,
    mixed: bool,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

fn text_of(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// (model, tool) → ratios real / estimate, for one transcript.
fn pairs(
    path: &Path,
    into: &mut BTreeMap<(String, String), Vec<f64>>,
    fit: &mut BTreeMap<String, Vec<(f64, f64)>>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut order: Vec<String> = Vec::new();
    let mut msgs: HashMap<String, Msg> = HashMap::new();
    let mut results: HashMap<String, String> = HashMap::new();
    // message id → a human turn arrived before the NEXT assistant message.
    let mut broken: HashMap<String, bool> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str() {
            Some("assistant") => {
                let m = &v["message"];
                let Some(id) = m["id"].as_str() else { continue };
                let u = &m["usage"];
                let e = msgs.entry(id.to_string()).or_insert_with(|| {
                    order.push(id.to_string());
                    Msg::default()
                });
                e.ctx = u["input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                e.out = e.out.max(u["output_tokens"].as_u64().unwrap_or(0));
                e.model = m["model"].as_str().unwrap_or("?").to_string();
                for b in m["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => e
                            .text
                            .get_or_insert_with(String::new)
                            .push_str(b["text"].as_str().unwrap_or("")),
                        _ => e.mixed = true,
                    }
                    if b["type"] == "tool_use" {
                        e.tools.push((
                            b["id"].as_str().unwrap_or("").to_string(),
                            b["name"].as_str().unwrap_or("?").to_string(),
                        ));
                    }
                }
            }
            Some("user") => {
                let c = &v["message"]["content"];
                let human = c.is_string()
                    || c.as_array()
                        .is_some_and(|a| a.iter().any(|b| b["type"] == "text"));
                if human {
                    if let Some(last) = order.last() {
                        broken.insert(last.clone(), true);
                    }
                }
                for b in c.as_array().into_iter().flatten() {
                    if b["type"] == "tool_result" {
                        results.insert(
                            b["tool_use_id"].as_str().unwrap_or("").to_string(),
                            text_of(&b["content"]),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    for m in msgs.values() {
        if let (Some(t), false) = (&m.text, m.mixed) {
            let est = tokenade_core::tokenizer::estimate(t);
            if est >= 300 {
                into.entry((m.model.clone(), "(output text)".to_string()))
                    .or_default()
                    .push(m.out as f64 / est as f64);
            }
        }
    }
    for w in order.windows(2) {
        let (a, b) = (&msgs[&w[0]], &msgs[&w[1]]);
        if broken.contains_key(&w[0])
            || a.tools.is_empty()
            || a.model != b.model
            || b.ctx <= a.ctx + a.out
        {
            continue;
        }
        let texts: Vec<&String> = a
            .tools
            .iter()
            .filter_map(|(id, _)| results.get(id))
            .collect();
        if texts.len() != a.tools.len() {
            continue;
        }
        let est: usize = texts
            .iter()
            .map(|t| tokenade_core::tokenizer::estimate(t))
            .sum();
        let real = (b.ctx - a.ctx - a.out) as f64;
        if est >= 300 {
            fit.entry(a.model.clone())
                .or_default()
                .push((est as f64, real));
        }
        if est < 2_000 {
            continue;
        }
        let tool = if a.tools.len() == 1 {
            a.tools[0].1.clone()
        } else {
            "(several)".to_string()
        };
        into.entry((a.model.clone(), tool))
            .or_default()
            .push(real / est as f64);
    }
}

/// Codex (`~/.codex/sessions/**/rollout-*.jsonl`): each `token_count` event
/// reports the request just answered (`last_token_usage`). Request k+1 carries
/// on top of request k its output and the tool outputs journaled between
/// `token_count` k and k+1: `in(k+1) − in(k) − out(k)` real tokens for them.
/// The other pairing (outputs between k−1 and k) looked right on one session
/// read by hand and scattered from 0.05 to 4× over all of them; this one lies
/// on the line (2026-10-05: gpt-5.5, slope 1.00, n = 47).
fn codex_pairs(path: &Path, fit: &mut BTreeMap<String, Vec<(f64, f64)>>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut model = String::from("?");
    // (input, output) of each request, and the tool outputs written before it.
    let mut reqs: Vec<(u64, u64)> = Vec::new();
    let mut outputs_before: Vec<String> = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let p = &v["payload"];
        if let Some(m) = p["model"].as_str() {
            model = m.to_string();
        }
        match p["type"].as_str() {
            Some("function_call_output") | Some("custom_tool_call_output") => {
                pending.push_str(
                    &p["output"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| p["output"].to_string()),
                );
            }
            Some("token_count") => {
                let u = &p["info"]["last_token_usage"];
                let Some(input) = u["input_tokens"].as_u64() else {
                    continue;
                };
                reqs.push((input, u["output_tokens"].as_u64().unwrap_or(0)));
                outputs_before.push(std::mem::take(&mut pending));
            }
            _ => {}
        }
    }
    for k in 0..reqs.len().saturating_sub(1) {
        let (ink, outk) = reqs[k];
        let (next, _) = reqs[k + 1];
        if next <= ink + outk {
            continue;
        }
        let est = tokenade_core::tokenizer::estimate(&outputs_before[k + 1]);
        if est >= 300 {
            fit.entry(format!("codex:{model}"))
                .or_default()
                .push((est as f64, (next - ink - outk) as f64));
        }
    }
}

fn tk_base_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn q(v: &[f64], p: f64) -> f64 {
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--codex"], &args);
    let codex = args.iter().any(|a| a == "--codex");
    let positional = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from);
    let root = positional.unwrap_or_else(|| {
        if codex {
            tk_base_home().join(".codex/sessions")
        } else {
            tokenade_bench::transcripts::transcript_root()
        }
    });
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut by: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    let mut fit: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for f in &files {
        if codex {
            codex_pairs(f, &mut fit);
        } else {
            pairs(f, &mut by, &mut fit);
        }
    }
    println!("{} transcripts · real Claude tokens per estimated token (ctx growth − output) / estimate(results ≥ 2k)", files.len());
    println!(
        "{:<34} {:<12} {:>6} {:>6} {:>6} {:>6}",
        "model", "tool", "n", "p25", "median", "p75"
    );
    let mut all: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for ((model, tool), mut v) in by {
        v.sort_by(f64::total_cmp);
        all.entry(model.clone()).or_default().extend(&v);
        if v.len() >= 20 {
            println!(
                "{model:<34} {tool:<12} {:>6} {:>6.2} {:>6.2} {:>6.2}",
                v.len(),
                q(&v, 0.25),
                q(&v, 0.5),
                q(&v, 0.75)
            );
        }
    }
    for (model, mut v) in all {
        v.sort_by(f64::total_cmp);
        if v.len() >= 20 {
            println!(
                "{model:<34} {:<12} {:>6} {:>6.2} {:>6.2} {:>6.2}",
                "ALL",
                v.len(),
                q(&v, 0.25),
                q(&v, 0.5),
                q(&v, 0.75)
            );
        }
    }
    // real = a·est + b: `a` is the tokenizer ratio, `b` what every tool
    // round adds whatever its size (framing, injected reminders). Theil–Sen
    // (median of pairwise slopes, on a sample) so a few outliers cannot drag it.
    for (model, pts) in fit {
        if pts.len() < 20 {
            continue;
        }
        let step = (pts.len() / 400).max(1);
        let sample: Vec<&(f64, f64)> = pts.iter().step_by(step).collect();
        let mut slopes = Vec::new();
        for i in 0..sample.len() {
            for j in i + 1..sample.len() {
                let (x1, y1) = sample[i];
                let (x2, y2) = sample[j];
                if (x2 - x1).abs() > 200.0 {
                    slopes.push((y2 - y1) / (x2 - x1));
                }
            }
        }
        slopes.sort_by(f64::total_cmp);
        let a = q(&slopes, 0.5);
        let mut icpt: Vec<f64> = pts.iter().map(|(x, y)| y - a * x).collect();
        icpt.sort_by(f64::total_cmp);
        println!("{model:<34} fit n={} slope {:.2} (p25 {:.2} · p75 {:.2}) · fixed per round {:.0} tokens", pts.len(), a, q(&slopes, 0.25), q(&slopes, 0.75), q(&icpt, 0.5));
    }
}
