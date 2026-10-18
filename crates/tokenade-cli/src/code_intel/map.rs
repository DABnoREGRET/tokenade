//! **`tokenade map`: the view you read first.**
//!
//! And above all: what we say when there is NOTHING. An empty index, a symbol
//! not found, a semantic search with no result — each has its own line,
//! because silence reads as "there is no code here" when the answer is
//! "nothing has been indexed".

use super::*;

pub(crate) fn cmd_map(rest: &[String]) -> i32 {
    // #631 — `--json` mode for CI: rebuild the same data
    // `structure_map_with_framework` formats as text, but emit as a
    // structured payload (by_dir[], top_files[]). The text path is
    // unchanged. Closes the JSON-flag class sweep started in #613.
    let json_mode = rest.iter().any(|a| a == "--json");
    let out_mode = match parse_output_mode(rest, "map") {
        Ok(m) => m,
        Err(code) => return code,
    };
    // #671 — `tokenade map <path>` previously silently ignored the
    // positional arg and dumped the whole workspace. The user
    // mental model is "show me a structure map of <path>". Until
    // the underlying Index gains a path-filter method, reject the
    // positional with a clear message rather than silently misbehave.
    // Skip the output-mode flag's value token (e.g. the `paths` in
    // `-o paths`) so it isn't mistaken for a stray positional.
    let mut skip_next = false;
    let mut extra_positional: Option<&String> = None;
    for a in rest.iter() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--output-mode" || a == "--format" || a == "-o" {
            skip_next = true;
            continue;
        }
        if a.starts_with('-') {
            // Silent-accept sweep, same class as `execute`'s (#812) and
            // `web`'s: #671 above already refuses a stray POSITIONAL rather
            // than quietly misbehaving, but every flag-shaped argument fell
            // through this `continue` unchecked. `tokenade map --jsn` — one
            // letter short of `--json` — printed the human map and exited 0,
            // so a caller parsing the output got prose where it asked for
            // JSON and nothing said why. `search`, `web` and `execute` all
            // name the bad flag; `map` was the last one that did not.
            //
            // `-o` and its value are consumed above, so they never reach here;
            // they are listed anyway so this stays correct if that order moves.
            const MAP_FLAGS: &[&str] =
                &["--json", "--output-mode", "--format", "-o", "--help", "-h"];
            if !MAP_FLAGS.contains(&a.as_str()) {
                eprintln!("tokenade map: unknown flag '{a}'. Try `tokenade map --help`.");
                return 2;
            }
            continue;
        }
        if extra_positional.is_some() {
            // One subtree per call. A second path silently ignored is the
            // silent-accept defect this file already refuses for flags:
            // the caller asked about two places and was answered about one,
            // with a zero exit code.
            eprintln!(
                "tokenade map: one path at a time; '{a}' is a second positional. \
                 Run `tokenade map` once per subtree."
            );
            return 2;
        }
        extra_positional = Some(a);
    }
    // #671 asked for `tokenade map <path>` and got a refusal that named no
    // alternative but "print the whole workspace" — which is the thing the
    // caller was narrowing away from. The index carries every symbol's file,
    // so the narrowing is a filter, not a new traversal.
    let prefix: Option<String> =
        extra_positional.map(|p| p.trim_start_matches("./").trim_end_matches('/').to_string());
    let r = root();
    let fw = framework::detect(&r);
    let idx = fresh_index();
    // A path spelled from the WORKSPACE ROOT resolves too. The index handed to
    // this command is re-based to the cwd (`fresh_index`), so from
    // `crates/tokenade-cli` the file is stored as `../tokenade-core/src/…` and
    // `map crates/tokenade-core/src` matched nothing — while the refusal it
    // printed said, in the same breath, that paths are taken "from the
    // workspace root". `impact` has accepted both spellings for a while, one
    // command over in this file.
    let prefix = prefix.map(|p| rebase_prefix_if_unmatched(&idx, p, &r));

    if json_mode {
        // Rebuild the same aggregation `structure_map_with_framework`
        // does internally — small, self-contained, no core API change.
        use std::collections::BTreeMap;
        let mut by_dir: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &idx.symbols {
            // The narrowing has to bind on BOTH routes. A `--json` that
            // ignored the path would answer a different question than the
            // text map printed from the same command line.
            if let Some(p) = prefix.as_deref() {
                if !tokenade_core::index::Index::path_is_under(&s.file, p) {
                    continue;
                }
            }
            *by_file.entry(&s.file).or_insert(0) += 1;
            let e = by_dir
                .entry(tokenade_core::index::parent_dir(&s.file).to_string())
                .or_insert((0, 0));
            e.1 += 1;
        }
        for file in by_file.keys() {
            if let Some(e) = by_dir.get_mut(tokenade_core::index::parent_dir(file)) {
                e.0 += 1;
            }
        }
        let by_dir_arr: Vec<serde_json::Value> = by_dir
            .iter()
            .map(|(d, (files, syms))| {
                serde_json::json!({
                    "dir": d,
                    "files": files,
                    "symbols": syms,
                })
            })
            .collect();
        // Same framework-canonical boost the text path applies so
        // CI and humans see the same "top files" ordering.
        let canon = fw.canonical_paths();
        let mut scored: Vec<(&&str, f64)> = by_file
            .iter()
            .map(|(f, n)| {
                let is_canon = canon.iter().any(|p| f.contains(p));
                let score = if is_canon {
                    (*n as f64) * 5.0 + 50.0
                } else {
                    *n as f64
                };
                (f, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let keep = if fw == framework::Framework::Unknown {
            10
        } else {
            30
        };
        scored.truncate(keep);
        let top_files: Vec<serde_json::Value> = scored
            .iter()
            .map(|(f, _)| {
                let syms = by_file.get(**f).copied().unwrap_or(0);
                let canonical = canon.iter().any(|p| f.contains(p));
                serde_json::json!({
                    "file": f,
                    "symbols": syms,
                    "framework_canonical": canonical,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "root": r.display().to_string(),
            "framework": fw.as_str(),
            "under": prefix,
            "files_indexed": match prefix.as_deref() {
                None => idx.files_indexed,
                Some(_) => by_file.len(),
            },
            "symbols": match prefix.as_deref() {
                None => idx.symbols.len(),
                Some(_) => by_dir.values().map(|(_, s)| *s).sum::<usize>(),
            },
            "by_dir": by_dir_arr,
            "top_files": top_files,
        });
        let body = tokenade_core::redact::redact(&serde_json::to_string_pretty(&payload).unwrap());
        // `--json` is the documented escape from the text map's directory cap
        // ("run map from a subdirectory for its detail, or --json for the full
        // list"), and it had no ceiling of its own. MEASURED in production:
        // one call at `/home/username` delivered **669 829 tokens** — 92 % of
        // everything this op has ever handed back, and more context than most
        // sessions can hold. The ledger booked it as a 57 % saving, which it
        // was, and it was still a context bomb.
        //
        // Stash the full document and return an envelope that is STILL VALID
        // JSON, so a script piping this into `jq` keeps a parseable object with
        // the counts it usually wants, plus the ref that reaches the rest.
        // Substituting prose here would have been cheaper and would have broken
        // the one contract `--json` exists to keep.
        let body_tokens = tokenade_core::tokenizer::estimate(&body);
        if body_tokens > tokenade_core::disclose::threshold_tokens() {
            if let Some((hash, path)) = tokenade_core::disclose::stash_for_reference(
                &body,
                &tokenade_core::disclose::SourceHint::Unknown,
            ) {
                let envelope = truncated_map_envelope(
                    &r.display().to_string(),
                    fw.as_str(),
                    idx.files_indexed,
                    idx.symbols.len(),
                    body_tokens,
                    &hash,
                    &path.display().to_string(),
                );
                println!("{envelope}");
                log_code_intel("map (repo)", "structure_map", &envelope);
                return 0;
            }
        }
        println!("{body}");
        log_code_intel("map (repo)", "structure_map", &body);
        return 0;
    }

    // Reduced modes (paths / count) short-circuit the full structure
    // map. `outline` has no distinct meaning for map (the map already
    // IS a signature-level outline) → falls through to Full.
    match out_mode {
        OutputMode::Paths => {
            use std::collections::BTreeMap;
            let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
            for s in &idx.symbols {
                *by_file.entry(&s.file).or_insert(0) += 1;
            }
            let mut v: Vec<(&str, usize)> = by_file.into_iter().collect();
            // Densest files first; stable tiebreak on path.
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            let mut body = String::new();
            for (f, _) in v.iter().take(30) {
                body.push_str(f);
                body.push('\n');
            }
            // Same rule as `query`: a list that stops at the cap without saying
            // so reads as the whole repo.
            if v.len() > 30 {
                body.push_str(&format!(
                    "[30 of {} files shown — densest first]\n",
                    v.len()
                ));
            }
            let body = tokenade_core::redact::redact(&body);
            print!("{body}");
            log_code_intel("map (repo)", "structure_map", &body);
            return 0;
        }
        OutputMode::Count => {
            let body = format!(
                "files: {}, symbols: {}\n",
                idx.files_indexed,
                idx.symbols.len()
            );
            print!("{body}");
            log_code_intel("map (repo)", "structure_map", &body);
            return 0;
        }
        OutputMode::Full | OutputMode::Outline => {}
    }

    let body = tokenade_core::redact::redact(&idx.structure_map_under(fw, prefix.as_deref()));
    println!("{body}");
    note_if_nothing_indexed(idx.files_indexed);
    log_code_intel("map (repo)", "structure_map", &body);
    0
}

/// Say so when there was nothing to look in.
///
/// Graceful degradation. With no indexable source, `map` is a dead end: the
/// agent otherwise improvises a defensive find+Read fallback — the cold-start
/// regression root-caused on data/log tasks (control has no scaffold, so it
/// never makes the dead call). A non-imperative stderr advisory points it at
/// the right primitives without manufacturing doubt (the Lever-E lesson). Cost
/// is bounded to the already-losing empty case; stdout/exit and the logged body
/// are unchanged.
///
/// Shared by `map`, `query` and `semantic` rather than inlined, and that is the
/// whole point of it existing: MEASURED 2026-08-08, only `map` said this.
/// `tokenade query fetch_detailed` run one directory outside the repo answered
/// `no symbol matching 'fetch_detailed'` — a claim about the CODE, false, and
/// byte-identical to a genuine miss. An agent in the wrong cwd is told the
/// symbol does not exist and has no way to tell the difference. `semantic` had
/// the same shape (`no match for '…'`).
/// The `no symbol matching` line, in ONE place, with the nearest indexed
/// names when there are any.
///
/// Both renderings of `query` printed the bare refusal, and a refusal whose
/// only named way out is nothing at all is the shape this repo keeps fixing.
/// Here it is also the command's most common
/// non-answer: MEASURED 2026-08-31, 28 of 196 real `tokenade query` calls came
/// back empty, against 2% for `skeleton` and `impact`, and 247 of the 425
/// code-intel calls ran a `grep` in the SAME command — the caller hedging
/// against this answer and paying for both.
pub(crate) fn miss_line(q: &str, idx: &tokenade_core::index::Index) -> String {
    let near = idx.nearest_symbols(q, 3);
    if near.is_empty() {
        // Nothing close either, so the answer carries no way forward at all.
        // Say what was searched — the size is what separates "this name is not
        // here" from "you are standing outside the project" (measured: a path
        // spelled from the root resolved to nothing from a subdirectory) —
        // and name the verb that searches by MEANING, which is what a caller
        // who has only a guess at the name actually needs. `map` has named
        // its corpus size on a miss for a while; this is the same line, on
        // the command whose miss is measured at 28 of 196 calls.
        return format!(
            "no symbol matching '{q}' ({} symbols in {} indexed file(s) searched). \
             Nothing is close to that name either — `tokenade semantic \"{q}\"` searches by \
             meaning rather than by name.",
            idx.symbols.len(),
            idx.files_indexed
        );
    }
    format!(
        "no symbol matching '{q}' — closest indexed: {}",
        near.join(", ")
    )
}

/// The same miss, for a renderer that was handed no index.
///
/// It cannot name the corpus — there is nothing to count — and that absence is
/// itself worth saying, because a bare `no symbol matching` is exactly the
/// answer a caller cannot tell apart from standing outside the project. Kept
/// HERE rather than at the call site: a sweep asserts every spelling of this
/// sentence lives in this module, and the fallback in the renderer was the
/// third spelling the declarant was written to prevent.
pub(crate) fn miss_line_no_index(q: &str) -> String {
    format!("no symbol matching '{q}' (no index was available to say what was searched)")
}

/// The `semantic` miss, in ONE place, for the same reason as [`miss_line`].
///
/// The declarant above says it is "shared by `map`, `query` and `semantic`",
/// and `semantic` printed its own literal instead — `no match for '…'`, with
/// nothing about the corpus and no way out. Measured 2026-09-04 on this
/// repository: a three-nonsense-word query answered that one line and nothing
/// else, against 1 680 indexed files. (The words themselves are deliberately
/// NOT written here: a probe string spelled out in the source becomes a match
/// for itself the moment the index is refreshed, and the miss stops being a
/// miss.)
///
/// The way out is deliberately NOT `--limit`: a query that matched nothing was
/// cut by relevance, not by depth, and raising the depth changes nothing (see
/// the depth footer, which refuses to appear for the same reason).
pub(crate) fn semantic_miss_line(q: &str, files_indexed: usize) -> String {
    format!(
        "no match for '{q}' ({files_indexed} indexed file(s) searched). \
         `grep -rn '{q}'` finds it if the words are literally in the code; \
         `tokenade query <name>` finds a symbol by name."
    )
}

pub(crate) fn note_if_nothing_indexed(files_indexed: usize) {
    if files_indexed != 0 {
        return;
    }
    eprintln!(
        "note: no indexable source code found here — for data/log/text tasks, \
         grep/find/Read work directly; `tokenade map`/`skeleton`/`query` only help in a code repo."
    );
}

/// Append a code-intelligence record to the global gain ledger.
///
/// Code-intel tools can't measure savings directly — the agent's
/// would-be Read+Grep sequence never ran. We apply the published
/// CodeGraph 0.9.4 ratio (57 % avg fewer tokens) via
/// [`tokenade_core::code_intel_heuristic::estimated_baseline`] so the
/// dashboard surfaces a defensible estimate. See that module for the
/// citation and the limits of the heuristic.
/// The envelope `map --json` returns in place of a document too large to hand
/// back whole. Split out of the command so its SHAPE can be asserted without
/// spawning a binary or indexing a tree.
///
/// It must stay valid JSON carrying the counts: `--json` exists so a script can
/// parse it, and a caller that only wanted `files_indexed` must not have to
/// follow the ref to get it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn truncated_map_envelope(
    root: &str,
    framework: &str,
    files_indexed: usize,
    symbols: usize,
    approx_tokens: usize,
    hash: &str,
    path: &str,
) -> String {
    let v = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": root,
        "framework": framework,
        "files_indexed": files_indexed,
        "symbols": symbols,
        "truncated": true,
        "approx_tokens": approx_tokens,
        "full_output_ref": hash,
        "full_output_path": path,
        "hint": format!(
            "by_dir and top_files omitted (~{approx_tokens} tokens). \
             Whole document → `tokenade expand-ref {hash} --raw`; \
             a question over it → `--prompt \"…\"`; \
             or run map from a subdirectory for its detail."
        ),
    });
    serde_json::to_string_pretty(&v).unwrap()
}

pub(crate) fn log_code_intel(label: &str, op: &str, response: &str) {
    use tokenade_core::code_intel_heuristic;
    let response_tokens = tokenade_core::tokenizer::estimate(response);
    let baseline = code_intel_heuristic::estimated_baseline(response_tokens as u64) as usize;
    // Double-count audit: this baseline is FABRICATED (x2.33 published
    // ratio, no real bytes avoided) yet landed in the EXACT ledger with
    // no marker — every downstream consumer (server ingest included) had
    // to re-implement the op-name list to keep it out of measured sums.
    // Tag the class at the source; op names and dashboard behavior are
    // unchanged.
    tokenade_core::gain::record_classed(
        "cli",
        op,
        label,
        "ci_heuristic",
        Savings::new(baseline, response_tokens),
    );
}
