//! `tokenade semantic` — search by MEANING, and the state of its indexes.
//!
//! Moved out of `main.rs`, where it lived between a byte formatter and the
//! stdin reader. One subject: ask one or more natural-language questions of
//! the project's semantic index, return the passages that answer, and say what
//! the indexes contain when asked.

use crate::*;

/// The name the semantic command answers under, in ONE place.
///
/// `semantic` is the primary: `list-commands` prints it, the top-level help
/// documents it as `semantic <query> … (aliases: ask, sem)`,
/// `install::help_text::alias_to_primary` maps `"ask" | "sem" => "semantic"`,
/// and the project rules tokenade scaffolds into `CLAUDE.md` teach it. Every
/// message used to open with `tokenade ask:` instead — the alias, the one
/// spelling nobody typed once in 60 days of transcripts while `semantic` was
/// typed 26 times. Six messages spelled the name by hand, across two functions;
/// one constant is what stops the seventh from drifting again.
const SEMANTIC_CMD: &str = "semantic";

pub(crate) fn cmd_semantic(rest: &[String]) -> i32 {
    if rest.is_empty() {
        eprintln!(
            "usage: tokenade {SEMANTIC_CMD} <natural-language query...>\n  \
             several QUOTED questions run against one index build: \
             `tokenade {SEMANTIC_CMD} \"where is auth\" \"how are errors logged\"`"
        );
        return 2;
    }
    let out_mode = match crate::code_intel::parse_output_mode(rest, SEMANTIC_CMD) {
        Ok(m) => m,
        Err(code) => return code,
    };
    // Strip the output-mode flag + its value so they never leak into
    // the natural-language query (`ask -o paths where is auth` must
    // search "where is auth", not "-o paths where is auth").
    // The hand-rolled version of this loop stripped `--output-mode/--format/-o`
    // and `--budget`, with a comment stating the principle exactly: leaving a
    // flag in makes `ask --budget 500 where is auth` SEARCH for the words
    // "--budget 500 where is auth". `--limit`/`-n` was added later (see
    // DEFAULT_SEMANTIC_LIMIT below) and never joined that list, so
    // `ask "where is auth" --limit 5` searched for "where is auth --limit 5":
    // the flag worked and quietly changed the question.
    //
    // Reusing the shared validator instead of extending the list is the point.
    // It is the same one `query` has used since #680, so it also closes a
    // second hole here — `ask foo --bogus` used to fold the typo into the
    // query — and it knows the `--flag=value` form (#944). One declaration,
    // no second list to keep in step.
    let q_parts = match collect_positionals_with_values(
        rest,
        SEMANTIC_CMD,
        &[],
        &[
            "--output-mode",
            "--format",
            "-o",
            "--budget",
            "--limit",
            "-n",
        ],
        "query",
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // The empty/whitespace query is refused ONCE, by the shared collector
    // above: #946 trim-rejects every whitespace-only positional and returns
    // Err when there is no positional at all, so `q_parts` cannot join to an
    // empty string here. The second check that used to stand at this point
    // (#938, written before #946) could not fire, and it named the alias.
    let r = root();
    // Auto-refresh: keep the symbol index incrementally up-to-date
    // (cheap when nothing changed), then keep the semantic index in
    // step. The user never has to discover `tokenade index` — search
    // paths are warm by construction.
    let idx = match index::Index::ensure_fresh(&r) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tokenade {SEMANTIC_CMD}: index build failed: {e}");
            return 1;
        }
    };
    let sem = match semantic::SemanticIndex::load(&r) {
        Some(prev) if !prev.mtimes.is_empty() => {
            let s = semantic::SemanticIndex::rebuild_incremental(&prev, &idx, &r);
            let _ = s.save(&r);
            s
        }
        _ => {
            let s = semantic::SemanticIndex::build_from_index(&idx, &r);
            let _ = s.save(&r);
            s
        }
    };
    // Re-base AFTER every save above: `semantic` names files for the reader to
    // open, exactly like `query`, so its paths owe the same base — but the
    // store must keep root-relative ones. `SemanticIndex::save` refuses a
    // re-based index, so the order here is belt and braces rather than the
    // only thing holding it.
    let mut sem = sem;
    let cwd = env::current_dir().unwrap_or_else(|_| r.clone());
    sem.rebase_to(&r, &cwd);
    let sem = sem;

    // Every positional carries a space, and there are several of them: they
    // can only have been QUOTED separately, because a shell splits an
    // unquoted sentence into single-word arguments. So this is N questions,
    // not one — the batched shape the rest of the CLI teaches (`tokenade web
    // u1 u2`, `tokenade search "q1" "q2"`, `tokenade skeleton a.rs b.rs`).
    // It used to be JOINED into one query, silently: the two answers were
    // blended into ONE ranking, and the better-scoring question pushed the
    // other one off the list. Nothing said so — output was byte-identical to
    // the concatenation.
    //
    // The index, the models and the framework probe load ONCE and every
    // question reuses them, which is the whole reason to batch.
    let queries = split_quoted_questions(&q_parts);
    let fw = framework::detect(&r);
    let multi = queries.len() > 1;
    let mut code = 0;
    for q in &queries {
        if multi {
            println!("\n===== {q} =====");
        }
        let rc = run_one_semantic_query(q, &sem, &idx, fw, rest, out_mode);
        if rc != 0 {
            code = rc;
        }
    }
    code
}

/// How many questions were asked. Every positional carrying a space, and
/// more than one of them, can only come from separate QUOTING: a shell splits
/// an unquoted sentence into single-word arguments, so a space inside one
/// argument is a quote mark that survived. Anything else is one question.
///
/// `tokenade ask "where is auth" "how are errors logged"` used to be joined
/// into `"where is auth how are errors logged"` — one blended ranking, in
/// which the better-scoring question pushes the other off the list, output
/// byte-identical to the concatenation and nothing saying so.
fn split_quoted_questions(parts: &[String]) -> Vec<String> {
    // No `len() >= 2` here, though it was written: for a single argument BOTH
    // branches compute the same value, so no input can tell the two apart and
    // the clause was arithmetic that only looked like a rule. `is_empty` is
    // the one that earns its place — `all` on nothing is TRUE, and returning
    // no questions is not the same answer as returning one empty one.
    if !parts.is_empty() && parts.iter().all(|p| p.contains(' ')) {
        return parts.to_vec();
    }
    vec![parts.join(" ")]
}

/// The two renderings of one ranked hit: whole, and trimmed to what the
/// reader acts on.
///
/// The file path is the answer; the matched terms are the explanation of why
/// it ranked. Under a budget the explanation is the half worth losing, and the
/// SCORE stays in both so a degraded row can still be compared with the one
/// above it.
fn semantic_row_forms(score: f64, file: &str, matched: &[String]) -> (String, String) {
    (
        redact::redact(&format!("{score:5.2}  {file}  [{}]", matched.join(", "))),
        redact::redact(&format!("{score:5.2}  {file}")),
    )
}

/// What actually goes out for one budget verdict.
///
/// A seam a test can reach: mutating this decision inside the rendering loop
/// left the suite green, because nothing exercised what the caller did with
/// the verdict — the third reading of a green mutation, no test at all. Same
/// treatment `render_kind_line` already got for the same reason.
fn semantic_row_to_emit(
    verdict: crate::code_intel::Admission,
    full: String,
    short: String,
) -> Option<String> {
    match verdict {
        crate::code_intel::Admission::Full => Some(full),
        crate::code_intel::Admission::Degraded => Some(short),
        crate::code_intel::Admission::Refused => None,
    }
}

/// One question against an already-loaded index. Split out of `cmd_semantic`
/// so several quoted questions share one index build and one model load.
fn run_one_semantic_query(
    q: &str,
    sem: &semantic::SemanticIndex,
    idx: &index::Index,
    fw: framework::Framework,
    rest: &[String],
    out_mode: crate::code_intel::OutputMode,
) -> i32 {
    // Full hybrid pipeline: BM25 → multilingual-e5-small bi-encoder cosine →
    // BGE-reranker-v2-m3 cross-encoder. On first call, fastembed downloads
    // the two models (~118 MB + ~568 MB). If that download fails (no
    // network), we surface the error to the user — we deliberately do NOT
    // fall back to BM25-only because a one-time blip would otherwise leave
    // every subsequent search permanently degraded.
    // Blend weight by query SHAPE instead of a fixed 0.5. `get_ranked_context`
    // and a bare `SomeStruct::method` want different things from the same
    // pipeline: an identifier is an exact-match problem where BM25 is right and
    // the bi-encoder mostly adds neighbours, while prose is the opposite. This
    // classifier already exists and is proven on `file-search`; `semantic` was
    // the one caller still hardcoding the midpoint.
    //
    // `ranking::resolve_alpha` returns the DENSE weight; `search_hybrid` takes
    // the BM25 weight. Flip it here rather than passing it through — the two
    // conventions are load-bearing in their own modules and silently swapping
    // one for the other would invert the intent (symbol queries would get LESS
    // lexical weight, exactly backwards).
    let alpha_bm25 = 1.0 - tokenade_core::ranking::resolve_alpha(q, None);
    // Depth is now the caller's to choose. It was a fixed 10 with no flag, so
    // there was no way to look past the first screen of a ranked search.
    const DEFAULT_SEMANTIC_LIMIT: usize = 10;
    let limit = rest
        .iter()
        .position(|a| a == "--limit" || a == "-n")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SEMANTIC_LIMIT);
    let (mut hits, retained) = match sem.search_hybrid_with_total(q, limit, alpha_bm25) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("tokenade {SEMANTIC_CMD}: {e}");
            return 1;
        }
    };
    if fw != framework::Framework::Unknown {
        for h in hits.iter_mut() {
            if fw.canonical_paths().iter().any(|p| h.file.contains(p)) {
                h.score *= 1.25;
                if !h.matched.iter().any(|m| m == "framework") {
                    h.matched.push("framework".into());
                }
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    if hits.is_empty() {
        // The semantic index is already fresh (`ensure_fresh` was
        // called above). Empty result means no document contained the
        // query terms — not a stale-cache issue. Fresh is not the same
        // as non-empty, though: an index of ZERO files is perfectly
        // fresh, and this line then claims the corpus lacks the terms
        // when the truth is there is no corpus.
        println!(
            "{}",
            crate::code_intel::semantic_miss_line(q, idx.files_indexed)
        );
        crate::code_intel::note_if_nothing_indexed(idx.files_indexed);
        return 0;
    }
    // Reduced modes: paths → deduped file list; count → hit count;
    // outline → `score  file` (no matched-terms/framework banner).
    // How deep the ranking went, said once, wherever the answer is a LIST.
    // `-o count` marks its own floor with a `+`; the listing modes said
    // nothing, so ten rows out of forty read as all there is.
    let depth_footer = crate::code_intel::semantic_depth_footer(
        hits.len(),
        limit,
        Some((retained, retained >= limit * 4)),
    );
    if out_mode != crate::code_intel::OutputMode::Full {
        let mut body = redact::redact(&crate::code_intel::output_mode_body_semantic_with_total(
            out_mode,
            &hits,
            Some((retained, retained >= limit * 4)),
        ));
        // Not on `count`, whose whole answer IS the number, and which already
        // carries the floor marker.
        if out_mode != crate::code_intel::OutputMode::Count {
            body.push_str(&depth_footer);
        }
        print!("{body}");
        crate::code_intel::log_code_intel(&format!("semantic {q}"), "semantic_search", &body);
        return 0;
    }
    if fw != framework::Framework::Unknown {
        println!("[framework: {}]", fw.as_str());
    }
    let mut budget = match crate::code_intel::parse_budget(rest, "semantic") {
        // `--budget 0` means NO CEILING — `parse_budget`'s own doc says so, and
        // `skeleton` spells it `.filter(|n| *n > 0)` four lines from here. This
        // handed the `Some(0)` straight to the budget, which enforced a literal
        // zero: `semantic <q> --budget 0` answered "1 shown, 9 withheld — 24
        // tokens spent against a 0-token ceiling" and advised raising the flag
        // the caller had just used to lift the ceiling. `pack` and
        // `expand-ref --raw` both honour 0; this was the one consumer of a
        // shared contract that did not.
        Ok(b) => crate::code_intel::TokenBudget::new(b.filter(|n| *n > 0)),
        Err(code) => return code,
    };
    let mut body = String::new();
    for h in hits {
        // The file is what the reader opens; the matched terms explain WHY it
        // ranked. Under a tight budget the explanation is the half worth
        // trimming, so a row that would not fit whole is still named.
        //
        // `admit` alone dropped it outright: measured on this repository,
        // `--budget 60` served 2 rows and withheld 8 while leaving 16 of its
        // 60 tokens unspent — a degraded row costs about a dozen. Mined from
        // `repowise` (2026-08-28), which found the same shape at a larger
        // scale: whole-lane drops served 0 of 58 mined comments where tail
        // trims serve 4.
        let (full, short) = semantic_row_forms(h.score, &h.file, &h.matched);
        let verdict = budget.admit_or_degrade(&full, &short);
        let Some(line) = semantic_row_to_emit(verdict, full, short) else {
            continue;
        };
        println!("{line}");
        body.push_str(&line);
        body.push('\n');
    }
    let footer = budget.footer();
    if !footer.is_empty() {
        println!("{footer}");
    }
    print!("{depth_footer}");
    // Same estimated-baseline accounting as query/impact/map — a semantic hit
    // list replaces the agent's would-be Read/Grep exploration round.
    crate::code_intel::log_code_intel(&format!("semantic {q}"), "semantic_search", &body);
    0
}

pub(crate) fn cmd_semantic_status(rest: &[String]) -> i32 {
    // #624 — `--json` for CI: emits the same data as the text path
    // (per-project rows + totals) with full paths and raw Unix
    // timestamps so consumers can format / threshold as they like.
    // #677 — silent-accept sweep continued. Same rationale as
    // cmd_index above; only `--json` is a real flag.
    if let Some(extra) = rest.iter().find(|a| *a != "--json") {
        eprintln!(
            "tokenade semantic-status: unexpected argument '{extra}' (this command takes no \
             positional args and the only flag is `--json`)."
        );
        return 2;
    }
    let json_mode = rest.iter().any(|a| a == "--json");
    let conn = match tokenade_core::semantic_db::open() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tokenade semantic-status: couldn't open db: {e}");
            return 1;
        }
    };
    let rows = match tokenade_core::semantic_db::list_projects(&conn) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tokenade semantic-status: query failed: {e}");
            return 1;
        }
    };
    if json_mode {
        let total_size: u64 = rows.iter().map(|r| r.2).sum();
        let total_files: u64 = rows.iter().map(|r| r.3).sum();
        let total_docs: u64 = rows.iter().map(|r| r.4).sum();
        let projects: Vec<serde_json::Value> = rows
            .iter()
            .map(|(path, last, size, files, docs)| {
                serde_json::json!({
                    "path": path.display().to_string(),
                    "files": files,
                    "docs": docs,
                    "size_bytes": size,
                    "last_indexed_unix": last,
                    "last_indexed_iso": unix_to_iso(*last),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "project_count": rows.len(),
            "total_files": total_files,
            "total_docs": total_docs,
            "total_size_bytes": total_size,
            "projects": projects,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }
    if rows.is_empty() {
        println!("(no projects indexed yet — run `tokenade index` from any project root)");
        return 0;
    }
    let total_size: u64 = rows.iter().map(|r| r.2).sum();
    let total_files: u64 = rows.iter().map(|r| r.3).sum();
    let total_docs: u64 = rows.iter().map(|r| r.4).sum();
    // #608 — semantic.db stores DOCUMENTS (embeddings), not symbols.
    // The schema's `symbol_count` column actually holds `docs.len()`
    // (see semantic.rs:634), which equals `mtimes.len()` (files) under
    // the current 1-doc-per-file chunking. Labeling the display column
    // "symbols" duplicates the "files" column visually and misleads
    // users into thinking semantic.db indexes symbols — that's the
    // symbol-index store (index.db), a different file. The dashboard
    // already uses the correct term: "4195 doc(s) 10496 term(s)".
    println!(
        "[~/.tokenade/semantic.db]  {} project(s)  ·  {} files  ·  {} docs  ·  {}",
        rows.len(),
        total_files,
        total_docs,
        human_bytes(total_size),
    );
    println!();
    println!(
        "{:<46} {:>10} {:>10} {:>10} {:>20}",
        "project", "files", "docs", "size", "last_indexed",
    );
    for (path, last, size, files, docs) in rows {
        let p = path.display().to_string();
        let p_short = shorten_path_for_column(&p, 46);
        println!(
            "{:<46} {:>10} {:>10} {:>10} {:>20}",
            p_short,
            files,
            docs,
            human_bytes(size),
            unix_to_iso(last),
        );
    }
    0
}

#[cfg(test)]
mod semantic_question_split {
    use super::split_quoted_questions;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    /// The batched shape the rest of the CLI teaches. Each question keeps its
    /// own ranking; joined, the better-scoring one pushes the other off.
    #[test]
    fn several_quoted_questions_stay_several() {
        assert_eq!(
            split_quoted_questions(&v(&["where is auth", "how are errors logged"])),
            v(&["where is auth", "how are errors logged"])
        );
    }

    /// A shell splits an unquoted sentence into single-word arguments, so not
    /// one of them can carry a space. This must stay ONE question — joining
    /// it is the feature, not the defect.
    #[test]
    fn an_unquoted_sentence_is_one_question() {
        assert_eq!(
            split_quoted_questions(&v(&["where", "is", "auth", "wired", "up"])),
            v(&["where is auth wired up"])
        );
    }

    /// The mixed shape is a quoted phrase with loose words around it, which
    /// is one sentence the user half-quoted — not two questions. ALL of them
    /// must carry a space, not merely one.
    #[test]
    fn a_half_quoted_sentence_is_one_question() {
        assert_eq!(
            split_quoted_questions(&v(&["where is auth", "wired"])),
            v(&["where is auth wired"])
        );
    }

    /// One quoted question is one question, and keeps its inner spaces.
    #[test]
    fn a_single_quoted_question_is_unchanged() {
        assert_eq!(
            split_quoted_questions(&v(&["zip bomb guard"])),
            v(&["zip bomb guard"])
        );
    }

    /// `all` on an empty slice is TRUE, so without the emptiness test no
    /// arguments would yield NO questions rather than one empty one.
    /// `cmd_semantic` rejects an empty query before it gets here, so this is
    /// the declarer's contract rather than a reachable path — and a future
    /// caller must not silently get zero questions run.
    #[test]
    fn no_arguments_still_yield_one_empty_question() {
        assert_eq!(split_quoted_questions(&[]), v(&[""]));
    }
}

#[cfg(test)]
mod semantic_row_budget {
    use super::semantic_row_forms;

    /// The trimmed form keeps the two things a reader needs — where to look,
    /// and how strongly it matched — and drops only the explanation.
    #[test]
    fn the_trimmed_row_keeps_the_file_and_the_score() {
        let terms = vec!["gitignore".to_string(), "rule".to_string()];
        let (full, short) = semantic_row_forms(0.72, "src/gitignore.rs", &terms);
        assert!(full.contains("src/gitignore.rs") && full.contains("gitignore, rule"));
        assert!(
            short.contains("src/gitignore.rs"),
            "the file was trimmed away: {short:?}"
        );
        assert!(
            short.contains("0.72"),
            "the score was trimmed away: {short:?}"
        );
        assert!(!short.contains('['), "the explanation survived: {short:?}");
        assert!(short.len() < full.len(), "the trim saved nothing");
    }

    /// A hit with no matched terms still renders both forms, and the trim
    /// still costs less — otherwise degrading it would be a pure loss.
    #[test]
    fn a_row_without_terms_still_shrinks() {
        let (full, short) = semantic_row_forms(0.10, "a.rs", &[]);
        assert!(short.len() < full.len(), "{full:?} vs {short:?}");
    }

    /// A verdict of `Degraded` must EMIT the trimmed row. Dropping it there
    /// undoes the whole point — the budget is charged for a row the reader
    /// never sees — and no test reached that decision until it was given a
    /// seam of its own.
    #[test]
    fn a_degraded_verdict_emits_the_trimmed_row() {
        use crate::code_intel::Admission;
        let f = || "FULL".to_string();
        let s = || "SHORT".to_string();
        assert_eq!(
            super::semantic_row_to_emit(Admission::Full, f(), s()).as_deref(),
            Some("FULL")
        );
        assert_eq!(
            super::semantic_row_to_emit(Admission::Degraded, f(), s()).as_deref(),
            Some("SHORT"),
            "a degraded row was dropped instead of trimmed"
        );
        assert_eq!(
            super::semantic_row_to_emit(Admission::Refused, f(), s()),
            None
        );
    }
}
