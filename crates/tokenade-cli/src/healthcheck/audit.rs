//! **`audit-claude-md`** — what the project's rules file promises.
//!
//! A `CLAUDE.md` is read by the agent at every session: what is written in it
//! costs tokens on every turn, and what is written WRONG in it steers the
//! agent against the repository.
//!
//! The audit lives here and not in the health checks because its subject is
//! the PROJECT, not the installation: a `CLAUDE.md` can be bad on a perfect
//! installation.

use super::*;

/// #831 — Audit the always-resident CLAUDE.md cascade. Reports per-file
/// token cost, the total prefix size, and the lines duplicated *across*
/// files (the recoverable waste). Accepts an optional positional starting
/// directory (defaults to cwd) and `--json`.
pub(crate) fn cmd_audit_claude_md(rest: &[String]) -> i32 {
    let mut json_mode = false;
    let mut apply = false;
    let mut compress = false;
    let mut stale = false;
    let mut start: Option<PathBuf> = None;
    for a in rest {
        if a == "--json" {
            json_mode = true;
        } else if a == "--apply" {
            apply = true;
        } else if a == "--compress" {
            compress = true;
        } else if a == "--stale" {
            stale = true;
        } else if a.starts_with('-') {
            eprintln!(
                "tokenade audit-claude-md: unknown flag '{a}' \
                 (try `tokenade audit-claude-md --help`)."
            );
            return 2;
        } else if start.is_none() {
            start = Some(PathBuf::from(a));
        } else {
            eprintln!(
                "tokenade audit-claude-md: unexpected extra argument '{a}' \
                 (takes at most one starting directory)."
            );
            return 2;
        }
    }
    let start = start.unwrap_or_else(root);
    let home = tokenade_core::platform::home_dir();

    // `--apply`: rewrite the cascade in place (exact + semantic passes),
    // then report what was removed. The unattended SessionStart hook runs
    // the exact-only variant automatically; this is the manual full pass.
    if apply {
        let res = tokenade_core::claude_md_audit::apply_dedup(&start, home.as_deref());
        let saved = res.tokens_before.saturating_sub(res.tokens_after);
        if json_mode {
            let root = serde_json::json!({
                "applied": true,
                "files_modified": res.files_modified,
                "tokens_before": res.tokens_before,
                "tokens_after": res.tokens_after,
                "tokens_saved": saved,
            });
            println!("{}", serde_json::to_string_pretty(&root).unwrap());
        } else if res.files_modified.is_empty() {
            println!(
                "cascade already deduplicated — nothing to remove ({} tokens).",
                res.tokens_before
            );
        } else {
            println!(
                "deduplicated cascade: -{saved} tokens ({} → {}). rewrote {} file(s):",
                res.tokens_before,
                res.tokens_after,
                res.files_modified.len()
            );
            for f in &res.files_modified {
                println!("  {f}");
            }
        }
        return 0;
    }

    // `--compress`: lossless whitespace compaction of the cascade in place
    // (#9). Content-preserving, so safe to run unattended.
    if compress {
        let res = tokenade_core::claude_md_audit::apply_compress(&start, home.as_deref());
        let saved = res.tokens_before.saturating_sub(res.tokens_after);
        if json_mode {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "compressed": true,
                    "files_modified": res.files_modified,
                    "tokens_saved": saved,
                }))
                .unwrap()
            );
        } else if res.files_modified.is_empty() {
            println!("cascade already whitespace-minimal — nothing to compress.");
        } else {
            println!(
                "whitespace-compressed cascade: -{saved} tokens across {} file(s).",
                res.files_modified.len()
            );
        }
        return 0;
    }

    // `--stale`: report cascade file references that no longer resolve (#8).
    if stale {
        let cascade = tokenade_core::claude_md_audit::read_cascade(&start, home.as_deref());
        let stale_refs = tokenade_core::claude_md_audit::stale_refs(&cascade, &start);
        if json_mode {
            let items: Vec<_> = stale_refs
                .iter()
                .map(|s| serde_json::json!({"source": s.source_file, "reference": s.reference}))
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"stale": items})).unwrap()
            );
        } else if stale_refs.is_empty() {
            println!("no stale file references in the rules cascade.");
        } else {
            println!(
                "{} stale file reference(s) — pointed at paths not found under {}:",
                stale_refs.len(),
                start.display()
            );
            for s in &stale_refs {
                println!(
                    "  {} → missing (referenced in {})",
                    s.reference, s.source_file
                );
            }
        }
        return 0;
    }

    let cascade = tokenade_core::claude_md_audit::read_cascade(&start, home.as_deref());
    let audit = tokenade_core::claude_md_audit::audit(&cascade);

    if json_mode {
        let files: Vec<serde_json::Value> = audit
            .files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "path": f.path,
                    "bytes": f.bytes,
                    "lines": f.lines,
                    "tokens": f.tokens,
                })
            })
            .collect();
        let dups: Vec<serde_json::Value> = audit
            .duplicates
            .iter()
            .map(|d| {
                serde_json::json!({
                    "text": d.text,
                    "tokens": d.tokens,
                    "wasted_tokens": d.wasted_tokens(),
                    "files": d.files,
                })
            })
            .collect();
        let root = serde_json::json!({
            "total_tokens": audit.total_tokens,
            "duplicated_tokens": audit.duplicated_tokens,
            "file_count": audit.files.len(),
            "files": files,
            "duplicates": dups,
        });
        println!("{}", serde_json::to_string_pretty(&root).unwrap());
    } else {
        print!("{}", tokenade_core::claude_md_audit::render(&audit));
    }
    0
}
