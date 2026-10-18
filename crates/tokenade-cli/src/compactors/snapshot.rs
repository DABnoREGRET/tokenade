//! **Snapshots and annotated passages.**
//!
//! Return an excerpt with its line numbers, once — repeating the hint on every
//! passage costs more than the hint pays back — and strip from a source what
//! an agent did not ask for.

use super::*;

/// `tokenade snapshot <file>` — compact a Playwright/`browser_snapshot` accessibility tree.
pub(crate) fn cmd_snapshot(rest: &[String]) -> i32 {
    let snap = match read_stdin_or_file_with_tty_hint(
        rest,
        "snapshot",
        "  compact a Playwright/browser_snapshot accessibility tree\n\
         from file:     `tokenade snapshot a11y.txt`",
    ) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (out, sav) = web::compact_a11y_tree_with_savings(&snap);
    // SECURITY: redact secrets from the a11y tree (form values, URLs with
    // tokens). Idempotent — no-op when nothing matches.
    let out = tokenade_core::redact::redact(&out);
    log_gain("snapshot", "a11y-compact", sav);
    // Same universal backstop as `cmd_auto`. The a11y compactor is
    // a per-format bet like every other, and on a large tree it loses: measured
    // 12 716 670 bytes in, **11 816 668 out** — ~3M tokens — where the same file
    // through `tokenade read` returns 1 814. This was the last unbounded exit in
    // the CLI; `wrap`, the MCP proxy, `web`, `read` and the Read hook were all
    // already bounded when audited.
    let bounded = bound_delivery(
        out,
        tokenade_core::disclose::SourceHint::web(),
        "a11y-compact",
    );
    // Third site of the same hole (wrap, read, this one). The comment
    // above quantifies exactly what went uncredited: the a11y compactor takes
    // 12 716 670 bytes to 11 816 668 — that ~900 kB is what `log_gain` books —
    // and the stash then takes the remaining ~11.8 MB down to a pointer, with
    // nobody counting. The row credited under a hundredth of the delivery.
    //
    // Tiles with the row above, same as its two siblings: a11y spans raw ->
    // compacted, this one compacted -> delivered.
    // Same rule as `read` above, through the same declarant: a tree the
    // compactor handed back unchanged is the caller's bytes.
    let passed_through = bounded == snap;
    print!("{bounded}");
    if crate::wrap::needs_terminator(&bounded, passed_through) {
        println!();
    }
    0
}

/// `tokenade read <source-file>`: apply the same comment strip the Read hook
/// applies, or decline. `None` means "not source, or nothing worth stripping"
/// and the caller carries on to the format compactors.
/// Retrieval, in ONE place: run the question over the text the agent would
/// otherwise read, and when the extractor declines, SAY so.
///
/// One declarant because "was the question applied?" can only be answered by
/// whoever ran the retrieval. `cmd_auto`'s dispatch used to be the only place
/// that ran it, and `try_source_strip` returns BEFORE that dispatch ever
/// executes — it never even received the prompt. MEASURED 2026-08-31 on a
/// 24 006-token `.rs`: stdout byte-identical with and without `--prompt`
/// (56 300 bytes both ways) and stderr empty. The question was dropped in
/// silence on exactly the files too big to read whole, which is the only case
/// where asking one is worth anything.
///
/// Say ONCE per context what the line labels are for.
///
/// The numbers are the file's own, so the next move is
/// `tokenade read <file> --lines A-B` — one region instead of the document
/// again. Repeating that sentence on every answer bills it on every call;
/// saying it once and never again loses it at the next compaction, which is
/// exactly where an agent's memory of it goes. So: once per session, and armed
/// again when the context has been compacted since it was last said.
pub(super) fn hint_line_range_once(file: &str) {
    if !crate::said_once::due("linehint") {
        return;
    }
    eprintln!(
        "[tokenade] the `[lines A-B]` / `[line N]` labels above are lines of {file} itself — read one \
         region with `tokenade read {file} --lines A-B` instead of the whole file again."
    );
}

/// Put the SOURCE line numbers on each passage of a scoped answer.
///
/// A retrieval that says WHERE it read is one you can act on: the next call is
/// `--lines A-B` on that region instead of the whole document again. Without
/// them the passages float — you know what the file says and not where, which
/// is the one thing a second read needs.
///
/// It labels BLOCKS, not one rendering of them: the extractor answers with
/// breadcrumbed passages when the embedder is available and with plain
/// excerpts separated by `…` when it falls back, and both are the same thing
/// to a reader who wants to go and look.
///
/// Every number is verified against `src`: a block whose first and last lines
/// are not both found there is left unlabelled rather than labelled with a
/// guess. A wrong line number is worse than none — it sends the next read to
/// the wrong place and looks authoritative doing it. Measured 2026-09-03 while
/// writing this: matching against the COMPACTED view instead of the file put a
/// passage at "lines 124-134" of PATCHNOTES.md, where both of those lines are
/// blank.
///
/// When the source is itself a slice (`read --lines`), its banner carries the
/// offset and the labels come out in FILE coordinates.
/// Non-blank source lines of `a..=b` (file coordinates, `base` away from
/// `src_lines`) that the block does not carry.
fn omitted_in(src_lines: &[&str], a: isize, b: isize, body: &[&str], base: isize) -> usize {
    let (from, to) = (
        (a - base - 1).max(0) as usize,
        ((b - base) as usize).min(src_lines.len()),
    );
    let present = src_lines
        .get(from..to)
        .unwrap_or_default()
        .iter()
        .filter(|l| !l.trim().is_empty())
        .count();
    present.saturating_sub(body.len())
}

pub(super) fn annotate_passage_lines(answer: &str, src: &str) -> String {
    let src_lines: Vec<&str> = src.lines().collect();
    let base: isize = src_lines
        .first()
        .and_then(|l| l.strip_prefix("[tokenade: lines "))
        .and_then(|r| r.split('-').next())
        .and_then(|a| a.parse::<isize>().ok())
        .map(|a| a - 2)
        .unwrap_or(0);
    // Short lines are not evidence: `}` or `## 1.1.4` occurs everywhere, and a
    // match on one would place the passage at the first of many.
    //
    // Two tiers, because the compacted view does not REWRITE lines — it drops
    // them and truncates them. Measured 2026-09-04 over the passages of one
    // real answer: 62 of 78 lines are byte-identical to a line of the file,
    // and 72 of 78 once a truncated line is matched by its first 40
    // characters. Normalising whitespace gained nothing, so it is not done:
    // a rule that buys no coverage is a rule that can only misfire.
    const PREFIX_EVIDENCE: usize = 40;
    let find = |needle: &str, from: usize| -> Option<usize> {
        let needle = needle.trim_end().trim_end_matches('…').trim_end();
        if needle.trim().len() < 12 {
            return None;
        }
        if let Some((i, _)) = src_lines
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, l)| **l == needle)
        {
            return Some(i + 1);
        }
        // A truncated line: the ANSWER is the shorter one, so the source line
        // starts with it. Long enough to be evidence, and only then.
        if needle.trim().len() < PREFIX_EVIDENCE {
            return None;
        }
        src_lines
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, l)| l.starts_with(needle))
            .map(|(i, _)| i + 1)
    };

    let is_sep = |l: &str| l.trim() == "…" || l.starts_with("### ") || l.starts_with("> ");
    let lines: Vec<&str> = answer.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 8);
    let mut i = 0usize;
    while i < lines.len() {
        if is_sep(lines[i]) || lines[i].trim().is_empty() {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && !is_sep(lines[i]) {
            i += 1;
        }
        let body: Vec<&str> = lines[start..i]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        // The FIRST line of a block is often a heading — `## 1.1.4` — which is
        // too short to be evidence and would place the block at the first of
        // many matches. Measured 2026-09-04 on one real answer: 14 of 17
        // blocks have a locatable line, and 4 of the 6 that were going
        // unlabelled opened on exactly such a heading. So the block is
        // anchored on the first line that CAN be located, and closed on the
        // last one, rather than refused because of its title.
        let range = body.iter().find_map(|l| find(l, 0)).and_then(|a| {
            // A block of N lines cannot honestly denote a region of 750.
            // The compactor DROPS lines, so a little slack is real; a span
            // many times the block's own length means the passages are
            // scattered, not contiguous, and "go read lines 17-767" is the
            // whole file with extra steps. Measured 2026-09-04: real blocks
            // span about their own length (2 lines -> 703-704, 4 -> 798-801,
            // 6 -> 1039-1044); the one merged block spanned 17-767 and is
            // what this bound rejects.
            //
            // Rejecting means NO label, not the anchor line on its own. That
            // fallback printed `[lines 27-27]` on a 21-line block whose
            // content reaches line 895 — the notation that means "the whole
            // passage is here" everywhere else would mean "one line of
            // twenty-one is here" on a merged block, with nothing to tell the
            // two apart. MEASURED 2026-09-04 over three real answers about
            // this repository's PATCHNOTES.md: 3 of the 12 labelled blocks
            // were that shape, reaching lines 199, 765 and 895 from anchors
            // at 17, 29 and 27. The passages around such a block carry their
            // own labels; this one has no honest single span, so it says
            // nothing.
            let b = body.iter().rev().find_map(|l| find(l, a - 1)).unwrap_or(a);
            (b >= a && b <= a + body.len() * 3 + 8).then(|| (a as isize + base, b as isize + base))
        });
        if let Some((a, b)) = range.filter(|(a, b)| *a >= 1 && b >= a) {
            // A ONE-line passage — a log record, a list entry — carries its
            // label inline, and the blank + `…` between two such lines go:
            // the numbers already say where the gaps are. Ten log hits cost
            // forty lines of label, record, blank and ellipsis (2026-10-03).
            if a == b && body.len() == 1 && !out.last().is_some_and(|p| p.starts_with("> ")) {
                if single_line_tail(&out) {
                    while out
                        .last()
                        .is_some_and(|l| l.trim().is_empty() || l.trim() == "…")
                    {
                        out.pop();
                    }
                }
                out.push(format!("[line {a}] {}", body[0]));
                continue;
            }
            // **A range that is not all there says so.** The compacted view
            // drops lines (`}`, `#[test]`) and a label alone reads as "these
            // are lines A-B": code copied from it lost its closing braces with
            // nothing to warn (2026-10-04, `[lines 2-19]` showing 12 of 15).
            let omitted = omitted_in(&src_lines, a, b, &body, base);
            let label = match omitted {
                0 => format!("[lines {a}-{b}]"),
                n => format!("[lines {a}-{b}, {n} omitted]"),
            };
            // On the breadcrumbed rendering the label rides on the breadcrumb
            // that just went out; otherwise it opens the block on its own.
            match out.last_mut() {
                Some(prev) if prev.starts_with("> ") => {
                    prev.push_str(&format!("  {label}"));
                }
                _ => out.push(label),
            }
        }
        out.extend(lines[start..i].iter().map(|l| l.to_string()));
    }
    let mut joined = out.join("\n");
    if answer.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Whether the output so far ends on an inline-labelled single line, past
/// only blanks and `…` separators.
fn single_line_tail(out: &[String]) -> bool {
    out.iter()
        .rev()
        .find(|l| !(l.trim().is_empty() || l.trim() == "…"))
        .is_some_and(|l| l.starts_with("[line ") && !l.starts_with("[lines "))
}

/// Returns the delivered text and whether the extractor actually answered.
///
/// `search` is what the question is asked of when it is not `text` itself:
/// the SOURCE, when `text` is a fold that dropped what the answer needs.
pub(super) fn retrieve_or_note(
    text: String,
    search: Option<&str>,
    prompt: Option<&str>,
    source: &str,
    tokens_after: usize,
) -> (String, bool) {
    let Some(q) = prompt.map(str::trim).filter(|p| !p.is_empty()) else {
        return (text, false);
    };
    nudge_batch_questions_if_repeated(source, q);
    let budget = tokenade_core::disclose::webfetch_scoped_budget_for(tokens_after);
    match tokenade_core::gain::prompt_scoped_extract_asked(
        search.unwrap_or(&text),
        q,
        budget,
        "read",
    ) {
        Some(scoped) => (scoped, true),
        None if text.len() >= UNANSWERED_NOTE_MIN_CHARS && !text.contains(UNANSWERED_NOTE) => {
            (format!("{UNANSWERED_NOTE}\n{text}"), false)
        }
        None => (text, false),
    }
}

pub(super) fn try_source_strip(
    file_path: Option<&str>,
    src: &str,
    prompt: Option<&str>,
) -> Option<i32> {
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_CODE_STRIP") {
        return None;
    }
    let p = std::path::Path::new(file_path?);
    let ext = p.extension().and_then(|e| e.to_str())?;
    let lang = tokenade_core::code_strip::language_for_extension(ext)?;
    if src.len() < 3_000 {
        return None;
    }
    // Same exception as the Read hook: below the ceiling and inside a git
    // work tree, the file is an edit target and the savings do not cover the
    // risk of showing the agent bytes it will not edit against.
    if tokenade_core::tokenizer::estimate(src) < crate::hooks::EDITABLE_SOURCE_MAX_TOKENS
        && crate::hooks::inside_git_work_tree(p)
    {
        return None;
    }
    let d = tokenade_core::code_strip::strip_for_delivery(src, lang)?;
    let lang_label = format!("{lang:?}").to_lowercase();
    // Retrieval runs on the STRIPPED text, the same posture the document path
    // takes: the extractor sees what the agent would otherwise read, so a
    // scoped answer can never surface a comment the delivery had removed.
    let (text, scoped) =
        retrieve_or_note(d.text, None, prompt, file_path.unwrap_or_default(), d.after);
    // The banner has to describe what is BELOW it. When retrieval answered,
    // `d.after`/`d.pct` describe the whole stripped file, which is no longer
    // what follows — recount, and name the mode `comments+scoped` so the
    // reader can tell an answer from a delivery.
    let (mode, after, pct) = if scoped {
        let after = tokenade_core::tokenizer::estimate(&text);
        let pct = d.before.saturating_sub(after) * 100 / d.before.max(1);
        (format!("{}+scoped", d.mode), after, pct)
    } else {
        (d.mode.to_string(), d.after, d.pct)
    };
    println!(
        "[tokenade:read-code-strip lang={lang_label} mode={mode}] {} ({}→{} tokens, -{}%); the file on disk is unchanged",
        p.display(),
        d.before,
        after,
        pct
    );
    print!("{text}");
    // A stripped-and-possibly-scoped view, announced by the banner above it:
    // nothing here is the file's own bytes, so it is a view to terminate.
    if crate::wrap::needs_terminator(&text, false) {
        println!();
    }
    // Book it. This path returns from `cmd_auto` before the recording below
    // it, so `tokenade read <source>` saved 57% (96% with a question) and
    // wrote ZERO ledger rows — measured 2026-08-31, the row count on
    // `~/.tokenade/gain.jsonl` was unchanged across both calls. The Read HOOK
    // credits the same bytes (`read_code_strip_lite`), so the two doors onto
    // one rule disagreed about whether the saving happened: the same file
    // counted when the agent opened it and vanished when it asked us to.
    //
    // `source=cli` and `op=auto-compact` like every other sibling in
    // `cmd_auto`, so `worth_recording`'s op-keyed gates apply unchanged.
    log_gain(
        if scoped {
            "auto:code-strip:scoped"
        } else {
            "auto:code-strip"
        },
        "auto-compact",
        Savings::new(d.before, after),
    );
    Some(0)
}
