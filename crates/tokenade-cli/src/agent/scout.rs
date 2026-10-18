//! Scout v2 — bounded SLM pipelines for code exploration.
//!
//! NOT an agent: the harness (deterministic Rust) does ALL the collection —
//! `semantic` file ranking, in-file lexical windows, `query` symbol hits — and
//! the small local model only does the bounded language step: answer + cite.
//! Every citation is verified VERBATIM (whitespace-normalized) against the
//! packed evidence; invalid citations get one retry with feedback, then the
//! scout ABSTAINS rather than return an unverified claim.
//!
//! Experimental: exercised by `experiments/slm-scout`; no gain accounting yet.

use super::local;
use serde_json::{json, Value};
use std::path::PathBuf;

mod citations;
mod evidence;
mod intel;
pub(crate) use citations::*;
pub(crate) use evidence::*;
pub(crate) use intel::*;

// Harness hyper-parameters. Each is env-overridable (TOKENADE_SCOUT_*) so the
// autoresearch loop (experiments/slm-scout/autoresearch.py) can mutate them
// per run without recompiling; the consts are the committed baselines.
// 4 (was 7): autoresearch-accepted 2026-07-13 — fewer, better-targeted files
// beat wider recall on all three axes (safe 0.871→0.935, acc 0.742→0.806,
// p50 21.7→18.0s on the 31q eval; journal.jsonl kind=full mutation=TOP_FILES=4).
const TOP_FILES: usize = 4;
const WINDOWS_PER_FILE: usize = 2;
const WINDOW_LINES: usize = 30;
const MAX_SNIPPETS: usize = 8;
const MAX_EVIDENCE_CHARS: usize = 12_000;
const GEN_TOKENS: usize = 600;
const THINK_GEN_TOKENS: usize = 1600;
const FAST_TEMP: f32 = 0.2;
const THINK_TEMP: f32 = 0.6;

fn knob(name: &str, default: usize) -> usize {
    std::env::var(format!("TOKENADE_SCOUT_{name}"))
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(default)
}

fn knob_f(name: &str, default: f32) -> f32 {
    std::env::var(format!("TOKENADE_SCOUT_{name}"))
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v: &f32| (0.0..=2.0).contains(&v))
        .unwrap_or(default)
}

fn top_files() -> usize {
    knob("TOP_FILES", TOP_FILES)
}

fn windows_per_file() -> usize {
    knob("WINDOWS_PER_FILE", WINDOWS_PER_FILE)
}

fn window_lines() -> usize {
    knob("WINDOW_LINES", WINDOW_LINES)
}

fn max_snippets() -> usize {
    knob("MAX_SNIPPETS", MAX_SNIPPETS)
}

fn max_evidence_chars() -> usize {
    knob("EVIDENCE_CHARS", MAX_EVIDENCE_CHARS)
}

/// Union plain-BM25 hits into the hybrid ranking (diversification) — knob so
/// the autoresearch loop can measure whether the extra recall pays for the
/// extra distractors.
fn bm25_union() -> bool {
    std::env::var("TOKENADE_SCOUT_BM25_UNION").ok().as_deref() != Some("0")
}

/// The "evidence must concern the question's SUBJECT" prompt line — same deal.
fn subject_check() -> bool {
    std::env::var("TOKENADE_SCOUT_SUBJECT_CHECK")
        .ok()
        .as_deref()
        != Some("0")
}

/// GBNF grammar body for the scout answer. Fixed schema, hand-audited: the
/// model physically cannot emit anything but `{answer, citations[],
/// confidence}` (optionally preceded by a free-form `<think>` block).
/// Citations are (snippet id, absolute line number) — models paraphrase when
/// asked to copy quotes verbatim (measured: 9/15 abstains on a 1.7B), but
/// picking a line NUMBER off numbered evidence has no copy burden, and the
/// harness recovers the exact line text itself.
const GRAMMAR_QA_BODY: &str = r#"
json ::= "{" ws "\"answer\"" ws ":" ws string ws "," ws "\"citations\"" ws ":" ws citations ws "," ws "\"confidence\"" ws ":" ws confidence ws "}"
citations ::= "[" ws "]" | "[" ws citation (ws "," ws citation)* ws "]"
citation ::= "{" ws "\"id\"" ws ":" ws int ws "," ws "\"line\"" ws ":" ws int ws "}"
confidence ::= "\"high\"" | "\"medium\"" | "\"low\""
int ::= [0-9]+
string ::= "\"" schar* "\""
schar ::= [^"\\\x00-\x1f] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
ws ::= [ \t\n]*
"#;

/// Reasoning materially improves small-model attribution (which of two plausible
/// values answers the question), so thinking is ON by default; the grammar opens
/// with a free-form `<think>` block, then locks into the JSON schema. The think
/// content only forbids `</` so the closing tag stays unambiguous.
fn grammar_qa(think: bool) -> String {
    if think {
        format!(
            "root ::= think json\nthink ::= \"<think>\" tchar* \"</think>\" ws\n\
             tchar ::= [^<] | \"<\" [^/]\n{GRAMMAR_QA_BODY}"
        )
    } else {
        format!("root ::= json\n{GRAMMAR_QA_BODY}")
    }
}

/// Thinking policy for scout calls. Measured on CPU (15-question eval):
/// always-think triples p50 wall (18s → 55-95s) for mixed accuracy gains, but
/// thinking DOES fix attribution errors (qwen2.5-3b safe-rate 0.53 → 0.87).
/// Default is therefore ADAPTIVE: attempt 0 runs fast without thinking, and the
/// retry — reached exactly when verification failed, i.e. on the hard cases —
/// runs with it. `TOKENADE_SCOUT_THINK=0` never thinks; `=always` thinks from
/// attempt 0.
enum ThinkPolicy {
    Never,
    Adaptive,
    Always,
}

fn scout_think() -> ThinkPolicy {
    match std::env::var("TOKENADE_SCOUT_THINK").ok().as_deref() {
        Some("0") => ThinkPolicy::Never,
        Some("always") => ThinkPolicy::Always,
        _ => ThinkPolicy::Adaptive,
    }
}

pub struct Snippet {
    pub file: PathBuf,
    pub start_line: usize, // 1-based
    pub text: String,
}

pub fn cmd_scout(args: &[String]) -> i32 {
    let mut question = String::new();
    let mut kind = "auto".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--kind" if i + 1 < args.len() => {
                kind = args[i + 1].clone();
                i += 2;
            }
            a if !a.starts_with("--") && question.is_empty() => {
                question = a.to_string();
                i += 1;
            }
            _ => i += 1,
        }
    }
    if question.is_empty() {
        eprintln!(
            "usage: tokenade scout \"<question>\"   (any code/web question or broad objective)\n  \
             --kind qa|locate|web|explore forces a specific pipeline (evals/debug); \
             the default auto mode picks and escalates by itself"
        );
        return 2;
    }
    let result = match kind.as_str() {
        "explore" => super::explore::run(&question),
        "auto" => run_auto(&question),
        _ => run(&question, &kind),
    };
    match result {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("scout: {e}");
            1
        }
    }
}

/// The ONE entry point (`--kind auto`, the default): fast single-pass answer
/// first; if it abstains, escalate automatically to the bounded explore loop
/// and synthesize the answer from its verified-fact dossier. The orchestrator
/// never chooses a mode — small questions cost one pass, broad ones scale up.
pub fn run_auto(question: &str) -> Result<Value, String> {
    let t0 = std::time::Instant::now();
    let fast = run(question, "qa")?;
    if fast["abstained"] != json!(true) {
        return Ok(fast);
    }
    let (fi, fo) = (
        fast["tokens"]["in"].as_u64().unwrap_or(0),
        fast["tokens"]["out"].as_u64().unwrap_or(0),
    );
    let explored = super::explore::run(question)?;
    let (xi, xo) = (
        explored["tokens"]["in"].as_u64().unwrap_or(0),
        explored["tokens"]["out"].as_u64().unwrap_or(0),
    );
    // Each verified fact becomes a 1-line snippet carrying its REAL file:line,
    // so the synthesis pass reuses the standard citation verifier unchanged.
    let empty = Vec::new();
    let dossier: Vec<Snippet> = explored["facts"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|f| {
            Some(Snippet {
                file: PathBuf::from(f["file"].as_str()?),
                start_line: f["line"].as_u64()? as usize,
                text: f["text"].as_str()?.to_string(),
            })
        })
        .collect();
    let mut out = if dossier.is_empty() {
        abstain(
            question,
            "auto",
            "exploration found no verified facts",
            0,
            0,
            t0,
        )
    } else {
        run_on_snippets(question, "qa", dossier, t0)?
    };
    out["kind"] = json!("auto");
    out["escalated"] = json!(true);
    out["explore"] = json!({
        "complete": explored["complete"],
        "stop_reason": explored["stop_reason"],
        "rubric": explored["rubric"],
        "facts": explored["facts"],
        "open_questions": explored["open_questions"],
    });
    let (si, so) = (
        out["tokens"]["in"].as_u64().unwrap_or(0),
        out["tokens"]["out"].as_u64().unwrap_or(0),
    );
    out["tokens"] = json!({"in": fi + xi + si, "out": fo + xo + so});
    out["wall_ms"] = json!(t0.elapsed().as_millis() as u64);
    Ok(out)
}

pub fn run(question: &str, kind: &str) -> Result<Value, String> {
    let t0 = std::time::Instant::now();
    let snippets = if kind == "web" {
        gather_web_evidence(question)?
    } else {
        gather_evidence(question)?
    };
    run_on_snippets(question, kind, snippets, t0)
}

fn run_on_snippets(
    question: &str,
    kind: &str,
    snippets: Vec<Snippet>,
    t0: std::time::Instant,
) -> Result<Value, String> {
    if snippets.is_empty() {
        return Ok(abstain(question, kind, "no evidence found", 0, 0, t0));
    }
    let evidence = pack(&snippets);
    let debug = std::env::var("TOKENADE_SCOUT_DEBUG").ok().as_deref() == Some("1");
    if debug {
        eprintln!(
            "=== evidence ({} snippets, {} chars) ===\n{evidence}",
            snippets.len(),
            evidence.len()
        );
    }
    let sys = system_prompt(kind);
    let user = format!("Evidence:\n{evidence}\nQuestion: {question}");
    let mut messages = vec![
        json!({"role": "system", "content": sys}),
        json!({"role": "user", "content": user}),
    ];
    let (mut tin, mut tout) = (0u64, 0u64);
    let policy = scout_think();

    for attempt in 0..2 {
        let think = match policy {
            ThinkPolicy::Never => false,
            ThinkPolicy::Always => true,
            ThinkPolicy::Adaptive => attempt > 0,
        };
        // Thinking needs headroom (the block precedes the JSON) + looser sampling.
        let (temp, gen_tokens) = if think {
            (
                knob_f("THINK_TEMP", THINK_TEMP),
                knob("THINK_GEN", THINK_GEN_TOKENS),
            )
        } else {
            (
                knob_f("FAST_TEMP", FAST_TEMP),
                knob("GEN_TOKENS", GEN_TOKENS),
            )
        };
        let (mut text, i, o) = local::complete_structured(
            &Value::Array(messages.clone()),
            &grammar_qa(think),
            temp,
            gen_tokens,
            think,
        )?;
        tin += i;
        tout += o;
        if debug {
            eprintln!("=== attempt {attempt} raw output ===\n{text}");
        }
        if let Some(pos) = text.rfind("</think>") {
            text = text[pos + "</think>".len()..].trim_start().to_string();
        }
        // The grammar guarantees a valid PREFIX, but the generation can still
        // hit max_tokens mid-JSON (e.g. the model copied a huge quote). That's
        // a model behaviour to correct, not a harness bug: retry once asking
        // for concision, then abstain.
        let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
            if attempt == 0 {
                messages.push(json!({"role": "assistant", "content": text}));
                messages.push(json!({"role": "user", "content":
                    "Your output was cut off before the JSON closed. Re-answer \
                     CONCISELY: short answer, at most 2 citations, each quote at \
                     most ~15 words."}));
                continue;
            }
            return Ok(abstain(
                question,
                kind,
                "output truncated twice",
                tin,
                tout,
                t0,
            ));
        };
        let (verified, mut failed) = verify_citations(&parsed, &snippets);
        let answer_text = parsed["answer"].as_str().unwrap_or("");
        // Ground hard tokens against the WHOLE evidence pack. Measured on a
        // 1.7B: grounding against only the cited lines (±2) is too strict —
        // under retry pressure the model mangles its answer to satisfy the
        // checker instead of fixing the citation (accuracy 0.60 → 0.27).
        // Evidence-wide grounding still kills pure inventions, which is the
        // failure mode that actually matters.
        for tok in ground_answer(answer_text, &evidence, question) {
            failed.push(format!(
                "answer mentions {tok:?} which appears nowhere in the evidence"
            ));
        }
        if failed.is_empty() && !verified.is_empty() {
            return Ok(json!({
                "kind": kind,
                "question": question,
                "answer": parsed["answer"],
                "confidence": parsed["confidence"],
                "citations": verified,
                "verified": true,
                "abstained": false,
                "tokens": {"in": tin, "out": tout},
                "wall_ms": t0.elapsed().as_millis() as u64,
            }));
        }
        // Model signalled insufficient evidence (no citations at all). On the
        // fast path that may just mean it needed to reason — escalate to the
        // thinking retry before giving up.
        if verified.is_empty() && failed.is_empty() {
            if attempt == 0 {
                messages.push(json!({"role": "assistant", "content": text}));
                messages.push(json!({"role": "user", "content":
                    "Reconsider carefully: re-read every evidence snippet before \
                     concluding the evidence is insufficient."}));
                continue;
            }
            return Ok(abstain(
                question,
                kind,
                "model reported insufficient evidence",
                tin,
                tout,
                t0,
            ));
        }
        if attempt == 0 {
            // One retry with precise feedback: which quotes are not verbatim.
            messages.push(json!({"role": "assistant", "content": text}));
            messages.push(json!({"role": "user", "content": format!(
                "Problems with your answer: {}. Re-answer using only facts and \
                 line numbers visible in the evidence. If the evidence cannot \
                 support an answer, return empty citations and confidence \"low\".",
                failed.join(" | ")
            )}));
        } else {
            // Retry failed too — abstain rather than return unverified claims.
            return Ok(abstain(
                question,
                kind,
                "citations could not be verified",
                tin,
                tout,
                t0,
            ));
        }
    }
    unreachable!("loop returns on attempt 1")
}

fn abstain(
    question: &str,
    kind: &str,
    why: &str,
    tin: u64,
    tout: u64,
    t0: std::time::Instant,
) -> Value {
    json!({
        "kind": kind,
        "question": question,
        "answer": Value::Null,
        "confidence": "low",
        "citations": [],
        "verified": false,
        "abstained": true,
        "abstain_reason": why,
        "tokens": {"in": tin, "out": tout},
        "wall_ms": t0.elapsed().as_millis() as u64,
    })
}

fn system_prompt(kind: &str) -> String {
    let goal = match kind {
        "locate" => "Identify the exact location(s) in the codebase that answer the question.",
        "web" => "Answer the question using the fetched web pages.",
        _ => "Answer the question about this codebase.",
    };
    let subject_line = if subject_check() {
        "First check each snippet actually concerns the SUBJECT of the question: \
         evidence about a different component, tool, or system does not answer \
         the question even when the words look similar — treat it as \
         insufficient evidence. "
    } else {
        ""
    };
    format!(
        "{goal} Use ONLY the evidence snippets provided — never prior knowledge. \
         Evidence lines are numbered `NNN|`. Every claim must cite the line(s) \
         proving it: snippet id + absolute line number. \
         {subject_line}\
         Answer with the CONCRETE fact (the actual value, name, or location), in \
         one or two full sentences — not just the name of a constant. \
         Example: for \"What is the default port?\" with evidence \
         `[2] src/net.rs:40-42` containing `41| const PORT: u16 = 8080;`, answer \
         {{\"answer\": \"The default port is 8080 (const PORT in src/net.rs).\", \
         \"citations\": [{{\"id\": 2, \"line\": 41}}], \"confidence\": \"high\"}}. \
         If the evidence is insufficient to answer, return empty citations and \
         confidence \"low\". \
         Reply as JSON: {{\"answer\": ..., \"citations\": [{{\"id\": N, \"line\": N}}], \
         \"confidence\": \"high|medium|low\"}}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(text: &str) -> Snippet {
        Snippet {
            file: PathBuf::from("a.rs"),
            start_line: 10,
            text: text.into(),
        }
    }

    #[test]
    fn verify_accepts_line_in_range_and_recovers_text() {
        let s = snip("fn parallelism() -> usize {\n    std::env::var(\"X\")\n}");
        let parsed = json!({"citations": [{"id": 0, "line": 11}]});
        let (ok, failed) = verify_citations(&parsed, &[s]);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(ok[0]["line"], 11);
        assert_eq!(ok[0]["text"], "std::env::var(\"X\")");
    }

    #[test]
    fn verify_remaps_crossed_id_when_line_is_unambiguous() {
        let a = snip("const A: u32 = 1;"); // line 10
        let b = Snippet {
            file: PathBuf::from("b.rs"),
            start_line: 50,
            text: "const B: u32 = 2;".into(),
        };
        // Model cited id 0 but line 50 — only snippet 1 owns line 50: remap.
        let parsed = json!({"citations": [{"id": 0, "line": 50}]});
        let (ok, failed) = verify_citations(&parsed, &[a, b]);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(ok[0]["file"], "b.rs");
        assert_eq!(ok[0]["text"], "const B: u32 = 2;");
    }

    #[test]
    fn verify_rejects_blank_cited_line() {
        let s = snip("const N: usize = 2;\n\nfn f() {}"); // line 11 is blank
        let parsed = json!({"citations": [{"id": 0, "line": 11}]});
        let (ok, failed) = verify_citations(&parsed, &[s]);
        assert!(ok.is_empty());
        assert_eq!(failed.len(), 1);
    }

    #[test]
    fn verify_rejects_line_owned_by_no_snippet() {
        let s = snip("const N: usize = 2;"); // one line: 10
        let parsed = json!({"citations": [
            {"id": 0, "line": 42},  // no snippet owns line 42 → reject
            {"id": 7, "line": 10},  // bad id but line 10 unique → heals
        ]});
        let (ok, failed) = verify_citations(&parsed, &[s]);
        assert_eq!(ok.len(), 1);
        assert_eq!(failed.len(), 1);
    }

    #[test]
    fn grounding_rejects_invented_values() {
        let ev = "41| const PORT: u16 = 8080;\n42| const HOST: &str = \"127.0.0.1\";";
        // 8080 grounded, 9090 invented, PORT grounded, MAX_CONN invented.
        let missing = ground_answer(
            "The default is 9090 not 8080 (PORT, MAX_CONN).",
            ev,
            "what port?",
        );
        assert_eq!(missing, vec!["9090".to_string(), "MAX_CONN".to_string()]);
    }

    #[test]
    fn grounding_accepts_tokens_from_question() {
        let missing = ground_answer(
            "TOKENADE_LOCAL_THINK must be 0.",
            "12| some unrelated evidence with TOKENADE_LOCAL_THINK",
            "Which env var disables thinking, TOKENADE_LOCAL_THINK?",
        );
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn hard_token_detection() {
        assert!(is_hard_token("8192"));
        assert!(is_hard_token("127.0.0.1:8919"));
        assert!(is_hard_token("MAX_GEN_TOKENS"));
        assert!(is_hard_token("llama_decode"));
        assert!(is_hard_token("src/net.rs"));
        assert!(is_hard_token("camelCase"));
        assert!(!is_hard_token("default"));
        assert!(!is_hard_token("The"));
        assert!(!is_hard_token("2"));
    }

    #[test]
    fn windows_center_on_keyword_density() {
        let mut lines = vec!["fill(); // padding"; 100];
        lines[50] = "let parallel_workers = env_var(\"TOKENADE_LOCAL_PARALLEL\");";
        let content = lines.join("\n");
        let ws = best_windows(
            &content,
            &["parallel_workers".into(), "tokenade_local_parallel".into()],
            &[1000, 1000],
        );
        assert_eq!(ws.len(), 1);
        assert!(ws[0].text.contains("parallel_workers"));
        assert!(ws[0].start_line <= 51 && 51 <= ws[0].start_line + WINDOW_LINES);
    }

    #[test]
    fn keywords_drop_stopwords() {
        let ks = keywords("What is the default number of worker contexts?");
        assert!(ks.contains(&"worker".to_string()));
        assert!(!ks.contains(&"the".to_string()));
        assert!(!ks.contains(&"what".to_string()));
    }

    #[test]
    fn expand_definitions_adds_missing_const_line() {
        let file = PathBuf::from("x.rs");
        let content = "pub const DEFAULT_MAX_EDGE: u32 = 1024;\n\
                       fn a() {}\nfn b() {}\nfn c() {}\n\
                       // uses [`DEFAULT_MAX_EDGE`] as the fallback\n"
            .to_string();
        // Packed window covers only lines 4-5 (mentions the const, not its def).
        let mut snips = vec![Snippet {
            file: file.clone(),
            start_line: 4,
            text: "fn c() {}\n// uses [`DEFAULT_MAX_EDGE`] as the fallback".into(),
        }];
        let contents = vec![(0usize, &file, content)];
        expand_definitions(&mut snips, &contents, None);
        assert_eq!(snips.len(), 2, "definition snippet appended");
        assert_eq!(snips[1].start_line, 1);
        assert!(snips[1].text.contains("= 1024"));
        // Idempotent: definition now visible → nothing more to add.
        expand_definitions(&mut snips, &contents, None);
        assert_eq!(snips.len(), 2);
    }

    #[test]
    fn symbol_detection() {
        assert!(looks_like_symbol("cmd_agent"));
        assert!(looks_like_symbol("TOKENADE_LOCAL_PARALLEL"));
        assert!(looks_like_symbol("readFile"));
        assert!(!looks_like_symbol("worker"));
        assert!(!looks_like_symbol("the"));
    }
}
