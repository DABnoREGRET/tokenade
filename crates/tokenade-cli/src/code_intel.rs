//! Code-intelligence CLI commands: expand-ref/search-stash/expand-fold +
//! query/impact/map/skeleton. Extracted from main.rs (refactor split).
#![allow(unused_imports)]
mod args;
pub(crate) mod expand_ref;
mod output;
use crate::*;
pub(crate) use args::*;
pub(crate) use expand_ref::*;
pub(crate) use output::*;
mod impact;
mod map;
mod query;
mod skeleton;
pub(crate) use impact::*;
pub(crate) use map::*;
pub(crate) use query::*;
pub(crate) use skeleton::*;

/// Value-flag names to register in the positional validators so the
/// output-mode flag + its value survive argument parsing.
pub(crate) const OUTPUT_MODE_FLAGS: &[&str] = &["--output-mode", "--format", "-o"];

/// `--budget N` — ceiling, in estimated tokens, on what a code-intel command
/// prints. Without it the agent guesses: `skeleton` on a batch of files and
/// `semantic` on a broad question both print whatever they find, and the only
/// way to discover the cost is to have already paid it.
pub(crate) const BUDGET_FLAGS: &[&str] = &["--budget"];

/// Greedy packer for a RANKED list of already-rendered items.
///
/// Greedy is the right algorithm here and an optimal knapsack would be the
/// wrong one: these items arrive sorted by relevance, so "the best subset that
/// fits" is not what the caller wants — reordering by value-per-token would
/// promote a cheap weak hit over an expensive strong one and quietly corrupt
/// the ranking. Take the longest prefix that fits, then say what was cut.
pub(crate) struct TokenBudget {
    limit: Option<usize>,
    used: usize,
    shown: usize,
    degraded: usize,
    dropped: usize,
}

/// One `kind (n): a, b ×3, c, +N more` line.
///
/// Extracted so the cap and the `×N` suffix have a seam a test can reach:
/// mutating either inside the rendering loop left the suite GREEN, because
/// the only tests were on [`collapse_repeats`] and nothing exercised what
/// the caller did with it (the second reading of a green mutation — the
/// fixture never reached the branch).
///
/// `n` stays the RAW declaration count: the file really does declare that
/// many things. The cap counts DISTINCT names, because a repeat spends a
/// slot and says nothing new.
fn render_kind_line(kind: &str, names: &[&str], cap: usize) -> String {
    let n = names.len();
    let names = drop_useless_owners(names);
    let collapsed = collapse_repeats(&names);
    let distinct = collapsed.len();
    let shown = collapsed
        .iter()
        .take(cap)
        .map(|(name, count)| {
            if *count > 1 {
                format!("{name} \u{d7}{count}")
            } else {
                (*name).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let overflow = if distinct > cap {
        format!(", +{} more", distinct - cap)
    } else {
        String::new()
    };
    format!("  {kind} ({n}): {shown}{overflow}\n")
}

/// Strip the owner prefix from every name it does not distinguish.
///
/// The index qualifies a member with what it hangs off, and that is the whole
/// point when a file declares `Timeout` in three structs. It is pure noise
/// when the file declares ONE enum: `variant (9): TokenadeError.Io,
/// TokenadeError.IoBare, …` repeats twelve characters nine times, eats the
/// twelve-name budget and discriminates nothing.
///
/// The rule is the prefix's own justification: keep it exactly where two
/// entries in this group share a bare name and differ once qualified. Four
/// `From` impls all owned by `TokenadeError` produce four IDENTICAL qualified
/// forms, so they stay bare and collapse to `from ×4` as before; six `impl`
/// blocks on the same type carry six different traits, so all six are named.
fn drop_useless_owners<'a>(names: &[&'a str]) -> Vec<&'a str> {
    fn bare(s: &str) -> &str {
        // `Owner.name` and `Trait for Type` both hang the discriminator in
        // front; the tail is the bare form in the first case and the whole
        // string is already distinct in the second.
        s.rsplit_once('.').map(|(_, t)| t).unwrap_or(s)
    }
    let mut qualified_forms: std::collections::HashMap<&str, std::collections::HashSet<&str>> =
        std::collections::HashMap::new();
    for name in names {
        qualified_forms.entry(bare(name)).or_default().insert(name);
    }
    names
        .iter()
        .map(|name| {
            let b = bare(name);
            if qualified_forms.get(b).is_some_and(|set| set.len() > 1) {
                *name
            } else {
                b
            }
        })
        .collect()
}

/// Names with their repeat counts, in order of FIRST appearance.
///
/// A file may declare the same constant in several function bodies, and the
/// index records each one; printing the name three times spends three slots
/// of a twelve-name cap and says nothing new. First-appearance order is what
/// keeps the declaration-order rule the caller relies on.
fn collapse_repeats<'a>(names: &[&'a str]) -> Vec<(&'a str, usize)> {
    let mut out: Vec<(&'a str, usize)> = Vec::new();
    for name in names {
        match out.iter_mut().find(|(n, _)| n == name) {
            Some((_, count)) => *count += 1,
            None => out.push((name, 1)),
        }
    }
    out
}

pub(crate) fn cmd_query(rest: &[String]) -> i32 {
    // #630 — `--json` mode for CI: emit hits[] with structured
    // {kind, name, file, line} so scripts can branch on exact-match
    // count or post-filter by kind without scraping the text.
    // #680 — silent-accept sweep continued: until this iteration
    // `tokenade query foo --bogus` silently dropped `--bogus`, and
    // `tokenade query foo bar` silently dropped `bar`. Shared
    // validator rejects both cases up front.
    // Phase 2 (#931) — `--context N` lets symbol_find double as
    // `grep -n -A N -B N`: surrounding source lines come back inline
    // so the agent doesn't have to follow up with a grep/Read. The
    // value-flag-aware validator is what unlocked the syntax.
    let queries = match collect_positionals_with_values(
        rest,
        "query",
        &["--json"],
        &[
            "--context",
            "-C",
            "--output-mode",
            "--format",
            "-o",
            "--file",
        ],
        "symbol",
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let json_mode = rest.iter().any(|a| a == "--json");
    let context_lines = match parse_context_flag(rest, "query") {
        Ok(n) => n,
        Err(code) => return code,
    };
    let out_mode = match parse_output_mode(rest, "query") {
        Ok(m) => m,
        Err(code) => return code,
    };
    // `--file <path>`, repeatable: answer about ONE file instead of the repo.
    //
    // Measured 2026-09-01 over this machine's transcripts, six real calls
    // spelled it and were refused with "unknown flag '--file'" — a caller
    // reaching for the right idea and told only that it does not exist. The
    // need is real and the numbers say so: `Format` matches 219 symbols here,
    // so an unscoped answer is thirty rows of which the ones you wanted may be
    // none. The scope is applied before the cap, not after; see
    // `Index::find_ranked_in`.
    let scope: Vec<String> = {
        let mut out = Vec::new();
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            if a == "--file" {
                match it.next() {
                    Some(v) => out.push(v.clone()),
                    None => {
                        eprintln!("tokenade query: --file requires a path");
                        return 2;
                    }
                }
            }
        }
        out
    };
    // #936 — empty + 1-char queries: `Index::find` does substring
    // match, so 1-char queries return ~everything containing that
    // letter (junk + context noise). Gate each symbol to ≥2 chars. In
    // single mode a too-short symbol is a hard error (unchanged); in
    // batch mode skip it with a note so one typo doesn't sink the rest.
    let single = queries.len() == 1;
    let mut symbols: Vec<&String> = Vec::new();
    for q in &queries {
        if q.trim().len() < 2 {
            if single {
                eprintln!(
                    "tokenade query: <symbol> must be at least 2 characters \
                     (got {:?}). The index does substring match — single-char \
                     queries return ~every symbol containing that letter. \
                     Try `tokenade map` for a project-wide overview, or pass \
                     a full identifier name.",
                    q
                );
                return 2;
            }
            eprintln!("tokenade query: skipping {q:?} — <symbol> must be at least 2 characters.");
            continue;
        }
        symbols.push(q);
    }
    if symbols.is_empty() {
        return 2;
    }
    let (idx, graph_available) = crate::fresh_index_with_calls();
    // Call-graph centrality, computed ONCE per invocation, ranks hits so the
    // symbol the rest of the codebase leans on floats above incidental matches.
    let cen = idx.centrality();
    let multi = symbols.len() > 1;
    if json_mode {
        // batch json: single symbol keeps the exact legacy shape;
        // multiple symbols nest under `queries` so the output stays
        // valid JSON instead of concatenated objects.
        let mut payloads: Vec<serde_json::Value> = Vec::new();
        for q in &symbols {
            let q = q.as_str();
            let (hits, total) = idx.find_ranked_in(q, 30, &cen, &scope);
            let arr: Vec<serde_json::Value> = hits
                .iter()
                .map(|s| {
                    let mut row = serde_json::json!({
                        "kind": s.kind,
                        "name": s.name,
                        "file": s.file,
                        "line": s.line,
                    });
                    // The payload ALREADY reported `context_lines`, and JSON
                    // mode ignored the flag: a consumer read "2" and got hits
                    // carrying nothing. A field that names a setting the
                    // output did not honour is worse than an absent one —
                    // the same rule, on the side that asserts rather than omits.
                    // Rendered through the same `render_with_context` the text
                    // mode uses, so the two cannot drift.
                    if context_lines > 0 {
                        let block = idx.render_with_context(&[*s], context_lines);
                        row["context"] = serde_json::Value::from(block);
                    }
                    row
                })
                .collect();
            // Through the SAME decision the text path uses. This half kept
            // the typed name after the text half stopped: `query run --json`
            // answered `"callers": []` with `"count": 6` on a tree that
            // spells it `Run`, and an empty array is this function's own
            // documented way of saying "nothing calls this" — a positive
            // claim, made to a machine, contradicting `query Run --json` on
            // the same index.
            let at = attribution_for(&idx, &hits, q, &scope);
            let (callers_arr, callers_total) = callers_json(&idx, &at, graph_available);
            let payload = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "query": q,
                "count": hits.len(),
                "callers": callers_arr,
                "callers_total": callers_total,
                // `count` is what survived the cap; `total` is what matched. A
                // consumer that sees only `count` cannot tell the two apart.
                "total": total,
                "truncated": total > hits.len(),
                "context_lines": context_lines,
                "hits": arr,
            });
            log_code_intel(&format!("query {q}"), "symbol_find", &payload.to_string());
            payloads.push(payload);
        }
        let out = if payloads.len() == 1 {
            serde_json::to_string_pretty(&payloads[0]).unwrap()
        } else {
            serde_json::to_string_pretty(&serde_json::json!({ "queries": payloads })).unwrap()
        };
        // SECURITY: redact secrets from source-bearing output. Idempotent.
        println!("{}", tokenade_core::redact::redact(&out));
        return 0;
    }
    if out_mode != OutputMode::Full {
        // Reduced modes: gather (symbol, hits) then render only the
        // requested slice. Short-circuits the full context render.
        let mut totals: Vec<usize> = Vec::with_capacity(symbols.len());
        let results: Vec<(String, Vec<tokenade_core::index::Symbol>)> = symbols
            .iter()
            .map(|q| {
                let (hits, total) = idx.find_ranked_in(q.as_str(), 30, &cen, &scope);
                totals.push(total);
                (q.to_string(), hits.into_iter().cloned().collect::<Vec<_>>())
            })
            .collect();
        let body = tokenade_core::redact::redact(&output_mode_body_query_with_totals(
            out_mode,
            &results,
            multi,
            &totals,
            Some(&idx),
        ));
        print!("{body}");
        log_code_intel("query (modal)", "symbol_find", &body);
        return 0;
    }
    let mut found: Vec<usize> = Vec::new();
    for (qi, q) in symbols.iter().enumerate() {
        let q = q.as_str();
        let (hits, total) = idx.find_in(q, 30, &scope);
        // head-style `==> name <==` separator only when batching, so a
        // single query's output is byte-identical to before.
        if multi {
            if qi > 0 {
                println!();
            }
            println!("==> {q} <==");
        }
        if hits.is_empty() {
            println!("{}", miss_line(q, &idx));
            note_if_nothing_indexed(idx.files_indexed);
            log_code_intel(&format!("query {q}"), "symbol_find", "");
            continue;
        }
        // SECURITY: source context can carry `const API_KEY="…"`. Redact.
        let body = tokenade_core::redact::redact(&idx.render_with_context(&hits, context_lines));
        if !body.is_empty() {
            // render_with_context emits trailing `\n` per line when
            // context_lines > 0 and joined-without-`\n` for the zero
            // case — print as-is then ensure a final newline.
            print!("{body}");
            // Rendered by `render_with_context`, never a file handed back
            // whole — a view, and the reason this needs a terminator at all.
            if crate::wrap::needs_terminator(&body, false) {
                println!();
            }
        }
        // A capped list that says nothing reads as the complete set. `query`
        // is what agents are told to run INSTEAD of reading files, so silence
        // here is an answer that is wrong without looking wrong.
        if total > hits.len() {
            print!("{}", truncation_note(hits.len(), total));
        }
        // Counted over the INDEX, not over `hits` — `hits` is capped at 30,
        // so counting exact matches in it reports the display window as the
        // ambiguity. The two numbers then contradicted each other in one
        // output: "30 of 972 matches shown" above, "30 declarations share it"
        // below, on a name 928 files declare.
        // The name the index holds, not what was typed — see `attribution_for`.
        let at = attribution_for(&idx, &hits, q, &scope);
        let cline = tokenade_core::redact::redact(&callers_line(&idx, &at, graph_available));
        print!("{cline}");
        log_code_intel(&format!("query {q}"), "symbol_find", &body);
        found.push(
            tokenade_core::tokenizer::estimate(&body) + tokenade_core::tokenizer::estimate(&cline),
        );
    }
    // Each symbol found is a `Grep` the agent did not run in a later turn.
    tokenade_core::gain::record_batch_cache_saving("query", "symbols", &found);
    0
}

#[cfg(test)]
include!("code_intel_tests.rs");
