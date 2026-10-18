//! **`tokenade search`: the command.**
//!
//! Several queries in one round-trip, and the guard that matters: a local
//! path that does not exist IS NOT a query. `tokenade search
//! /nope/missing.html` answered with exit code 0 — a typo that reads as a
//! successful read, and a local path (a person's, a client's or a project's
//! name) handed to two search engines.

use super::*;

/// Same treatment as [`WEB_USAGE`]: one text, reachable by `--help` and shown
/// on a bad invocation.
pub(super) const SEARCH_USAGE: &str = "usage: tokenade search \"<query>\"… [--answer] [--json]\n  \
     tokenade search \"<q1>\" \"<q2>\"       SEVERAL queries, run concurrently\n  \
     tokenade search <file>|-            compact a SAVED results page\n  \
     tokenade search --diagnose          probe every engine\n\n  \
     web search across independent engines, merged, deduped and ranked\n  \
     --answer  fetch the top results and return the passages, not links\n  \
     --json    machine-readable: engines, query, refused, results[] (title, url, snippet)\n  \
     --html, --serp  treat the argument as a saved results page even if it reads like a query";

/// True when an argument is written like a filesystem path and no file is
/// there. `search` reads a readable file as a saved results page and treats
/// anything else as a web query, so without this a missing path becomes the
/// query itself.
///
/// Narrow on purpose, because a real query may carry a slash:
/// `BurntSushi/ripgrep` is a search anyone might type, and refusing it to
/// catch a typo trades a working use for a broken one. Only an argument with
/// no whitespace that either OPENS like a path or ENDS like a saved page
/// counts.
pub(super) fn path_shaped_but_missing(q: &str) -> bool {
    // Whitespace makes it a sentence, not a path: `difference between
    // index.html and index.htm` ends like a page and is plainly a question.
    // (`-` and the empty string need no clause of their own — neither opens
    // like a path nor ends like a page, and a condition no test can kill is
    // one nobody can trust.)
    if q.contains(char::is_whitespace) {
        return false;
    }
    let opens_like_a_path =
        q.starts_with('/') || q.starts_with("./") || q.starts_with("../") || q.starts_with("~/");
    let ends_like_a_page = {
        let lower = q.to_ascii_lowercase();
        [".html", ".htm", ".xhtml", ".mhtml"]
            .iter()
            .any(|e| lower.ends_with(e))
    };
    (opens_like_a_path || ends_like_a_page) && !std::path::Path::new(q).is_file()
}

pub(crate) fn cmd_search(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("{SEARCH_USAGE}");
        return 0;
    }
    // The switch that keeps tokenade out of web search covers the command
    // too: an agent told to use its own search must not reach ours by Bash.
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_WEBSEARCH") {
        eprintln!(
            "tokenade search: web search is turned off here (TOKENADE_NO_WEBSEARCH=1) — \
             use your agent's own web search"
        );
        return 3;
    }
    let mut queries: Vec<String> = Vec::new();
    let mut json = false;
    let mut answer = false;
    let mut diagnose = false;
    let mut force_html = false;
    let iter = rest.iter();
    for a in iter {
        match a.as_str() {
            "--json" => json = true,
            // Fetch the top results and distil them, instead of handing back
            // links the caller then pays to read one by one.
            "--answer" => answer = true,
            "--diagnose" => diagnose = true,
            // Force the saved-page reading even when the argument would parse
            // as a query (a file literally named like a sentence).
            "--html" | "--serp" => force_html = true,
            // A MISTYPED flag used to become a query of its own, so
            // `tokenade search --josn "rust async"` ran two web searches — one
            // of them for `--josn` — spending network and quota on a typo and
            // mixing its results into the answer. Same family as unknown-flag
            // typos elsewhere, in the
            // command that actually costs something.
            //
            // Flag-SHAPED, not merely hyphen-leading: `tokenade search
            // "-Wall meaning"` is one legitimate argument that starts with a
            // dash, and rejecting it would break a working query while fixing
            // a typo. A real flag has no whitespace in it.
            other if other.starts_with("--") && !other.contains(char::is_whitespace) => {
                eprintln!("tokenade search: unknown flag '{other}'. Try `tokenade search --help`.");
                return 2;
            }
            // Each positional is a SEPARATE query. `tokenade search "q1" "q2"`
            // runs them concurrently (D11); a multi-word query must be quoted.
            other => queries.push(other.to_string()),
        }
    }
    if diagnose {
        return search_diagnose();
    }
    // A path that does not exist is NOT a query. The rule below ("a readable
    // file is a results PAGE, anything else is a QUERY") sent a mistyped or
    // missing path to the engines as the literal query string: measured
    // 2026-09-04, `tokenade search /nope/missing.html` returned
    // `[tokenade:search via bing+duckduckgo] "/nope/missing.html"` with exit 0,
    // so a typo reads as a successful read of the file, and a local path —
    // which carries a user name, a client name, a project name — leaves the
    // machine. Same family as the mistyped flag two matches below, one
    // argument over.
    //
    // Checked for EVERY positional, before the single/batch split, so the
    // batch path cannot leak what the single path refuses.
    // `--html` says "read it as a page" explicitly, so let the reader report
    // its own error rather than answering a request with advice to make it.
    if let Some(bad) = queries
        .iter()
        .find(|q| !force_html && path_shaped_but_missing(q))
    {
        eprintln!(
            "tokenade search: {bad}: No such file or directory. A path-shaped argument is read \
             as a saved results page, so nothing was sent to a search engine. Use `--html` to \
             force the page reading, or quote a real query."
        );
        return 2;
    }
    // D11: several queries in one invocation, served concurrently. Same-engine
    // requests still serialise through the per-engine permit in `run_search`.
    if queries.len() > 1 {
        // D13: the agent batched — reset the sequential-call counter.
        nudge_sequential_web_calls("cli-search", "search", true);
        return cmd_search_batch(&queries, json, answer);
    }
    let query = queries.into_iter().next().unwrap_or_default();

    // ONE search surface. `tokenade serp` used to be a second command for the
    // saved-page half of the same job; a caller had to know which of two to
    // reach for, and the two shared no code. The argument decides: a readable
    // file (or `-`) is a results PAGE, anything else is a QUERY.
    let looks_like_html_input =
        force_html || query == "-" || (!query.is_empty() && std::path::Path::new(&query).is_file());
    if looks_like_html_input {
        return compact_saved_serp(&query);
    }

    if query.trim().is_empty() {
        eprintln!("{SEARCH_USAGE}");
        return 2;
    }
    // D13: a single-query search counts toward the sequential-web-call nudge.
    nudge_sequential_web_calls("cli-search", "search", false);
    let now = tokenade_core::platform::now_unix_secs();
    // Cross-process-safe rotation + breaker persistence (D12).
    let outcome = tokenade_core::search::run_search(&query, now);

    if json {
        let items: Vec<_> = outcome
            .results
            .iter()
            .map(|r| serde_json::json!({"title": r.title, "url": r.url, "snippet": r.snippet}))
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "query": query,
                "engines": outcome.used,
                "refused": outcome.refused.iter()
                    .map(|(e, r)| serde_json::json!({"engine": e, "why": r.as_str()}))
                    .collect::<Vec<_>>(),
                "results": items,
            })
        );
        return if outcome.is_usable() { 0 } else { 1 };
    }

    let via = if outcome.used.is_empty() {
        String::from("no engine")
    } else {
        outcome.used.join("+")
    };
    let body = if answer && outcome.is_usable() {
        answer_from_results(&query, &outcome)
    } else {
        // SECURITY: same scrub as every other served channel. A SERP snippet can
        // quote a leaked key as readily as a page can.
        tokenade_core::redact::redact_for_model(&tokenade_core::search::render(&query, &outcome))
    };
    println!("[tokenade:search via {via}] {query:?}:\n\n{body}");
    if outcome.is_usable() {
        log_gain(
            "search",
            if answer {
                "web-search-answer"
            } else {
                "web-search"
            },
            // The SEARCH assiette, the same one the WebSearch hook books.
            //
            // This booked `credit_binary` — the BINARY-DOCUMENT model, whose
            // whole subject is the fumbling turns avoided on a PDF. For
            // `claude-code` that model is `NativePdf`, which credits ZERO by
            // design, so every search this command served was worth nothing and
            // fell under the ledger's noise threshold. Measured 2026-08-28: the
            // measured ledger holds **not one** `web-search`, `web-search-batch`
            // or `serp_compact` row on a machine whose estimated ledger holds
            // 603 search batches — and a live batch run under the same build
            // books the batching credit and no measured row at all.
            //
            // `web_search_credit_savings` exists for exactly this and had ONE
            // caller: the hook. Same capability, two doors, two models — and
            // the same rule reaches here too: what a search is worth cannot
            // depend on how the agent reached it.
            tokenade_core::coefficients::web_search_credit_savings(
                tokenade_core::tokenizer::estimate(&body),
            ),
        );
        0
    } else {
        // A search that could not look is not a search that found nothing.
        1
    }
}

/// #515 — Shared helper for compact commands that read stdin-or-file.
/// Returns `Err(2)` after printing a usage hint when invoked with no
/// path arg AND stdin is a TTY (interactive user hit Enter, would
/// otherwise hang silently on read_to_string). Returns `Err(1)` on
/// genuine I/O failure. Returns `Ok(input)` when piped input or a
/// readable file path is supplied.
///
/// `hint` is the trailing usage block printed under the `usage:`
/// line — should include `pipe input:` and `from file:` examples
/// so the user immediately sees both shapes.
pub(crate) fn read_stdin_or_file_with_tty_hint(
    rest: &[String],
    cmd_name: &str,
    hint: &str,
) -> Result<String, i32> {
    use std::io::IsTerminal;
    // L17-class — silent-accept sweep: this helper was only inspecting
    // `rest.first()`, so `tokenade dns - --bogus extra` and friends
    // silently dropped everything past index 0. Reject unknown flags
    // and extra positionals up front so typos fail loudly. None of
    // the 17 callers expose any flags today; if one ever needs to,
    // switch to validate_stdin_or_single_file directly.
    let path_or_dash = match validate_stdin_or_single_file(rest, cmd_name, &[]) {
        Ok(Some(p)) => Some(p),
        Ok(None) => None,
        Err(code) => return Err(code),
    };
    let no_arg_no_pipe = path_or_dash.is_none() && std::io::stdin().is_terminal();
    if no_arg_no_pipe {
        eprintln!("usage: tokenade {cmd_name} <file>|-\n{hint}");
        return Err(2);
    }
    match path_or_dash {
        None => {
            let mut s = String::new();
            if crate::bounded_input::read_stdin_lossy(&mut s).is_err() {
                return Err(1);
            }
            Ok(s)
        }
        Some(path) => {
            // #983 — `fs::read_to_string("/some/dir")` returns the bare
            // OS error `Is a directory (os error 21)`, which leaves the
            // user with no idea what tokenade expects. The MCP-side
            // skeleton / code_strip handlers already paper over this
            // with a clear hint (#428); mirror it across all 17+ shared
            // callers (auto, yaml, json, diff, stack, csv, snapshot,
            // notebook, web, serp, stealth, sourcemap, openapi, ansible,
            // ghactions, logs, curl, metrics, pkginstall, archive).
            let p = std::path::Path::new(&path);
            if p.is_dir() {
                eprintln!(
                    "tokenade {cmd_name}: `{path}` is a directory — \
                     pass a single file (or `-` to read stdin). For a tree, \
                     enumerate first: `find {path} -type f | xargs -I{{}} \
                     tokenade {cmd_name} {{}}`."
                );
                return Err(1);
            }
            crate::bounded_input::read_file_lossy(&path).map_err(|e| {
                eprintln!("tokenade {cmd_name}: {path}: {e}");
                1
            })
        }
    }
}

/// #641 — Levenshtein has moved to `tokenade_core::text_distance`
/// (shared with the MCP server). This thin alias avoids touching every
/// call site, and keeps the typo-hint pattern readable: `levenshtein(a,
/// b)` rather than the longer qualified path.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    tokenade_core::text_distance::levenshtein(a, b)
}
