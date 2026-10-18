//! **`tokenade skeleton`: the signatures, without the bodies.**
//!
//! What an agent reads to know what a file contains without reading it. The
//! token budget lives here because this is the command called on several
//! files at once, and the cut must fall between two signatures, never in the
//! middle of one.

use super::*;

pub(crate) fn cmd_skeleton(rest: &[String]) -> i32 {
    // #680 — silent-accept sweep: `skeleton --bogus` must reject, not
    // treat the flag as a path. skeleton takes no flags.
    // #1072 — batchable: collect every positional so the agent can ask
    // `tokenade skeleton a.rs b.rs c.rs` in ONE turn instead of N. A
    // single file keeps byte-identical output (no header).
    let value_flags: Vec<&str> = OUTPUT_MODE_FLAGS
        .iter()
        .chain(BUDGET_FLAGS.iter())
        .copied()
        .collect();
    let files = match collect_positionals_with_values(rest, "skeleton", &[], &value_flags, "file") {
        Ok(p) => p,
        Err(code) => return code,
    };
    let out_mode = match parse_output_mode(rest, "skeleton") {
        Ok(m) => m,
        Err(code) => return code,
    };
    // Was a ceiling ASKED for, as opposed to the default one applied below?
    // Only an explicit ask can be reported back as unable to bind.
    let budget_was_explicit = matches!(parse_budget(rest, "skeleton"), Ok(Some(_)));
    let mut budget = match parse_budget(rest, "skeleton") {
        // A DEFAULT ceiling, not an opt-in one. `parse_budget`'s own doc says
        // it: without a budget "the only way to discover the cost is to have
        // already paid it" — and the scaffolded project rules tell agents to
        // pass MANY files in one call, which is exactly the shape that runs
        // away. MEASURED on this repo: 60 files → 79 KB, 120 files → 152 KB
        // (~39 k tokens), linear and unbounded, from a command whose purpose
        // is to avoid reading those files.
        //
        // Nothing is lost to the ceiling: over budget a file DEGRADES to a
        // one-line census naming it and its signature count, and the footer
        // says what was withheld. `--budget N` still overrides, and
        // `--budget 0` restores the unbounded behaviour for a caller that
        // really wants every byte.
        Ok(b) => TokenBudget::new(
            Some(b.unwrap_or(tokenade_core::coefficients::DEFAULT_CAP_READ_TOKENS as usize))
                .filter(|n| *n > 0),
        ),
        Err(code) => return code,
    };
    let multi = files.len() > 1;
    let mut rc = 0;
    // Files the budget could not fit AT ALL — not even their one-line census.
    // The footer counts them; without their names the caller cannot re-run the
    // ones they wanted, and "59 withheld" is a number they can do nothing with.
    let mut refused: Vec<&str> = Vec::new();
    // Whether the LAST skeleton rendered came back cut short. Only read when
    // there is exactly one file, where it is that file's answer — the
    // `--budget` diagnostic below has to know it, because the two bounds are
    // different and it used to claim the one it does not control.
    let mut was_truncated = false;
    // Sizes of the outlines served whole: each is a `Read`-like call the
    // agent did not have to make — booked as batching below.
    let mut served: Vec<usize> = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        let text = match crate::bounded_input::read_file_lossy(file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("tokenade skeleton: {file}: {e}");
                // batch: skip the missing file, keep the rest.
                rc = rc.max(1);
                continue;
            }
        };
        // #1 — pass the extension so source files get the tree-sitter
        // (AST) skeleton with multi-line signatures and no false matches
        // in strings/comments; unknown extensions fall back to the
        // line-based heuristic inside `skeleton_ext`.
        let ext = std::path::Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        let (sk, sav) = tokenade_core::skeleton::skeleton_with_savings_ext_for(
            &text,
            ext.as_deref(),
            files.len(),
        );
        // SECURITY: a top-level `const API_KEY="…"` survives skeletonization.
        let sk = tokenade_core::redact::redact(&sk);
        was_truncated = tokenade_core::skeleton::skeleton_was_truncated(&sk);
        log_gain(&format!("skeleton {file}"), "skeleton", sav);
        // skeleton IS the signatures-only outline, so Outline == Full.
        // paths → bare file list; count → signature count per file.
        match out_mode {
            OutputMode::Paths => println!("{file}"),
            OutputMode::Count => {
                if multi {
                    println!("{file}: {}", skeleton_signature_count(&sk));
                } else {
                    println!("{}", skeleton_signature_count(&sk));
                }
            }
            OutputMode::Full | OutputMode::Outline => {
                // Charged per FILE, so a batch stops at a whole-file boundary
                // rather than serving half a skeleton — a truncated signature
                // list reads as "this file has no more symbols", which is a
                // lie the caller cannot detect.
                // A file that does not fit is not dropped, it shrinks to a
                // one-line census. Silently omitting it reads as "this file
                // has nothing", which is the one thing the caller cannot
                // check; naming it with its symbol count costs ~15 tokens
                // and turns a lie into a lead.
                let summary = format!(
                    "==> {file} <== ({} signatures, withheld for budget — \
                     `tokenade skeleton {file}` for the full outline)",
                    skeleton_signature_count(&sk)
                );
                match budget.admit_or_degrade(&sk, &summary) {
                    Admission::Refused => {
                        refused.push(file.as_str());
                        continue;
                    }
                    Admission::Degraded => {
                        if fi > 0 {
                            println!();
                        }
                        println!("{summary}");
                        continue;
                    }
                    Admission::Full => {}
                }
                if multi {
                    if fi > 0 {
                        println!();
                    }
                    println!("==> {file} <==");
                }
                println!("{sk}");
                served.push(tokenade_core::tokenizer::estimate(&sk));
            }
        }
    }
    if !refused.is_empty() {
        // Capped in turn — a list of 500 paths would be the very problem the
        // budget exists to prevent — and the cap says so.
        const NAME_CAP: usize = 20;
        let shown: Vec<&str> = refused.iter().copied().take(NAME_CAP).collect();
        let tail = if refused.len() > NAME_CAP {
            format!(" (+{} more)", refused.len() - NAME_CAP)
        } else {
            String::new()
        };
        println!(
            "[tokenade:budget] not shown at all: {}{tail}",
            shown.join(" ")
        );
    }
    let footer = budget.footer();
    if !footer.is_empty() {
        println!("{footer}");
    }
    tokenade_core::gain::record_batch_cache_saving("skeleton", "files", &served);
    // A ceiling that cannot bind, said out loud. The first target is always
    // admitted — degrading the ONLY file asked for to a one-line census would
    // answer nothing — so on a single file `--budget` can never do anything,
    // and it did it silently: same bytes at `--budget 1` as with no flag at
    // all. Measured across three files and two values before this line
    // existed. With several targets the ceiling works and the footer already
    // reports what it withheld.
    if budget_was_explicit && files.len() == 1 && footer.is_empty() {
        // Say which bound this is. `--budget` chooses WHICH targets are
        // shown; the per-file skeleton ceiling chooses how much of one. The
        // message used to claim "the one target you asked for is always shown
        // in full" — printed one line under this file's own footer reading
        // "truncated at ~2500 tokens; +8 more symbol(s) hidden". Whichever
        // half the caller believed, the tool had contradicted itself, and the
        // half that sounds reassuring is the one that reads as a guarantee.
        // NOT `-o outline`: measured, it returns the same bytes as the
        // default for this command — a skeleton already IS the outline.
        // Naming it would reintroduce a misleading-hint defect in its own fix.
        if was_truncated {
            eprintln!(
                "[tokenade:budget] --budget chooses WHICH targets are shown, not how much of \
                 one, so it cannot bind on a single file. This file still hit the per-file \
                 skeleton ceiling: the footer above says how many symbols it hid, and \
                 `tokenade query \"<name>\"` returns any one of them."
            );
        } else {
            eprintln!(
                "[tokenade:budget] --budget chooses WHICH targets are shown, not how much of \
                 one, so it cannot bind on a single file: there is nothing to withhold, and \
                 a skeleton is already its reduced form. For less than that, \
                 `tokenade query \"<name>\"` returns one symbol."
            );
        }
    }
    rc
}

/// Byte-identical to `tokenade_core::dashboard::human_tokens`, which is where
/// it now lives. Two copies of a FORMATTER means two surfaces can one day
/// print "1.5k" and "1500" for the same number, and nothing fails.
pub(crate) fn human_tokens(n: u64) -> String {
    tokenade_core::dashboard::human_tokens(n)
}
