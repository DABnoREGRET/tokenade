//! **A secret leaked into what we stored.**
//!
//! The worst case for this product: an API key passed through a tool output,
//! stored in the dedup database, and returned as is by `expand-ref` months
//! later.
//!
//! Hence the shape of these commands — a dry run first, which SAYS how many
//! occurrences and where, then the confirmation. An erasure that hits the
//! wrong target destroys recoverable context; an erasure that did not happen
//! leaves the secret. Both mistakes are serious, and only the first one is
//! reversible.

use super::*;

/// `tokenade scrub-secret <substring> [--dry-run]` — first-class
/// scrubbing tool for the "I accidentally leaked X into a logged
/// command" emergency. Codifies the manual sed+sqlite workflow once used
/// to clean up a leaked password: walks every
/// tokenade-owned text-or-sqlite file, replaces every occurrence of
/// `<substring>` with `<redacted>`, reports per-location counts.
///
/// Covered:
///   * `~/.tokenade/debug.log`       (JSONL)
///   * `~/.tokenade/gain.jsonl`      (JSONL)
///   * `~/.tokenade/dedup.db`        (sqlite, content column)
///   * `~/.cache/tokenade/stash/*`   (every file in the stash dir)
///
/// NOT touched: Claude Code transcripts (`~/.claude/projects/.../*.jsonl`)
/// belong to the agent runtime, not tokenade — out of scope.
pub(crate) fn cmd_scrub_secret(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("usage: tokenade scrub-secret <substring> [--dry-run] [--yes]");
        println!();
        println!("Replaces every occurrence of <substring> with `<redacted>` in:");
        println!("  • every ~/.tokenade/*.jsonl and *.log (debug.log, gain.jsonl, …)");
        println!("  • ~/.tokenade/dedup.db");
        println!("  • ~/.tokenade/semantic.db (affected project indexes are DROPPED, not patched)");
        println!(
            "  • ~/.tokenade/file_search.db (affected files' chunks are DROPPED, not patched)"
        );
        println!("  • ~/.cache/tokenade/stash/*");
        println!();
        println!("`--dry-run` reports counts without modifying any file.");
        println!();
        println!("The files are modified IN PLACE and NO backup is kept: a scrub that");
        println!("kept one would keep the secret. So a real run shows what it found and");
        println!("asks before writing. `--yes` answers that question up front, and is");
        println!("REQUIRED when stdin is not a terminal — an agent cannot rewrite your");
        println!("stores without being told to.");
        println!();
        println!("AFTER scrubbing, rotate the secret if it's a credential");
        println!("(the file scrub doesn't undo any prior exfiltration).");
        return 0;
    }
    // #657 — reject unknown flags + extra positionals up front. Before
    // this, `scrub-secret foo --dryrun` (typo) silently performed a
    // real scrub, and `scrub-secret foo bar` dropped "bar" without
    // warning (user forgot to quote a space-bearing secret).
    if let Err(code) = validate_scrub_secret_flags(rest) {
        return code;
    }
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    let needle: Option<&String> = rest
        .iter()
        .find(|a| !a.starts_with("--") && !a.starts_with('-'));
    let Some(needle) = needle else {
        eprintln!("usage: tokenade scrub-secret <substring> [--dry-run]");
        eprintln!("       (run with --help for details)");
        return 2;
    };
    if needle.is_empty() {
        eprintln!("tokenade scrub-secret: <substring> cannot be empty");
        return 2;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("tokenade scrub-secret: HOME not resolvable");
        return 1;
    };
    let assume_yes = rest.iter().any(|a| a == "--yes");
    // Preview FIRST when this run will actually write: the user sees the same
    // per-file counts the real pass prints, then decides. A scrub is
    // irreversible BY DESIGN — it used to leave the old bytes in a
    // `<file>.tokenade-original` beside each one, which is a copy of exactly
    // what the command exists to destroy (measured 2026-09-03: "scrubbed 3002
    // occurrence(s)" with 3000 of them still on disk). With no backup to fall
    // back on, the preview and the choice are what stand in its place.
    if !dry_run {
        let found = run_scrub(needle, &home, ScrubMode::Preview);
        if found == 0 {
            return 0;
        }
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if !confirm_scrub(found, assume_yes) {
            // stderr, like the refusal itself: a cancelled destructive run is
            // not a result, and mixing the two streams put "cancelled" above
            // the reason for it.
            eprintln!("  cancelled — nothing was modified.");
            // A user who answered "no" got what they asked for; a run with
            // nobody to ask and no `--yes` is a usage error.
            return if assume_yes || interactive { 0 } else { 2 };
        }
    }
    run_scrub(
        needle,
        &home,
        if dry_run {
            ScrubMode::DryRun
        } else {
            ScrubMode::Apply
        },
    );
    0
}

/// What a pass is for. THREE, not two: a pass that writes, a pass the user
/// asked for with `--dry-run`, and the preview taken before the confirmation.
/// The first two were the only ones, so the preview announced itself as
/// `[scrub-secret --dry-run]` and suggested re-running without a flag the user
/// had never typed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrubMode {
    Apply,
    DryRun,
    Preview,
}

/// One pass of the scrub. Returns how many occurrences it found.
///
/// Parameterised on the mode so the SAME code answers "what would change?" and
/// "change it": the preview a user confirms has to be produced by the pass that
/// will run, not by a second implementation of it that can drift.
pub(crate) fn run_scrub(needle: &str, home: &std::path::Path, mode: ScrubMode) -> usize {
    let dry_run = mode != ScrubMode::Apply;
    let replacement = "<redacted>";
    let glyph = if dry_run { "◐" } else { "✓" };
    let verb = match mode {
        ScrubMode::Apply => "replaced",
        ScrubMode::DryRun => "would replace",
        ScrubMode::Preview => "will replace",
    };
    let mut total = 0usize;
    let prefix = match mode {
        ScrubMode::Apply | ScrubMode::Preview => "[scrub-secret] ",
        ScrubMode::DryRun => "[scrub-secret --dry-run] ",
    };
    println!("{prefix}scanning for occurrences of {needle:?}");

    // 1. Text/JSONL files: read, count, replace, write.
    // Every text ledger tokenade owns, not two of them. `gain.jsonl` was
    // listed and `gain.rollup.jsonl` was not, though a rotation moves rows
    // from one into the other; `mistakes.jsonl` and `bypass_lost.jsonl` carry
    // command lines for the same reason `gain.jsonl` does.
    let tdir = home.join(".tokenade");
    let text_targets = tokenade_core::platform::state_text_ledgers(&tdir);
    let text_targets: &[std::path::PathBuf] = &text_targets;
    for path in text_targets {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let count = content.matches(needle).count();
        if count == 0 {
            continue;
        }
        if !dry_run {
            let new_content = content.replace(needle, replacement);
            if let Err(e) = tokenade_core::paths::write_atomic(path, &new_content) {
                eprintln!("  ✗ {}: write failed: {e}", path.display());
                continue;
            }
        }
        println!(
            "  {glyph} {} — {verb} {count} occurrence(s)",
            path.display()
        );
        total += count;
    }

    // 2. dedup.db content column — delegated to tokenade_core to
    // avoid pulling rusqlite into tokenade-cli.
    let dedup_count = tokenade_core::dedup_ref::count_content_matches(needle);
    if dedup_count > 0 {
        if !dry_run {
            let touched = tokenade_core::dedup_ref::replace_content_substring(needle, replacement);
            println!("  {glyph} dedup.db — {verb} substring in {touched} row(s)");
        } else {
            println!("  {glyph} dedup.db — {verb} substring in {dedup_count} row(s)");
        }
        total += dedup_count;
    }

    // 3. semantic.db — DELETE, never patch.
    //
    // A semantic index is seeded with each file's symbols AND its first
    // comments, so a credential written in a comment is copied into a global
    // store outside the repository. The blob is postcard-encoded: replacing
    // bytes inside it would corrupt the length prefixes framing every field,
    // and the index is derived data that rebuilds on the next query — so the
    // honest remedy is to drop the affected project indexes.
    //
    // This store answered "no occurrences found" while holding the needle,
    // which is the worst answer an emergency tool can give: the user stops
    // looking.
    let (sem_matched, _sem_removed) = tokenade_core::semantic_db::purge_containing(needle, dry_run);
    if sem_matched > 0 {
        let what = if dry_run { "would drop" } else { "dropped" };
        println!(
            "  {glyph} semantic.db — {what} {sem_matched} project index(es) (they rebuild on the next query)"
        );
        total += sem_matched;
    }

    // 3b. file_search.db — DELETE, never patch, for one more reason than
    // semantic.db has. This store keeps the file's own TEXT, and beside each
    // chunk an embedding computed from that text: patching the snippet alone
    // would leave a vector describing words the snippet no longer contains.
    // Dropping the file's chunks costs one re-index on the next question.
    let (fs_matched, _fs_removed) = tokenade_core::file_search::purge_containing(needle, dry_run);
    if fs_matched > 0 {
        let what = if dry_run { "would drop" } else { "dropped" };
        println!(
            "  {glyph} file_search.db — {what} {fs_matched} indexed file(s) (they re-index on the next question)"
        );
        total += fs_matched;
    }

    // 4. Stash dir: walk every file, text-replace.
    if let Some(cache_dir) = tokenade_core::platform::cache_dir() {
        let stash_dir = cache_dir.join("tokenade").join("stash");
        if let Ok(entries) = fs::read_dir(&stash_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else {
                    // Binary stash — skip (the secret should be in
                    // text-shaped stashes; binaries are images / archives).
                    continue;
                };
                let count = content.matches(needle).count();
                if count == 0 {
                    continue;
                }
                if !dry_run {
                    let new_content = content.replace(needle, replacement);
                    if let Err(e) = tokenade_core::paths::write_atomic(&path, &new_content) {
                        eprintln!("  ✗ {}: write failed: {e}", path.display());
                        continue;
                    }
                }
                println!(
                    "  {glyph} {} — {verb} {count} occurrence(s)",
                    path.display()
                );
                total += count;
            }
        }
    }

    println!();
    // What this command does NOT reach, said on BOTH outcomes.
    //
    // The stores above are tokenade's own. The largest record of tool output
    // on the machine is the AGENT's transcript, which tokenade writes nothing
    // to and therefore never scrubs — and on a miss, "no occurrences found"
    // reads as "your machine is clean". Same rule as the recovery command's
    // grep: an empty answer must name what it searched. `redact` is
    // the verb that can reach a file we do not own, so it is named here
    // rather than left for the reader to find.
    const OUT_OF_SCOPE: &str =
        "  Not scanned: your agent's own transcript (Claude Code, opencode, …) \
                                and anything outside tokenade's stores — `tokenade redact <file…> \
                                --in-place` reaches those.";
    if total == 0 {
        println!("  {glyph} no occurrences found in tokenade's own stores");
        println!("{OUT_OF_SCOPE}");
    } else if mode == ScrubMode::DryRun {
        println!(
            "  ◐ dry-run preview: would replace {total} occurrence(s). \
             Re-run without --dry-run to apply."
        );
        println!("{OUT_OF_SCOPE}");
    } else if mode == ScrubMode::Preview {
        println!("  ◐ {total} occurrence(s) will be replaced.");
    } else {
        println!(
            "  ✓ scrubbed {total} occurrence(s). \
             Reminder: rotate the underlying secret — file scrub doesn't undo prior exfiltration."
        );
        println!("{OUT_OF_SCOPE}");
    }
    total
}

/// Ask before rewriting the user's stores.
///
/// Same shape as `report`'s consent gate, and for the same reason: on a
/// terminal, ask; without one, refuse unless the caller said `--yes`, so an
/// agent cannot silently rewrite a user's stores. The difference is what is at
/// stake — `report` sends data out, this destroys data in place with nothing
/// kept to undo it.
pub(crate) fn confirm_scrub(total: usize, assume_yes: bool) -> bool {
    use std::io::IsTerminal;
    if assume_yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        eprintln!();
        eprintln!(
            "  Refusing to modify {total} occurrence(s) without a confirmation: nothing \
             here can be undone, and there is nobody to ask (stdin is not a terminal). \
             Re-run with `--yes` to apply, or `--dry-run` to look first."
        );
        return false;
    }
    println!();
    println!(
        "  These files will be MODIFIED in place. No backup is kept — a scrub that kept \
         one would keep the secret."
    );
    print!("  Continue? [y/N] ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
