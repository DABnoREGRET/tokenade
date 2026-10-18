//! **The READ hook: what the agent has just opened.**
//!
//! A file read is not a command run: it has an extension, a size, and often a
//! clue about what the agent was looking for. The hint category decides
//! whether it is better to offer it `tokenade read --prompt` or a search — and
//! getting it wrong here offers an action more expensive than the one it was
//! about to take.

use super::*;

/// `Read` PostToolUse handler — passive skeleton-hint.
///
/// Fires when the model just read a CODE-shaped file larger than the
/// threshold (default 5 000 tokens). Emits a one-line `additionalContext`
/// suggesting `tokenade skeleton` for next time. Does NOT touch
/// the file content — the model still receives the full Read result.
///
/// Trigger conditions:
///   * `Read` tool with `file_path` extension in CODE_EXTS
///   * Tool result text > 5 000 tokens (`TOKENADE_READ_HINT_TOKENS` override)
///   * Result not already substituted by dedup (the dedup-hit path
///     would have replaced the content with a §ref§ marker, so the
///     content size we measure would be tiny anyway)
///   * Opt-out: `TOKENADE_NO_READ_HINT=1`
///
/// Net cost: ~50 tokens of banner per qualifying Read; benefit kicks
/// in when the model pivots to `skeleton` on the next exploration of
/// that file (saves 70-85%).
/// #691 / #692 — four-way classification for the PostToolUse Read
/// hint. The hint always offers ONE actionable next step tailored
/// to the file's shape:
///   * `Code` → `tokenade skeleton` (signatures only, ~80% smaller).
///   * `TextData` → `tokenade read` (drops repetition, wraps log runs,
///     strips markdown chrome) + `tokenade semantic` (natural-language
///     queries beat re-reading).
///   * `StructuredData` → `tokenade read` (auto-routes to json_compact /
///     yaml_compact / csv_compact / lockfile_compact / sourcemap_compact
///     based on extension — each shaves 30-95% of bytes).
///   * `None` → no hint (anything where suggesting compaction would
///     mis-direct: binaries, images, lock files we don't compact).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum ReadHintCategory {
    Code,
    TextData,
    StructuredData,
    None,
}

pub(crate) fn read_hint_category_for(file_path: &str) -> ReadHintCategory {
    // Code-shaped extensions where `skeleton` actually provides value
    // (signatures + bodies-elided). NOT data files (JSON, CSV, MD) where
    // every line is content.
    // #707 — added shell (.sh/.bash/.zsh/.fish/.ksh/.ps1/.psm1/.bat),
    // SQL (.sql), Terraform/HCL (.tf/.tfvars/.hcl), proto (.proto),
    // and other code-shaped extensions (.r/.jl/.tcl/.vim). These were
    // all in `auto.rs::is_source_code_extension` so they were getting
    // passthrough at the pretool layer, but with NO posttool hint they
    // got NO nudge toward skeleton / semantic_search.
    const CODE_EXTS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go", ".java", ".kt",
        ".scala", ".swift", ".m", ".mm", ".c", ".cc", ".cpp", ".cxx", ".h", ".hpp", ".hxx", ".cs",
        ".rb", ".php", ".pl", ".lua", ".dart", ".ex", ".exs", ".elm", ".erl", ".hrl", ".clj",
        ".cljs", ".cljc", ".hs", ".ml", ".mli", ".fs", ".fsi", ".zig", ".nim", ".sh", ".bash",
        ".zsh", ".fish", ".ksh", ".ps1", ".psm1", ".bat", ".sql", ".proto", ".tf", ".tfvars",
        ".hcl", ".r", ".jl", ".tcl", ".vim",
    ];
    // Text-data files: large prose / log content. No AST so skeleton
    // can't help, but auto_compact / semantic_search are the right
    // next step. Without this branch an agent reading a 30K .txt got
    // NO hint and was likely to "try many methods" hunting for a peek.
    // #707 — added .html/.htm (html_to_markdown via auto_compact strips
    // chrome / nav / footer down to outline-ish text), and .diff/.patch
    // (line-oriented; auto_compact's UnifiedDiff path elides repeated
    // hunk headers and trims trailing whitespace).
    const TEXT_DATA_EXTS: &[&str] = &[
        ".txt",
        ".log",
        ".md",
        ".markdown",
        ".mdx",
        ".rst",
        ".org",
        ".adoc",
        ".asciidoc",
        ".tex",
        ".html",
        ".htm",
        ".diff",
        ".patch",
    ];
    // #692 — structured-data files that DO have a dedicated compactor
    // route via `auto_compact` (extension-keyed dispatch). Each one
    // saves 30-95% on typical inputs:
    //   .json     → json_compact   (drops *_url chrome, _links, etag, caps arrays)
    //   .yaml/yml → yaml_compact   (drops managedFields, resourceVersion, uid…)
    //   .csv/tsv  → csv_compact    (drops empty cols, caps rows, truncates cells)
    //   .lock     → lockfile_compact (flatten to `name version` list)
    //   .map      → sourcemap_compact (drops mappings + sourcesContent — 99% of bytes)
    //   .ndjson   → ndjson_compact (severity histogram + key-union schema)
    // #707 — added .toml/.ini/.cfg/.conf/.properties (ini/toml compactor
    // routes), .xml/.xsd/.xsl/.xslt/.svg (xml compactor), and .ipynb
    // (notebook → cells outline + outputs preview) — all wired in
    // `kind_from_path_hint` already.
    // semantic_search isn't useful here (structure matters), so the
    // hint omits it for this category.
    const STRUCTURED_DATA_EXTS: &[&str] = &[
        ".json",
        ".yaml",
        ".yml",
        ".csv",
        ".tsv",
        ".lock",
        ".map",
        ".ndjson",
        ".jsonl",
        ".toml",
        ".ini",
        ".cfg",
        ".conf",
        ".properties",
        ".xml",
        ".xsd",
        ".xsl",
        ".xslt",
        ".svg",
        ".ipynb",
    ];
    let lower = file_path.to_ascii_lowercase();
    if CODE_EXTS.iter().any(|ext| lower.ends_with(ext)) {
        ReadHintCategory::Code
    } else if TEXT_DATA_EXTS.iter().any(|ext| lower.ends_with(ext)) {
        ReadHintCategory::TextData
    } else if STRUCTURED_DATA_EXTS.iter().any(|ext| lower.ends_with(ext)) {
        ReadHintCategory::StructuredData
    } else {
        ReadHintCategory::None
    }
}

pub(crate) fn handle_read_posttool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    // #1007 — the hint bodies advertise `TOKENADE_NO_READ_HINT=1` as the
    // silencer; pre-fix the env var was NEVER read so the documented
    // opt-out did nothing (same fictional-knob class as
    // TOKENADE_NO_SEARCH_HINT in search_suggestion_hint). Honour it at
    // the entry so all 3 hint branches (Code / TextData / StructuredData)
    // get gated uniformly.
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_READ_HINT") {
        return 0;
    }
    let file_path = parsed
        .get("tool_input")
        .and_then(|i| i.get("file_path"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    if file_path.is_empty() {
        return 0;
    }
    let category = read_hint_category_for(file_path);
    if matches!(category, ReadHintCategory::None) {
        return 0;
    }
    // Extract the result text. Read returns a single text content block.
    let response = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"));
    let result_text = response
        .and_then(|r| {
            // Two shapes: r is a string (rare), or r is {content: [{type:"text", text:"..."}]}
            if let Some(s) = r.as_str() {
                Some(s.to_string())
            } else if let Some(arr) = r.get("content").and_then(|c| c.as_array()) {
                let mut acc = String::new();
                for block in arr {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        acc.push_str(t);
                    }
                }
                Some(acc)
            } else {
                r.get("text")
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string())
            }
        })
        .unwrap_or_default();
    if result_text.is_empty() {
        return 0;
    }
    // If the result was already substituted by tokenade's dedup path,
    // we'll see the `§ref:...` marker — bail out (already compacted).
    if tokenade_core::fold_marker::has_dedup_ref(&result_text) {
        return 0;
    }
    let result_tokens = tokenade_core::tokenizer::estimate(&result_text);
    // #1010 — pre-fix a typo (`=10k`, `=5,000`, `=-1`) silently fell back
    // to 5_000 with no trace. Route through the logging helper so an
    // operator who set the var and "saw no change" can grep debug.log
    // for `parse_usize_rejected` and find their typo.
    let threshold: usize =
        tokenade_core::env_compat::parse_usize_or("TOKENADE_READ_HINT_TOKENS", 5_000);
    // #693 — pagination signal. When `offset > 0` the agent is
    // EXPLICITLY paging through a file that's bigger than the
    // default 2000-line window. The exact size of *this* slice
    // doesn't matter — the parent file is large by definition,
    // which is precisely when auto_compact / skeleton beat a
    // chain of Read offsets. So skip the result_tokens threshold
    // when paginating. Captures the "agent struggling to read a
    // txt entirely, trying many methods" pattern the user flagged.
    let offset = parsed
        .get("tool_input")
        .and_then(|i| i.get("offset"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    let is_paginating = offset > 0;
    if !is_paginating && result_tokens < threshold {
        return 0;
    }
    // #691 / #692 — hint dispatched by category. Skeleton for code
    // (signatures-only, ~80% smaller). auto_compact for prose/log
    // (drops repetition / wraps log runs / strips markdown chrome)
    // PLUS semantic_search as an alternative to re-reading. For
    // structured-data files (.json/.yaml/.csv/.lock/.map/.ndjson),
    // auto_compact routes to the matching format compactor — but
    // semantic_search is less useful (structure matters) so the
    // hint omits it. Same envelope: TOKENADE_NO_READ_HINT=1 silences.
    // #693 — when paginating (offset > 0), lead with that context so
    // the agent recognises the pattern and can break out of the
    // offset-loop on the next call instead of paging through.
    let size_or_slice = if is_paginating {
        format!("slice at offset={offset} is ~{result_tokens} tokens (file is paged)")
    } else {
        format!("is ~{result_tokens} tokens")
    };
    let hint = match category {
        ReadHintCategory::Code => format!(
            "[tokenade:read-hint] {file_path} {size_or_slice}. \
             If you only need the API surface, \
             `tokenade skeleton {file_path}` returns signatures only \
             (~80% smaller). Set TOKENADE_NO_READ_HINT=1 to silence."
        ),
        ReadHintCategory::TextData => format!(
            "[tokenade:read-hint] {file_path} {size_or_slice}. \
             For a compacted view (drops repetition, wraps log runs, strips \
             markdown chrome): `tokenade read {file_path}`. \
             For natural-language search instead of re-reading: \
             `tokenade semantic \"…\"`. \
             Set TOKENADE_NO_READ_HINT=1 to silence."
        ),
        ReadHintCategory::StructuredData => format!(
            "[tokenade:read-hint] {file_path} {size_or_slice}. \
             For a compacted view (auto-routes to the matching format compactor — \
             json/yaml/csv/lockfile/sourcemap/ndjson — typically 30-95% smaller): \
             `tokenade read {file_path}`. \
             Set TOKENADE_NO_READ_HINT=1 to silence."
        ),
        ReadHintCategory::None => return 0,
    };
    // Anti-spam: agents page through the same large file or re-read it
    // repeatedly within a session. The detection still runs every time
    // (the steer stays available), but once the skeleton/auto advice for
    // this CATEGORY is in context we don't re-inject it. Key by category
    // so Code / TextData / StructuredData each surface once per session.
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    let hint_key = match category {
        ReadHintCategory::Code => "read-code",
        ReadHintCategory::TextData => "read-text",
        ReadHintCategory::StructuredData => "read-struct",
        ReadHintCategory::None => return 0,
    };
    if !hint_fresh_this_session(session_id, hint_key) {
        tokenade_core::debug_log::log(
            "hookwrap",
            "read_hint_throttled",
            serde_json::json!({
                "file": file_path,
                "hint_key": hint_key,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": hint,
        }
    });
    log_emitted_additional_context("hookwrap-read-hint", "PostToolUse", &hint);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap",
        "read_hint_emitted",
        serde_json::json!({
            "file": file_path,
            "result_tokens": result_tokens,
            "threshold": threshold,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Returns a one-line additional-context hint when a Bash invocation of a
/// search tool produced a result that almost certainly leads to another
/// search step:
///   - 0 matches → the model is about to rephrase and retry. Suggest
///     `semantic_search` (natural-language) or `symbol_find` (identifier).
///   - more than 50 matches → the model is about to add `| head` or
///     `| grep` to narrow. Suggest `symbol_find` (exact stem) or
///     `semantic_search` (semantic narrowing).
///
/// Tool coverage:
///   - content search: `grep` / `egrep` / `fgrep` / `zgrep` / `rg` /
///     `ripgrep` / `ag` (the_silver_searcher) / `ack` / `ack-grep` /
///     `ug` / `ugrep` / `git grep`
///   - filename / path search: `find` / `fd` / `fdfind` / `locate` /
///     `mlocate` / `plocate`
///   - process search (different semantics — no escalation suggestion):
///     `pgrep`
///
/// Returns `None` for non-search commands, commands using counter flags
/// (`-c`, `-l`, `--files-with-matches`) where line-count doesn't reflect
/// match-count, and commands where output is plausibly in the sweet
/// spot (1–50 matches).
/// If `cmd` looks like `<path>?tokenade wrap '<inner>'` (with the
/// inner segment single-quoted as emitted by [`posix_quote`]), return
/// `Some(inner)`. Returns `None` for any other shape — including
/// double-quoted variants, chains (`tokenade wrap A && cmd_b`), or
/// non-wrap commands.
///
/// Pure-string parse: no shell evocation, no escape handling beyond
/// the single-quoted "everything literal" POSIX shape. The Axe-2
/// pre-rewriter always emits this shape, so this covers the common
/// post-deploy case without risking false-positive on user-typed
/// commands that happen to contain `tokenade wrap`.
pub(crate) fn unwrap_tokenade_wrap(cmd: &str) -> Option<&str> {
    let s = cmd.trim();
    // Find the prog token (strip optional path).
    let first = s.split_ascii_whitespace().next()?;
    let prog = first.rsplit(['/', '\\']).next().unwrap_or(first);
    if prog != "tokenade" {
        return None;
    }
    // Must be followed by " wrap '".
    let after_prog = &s[first.len()..].trim_start();
    let after_wrap = after_prog.strip_prefix("wrap")?.trim_start();
    let inner_start = after_wrap.strip_prefix('\'')?;
    // Inner extends up to the LAST single-quote. POSIX single-quoted
    // strings can't contain escaped quotes, so the matching quote is
    // always the last `'` in the rest of the input. We DO want the
    // last (not first) `'` to handle inner content that contains an
    // apostrophe — but posix_quote itself doesn't emit unescaped `'`
    // inside the body, so the inner shouldn't have any. Fall back
    // gracefully if the shape is unexpected.
    let inner_end = inner_start.rfind('\'')?;
    Some(&inner_start[..inner_end])
}

pub(crate) fn search_suggestion_hint(cmd: &str, output: &str) -> Option<String> {
    // #1007 — the hint body itself advertises `TOKENADE_NO_SEARCH_HINT=1`
    // as the silencer, but pre-fix the env var was NEVER read. Users
    // following the advice got no relief (a discoverability/trust bug
    // worse than a missing feature — the advice was actively wrong).
    // Honour the documented opt-out at the entry so the hint body's
    // promise is kept regardless of which downstream branch fires.
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_SEARCH_HINT") {
        return None;
    }
    // L80 — unwrap `tokenade wrap '<inner>'` so the hint fires on
    // wrapped searches. After Axe-2 deploy, ~all bash searches are
    // rewritten to `tokenade wrap 'grep …'`; without this unwrap the
    // first-word check below sees "tokenade" and bails. Production
    // data: 286+ tokenade-prog events with prog="tokenade", ~30%
    // wrapped searches → ~85 missed hints/day.
    let cmd: &str = match unwrap_tokenade_wrap(cmd) {
        Some(inner) => inner,
        None => cmd,
    };
    let trimmed = cmd.trim_start();
    // Strip a leading `LANG=C` / `LC_ALL=C` / similar env prefix so the
    // tool name is the next token.
    let after_env = trimmed
        .split_ascii_whitespace()
        .find(|tok| !tok.contains('='))?;
    // Strip any leading path component: `/usr/bin/rg` → `rg`.
    let bare = after_env.rsplit('/').next().unwrap_or(after_env);
    let kind = match bare {
        "grep" | "egrep" | "fgrep" | "zgrep" => SearchKind::Grep,
        "rg" | "ripgrep" => SearchKind::Ripgrep,
        "ug" | "ugrep" => SearchKind::Ugrep,
        "ag" => SearchKind::Ag,
        "ack" | "ack-grep" => SearchKind::Ack,
        "find" => SearchKind::Find,
        "fd" | "fdfind" => SearchKind::Fd,
        "locate" | "mlocate" | "plocate" => SearchKind::Locate,
        "pgrep" => SearchKind::Pgrep,
        "git" => {
            // Only `git grep` — other git subcommands aren't search.
            let rest: Vec<&str> = trimmed.split_ascii_whitespace().collect();
            if rest.get(1).copied() == Some("grep") {
                SearchKind::Grep
            } else {
                return None;
            }
        }
        _ => return None,
    };

    // Bail when the caller asked for a counter/list-only output — line
    // count then doesn't mean match count.
    for tok in trimmed.split_ascii_whitespace().skip(1) {
        // Stop at the first non-flag argument so we don't false-match on
        // a pattern that happens to start with `-c`.
        if !tok.starts_with('-') {
            break;
        }
        match tok {
            "-c"
            | "--count"
            | "-l"
            | "--files-with-matches"
            | "-L"
            | "--files-without-match"
            | "-q"
            | "--quiet"
            | "--silent" => return None,
            _ => {}
        }
    }

    // Count non-empty stdout lines as the match approximation.
    let matches = output.lines().filter(|l| !l.trim().is_empty()).count();

    // pgrep searches processes, not source code — semantic_search and
    // symbol_find don't help, so we skip the nudge entirely.
    if matches!(kind, SearchKind::Pgrep) {
        return None;
    }

    const NOISY_THRESHOLD: usize = 50;
    let tool_label = kind.label();
    if matches == 0 {
        Some(format!(
            "[tokenade:search-hint] {tool_label} returned 0 matches. \
             For natural-language search try `tokenade semantic \"…\"`. \
             For a known identifier (case-sensitive) try \
             `tokenade query \"name\"`. \
             Set TOKENADE_NO_SEARCH_HINT=1 to silence."
        ))
    } else if matches > NOISY_THRESHOLD {
        Some(format!(
            "[tokenade:search-hint] {tool_label} returned {matches} matches \
             (likely too broad — narrowing is the next step). For exact \
             identifiers try `tokenade query \"name\"`; for semantic \
             narrowing try `tokenade semantic \"…\"`. \
             Set TOKENADE_NO_SEARCH_HINT=1 to silence."
        ))
    } else {
        None
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SearchKind {
    Grep,
    Ripgrep,
    Ugrep,
    Ag,
    Ack,
    Find,
    Fd,
    Locate,
    Pgrep,
}

impl SearchKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SearchKind::Grep => "grep",
            SearchKind::Ripgrep => "ripgrep",
            SearchKind::Ugrep => "ugrep",
            SearchKind::Ag => "ag",
            SearchKind::Ack => "ack",
            SearchKind::Find => "find",
            SearchKind::Fd => "fd",
            SearchKind::Locate => "locate",
            SearchKind::Pgrep => "pgrep",
        }
    }
}
