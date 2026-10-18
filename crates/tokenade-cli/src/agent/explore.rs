//! `scout explore` — bounded exploration loop with a separate judge.
//! One entry point for ANY code exploration:
//! the harness owns the control flow; the SLM makes three bounded,
//! grammar-constrained choices per turn (plan an action, extract facts, judge
//! completion). State is a compact dossier of VERIFIED facts — never a growing
//! chat history. Hard guards: turn budget, action dedup, no-progress cutoff.

use super::local;
use super::scout::{self, Snippet};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

const MAX_TURNS_DEFAULT: usize = 8;
const NO_PROGRESS_DEFAULT: usize = 3;

fn no_progress_limit() -> usize {
    std::env::var("TOKENADE_SCOUT_NO_PROGRESS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| (1..=8).contains(&v))
        .unwrap_or(NO_PROGRESS_DEFAULT)
}
const MAX_FACTS: usize = 40;
const MAX_OPEN_QUESTIONS: usize = 6;
const PLAN_GEN: usize = 220;
const EXTRACT_GEN: usize = 900;
const JUDGE_GEN: usize = 400;

fn max_turns() -> usize {
    std::env::var("TOKENADE_SCOUT_MAX_TURNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| (1..=24).contains(&v))
        .unwrap_or(MAX_TURNS_DEFAULT)
}

/// One verified fact in the dossier.
struct Fact {
    claim: String,
    file: String,
    line: usize,
    text: String,
}

/// The closed tool menu. The planner grammar can emit exactly these, nothing
/// else — "generate new tool calls" without an open action space.
#[derive(Debug, PartialEq)]
enum Action {
    Semantic { query: String },
    Query { symbol: String },
    Skeleton { file: String },
    Window { file: String, line: usize },
    Impact { file: String },
    Done,
}

/// Planner grammar: {"tool": "...", "arg": "...", "line": N}. `line` only
/// matters for window; harmless elsewhere. Keeping ONE fixed shape (instead of
/// per-tool schemas) keeps the grammar small and the 1.7B reliable.
const GRAMMAR_PLAN: &str = r#"
root ::= "{" ws "\"tool\"" ws ":" ws tool ws "," ws "\"arg\"" ws ":" ws string ws "," ws "\"line\"" ws ":" ws int ws "}"
tool ::= "\"semantic\"" | "\"query\"" | "\"skeleton\"" | "\"window\"" | "\"impact\"" | "\"done\""
int ::= [0-9]+
string ::= "\"" schar* "\""
schar ::= [^"\\\x00-\x1f] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
ws ::= [ \t\n]*
"#;

/// Extractor grammar: facts tied to (id, line) of the CURRENT turn's numbered
/// output — same designate-don't-copy contract as scout qa.
const GRAMMAR_EXTRACT: &str = r#"
root ::= "{" ws "\"facts\"" ws ":" ws facts ws "}"
facts ::= "[" ws "]" | "[" ws fact (ws "," ws fact)* ws "]"
fact ::= "{" ws "\"claim\"" ws ":" ws string ws "," ws "\"id\"" ws ":" ws int ws "," ws "\"line\"" ws ":" ws int ws "}"
int ::= [0-9]+
string ::= "\"" schar* "\""
schar ::= [^"\\\x00-\x1f] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
ws ::= [ \t\n]*
"#;

/// Rubric grammar: the objective decomposed into concrete sub-questions.
const GRAMMAR_RUBRIC: &str = r#"
root ::= "{" ws "\"subs\"" ws ":" ws "[" ws string (ws "," ws string)* ws "]" ws "}"
string ::= "\"" schar* "\""
schar ::= [^"\\\x00-\x1f] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
ws ::= [ \t\n]*
"#;

/// Judge grammar v2: per-rubric-item coverage verdicts. The HARNESS derives
/// done/continue from them (all items covered → done) — the model can no
/// longer wave the whole objective through with one lenient "done".
const GRAMMAR_JUDGE: &str = r#"
root ::= "{" ws "\"items\"" ws ":" ws "[" ws item (ws "," ws item)* ws "]" ws "}"
item ::= "{" ws "\"i\"" ws ":" ws int ws "," ws "\"covered\"" ws ":" ws bool ws "}"
bool ::= "true" | "false"
int ::= [0-9]+
string ::= "\"" schar* "\""
schar ::= [^"\\\x00-\x1f] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
ws ::= [ \t\n]*
"#;

/// Decompose the objective into a coverage rubric (2-6 concrete sub-questions).
/// The rubric is FROZEN for the whole run: the judge scores against it every
/// turn, and "complete" means every item is covered — the completeness bar is
/// set once, up front, not re-negotiated by the model each turn. Falls back to
/// a single-item rubric (the objective itself) if the call fails.
fn decompose(objective: &str) -> Result<(Vec<String>, u64, u64), String> {
    let prompt = json!([
        {"role": "system", "content":
            "Break the exploration objective into the concrete sub-questions \
             that must ALL be answered for the objective to be fully covered. \
             2 to 6 items, each specific and independently checkable. \
             If the objective already names its parts (e.g. 'X, Y, and when Z'), \
             use exactly those parts. \
             Reply as JSON: {\"subs\": [\"...\", ...]}."},
        {"role": "user", "content": objective},
    ]);
    let (text, i, o) = local::complete_structured(&prompt, GRAMMAR_RUBRIC, 0.2, 300, false)?;
    let subs: Vec<String> = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v["subs"].as_array().map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::trim).filter(|s| s.len() >= 8))
                    .map(str::to_string)
                    .take(6)
                    .collect()
            })
        })
        .unwrap_or_default();
    Ok((
        if subs.is_empty() {
            vec![objective.to_string()]
        } else {
            subs
        },
        i,
        o,
    ))
}

pub fn run(objective: &str) -> Result<Value, String> {
    let t0 = std::time::Instant::now();
    let intel = scout::load_intel()?;
    let mut facts: Vec<Fact> = Vec::new();
    let mut done_actions: BTreeSet<String> = BTreeSet::new();
    let mut turns_without_progress = 0usize;
    let mut force_sweep = false;
    let (mut tin, mut tout) = (0u64, 0u64);
    let mut stop_reason = "turn budget exhausted";
    let debug = std::env::var("TOKENADE_SCOUT_DEBUG").ok().as_deref() == Some("1");

    // Coverage rubric, frozen for the run. `covered` is judge-maintained;
    // `open` (the planner's worklist) = the uncovered items.
    let (rubric, ri, ro) = decompose(objective)?;
    tin += ri;
    tout += ro;
    let mut covered = vec![false; rubric.len()];
    let mut open: Vec<String> = rubric.clone();
    if debug {
        eprintln!("[explore] rubric: {rubric:?}");
    }

    for turn in 0..max_turns() {
        // -- 1. PLAN: pick the next bounded action --------------------------
        // Turn 0 is deterministic: every exploration starts with a semantic
        // sweep of the objective itself (measured: a cold 1.7B planner picks
        // degenerate symbol queries like `query tokenade` before it has seen
        // any evidence). The SLM starts planning once a dossier exists.
        let mut action = if turn == 0 || force_sweep {
            // Forced sweep: turn 0, or the previous turn produced nothing new —
            // rather than let the planner retry its own idea, sweep the first
            // uncovered rubric item that hasn't been swept yet (deterministic
            // recovery tied to the coverage worklist).
            force_sweep = false;
            open.iter()
                .chain(std::iter::once(&objective.to_string()))
                .map(|q| Action::Semantic { query: q.clone() })
                .find(|a| !done_actions.contains(&format!("{a:?}")))
        } else {
            let plan_prompt = plan_prompt(objective, &facts, &open, &done_actions);
            let (ptext, i1, o1) =
                local::complete_structured(&plan_prompt, GRAMMAR_PLAN, 0.2, PLAN_GEN, false)?;
            tin += i1;
            tout += o1;
            if debug {
                eprintln!("[explore t{turn}] plan: {ptext}");
            }
            parse_action(&ptext)
        };
        // Invalid or repeated plan → deterministic fallback: a semantic sweep
        // of the first open question not yet swept. The loop keeps moving on
        // harness power; only genuine dead-ends stop it.
        let is_repeat = action
            .as_ref()
            .is_some_and(|a| *a != Action::Done && done_actions.contains(&format!("{a:?}")));
        if action.is_none() || is_repeat {
            action = open
                .iter()
                .map(|q| Action::Semantic { query: q.clone() })
                .find(|a| !done_actions.contains(&format!("{a:?}")));
        }
        let Some(action) = action else {
            turns_without_progress += 1;
            if turns_without_progress >= no_progress_limit() {
                stop_reason = "no progress (no fresh action available)";
                break;
            }
            continue;
        };
        if action == Action::Done {
            stop_reason = "planner done";
            break;
        }
        done_actions.insert(format!("{action:?}"));

        // -- 2. EXECUTE deterministically ----------------------------------
        let snippets = execute(&intel, &action);
        if snippets.is_empty() {
            turns_without_progress += 1;
            if turns_without_progress >= no_progress_limit() {
                stop_reason = "no progress (empty results)";
                break;
            }
            continue;
        }
        let evidence = scout::pack(&snippets);

        // -- 3. EXTRACT verified facts --------------------------------------
        let ex_prompt = json!([
            {"role": "system", "content":
                "Extract NEW facts relevant to the objective from the numbered \
                 evidence. Each fact: one concrete SHORT claim (one sentence) + \
                 the snippet id and absolute line number proving it. Only claims \
                 directly stated by the cited line. AT MOST 5 facts — pick the \
                 most decisive ones. Empty list if nothing relevant."},
            {"role": "user", "content": format!(
                "Objective: {objective}\nAlready known:\n{}\nEvidence:\n{evidence}",
                dossier_brief(&facts)
            )},
        ]);
        let (etext, i2, o2) =
            local::complete_structured(&ex_prompt, GRAMMAR_EXTRACT, 0.2, EXTRACT_GEN, false)?;
        tin += i2;
        tout += o2;
        if debug {
            eprintln!(
                "[explore t{turn}] evidence {} snips / {} chars; extractor: {etext}",
                snippets.len(),
                evidence.len()
            );
        }
        let fresh = verify_facts(&etext, &snippets, &facts);
        if debug {
            eprintln!(
                "[explore t{turn}] {:?} → {} new fact(s)",
                action,
                fresh.len()
            );
        }
        if fresh.is_empty() {
            turns_without_progress += 1;
            force_sweep = true; // recover via the rubric worklist, not the planner
        } else {
            turns_without_progress = 0;
            facts.extend(fresh);
            facts.truncate(MAX_FACTS);
        }
        if turns_without_progress >= no_progress_limit() {
            stop_reason = "no progress (no new verified facts)";
            break;
        }

        // -- 4. JUDGE: score coverage per rubric item; the harness decides ----
        let rubric_list = rubric
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{i}. {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        let judge_prompt = json!([
            {"role": "system", "content":
                "You are a strict reviewer (not the researcher). For EACH \
                 numbered sub-question, decide whether the verified facts FULLY \
                 answer it. covered=true only if a fact states the specific \
                 mechanism, name, or value — a fact merely mentioning the topic \
                 is NOT coverage. When in doubt, covered=false. Judge only on \
                 the facts shown. \
                 Reply as JSON: {\"items\": [{\"i\": N, \"covered\": true|false}, ...]} \
                 with one entry per sub-question."},
            {"role": "user", "content": format!(
                "Sub-questions:\n{rubric_list}\nVerified facts:\n{}",
                dossier_brief(&facts)
            )},
        ]);
        // Judge thinks (adaptive rationale: this is the hard judgment call).
        let (jtext, i3, o3) = local::complete_structured(
            &judge_prompt,
            &format!(
                "root ::= think jroot\nthink ::= \"<think>\" tchar* \"</think>\" ws\n\
                 tchar ::= [^<] | \"<\" [^/]\njroot ::= jbody\n{}",
                GRAMMAR_JUDGE.replacen("root ::=", "jbody ::=", 1)
            ),
            0.6,
            JUDGE_GEN + 600,
            true,
        )?;
        tin += i3;
        tout += o3;
        let jbody = jtext.rsplit("</think>").next().unwrap_or(&jtext).trim();
        if debug {
            eprintln!("[explore t{turn}] judge: {jbody}");
        }
        // Missing/duplicate indices default to uncovered — leniency by
        // omission is impossible. A judged-covered item STAYS covered across
        // turns (facts only accumulate).
        let mut this_turn = vec![false; rubric.len()];
        if let Ok(j) = serde_json::from_str::<Value>(jbody) {
            for it in j["items"].as_array().unwrap_or(&Vec::new()) {
                let i = it["i"].as_u64().unwrap_or(u64::MAX) as usize;
                if i < rubric.len() && it["covered"] == json!(true) {
                    this_turn[i] = true;
                }
            }
        }
        for (c, t) in covered.iter_mut().zip(&this_turn) {
            *c = *c || *t;
        }
        if covered.iter().all(|&c| c) && !facts.is_empty() {
            stop_reason = "judge done";
            break;
        }
        open = rubric
            .iter()
            .zip(&covered)
            .filter(|(_, &c)| !c)
            .map(|(q, _)| q.clone())
            .take(MAX_OPEN_QUESTIONS)
            .collect();
        if open.is_empty() {
            open.push(objective.to_string());
        }
    }

    let complete = stop_reason == "judge done" || stop_reason == "planner done";
    Ok(json!({
        "kind": "explore",
        "objective": objective,
        "complete": complete,
        "stop_reason": stop_reason,
        "rubric": rubric
            .iter()
            .zip(&covered)
            .map(|(q, &c)| json!({"sub_question": q, "covered": c}))
            .collect::<Vec<_>>(),
        "facts": facts.iter().map(|f| json!({
            "claim": f.claim, "file": f.file, "line": f.line, "text": f.text,
        })).collect::<Vec<_>>(),
        "open_questions": if complete { Vec::new() } else { open },
        "tokens": {"in": tin, "out": tout},
        "wall_ms": t0.elapsed().as_millis() as u64,
    }))
}

fn plan_prompt(
    objective: &str,
    facts: &[Fact],
    open: &[String],
    done_actions: &BTreeSet<String>,
) -> Value {
    json!([
        {"role": "system", "content":
            "You plan ONE next exploration step in a codebase. Tools: \
             semantic (arg=natural-language query) — rank files by meaning; \
             query (arg=symbol name) — find a symbol's declarations; \
             skeleton (arg=file path) — list a file's signatures; \
             window (arg=file path, line=N) — read ~40 lines around a line; \
             impact (arg=file path) — list files that depend on it; \
             done — the open questions are answered. \
             Never repeat an action from the done list. Prefer window on a \
             file:line you already saw cited. \
             Reply as JSON: {\"tool\": ..., \"arg\": ..., \"line\": N} (line=0 \
             unless tool=window)."},
        {"role": "user", "content": format!(
            "Objective: {objective}\nOpen questions:\n- {}\nKnown facts:\n{}\nAlready done:\n{}",
            open.join("\n- "),
            dossier_brief(facts),
            done_actions.iter().cloned().collect::<Vec<_>>().join("\n"),
        )},
    ])
}

fn dossier_brief(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return "(none yet)".into();
    }
    facts
        .iter()
        .map(|f| format!("- {} [{}:{}]", f.claim, f.file, f.line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_action(text: &str) -> Option<Action> {
    let v: Value = serde_json::from_str(text).ok()?;
    let arg = v["arg"].as_str().unwrap_or("").trim().to_string();
    let line = v["line"].as_u64().unwrap_or(0) as usize;
    Some(match v["tool"].as_str()? {
        "done" => Action::Done,
        "semantic" if !arg.is_empty() => Action::Semantic { query: arg },
        // Generic lowercase words ("tokenade", "gate") explode into hundreds
        // of substring hits — only accept identifier-shaped symbols.
        "query" if looks_like_ident(&arg) => Action::Query { symbol: arg },
        "skeleton" if !arg.is_empty() => Action::Skeleton { file: arg },
        "window" if !arg.is_empty() && line > 0 => Action::Window { file: arg, line },
        "impact" if !arg.is_empty() => Action::Impact { file: arg },
        _ => return None,
    })
}

/// Identifier-shaped: snake_case, ::path, CamelCase, or dotted/slashed —
/// something the AST index can plausibly hold as a symbol name.
fn looks_like_ident(s: &str) -> bool {
    s.len() >= 4
        && (s.contains('_')
            || s.contains("::")
            || (s.chars().next().is_some_and(|c| c.is_lowercase())
                && s.chars().any(|c| c.is_uppercase()))
            || s.chars().next().is_some_and(|c| c.is_uppercase()))
}

/// Execute one action through the SAME deterministic collectors scout qa uses.
fn execute(intel: &scout::Intel, action: &Action) -> Vec<Snippet> {
    match action {
        Action::Semantic { query } => scout::evidence_for(intel, query),
        Action::Query { symbol } => intel
            .query_hits(symbol)
            .into_iter()
            .take(4)
            .filter_map(|(f, line)| window_snippet(&f, line, 20))
            .collect(),
        Action::Window { file, line } => window_snippet(&resolve(intel, file), *line, 40)
            .into_iter()
            .collect(),
        Action::Skeleton { file } => {
            let p = resolve(intel, file);
            let Ok(text) = std::fs::read_to_string(&p) else {
                return Vec::new();
            };
            let ext = p.extension().and_then(|e| e.to_str());
            let sk = tokenade_core::skeleton::skeleton_ext(&text, ext);
            (!sk.trim().is_empty())
                .then(|| {
                    vec![Snippet {
                        file: p,
                        start_line: 1,
                        text: sk,
                    }]
                })
                .unwrap_or_default()
        }
        Action::Impact { file } => {
            let p = resolve(intel, file);
            let deps = intel.dependents_of(&p);
            (!deps.is_empty())
                .then(|| {
                    vec![Snippet {
                        file: p,
                        start_line: 1,
                        text: deps
                            .iter()
                            .map(|d| format!("dependent: {}", d.display()))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    }]
                })
                .unwrap_or_default()
        }
        Action::Done => Vec::new(),
    }
}

fn resolve(intel: &scout::Intel, file: &str) -> PathBuf {
    let p = PathBuf::from(file);
    if p.exists() {
        p
    } else {
        intel.root().join(file)
    }
}

fn window_snippet(file: &PathBuf, line: usize, half: usize) -> Option<Snippet> {
    let text = std::fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let center = line.clamp(1, lines.len());
    let s = center.saturating_sub(half + 1);
    let e = (center + half).min(lines.len());
    Some(Snippet {
        file: file.clone(),
        start_line: s + 1,
        text: lines[s..e].join("\n"),
    })
}

/// Salvage a generation truncated at max_tokens mid-JSON: cut back to the last
/// COMPLETE fact object and close the brackets. Loses at most the final,
/// half-written fact instead of silently dropping the whole turn (measured:
/// a fact-rich turn hit the 900-token cap and yielded 0 facts).
fn salvage_truncated(etext: &str) -> Option<Value> {
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    let mut last_complete = None;
    for (i, c) in etext.char_indices() {
        if in_str {
            match c {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 2 && c == '}' {
                    last_complete = Some(i); // closed a fact, back inside the array
                }
            }
            _ => {}
        }
    }
    let i = last_complete?;
    serde_json::from_str(&format!("{}]}}", &etext[..=i])).ok()
}

/// Verify extractor output: (id, line) must name a real, non-blank line of
/// this turn's snippets; claims must be new (dedup vs dossier) and pass the
/// hard-token grounding against the snippet text.
fn verify_facts(etext: &str, snippets: &[Snippet], existing: &[Fact]) -> Vec<Fact> {
    let Some(v) = serde_json::from_str::<Value>(etext)
        .ok()
        .or_else(|| salvage_truncated(etext))
    else {
        return Vec::new();
    };
    let evidence: String = snippets.iter().map(|s| s.text.as_str()).collect();
    let mut out = Vec::new();
    let empty = Vec::new();
    for f in v["facts"].as_array().unwrap_or(&empty) {
        let claim = f["claim"].as_str().unwrap_or("").trim().to_string();
        let id = f["id"].as_u64().unwrap_or(u64::MAX) as usize;
        let line = f["line"].as_u64().unwrap_or(0) as usize;
        if claim.len() < 8 {
            continue;
        }
        let Some(snip) = snippets
            .get(id)
            .filter(|s| line >= s.start_line && line < s.start_line + s.text.lines().count())
        else {
            continue;
        };
        let text = snip
            .text
            .lines()
            .nth(line - snip.start_line)
            .unwrap_or_default();
        if text.trim().len() < 3 {
            continue;
        }
        if !scout::ground_answer(&claim, &evidence, "").is_empty() {
            continue; // claim mentions values not present anywhere in evidence
        }
        // Anti-vague-filler: the claim must lexically share something with the
        // line it cites. Hard-token grounding can't catch soft hallucination
        // ("through a signal or completion channel" citing an unrelated line)
        // because vague prose contains no hard tokens; requiring content-word
        // overlap with the cited line ties every claim to its proof.
        if !claim_matches_line(&claim, text) {
            continue;
        }
        // Near-duplicate filter: exact-match dedup let paraphrases through
        // ("begins with the orchestrator setting limits…" ×2 with one word
        // changed), padding the dossier without informing the judge.
        let dup = existing
            .iter()
            .chain(out.iter())
            .any(|e| e.claim == claim || claims_similar(&e.claim, &claim));
        if dup {
            continue;
        }
        out.push(Fact {
            claim,
            file: snip.file.display().to_string(),
            line,
            text: text.trim().to_string(),
        });
    }
    out
}

/// Two claims are near-duplicates when ≥80% of the shorter one's content
/// words appear in the other.
fn claims_similar(a: &str, b: &str) -> bool {
    let words = |s: &str| -> BTreeSet<String> {
        s.split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_lowercase)
            .filter(|w| w.len() >= 4)
            .collect()
    };
    let (wa, wb) = (words(a), words(b));
    let small = wa.len().min(wb.len());
    if small == 0 {
        return false;
    }
    wa.intersection(&wb).count() * 100 >= small * 80
}

/// A claim and its cited line must share ≥1 content word (len≥4, not a
/// code/prose stopword) or a hard token. Split on non-alphanumerics so
/// snake_case/CamelCase fragments count.
fn claim_matches_line(claim: &str, line: &str) -> bool {
    const STOP: &[&str] = &[
        "this", "that", "with", "from", "into", "when", "where", "which", "have", "does", "uses",
        "used", "using", "value", "values", "default", "defaults", "const", "static", "return",
        "returns", "true", "false", "self", "then",
    ];
    let words = |s: &str| -> BTreeSet<String> {
        s.split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_lowercase)
            .filter(|w| w.len() >= 4 && !STOP.contains(&w.as_str()))
            .collect()
    };
    !words(claim).is_disjoint(&words(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_covers_menu_and_rejects_garbage() {
        assert_eq!(
            parse_action(r#"{"tool":"semantic","arg":"auth flow","line":0}"#),
            Some(Action::Semantic {
                query: "auth flow".into()
            })
        );
        assert_eq!(
            parse_action(r#"{"tool":"window","arg":"src/a.rs","line":42}"#),
            Some(Action::Window {
                file: "src/a.rs".into(),
                line: 42
            })
        );
        assert_eq!(
            parse_action(r#"{"tool":"done","arg":"","line":0}"#),
            Some(Action::Done)
        );
        // window without a line and empty args are invalid, not guessed at.
        assert_eq!(
            parse_action(r#"{"tool":"window","arg":"src/a.rs","line":0}"#),
            None
        );
        assert_eq!(
            parse_action(r#"{"tool":"semantic","arg":"","line":0}"#),
            None
        );
    }

    #[test]
    fn verify_facts_checks_line_blank_grounding_and_dedup() {
        let snips = vec![Snippet {
            file: PathBuf::from("a.rs"),
            start_line: 10,
            text: "const POOL_SIZE: usize = 2;\n\nfn f() {}".into(),
        }];
        let etext = r#"{"facts":[
            {"claim":"POOL_SIZE defaults to 2","id":0,"line":10},
            {"claim":"POOL_SIZE defaults to 2","id":0,"line":10},
            {"claim":"blank line proves nothing at all","id":0,"line":11},
            {"claim":"MAX_CONN defaults to 99","id":0,"line":10},
            {"claim":"something out of range","id":0,"line":99}
        ]}"#;
        let facts = verify_facts(etext, &snips, &[]);
        assert_eq!(
            facts.len(),
            1,
            "dedup + blank + grounding(99) + range all filtered"
        );
        assert_eq!(facts[0].claim, "POOL_SIZE defaults to 2");
        assert_eq!(facts[0].text, "const POOL_SIZE: usize = 2;");
    }

    #[test]
    fn near_duplicate_claims_are_detected() {
        assert!(claims_similar(
            "The lifecycle of a light-agent run begins with the orchestator setting limits",
            "The lifecycle of a light-agent run begins with the orchestrator setting limits for a spawn"
        ));
        assert!(!claims_similar(
            "meta.json records the task metadata",
            "transcript.jsonl accumulates per-turn events"
        ));
    }

    #[test]
    fn claim_line_overlap_rejects_vague_filler() {
        // Real failure: vague claim citing an unrelated line passed grounding.
        assert!(!claim_matches_line(
            "The orchestrator collects the result through a signal or completion channel",
            r#".or_else(|| e["kind"].as_str())"#
        ));
        // Legit claims share content words (incl. snake_case fragments).
        assert!(claim_matches_line(
            "The gate falls back to the native tool if every engine misses",
            "/// (rotate to the next), and native fallback if every engine misses."
        ));
        assert!(claim_matches_line(
            "WEBSEARCH_CB_MAX_FAILS is set to 2",
            "pub(crate) const WEBSEARCH_CB_MAX_FAILS: u32 = 2;"
        ));
    }

    #[test]
    fn salvage_recovers_complete_facts_from_truncated_json() {
        let truncated = r#"{"facts": [
            {"claim": "the pool size equals one", "id": 0, "line": 10},
            {"claim": "the retry count equals two", "id": 0, "line": 11},
            {"claim": "the timeout is trunc"#;
        let snips = vec![Snippet {
            file: PathBuf::from("a.rs"),
            start_line: 10,
            text: "pool size equals one here\nretry count equals two here".into(),
        }];
        let facts = verify_facts(truncated, &snips, &[]);
        assert_eq!(
            facts.len(),
            2,
            "two complete facts salvaged, half-written one dropped"
        );
        assert!(
            salvage_truncated("{\"facts\": [").is_none(),
            "nothing complete → None"
        );
    }

    #[test]
    fn window_snippet_clamps_to_file_bounds() {
        // Was a FIXED path shared by every process on the host, never
        // cleaned up. pid keeps concurrent `cargo test` runs apart.
        let dir =
            std::env::temp_dir().join(format!("tk-scout-explore-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("w.txt");
        std::fs::write(&f, "a\nb\nc\nd\ne\n").unwrap();
        let s = window_snippet(&f, 1, 40).unwrap();
        assert_eq!(s.start_line, 1);
        assert_eq!(s.text.lines().count(), 5);
    }
}
