#![allow(unused_imports)]
//! Session/PreCompact/UserPrompt lifecycle hook handlers, split out of the
//! monolithic `hooks.rs` (mechanical module split, no behaviour change).
//! Wired back into the `hooks` namespace by mod.rs via `pub(crate) use session_lifecycle::*;`.
mod session_state;
mod write_tracking;
use super::*;
use crate::*;
pub(crate) use session_state::*;
pub(crate) use write_tracking::*;
mod compaction;
mod credit;
mod tool_tick;
pub(crate) use compaction::*;
pub(crate) use credit::*;
pub(crate) use tool_tick::*;

/// `tokenade hookwrap-sessionstart` — Claude Code `SessionStart`
/// hook entry point. Reads the hook JSON from stdin
///
///     { "hook_event_name": "SessionStart", "source": "startup"|"resume"|"compact" }
///
/// and emits a ONE-LINE `additionalContext` describing what tokenade
/// has indexed for THIS project: semantic-index doc count. The agent
/// then knows to reach for `semantic_search` / `symbol_find` /
/// `structure_map` instead of re-discovering via Read/Grep.
///
/// #449 cleanup: this used to also advertise `memory_recall` and
/// `wiki_get` doc counts, but both MCP tools were retired (memory
/// subsystem replaced by Claude Code's native auto-memory; wiki was
/// never finished beyond a name). The hint is now scoped to what
/// actually ships.
///
/// Total cost: ~30-50 tokens once per session boot. Even if the
/// agent ignores it, the line is small enough not to matter.
///
/// Bypass:
///   - `TOKENADE_HOOK_DISABLED=1` (or `STS_HOOK_DISABLED=1`) skips all.
///   - Empty project (no semantic index) → silent (no line worth emitting).
///   - Source==`compact` → silent (a mid-session compaction doesn't
///     need to re-advertise the same state).
pub(crate) fn cmd_hookwrap_sessionstart() -> i32 {
    let _p_all = tokenade_core::profile::phase("sessionstart:TOTAL");
    if hook_disabled_by_env() {
        return 0;
    }
    let started = std::time::Instant::now();
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-userprompt") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        return 0;
    };
    // Session-tag every row this hook process books (incl. the output-lean
    // and compounding ESTIMATES — the session ticker counts them too).
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }

    // Every store tokenade keeps on disk had a retention policy on paper that
    // nothing invoked. The byte caps were reachable only through the manual
    // `evict-stale` command; Measured on this install 2026-08-03 — dedup.db held
    // 161 583 703 bytes against a 150 MiB cap, with an empty freelist, so that
    // was real content over the bound rather than a file needing VACUUM. A user
    // who never types a maintenance command never gets the bound they were
    // promised, and disk is the one resource we take from them silently.
    //
    // SessionStart, once a day, is the right home: it is the only hook that
    // fires per session rather than per tool call, and the sweep is O(store) —
    // exactly the shape that must never sit on the hot path (see
    // `platform::tighten_stash` for the version of that mistake we already
    // paid for). Best-effort throughout: a failure here must never affect the
    // session.
    {
        let _p = tokenade_core::profile::phase("sessionstart:daily_sweep");
        daily_local_state_sweep();
    }

    let source = parsed
        .get("source")
        .or_else(|| parsed.get("hook_source"))
        .and_then(|s| s.as_str())
        .unwrap_or("startup");
    if source == "compact" {
        return emit_compact_digest(started, crate::hooks::session_id_of(&parsed));
    }
    // A `/clear` that keeps the session id empties the context like a
    // compaction (Claude Code opens a new session instead: then this resets
    // nothing).
    if source == "clear" {
        compaction::note_context_reset(crate::hooks::session_id_of(&parsed).unwrap_or(""));
    }

    // Windows: repair a HOME→USERPROFILE home split BEFORE reading ~/.tokenade
    // (so an updated install finds its migrated ledger). No-op off
    // Windows and after the first successful migration (marker-gated).
    {
        let _p = tokenade_core::profile::phase("sessionstart:migrate_home");
        tokenade_core::platform::migrate_windows_home_once();
    }

    let root = root();
    let home = tokenade_core::platform::home_dir();

    // Restore rules a 1.x quota pause retracted from this project (no-op when
    // nothing was retracted: one `stat`).
    let _ = tokenade_core::project_scaffold::sync_gate_state(&root, false);

    // Hook-first auto-scaffold (2026-06 pivot): ensure this project's
    // agent-rules files carry the tokenade CLI nudge. Idempotent and
    // flag-gated, so it's a no-op after the first session. This is the
    // session-after-session scaffold trigger that used to live in the
    // MCP `initialize` handler — now that the MCP server isn't installed
    // by default on Claude Code, the SessionStart hook owns it.
    let scaffold = tokenade_core::profile::timed("sessionstart:scaffold_ensure", || {
        tokenade_core::project_scaffold::ensure(&root)
    });

    // Cold-`/resume` transcript compaction. Claude re-sends a resumed session's
    // whole history, so large stale tool_result payloads are re-billed on every
    // resume. Mask the biggest ones in this project's OTHER (non-active, idle)
    // transcripts so a future resume rebuilds cheaper — NEVER the live file, and
    // recoverable via the stash. No gain is booked here; the saving is credited
    // (MEASURED) only when a session is actually resumed, below. Opt out:
    // TOKENADE_NO_SESSION_COMPACT=1.
    let transcript_path = parsed
        .get("transcript_path")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    compact_session_transcripts(&parsed, transcript_path, source, &home);

    // 0.6.4 — book the scaffold's per-session OVERHEAD. The tokenade rules
    // block lives in the project CLAUDE.md and is loaded into the model's
    // context every session, so it's a real (small) cost. Now that we also
    // credit the savings it enables (output-lean, batching), honest net
    // accounting must subtract it — counting one side without the other
    // would inflate. `class=overhead` → the dashboard treats `saved` as a
    // cost to subtract. Recorded once per session_id: a resume re-fires
    // SessionStart but reuses the id, so a marker file dedups it.
    if let Some(h) = home.as_ref() {
        let session_id = crate::hooks::session_id_of(&parsed).unwrap_or("");
        if !session_id.is_empty() {
            let marker = h
                .join(".tokenade")
                .join("sessions")
                .join(format!("{}-overhead", sanitize_session_id(session_id)));
            if !marker.exists() {
                if let Some(dir) = marker.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&marker, b"1");
                // What this agent's rules files hold, not what we would write:
                // a block the user trimmed or removed costs what is left.
                let cwd = std::env::current_dir().ok();
                let agent = match tokenade_core::detect::from_env() {
                    tokenade_core::detect::Agent::Unknown => {
                        tokenade_core::detect::Agent::ClaudeCode
                    }
                    a => a,
                };
                let cost =
                    tokenade_core::project_scaffold::loaded_block_tokens(agent, h, cwd.as_deref());
                if cost > 0 {
                    gain_log::record_estimated(
                        "hookwrap-sessionstart",
                        "scaffold_overhead",
                        "",
                        "overhead",
                        tokenade_core::Savings::new(cost, 0),
                    );
                }
            }
        }
    }

    // Self-heal the hook suite itself (not just CLAUDE.md): if another tool or
    // an agent upgrade overwrote settings.json, the session that still runs
    // this hook re-asserts the full suite. Sentinel-gated, no throttle —
    // idempotent, so `AlreadyPresent` writes nothing.
    //
    // Two corrections to what this comment used to say, both measured
    // 2026-08-03:
    //   * it claimed "rare" as the reason the throttle is bypassed. SessionStart
    //     fires on every session, every resume and every /clear. Not rare.
    //   * a first pass of mine then blamed this call for SessionStart's ~1 s
    //     wall time. That was WRONG: `tokenade ensure-hooks` on the real home
    //     runs in 4 ms, and a synthetic home with the install sentinel and a
    //     real settings.json completes the whole hook in 14 ms. The second is
    //     still unattributed (the SQLite stores, the stash and the agent-detection
    //     scan have each been ruled out).
    //
    // One property does hold regardless of where the time goes: a repair made
    // here cannot help the session that waits for it, because the agent read
    // its settings.json before this hook ever ran. It only ever takes effect on
    // the NEXT session.
    {
        let _p = tokenade_core::profile::phase("sessionstart:ensure_hooks");
        let _ = ensure_hooks(home.as_deref(), false);
    }

    // Cascade dedup (restored 2026-06): Claude Code loads EVERY CLAUDE.md
    // in the ancestor chain (global ~/.claude → project) into context at
    // boot. Nested projects + a global file accumulate the same boilerplate
    // (tokenade scaffold blocks, repeated house rules) at every level, all
    // paid for on each session. This pass removes lines that appear in more
    // than one cascade file, keeping each in its most-specific file. Exact-
    // match only (no fuzzy paraphrase removal) so it's safe to run
    // unattended; idempotent and silent. This trigger used to live in the
    // MCP `initialize` handler and was orphaned by the 2026-06 zero-MCP
    // pivot — the SessionStart hook now owns it, same as the scaffold.
    let dedup = tokenade_core::claude_md_audit::apply_dedup_exact(&root, home.as_deref());
    if !dedup.files_modified.is_empty() {
        tokenade_core::debug_log::log(
            "hookwrap-sessionstart",
            "cascade_dedup",
            serde_json::json!({
                "files_modified": dedup.files_modified,
                "tokens_before": dedup.tokens_before,
                "tokens_after": dedup.tokens_after,
                "tokens_saved": dedup.tokens_before.saturating_sub(dedup.tokens_after),
            }),
        );
    }

    let mut additions: Vec<String> = Vec::new();

    // No scaffold additionalContext (2026-06, A/B-proven): an earlier
    // version re-injected the scaffold rules block via additionalContext
    // on the session that first wrote/upgraded CLAUDE.md, on the theory
    // that Claude Code loads CLAUDE.md BEFORE SessionStart hooks run (so a
    // freshly-written file would be invisible until the next session).
    // Real bench data refutes that theory: a workspace that scaffolds
    // CLAUDE.md and emits NO additionalContext keeps the full behavioural
    // gain — code-migration-py stays at 7.0 turns / −36% vs control,
    // identical to AC-on, score 1.0 — while shedding ~560-710 cache_creation
    // tokens per session (which halved the fixed-tax loss on short 2-turn
    // tasks: config-peek +12.3% → +5.9%, doc-digest +6.6% → +2.4%). The
    // CLAUDE.md IS loaded this session, so the AC was pure duplication paid
    // for (created + re-read) every single session. The file is now the
    // sole channel for the rules block; the AC is gone.
    let just_scaffolded = scaffold.files_written.iter().any(|f| f == "CLAUDE.md");

    // Best-effort: a failed semantic-db open is silent — we don't
    // want a hook to ever crash a session boot.
    let semantic_docs = tokenade_core::semantic_db::open()
        .ok()
        .and_then(|conn| {
            tokenade_core::semantic_db::load_blob(&conn, &root)
                .ok()
                .flatten()
        })
        .map(|blob| blob.len() as u64)
        .unwrap_or(0);

    if semantic_docs > 0 {
        additions.push(format!(
            "[tokenade:project-state] semantic index {} KB — reach for `tokenade semantic \"<query>\"` / `tokenade query <symbol>` / `tokenade map` instead of re-discovering via Read/Grep.",
            (semantic_docs / 1024).max(1),
        ));
    }

    // Recorded dead ends. `mistakes` exists so the agent "doesn't cheerfully
    // re-propose it next session" — its own opening line — and `load` had
    // exactly one caller: the `tokenade mistake list` subcommand a human has to
    // type. Written, and never read where it mattered.
    //
    // Safe to add to a channel this file is otherwise strict about, because it
    // emits NOTHING on an empty ledger: a user who has recorded no dead end
    // pays no tokens for this line, which is every user until they record one.
    if let Some(line) = tokenade_core::mistakes::session_reminder(
        &tokenade_core::mistakes::load(),
        tokenade_core::mistakes::REMINDER_SHOWN,
    ) {
        additions.push(line);
    }

    if additions.is_empty() {
        // Brand-new project, scaffold already current → nothing to add.
        return 0;
    }
    let line = additions.join("\n\n");
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": line,
        }
    });
    log_emitted_additional_context("hookwrap-sessionstart", "SessionStart", &line);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-sessionstart",
        "state_injected",
        serde_json::json!({
            "source": source,
            "semantic_docs": semantic_docs,
            "scaffold_written": just_scaffolded,
            "line_chars": line.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// `tokenade hookwrap-userprompt` — Claude Code `UserPromptSubmit`
/// hook entry point. Reads the hook JSON from stdin
///
///     { "hook_event_name": "UserPromptSubmit", "prompt": "…" }
///
/// loads the persisted response style (`~/.tokenade/style.json`,
/// set by `tokenade style auto|chat|coding|off`), and emits the style
/// preamble via `hookSpecificOutput.additionalContext` — Claude Code
/// then prepends it to the model's view of the conversation for THIS
/// turn only.
///
/// Preamble cost: ~50 tokens. Reward: agents return concise responses
/// (style=chat trims ~80% of conversational filler; style=coding
/// produces dense ASCII status lines instead of paragraphs).
///
/// Bypass:
///   - `tokenade style off` → style is `Style::Off` → empty preamble
///     → handler emits nothing (exit 0, no JSON), passthrough.
///   - `TOKENADE_HOOK_DISABLED=1` (or `STS_HOOK_DISABLED=1`) → all hooks off.
///   - Malformed stdin → log + passthrough (no crash).
pub(crate) fn cmd_hookwrap_userprompt() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let started = std::time::Instant::now();
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-precompact") else {
        return 0;
    };
    // Hook JSON parse — failures are silent so we never crash a session.
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        return 0;
    };
    // Session-tag every row this hook process books (incl. the output-lean
    // and compounding ESTIMATES — the session ticker counts them too).
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }

    let prompt = parsed.get("prompt").and_then(|p| p.as_str()).unwrap_or("");
    if prompt.is_empty() {
        return 0;
    }
    // Style preference. `Auto` (the default) classifies THIS prompt into a
    // concrete preset; the fixed presets pass through. `Off` / an ambiguous
    // auto-classification → empty preamble → nothing to add.
    let root = root();
    let style = tokenade_core::style::load(&root);
    let effective = style.resolve(prompt);
    let preamble = effective.preamble();
    // 0.6.4 — session-end-free accounting, all delta-based & keyed by
    // session_id. Bump the turn clock first, then credit compounding
    // (compaction surviving into this turn), output-lean and native
    // batching. Cheap, best-effort, never affects the emit.
    // MUST run before the preamble gate below: with style Off (or an
    // ambiguous auto-classification) the empty-preamble return used to
    // skip the WHOLE per-turn accounting — turn clock frozen, compounding
    // and output-lean never credited, overhead never re-billed (lab catch
    // 2026-07-04).
    bump_session_turn(&parsed);
    // Fail-open time budget (field report 2026-07-14): the recorders below
    // are best-effort telemetry, never this hook's deliverable — that's the
    // preamble emit further down. A slow accounting step must never consume
    // Claude Code's 30s hook timeout and get the preamble "output discarded".
    // The scan behind `record_userprompt_estimates` is now byte-capped at the
    // source (session_estimate_cached), so this is belt-and-suspenders: if the
    // process is somehow already near budget, skip accounting and go straight
    // to the emit. Cheap insurance against any future heavy step landing here.
    const ACCOUNTING_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    if started.elapsed() < ACCOUNTING_BUDGET {
        // Output-lean/batching credit the scaffold's effect; the overhead
        // re-bill prices its injected cost. Compounding prices past folds
        // still resident in this session's context.
        record_compounding_estimate(&parsed);
        record_overhead_rebill(&parsed);
        record_userprompt_estimates(&parsed);
        // Cache-thrash telemetry (2026-07-12): detect full-context cache
        // rewrites inside the live cache window; neutral `thrash:*` rows in
        // the local ledger. Measurement only, never blocks, stays local.
        super::cache_health::record_thrash_events(&parsed);
    } else {
        tokenade_core::debug_log::log(
            "hookwrap-userprompt",
            "accounting_skipped_budget",
            serde_json::json!({ "elapsed_ms": started.elapsed().as_millis() as u64 }),
        );
    }
    if preamble.is_empty() {
        return 0;
    }

    // Anti-spam: the preamble is a standing instruction that stays in the
    // conversation once injected. Re-stating it every turn wastes ~50 input
    // tokens/turn. Inject ONLY when the effective style is not already the one
    // active in this session — i.e. first turn, a style TRANSITION
    // (coding↔chat), or after a PreCompact reset (which clears the marker so
    // the squashed instruction gets re-stated). Consecutive same-style turns
    // skip the emit.
    if style_already_active(&parsed, effective) {
        tokenade_core::debug_log::log(
            "hookwrap-userprompt",
            "preamble_skipped_sticky",
            serde_json::json!({
                "style": effective.as_str(),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    mark_style_active(&parsed, effective);

    // Emit the additionalContext. Claude Code prepends this to the
    // model's view of the conversation (per its hook spec) without
    // editing the user's actual prompt text.
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": preamble,
        }
    });
    log_emitted_additional_context("hookwrap-userprompt", "UserPromptSubmit", preamble);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-userprompt",
        "preamble_injected",
        serde_json::json!({
            "style": effective.as_str(),
            "auto": style == tokenade_core::style::Style::Auto,
            "preamble_chars": preamble.len(),
            "prompt_chars": prompt.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

#[cfg(test)]
include!("session_lifecycle_tests.rs");
