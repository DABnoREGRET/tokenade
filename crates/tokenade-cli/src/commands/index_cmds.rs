//! **The INDEX commands: build, touch, watch, replay.**
//!
//! They share an expensive state — the index itself — and one question: when
//! must it be rebuilt? A stale index answers fast and WRONG; an index rebuilt
//! too often costs more than it saves.

use crate::*;

pub(crate) fn build_index() -> index::Index {
    index::Index::build(&root()).unwrap_or_else(|e| {
        eprintln!("tokenade: index build failed: {e}");
        std::process::exit(1);
    })
}

/// `tokenade index-touch` — fast incremental re-index for the
/// PostToolUse-on-Edit hook. Idempotent and cheap when nothing changed
/// (one fs::metadata call per known file via `mtime` diff). On a
/// project where 1 file just changed, re-extracts only that file's
/// symbols. Designed to be called after EVERY agent Edit/Write/
/// MultiEdit — so the search paths (`symbol_find`,
/// `semantic_search`, `impact_of`) are always warm and the user never
/// pays index-build latency on the first query.
///
/// Reads the hook input JSON from stdin (Claude Code's hook contract)
/// to discover which file changed, but the actual reindex uses the
/// `Index::ensure_fresh` pipeline (load → incremental rebuild via mtime
/// → persist) which auto-detects ALL changed files since the last
/// snapshot — defensive against agent batches or out-of-band edits.
pub(crate) fn cmd_index_touch() -> i32 {
    let started = std::time::Instant::now();
    // Consume stdin: #694 needs the JSON to extract tool_input.file_path
    // for the file_diff hint. Bounded read so a runaway producer can't
    // wedge us. Empty/malformed stdin is fine — the hint just stays
    // silent (matches the legacy "drain and discard" behaviour).
    let stdin_json = crate::hooks::stdin::read_event("edit-hint");

    // Project root = cwd. Edits are tied to a project so cwd is the
    // right anchor. The semantic index also keys off the same root.
    let r = root();

    // #712 — when the edited file is OUTSIDE the project root (e.g.
    // agent edits `~/.tokenade/style.json` while cwd is in a tokenade
    // project, or touches a sibling repo), the index walk is pure
    // waste: it can't possibly find new files for `r`'s symbol table.
    // Observed pre-#712: 126ms mean / 217ms max per cmd_index_touch.
    // Out-of-root edits hit the same cost for zero return.
    if let Some(file_path) = edited_file_path(stdin_json.as_deref()) {
        if !path_within_root(&r, &file_path) {
            let cross_touched = tokenade_core::auto::is_source_code_extension(&file_path)
                && touch_cross_project_index(&file_path, started);
            tokenade_core::debug_log::log(
                "index-touch",
                "skipped_outside_root",
                serde_json::json!({
                    "file_path": file_path,
                    "root": r.display().to_string(),
                    "cross_project_touch": cross_touched,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        }
        // #713 — non-source-extension edits (`.md`, `.json`, `.yaml`,
        // `.toml`, CHANGELOG entries, etc.) don't feed the symbol
        // index: `Index::rebuild_incremental` filters by
        // `lang_for(ext).is_some()` and only ingests known source
        // languages. The tree walk still happens, costs ~80-120ms,
        // and finds zero indexable changes. Skip it. The file_diff
        // hint still emits (cheap; just stat + dedup_ref lookup).
        // Staleness impact: zero — non-source files weren't in the
        // index in the first place.
        if !tokenade_core::auto::is_source_code_extension(&file_path) {
            let hint_emitted = emit_edit_diff_hint_from_stdin(&r, stdin_json.as_deref());
            tokenade_core::debug_log::log(
                "index-touch",
                "skipped_nonsource_ext",
                serde_json::json!({
                    "file_path": file_path,
                    "edit_diff_hint": hint_emitted,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        }
    }

    // Latency audit (p50=300ms, p99=6.4s, max 17s — this is a SYNCHRONOUS
    // PostToolUse hook, so every Edit paid the index walk): emit the cheap
    // additionalContext hint inline (it must reach stdout before we exit),
    // then hand the heavy index/semantic refresh to a DETACHED child.
    // Debounce: skip the spawn when a recent child is already at work —
    // an edit burst otherwise forks one indexer per Edit.
    let hint_emitted = emit_edit_diff_hint_from_stdin(&r, stdin_json.as_deref());
    if std::env::var("TOKENADE_INDEX_FG").is_err() {
        let lock = r.join(".tokenade").join("index-touch.running");
        let busy = std::fs::metadata(&lock)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|e| e.as_secs() < 30)
            .unwrap_or(false);
        if !busy {
            tokenade_core::platform::ensure_parent_dir(&lock);
            let _ = std::fs::write(&lock, "");
            if let Ok(exe) = std::env::current_exe() {
                let mut c = std::process::Command::new(exe);
                c.arg("index-touch")
                    .env("TOKENADE_INDEX_FG", "1")
                    .current_dir(&r)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                tokenade_core::process_util::configure_no_window(&mut c);
                // Same detach recipe as updater::spawn_background_check:
                // plain spawn with null stdio survives the ms-lifetime
                // hook parent (e2e-proven for auto-update).
                let _ = c.spawn();
            }
        }
        tokenade_core::debug_log::log(
            "index-touch",
            "detached",
            serde_json::json!({
                "spawned": !busy,
                "edit_diff_hint": hint_emitted,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    let idx = match index::Index::ensure_fresh(&r) {
        Ok(i) => i,
        Err(e) => {
            tokenade_core::debug_log::log(
                "index-touch",
                "build_err",
                serde_json::json!({"error": e.to_string()}),
            );
            return 0; // never block the agent: silent on failure
        }
    };
    // Semantic index incremental refresh. Cheap when no files changed.
    let sem = match semantic::SemanticIndex::load(&r) {
        Some(prev) if !prev.mtimes.is_empty() => {
            semantic::SemanticIndex::rebuild_incremental(&prev, &idx, &r)
        }
        _ => semantic::SemanticIndex::build_from_index(&idx, &r),
    };
    let _ = sem.save(&r);
    let _ = std::fs::remove_file(r.join(".tokenade").join("index-touch.running"));

    let elapsed_ms = started.elapsed().as_millis() as u64;
    tokenade_core::debug_log::log(
        "index-touch",
        "done",
        serde_json::json!({
            "files_indexed": idx.files_indexed,
            "symbols": idx.symbols.len(),
            "semantic_docs": sem.docs.len(),
            "elapsed_ms": elapsed_ms,
        }),
    );
    0
}

// `tokenade replay <hash> <age_secs>` — hidden; emitted ONLY by the
// PreToolUse replay rewrite (hooks/replay.rs). Prints a compact
// "unchanged" answer with the prior output's tail, and books the MEASURED
// saving: the re-run would have re-delivered the full prior output.

/// `tokenade trust-filters` — consent gate for PROJECT-LOCAL compactor
/// filters (`<cwd>/.tokenade/filters/*.toml`). Without trust they are
/// skipped: a cloned repo must not rewrite your agent's outputs silently.
pub(crate) fn cmd_trust_filters(rest: &[String]) -> i32 {
    // `--list` is the only flag, and it was read off `rest.first()` alone:
    // `trust-filters --lst` trusted every project filter it found, which is
    // exactly what the caller asking to only LOOK did not want.
    if let Err(code) =
        crate::code_intel::refuse_all_positionals(rest, "trust-filters", TRUST_FILTERS_FLAGS, &[])
    {
        return code;
    }
    if crate::help_routing::asks_for_help(rest) {
        println!(
            "usage: tokenade trust-filters [--list]\n\n\
             Project-local filters (.tokenade/filters/*.toml in the repo) run ONLY\n\
             after you trust them — a cloned repository must not get to rewrite\n\
             your agent's tool outputs silently. Trust is per file CONTENT:\n\
             editing a trusted file requires re-trusting it.\n\n\
             --list   show untrusted project filters without trusting anything"
        );
        return 0;
    }
    // Named BEFORE the trust list: trusting a file that cannot parse prints
    // "they now apply to matching command outputs" about a filter that can
    // never apply. It is no longer offered for trust at all.
    for (path, reason) in tokenade_core::user_filters::invalid_filters() {
        let first = reason.lines().next().unwrap_or(&reason).trim().to_string();
        eprintln!("skipped (does not parse): {} — {first}", path.display());
    }
    let pending = tokenade_core::user_filters::untrusted_project_filters();
    if pending.is_empty() {
        println!("no untrusted project-local filters under this directory.");
        return 0;
    }
    // Read anywhere in the arguments, not only in first position. Both this
    // and the help branch above used to test `rest.first()` alone, so
    // `trust-filters --json --list` went through the TRUSTING branch: the flag
    // that exists to look without trusting was silently dropped for being
    // second.
    let list_only = rest.iter().any(|a| a == "--list");
    for (path, name) in &pending {
        if list_only {
            println!("untrusted: {name} ({})", path.display());
            continue;
        }
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|src| {
                tokenade_core::user_filters::trust_filter(&src).map_err(|e| e.to_string())
            }) {
            Ok(_) => println!("trusted: {name} ({})", path.display()),
            Err(e) => eprintln!("tokenade trust-filters: {}: {e}", path.display()),
        }
    }
    if !list_only {
        println!(
            "{} filter(s) trusted — they now apply to matching command outputs. \
             Editing a file voids its trust.",
            pending.len()
        );
    }
    0
}

pub(crate) fn cmd_replay(rest: &[String]) -> i32 {
    let Some(hash) = rest.first() else {
        eprintln!("usage: tokenade replay <hash> <age-secs>");
        return 1;
    };
    let age: u64 = rest.get(1).and_then(|a| a.parse().ok()).unwrap_or(0);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let Some(prior) = tokenade_core::dedup_ref::lookup(&cwd, hash) else {
        // Stash rotated between rewrite and execution — degrade honestly.
        println!(
            "[tokenade:replay] state unchanged since the previous run {age}s ago, but the \
             cached output aged out — re-run the command if you need its bytes again."
        );
        return 0;
    };
    let tail_lines: Vec<&str> = prior.trim_end().lines().collect();
    let n = tail_lines.len().min(5);
    let tail = tail_lines[tail_lines.len() - n..].join("\n");
    let display = format!(
        "[tokenade:replay] unchanged — no edits and no state change since this exact \
         command ran {age}s ago in this session; its full output is above in your \
         context. Last {n} line(s) for reference:\n{tail}\n\
         (Full bytes without re-running: `tokenade expand-ref {hash}`.)"
    );
    let before = tokenade_core::tokenizer::estimate(&prior);
    let after = tokenade_core::tokenizer::estimate(&display);
    if before > after {
        tokenade_core::gain::record(
            "hook",
            "replay",
            "replay",
            tokenade_core::Savings::new(before, after),
        );
    }
    tokenade_core::debug_log::log(
        "replay",
        "replay_served",
        serde_json::json!({"hash": hash, "age_secs": age, "before": before, "after": after}),
    );
    println!("{display}");
    0
}

pub(crate) fn cmd_index(rest: &[String]) -> i32 {
    // #630 — `--json` mode for CI: emits rebuild stats (file count,
    // symbol count, doc count, mode, duration_ms) as a flat object.
    // Useful for time-series tracking — was the rebuild incremental?
    // How many files did it touch? How long?
    // #677 — silent-accept sweep continued (after #671 cmd_map):
    // until this iteration `tokenade index --bogus` silently
    // rebuilt the index, dropping the typo. cmd_index has no
    // positional args and only `--json` is a real flag, so reject
    // anything else up front.
    if let Some(extra) = rest.iter().find(|a| *a != "--json" && *a != "--force") {
        eprintln!(
            "tokenade index: unexpected argument '{extra}' (this command takes no positional \
             args; the flags are `--json` and `--force`)."
        );
        return 2;
    }
    let json_mode = rest.iter().any(|a| a == "--json");
    // `--force`: rebuild from scratch even when the sidecar looks fresh.
    //
    // The incremental path asks the filesystem "did any file change", and the
    // answer is no whenever what changed is tokenade ITSELF — a grammar, an
    // extraction rule, a tokenizer. Within one released version nothing can
    // notice that, so the only way to pick up such a change was to delete two
    // files by hand (`.tokenade/index.json` AND the row in
    // `~/.tokenade/index.db`), which is not a thing to ask of anybody.
    let force = rest.iter().any(|a| a == "--force");
    let r = root();
    let dir = r.join(".tokenade");
    let _ = fs::create_dir_all(&dir);
    let index_path = dir.join("index.json");

    // Incremental path: if a previous index exists with mtime data, only
    // re-walk files whose on-disk mtime is newer. On a 100k-file monorepo
    // this drops the rebuild from ~30 s to <1 s when nothing changed.
    let started = std::time::Instant::now();
    // `Index::load`, not a hand-rolled read of the sidecar.
    //
    // `load` refuses an index stamped by another tokenade, which is what makes
    // an upgrade pick up a changed extractor. Reading and deserialising the
    // file here skipped that guard, so `tokenade index` answered `incremental`
    // and reused the previous build's symbols while every query path in the
    // same binary correctly refused the same file. The semantic index two
    // dozen lines below has always gone through its own `load`; this is the
    // sibling that did not.
    let prev = if force { None } else { index::Index::load(&r) };
    let (idx, mode) = match prev {
        Some(prev) if !prev.mtimes.is_empty() => match index::Index::rebuild_incremental(&prev, &r)
        {
            Ok(i) => (i, "incremental"),
            Err(_) => (build_index(), "full (incremental failed)"),
        },
        _ if force => (build_index(), "full (forced)"),
        _ => (build_index(), "full"),
    };

    // Persist to the store every OTHER command reads.
    //
    // This wrote `.tokenade/index.json` and NOTHING else. Since storage moved
    // to the global `~/.tokenade/index.db`, that sidecar is only ever consulted
    // as a migration fallback for a project the db does not hold yet — so on
    // any project that has been queried once, a rebuild here landed where
    // nothing reads it and the db kept the previous blob.
    //
    // `--force` is the command that made it visible, because forcing is the
    // one thing the incremental path cannot do for itself: it exists for a
    // change in tokenade ITSELF (a grammar, an extraction rule, a tokenizer),
    // which no filesystem fingerprint can notice. Measured on a two-file
    // repository whose source changed with its size and mtime pinned:
    // `index --force` answered `mode: "full (forced)"`, and the very next
    // `query` still named the OLD symbol and denied the new one.
    //
    // The call graph is loaded FIRST. `Index::load` deliberately leaves it
    // behind and an incremental rebuild that saw no change never
    // fetches it, so saving straight away would persist an EMPTY graph over a
    // real one. A load failure skips the save rather than truncating it — the
    // same rule `ensure_fresh` follows.
    let mut idx = idx;
    let stored = match idx.load_calls(&r) {
        Ok(()) => idx.save(&r).is_ok(),
        Err(e) => {
            tokenade_core::debug_log::log(
                "index",
                "store_skipped_graph_unreadable",
                serde_json::json!({"root": r.display().to_string(), "error": e.to_string()}),
            );
            false
        }
    };
    // The sidecar is written ONLY when the store refused, and then it is the
    // one remaining copy `Index::load` can fall back to. When the store took
    // the index, `save` removes the sidecar for us — writing it back here is
    // what kept a 22 MB stale duplicate alive in every indexed project, and
    // undid `migrate_local_index` on every run.
    if !stored {
        let json = serde_json::to_string(&idx).unwrap();
        let _ = fs::write(&index_path, &json);
    }
    // Semantic index: incremental rebuild if a previous snapshot exists
    // with mtime data, otherwise full build.
    let (sem, sem_mode) = match if force {
        None
    } else {
        semantic::SemanticIndex::load(&r)
    } {
        Some(prev) if !prev.mtimes.is_empty() => (
            semantic::SemanticIndex::rebuild_incremental(&prev, &idx, &r),
            "incremental",
        ),
        // Reported separately from the symbol index's mode, because the two
        // can differ and only one of them answers the question `--force` is
        // usually asked for: the postings ARE the tokenizer's output, so a
        // forced rebuild that reused them would miss exactly the change the
        // caller was forcing for.
        _ => (semantic::SemanticIndex::build_from_index(&idx, &r), "full"),
    };
    let _ = sem.save(&r);
    let duration_ms = started.elapsed().as_millis();
    if json_mode {
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "root": r.display().to_string(),
            "files_indexed": idx.files_indexed,
            "symbols": idx.symbols.len(),
            "semantic_docs": sem.docs.len(),
            "mode": mode,
            "semantic_mode": sem_mode,
            "duration_ms": duration_ms,
            // Where the index actually landed. It named the sidecar
            // unconditionally, including on the runs that did not write one.
            "index_path": if stored {
                "the index store".to_string()
            } else {
                index_path.display().to_string()
            },
            // Whether the rebuild reached the store the other commands read.
            // A run that answers `false` has NOT changed what `query`,
            // `impact`, `map` or `semantic` will see.
            "stored": stored,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }
    println!(
        "indexed {} files, {} symbols → {} [{mode}, {} ms] (+ {} semantic docs)",
        idx.files_indexed,
        idx.symbols.len(),
        if stored {
            "the index store"
        } else {
            ".tokenade/index.json only (the store refused the write)"
        },
        duration_ms,
        sem.docs.len(),
    );
    0
}

/// The flags `watch` answers to. It takes none of its own; `--help` and `-h`
/// are routed centrally and are listed so the validator lets them through.
pub(crate) const WATCH_FLAGS: &[&str] = &["--help", "-h"];

/// The flags `trust-filters` answers to.
pub(crate) const TRUST_FILTERS_FLAGS: &[&str] = &["--help", "-h", "--list"];

/// `tokenade watch` — file-watcher that incrementally re-indexes on any source
/// file change. Debounces 500 ms so a fast save → re-save burst counts as
/// one rebuild. Exits on Ctrl+C / EOF.
pub(crate) fn cmd_watch(rest: &[String]) -> i32 {
    // This command took its arguments and dropped them on the floor: every
    // flag and every path was accepted in silence and none of them did
    // anything. A watcher that answers the same way to `watch --dry-run` and
    // to `watch` teaches the flag exists.
    if let Err(code) = crate::code_intel::refuse_all_positionals(rest, "watch", WATCH_FLAGS, &[]) {
        return code;
    }
    use notify::{event::EventKind, Config, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let r = root();
    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = match notify::Watcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = tx.send(ev);
            }
        },
        Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("tokenade watch: cannot create file watcher: {e}");
            return 1;
        }
    };
    if let Err(e) = watcher.watch(&r, RecursiveMode::Recursive) {
        eprintln!("tokenade watch: cannot watch {}: {e}", r.display());
        return 1;
    }
    eprintln!(
        "watching {} — incremental reindex on change (Ctrl+C to stop)",
        r.display()
    );

    // Build the initial index up-front so the first event is a cheap diff.
    // `cmd_index` prints its own duration so we don't need a wall-clock here.
    let _ = cmd_index(&[]);

    const DEBOUNCE: Duration = Duration::from_millis(500);
    let interesting = |ev: &notify::Event| -> bool {
        if !matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return false;
        }
        ev.paths.iter().any(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                matches!(
                    ext,
                    "rs" | "py"
                        | "js"
                        | "ts"
                        | "tsx"
                        | "jsx"
                        | "go"
                        | "java"
                        | "kt"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "rb"
                        | "php"
                        | "swift"
                        | "cs"
                        | "scala"
                        | "ex"
                        | "exs"
                )
            }) && !p.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == ".tokenade" || s == ".git" || s == "target" || s == "node_modules"
            })
        })
    };

    let mut last_trigger: Option<Instant> = None;
    loop {
        let ev = match rx.recv_timeout(DEBOUNCE) {
            Ok(e) => Some(e),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if let Some(ev) = ev {
            if interesting(&ev) {
                last_trigger = Some(Instant::now());
            }
        }
        if let Some(t) = last_trigger {
            if t.elapsed() >= DEBOUNCE {
                let _ = cmd_index(&[]);
                last_trigger = None;
            }
        }
    }
    0
}

/// `tokenade style chat|coding|off` — pick a response shape that hooks inject
/// before every user prompt. Tokenless reports ~80 % response-token cut
/// on conversational workloads with the `chat` preset.
pub(crate) fn cmd_style(rest: &[String]) -> i32 {
    use tokenade_core::style::{self, Style};
    let r = root();
    // `--preamble [--prompt <text>]` — print exactly the text a prompt-time
    // hook would inject, and nothing at all when the answer is "inject
    // nothing" (style off, or `auto` on an ambiguous prompt).
    //
    // It exists because the preamble had exactly one way out of the binary: a
    // Claude-shaped `UserPromptSubmit` hook. Every host without that shape —
    // the six plugin BRIDGES, which have a message seam and no hook — could
    // not reach it, and the compat bench reported "no prompt-time hook exists
    // for this agent" on thirteen rows. A caller that is not a hook needs a
    // way to ASK, so this is it: one resolution, in the binary, for every
    // channel that wants it.
    if rest.iter().any(|a| a == "--preamble") {
        let prompt = rest
            .iter()
            .position(|a| a == "--prompt")
            .and_then(|i| rest.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("");
        let text = style::load(&r).resolve(prompt).preamble();
        if !text.is_empty() {
            println!("{text}");
            // `--book`: the caller INJECTS what it asked for. Charged like
            // every other injection, under the session it lands in — a bridge
            // that put the preamble in each request had it cost nothing.
            if rest.iter().any(|a| a == "--book") {
                if let Some(sid) = rest
                    .iter()
                    .position(|a| a == "--session")
                    .and_then(|i| rest.get(i + 1))
                {
                    let skey: String = sid
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .take(40)
                        .collect();
                    tokenade_core::session::set_session_key_authoritative(&skey);
                }
                crate::log_emitted_additional_context("style-preamble", "prompt", text);
            }
        }
        return 0;
    }
    let Some(arg) = rest.first() else {
        let current = style::load(&r);
        println!(
            "current style: {} (set with `tokenade style auto|chat|coding|off`)\n  \
             auto    (default) pick chat/coding per prompt automatically\n  \
             chat    concise human-readable answers\n  \
             coding  one dense ASCII status line per tool call\n  \
             off     never touch the agent's voice",
            current.as_str()
        );
        return 0;
    };
    let Some(style) = Style::parse(arg) else {
        eprintln!("tokenade style: unknown style `{arg}`. Try `auto`, `chat`, `coding`, or `off`.");
        return 2;
    };
    match style::save(&r, style) {
        Ok(()) => {
            // Print the path `style::save` WROTE, not a guess at it: the two
            // disagreed, and the guess named a project file that never exists.
            println!(
                "style set to `{}` → {} (applies to every project on this machine)",
                style.as_str(),
                style::config_path(&r).display()
            );
            match style {
                Style::Auto => {
                    println!(
                        "per-prompt auto-selection; the preamble is injected on style \
                              transitions (not every turn), via the UserPromptSubmit hook."
                    );
                    println!("(Pin a single style with `tokenade style chat|coding`, or `off`.)");
                }
                Style::Chat | Style::Coding => {
                    println!(
                        "preamble injected before each user prompt by the UserPromptSubmit hook."
                    );
                    println!("(Disable any time with `tokenade style off`.)");
                }
                Style::Off => {}
            }
            0
        }
        Err(e) => {
            eprintln!("tokenade style: failed to save: {e}");
            1
        }
    }
}

/// Shared fast-path: load + incremental-refresh + persist. Subsequent
/// CLI invocations (query, impact, map, skeleton) reuse the warm
/// cache. Falls back to a full build on cold start. Logs the cold-
/// build noise via the eprintln! lines in `Index::build` itself.
pub(crate) fn fresh_index() -> index::Index {
    let mut idx = fresh_index_at_root();
    rebase_to_cwd(&mut idx);
    idx
}

fn fresh_index_at_root() -> index::Index {
    index::Index::ensure_fresh(&root()).unwrap_or_else(|e| {
        eprintln!("tokenade: index build failed: {e}");
        std::process::exit(1);
    })
}

/// `ensure_fresh` has already persisted whatever changed, so re-basing the
/// copy we hand out cannot reach the store — and `Index::save` refuses a
/// re-based index anyway. Every path the user sees, and every path they type,
/// is relative to where they are standing.
fn rebase_to_cwd(idx: &mut index::Index) {
    let cwd = env::current_dir().unwrap_or_else(|_| root());
    idx.rebase_to(&cwd);
}

/// As [`fresh_index`], plus the call graph. Only the two commands that read
/// it pay for it: `query` prints `called by:` and `impact` attributes calls to
/// dependents. `map`, `skeleton` and `semantic` deliberately do not.
pub(crate) fn fresh_index_with_calls() -> (index::Index, bool) {
    // The calls are loaded BEFORE the re-base, so they move with the symbols.
    // Loaded after it, they kept root-relative paths beside cwd-relative
    // symbols: from `src/`, `query alpha_fn` printed `lib.rs:1` and
    // `called by: beta_fn (src/lib.rs:2)`, and a qualified call
    // (`crate::alpha_fn()` in `src/g.rs`) matched no file and vanished from
    // `called by` — the answer depended on the directory it was asked from.
    let mut idx = fresh_index_at_root();
    let loaded = idx.load_calls(&root());
    rebase_to_cwd(&mut idx);
    match loaded {
        Ok(()) => (idx, true),
        Err(e) => {
            // Not fatal — the symbol half of the answer is intact. But the
            // caller MUST know, because an empty call graph and a graph that
            // could not be read produce the same output, and that output is a
            // positive claim: "nothing calls this". Mined from `entroly`
            // (2026-08-28): an absent scan and a clean scan must not serialise
            // to the same thing.
            eprintln!("tokenade: call graph unavailable: {e}");
            (idx, false)
        }
    }
}
