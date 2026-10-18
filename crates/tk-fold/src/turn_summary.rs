//! `turn_summary` — extractive summarization of a single conversation turn
//! (the `conflux` idea from a competitor comparison).
//!
//! A long agent/user turn that has already served its purpose can be folded to
//! its load-bearing sentences before it goes into the next prompt window. This
//! is **extractive** (it selects whole original sentences — never invents
//! text), keeps them in document order, and is bounded by a sentence budget.
//!
//! Pipeline: strip conversational filler via [`crate::prose::strip`], split
//! into sentences, score each by intrinsic salience, keep the top-`N` budgeted
//! sentences, then re-emit them in their original order. Pure and
//! deterministic — no clock, no IO, no model.

use crate::prose;
use std::collections::HashSet;

/// Default number of sentences kept when the caller doesn't specify.
pub const DEFAULT_MAX_SENTENCES: usize = 5;

/// Hard cap on how many constraint sentences the preservation floor pins, so a
/// pathological all-rules input can't defeat compaction entirely. Beyond this
/// the most salient constraints win (still recall-favoured vs. plain salience).
/// Constraint lines are pinned before the budget is spent and are never
/// dropped, so a turn made of rules returns up to this many sentences however
/// small `--max` is. It is a FLOOR on the output, and the only thing that can
/// exceed the cap — which is why [`summarize_counted`] hands the real count
/// back for the caller to disclose.
pub const MAX_PINNED_CONSTRAINTS: usize = 10;

/// Split text into sentences on `.`/`!`/`?` boundaries, keeping the terminator.
/// Newlines also act as soft boundaries so bullet/log lines aren't glued
/// together. Returns trimmed, non-empty fragments in document order.
///
/// A terminator ends a sentence only when whitespace or the end follows it,
/// and never inside an inline code span: every `.` used to cut, so
/// `notes.txt` came back `notes. txt` and `v1.2 too.` lost its `2 too.` as a
/// separate, low-salience "sentence" (2026-10-03).
///
/// **A fenced code block is ONE unit**, kept or dropped whole, its lines and
/// indentation intact: each of its lines used to be a "sentence", so one
/// `let token = …;` was picked out of its block and glued between two prose
/// sentences, fence and context gone (2026-10-03).
#[cfg(test)]
fn split_sentences(text: &str) -> Vec<String> {
    split_units(text).into_iter().map(|(s, _)| s).collect()
}

/// [`split_sentences`], each unit with whether it OPENS a source line: such a
/// unit goes back on its own line ([`join_units`]). A list joined by spaces
/// read `… startup: Install the plugin. Restart the editor. - The crash …`,
/// one paragraph where the source had six lines (2026-10-03).
fn split_units(text: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let push = |out: &mut Vec<(String, bool)>, cur: &mut String, starts_line: bool| {
        let s = cur.trim();
        if !s.is_empty() {
            out.push((s.to_string(), starts_line));
        }
        cur.clear();
    };
    let mut lines = text.lines().peekable();
    let is_row = |l: &str| l.trim_start().starts_with('|');
    while let Some(line) = lines.next() {
        // A table is one unit too: one row picked alone came back as
        // `| redis cache layer | 1 ms | 9 ms |`, figures without the header
        // that says what they measure (2026-10-03).
        if is_row(line) {
            let mut table = vec![line];
            while let Some(l) = lines.next_if(|l| is_row(l)) {
                table.push(l);
            }
            out.push((table.join("\n"), true));
            continue;
        }
        if let Some(fence) = fence_of(line) {
            let mut block = vec![line];
            for l in lines.by_ref() {
                block.push(l);
                if fence_of(l).is_some_and(|f| f.starts_with(fence) && l.trim() == f) {
                    break;
                }
            }
            out.push((block.join("\n"), true));
            continue;
        }
        let mut starts_line = true;
        let code = prose::inline_code_spans(line);
        let in_code = |i: usize| code.iter().any(|r| r.contains(&i));
        let mut chars = line.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            cur.push(ch);
            let at_boundary = chars.peek().is_none_or(|&(_, next)| next.is_whitespace());
            // `1.` opening a line is a list marker, not a sentence: split
            // off, it scored nothing and the numbering was dropped.
            let marker = starts_line
                && cur
                    .trim()
                    .trim_end_matches(['.', ')'])
                    .bytes()
                    .all(|b| b.is_ascii_digit());
            if matches!(ch, '.' | '!' | '?') && at_boundary && !in_code(i) && !marker {
                push(&mut out, &mut cur, starts_line);
                starts_line = false;
            }
        }
        push(&mut out, &mut cur, starts_line);
    }
    out
}

/// The fence run (3+ backticks or tildes) that opens or closes a code block.
fn fence_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = t.chars().take_while(|&x| x == c).count();
    (n >= 3).then(|| &t[..n])
}

/// Units joined back: a unit that opened a source line on a new line, the
/// rest of a line's sentences by a space; a code block on its own lines.
fn join_units<'a>(units: impl IntoIterator<Item = (&'a str, bool)>) -> String {
    let mut out = String::new();
    let mut prev_block = false;
    for (u, starts_line) in units {
        let block = fence_of(u).is_some();
        if !out.is_empty() {
            out.push_str(if block || prev_block || starts_line {
                "\n"
            } else {
                " "
            });
        }
        out.push_str(u);
        prev_block = block;
    }
    out
}

/// Intrinsic salience of a sentence: how much distinct content it carries.
/// Score = number of *distinct* content words (alphanumeric, length ≥ 4),
/// gently dampened by a length penalty so a sprawling sentence doesn't crowd
/// out several tighter ones. Deterministic and stopword-list-free.
pub fn salience(sentence: &str) -> f64 {
    let mut distinct: HashSet<String> = HashSet::new();
    let mut total_words = 0usize;
    for raw in sentence.split(|c: char| !c.is_alphanumeric()) {
        if raw.len() < 4 {
            continue;
        }
        total_words += 1;
        distinct.insert(raw.to_ascii_lowercase());
    }
    if total_words == 0 {
        return 0.0;
    }
    let content = distinct.len() as f64;
    // Diminishing returns past ~30 words: divide by sqrt of the overflow.
    let penalty = if total_words > 30 {
        ((total_words as f64) / 30.0).sqrt()
    } else {
        1.0
    };
    content / penalty
}

/// Summarize a turn to `max_sentences` of its own sentences, in document
/// order. An already-short turn (≤ budget sentences) is returned
/// filler-stripped but otherwise whole.
///
/// NOT "at most", which this said until it was measured: constraint lines are
/// pinned first and never dropped, so up to [`MAX_PINNED_CONSTRAINTS`] come
/// back however small the budget is. `--max 1` on a rules-heavy turn returned
/// ten sentences, and the default of five returned ten, with nothing saying
/// so. Use [`summarize_counted`] when the caller has to tell the reader.
pub fn summarize(text: &str, max_sentences: usize) -> String {
    summarize_counted(text, max_sentences).0
}

/// Like [`summarize`], plus the number of sentences actually kept.
///
/// The two differ exactly when the pinned constraints alone meet or exceed the
/// budget. A cap that is quietly exceeded is a cap the reader trusts wrongly
/// the next time, so the count exists to be disclosed, not to be checked and
/// discarded.
pub fn summarize_counted(text: &str, max_sentences: usize) -> (String, usize) {
    let budget = max_sentences.max(1);
    let stripped = prose::strip(text);
    let units = split_units(&stripped);
    let sentences: Vec<String> = units.iter().map(|(s, _)| s.clone()).collect();
    if sentences.len() <= budget {
        return (
            join_units(units.iter().map(|(s, l)| (s.as_str(), *l))),
            sentences.len(),
        );
    }

    // Salience-first ordering, shared by both the constraint cap and the budget
    // fill: highest salience first, ties broken by earlier position.
    let by_salience = |a: &usize, b: &usize| {
        salience(&sentences[*b])
            .partial_cmp(&salience(&sentences[*a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    };

    // Constraint-preservation floor: a line stating a rule ("never force-push",
    // "must use the v2 API", "rm -rf …") carries few distinct content words, so
    // pure salience prunes it — after which the model can silently violate a
    // now-invisible constraint. Pin such lines verbatim FIRST (capped so an
    // all-rules input can't defeat compaction), then spend the remaining budget
    // on the highest-salience non-pinned sentences.
    let mut selected: HashSet<usize> = HashSet::new();
    let mut constraints: Vec<usize> = sentences
        .iter()
        .enumerate()
        .filter(|(_, s)| prose::is_constraint(s))
        .map(|(i, _)| i)
        .collect();
    if constraints.len() > MAX_PINNED_CONSTRAINTS {
        constraints.sort_by(by_salience);
        constraints.truncate(MAX_PINNED_CONSTRAINTS);
    }
    selected.extend(&constraints);

    // Fill any remaining budget with the most salient sentences not already
    // pinned. When constraints already meet/exceed the budget we add none —
    // keeping every pinned rule is worth slightly overshooting the budget.
    let mut rest: Vec<usize> = (0..sentences.len())
        .filter(|i| !selected.contains(i))
        .collect();
    rest.sort_by(by_salience);
    let remaining = budget.saturating_sub(selected.len());
    selected.extend(rest.into_iter().take(remaining));

    let mut keep: Vec<usize> = selected.into_iter().collect();
    keep.sort_unstable();
    let kept = keep.len();
    (
        join_units(keep.iter().map(|&i| (units[i].0.as_str(), units[i].1))),
        kept,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table is kept whole with its header, or not at all.
    #[test]
    fn a_table_is_one_unit() {
        let table = "| backend | p50 |\n|---|---|\n| sqlite | 3 ms |\n| redis | 1 ms |";
        let text = format!("Results are below.\n{table}\nWe picked postgres.");
        assert_eq!(
            split_sentences(&text),
            ["Results are below.", table, "We picked postgres."]
        );
        assert!(
            summarize(&text, 1).lines().all(|l| !l.starts_with('|'))
                || summarize(&text, 1).contains(table)
        );
    }

    /// A list keeps its markers and its lines.
    #[test]
    fn a_list_keeps_its_numbers_and_its_lines() {
        let text = "Steps to reproduce:\n1. Install the plugin. Then wait.\n2. Restart the editor.\n- Only on Linux.";
        assert_eq!(
            split_sentences(text),
            [
                "Steps to reproduce:",
                "1. Install the plugin.",
                "Then wait.",
                "2. Restart the editor.",
                "- Only on Linux."
            ]
        );
        assert_eq!(
            summarize(text, 10),
            "Steps to reproduce:\n1. Install the plugin. Then wait.\n2. Restart the editor.\n- Only on Linux."
        );
    }

    /// A code block is one unit: whole, on its own lines, or absent.
    #[test]
    fn a_code_block_is_kept_whole_or_not_at_all() {
        let block = "```\nfn main() {\n    let token = refresh_token_from_registry_credential_store();\n    deploy_staging_environment(token);\n}\n```";
        let text = format!("The root cause is the expired registry credential.\nHere is the patch:\n{block}\nThe weather was nice.");
        let units = split_sentences(&text);
        assert!(units.iter().any(|u| u == block), "{units:?}");
        for max in 1..=4 {
            let out = summarize(&text, max);
            let fences = out.matches("```").count();
            assert!(
                fences == 0 || out.contains(&format!("\n{block}")) || out.starts_with(block),
                "max {max}: {out}"
            );
        }
        assert!(summarize(&text, 10).contains(&format!(":\n{block}\nThe")));
    }

    /// A dot inside a file name, a version or a code span ends no sentence.
    #[test]
    fn a_dot_that_no_space_follows_ends_no_sentence() {
        let s = split_sentences("Open notes.txt and v1.2 now. Run `a.b. c` then. Done!");
        assert_eq!(
            s,
            [
                "Open notes.txt and v1.2 now.",
                "Run `a.b. c` then.",
                "Done!"
            ]
        );
    }

    /// The count handed back is the count that is THERE.
    ///
    /// `summarize` promised "at most `max_sentences`" and could not keep it:
    /// constraint lines are pinned before the budget is spent and never
    /// dropped, so `--max 1` on a turn of rules returned ten sentences and the
    /// DEFAULT of five returned ten, with nothing in the output saying so. The
    /// floor is worth keeping — a dropped rule is silently violated later — so
    /// what had to change is that it is now countable, and counted.
    #[test]
    fn the_pinned_floor_is_reported_not_hidden() {
        let rules = (0..15)
            .map(|i| format!("Never force-push to branch number {i}."))
            .collect::<Vec<_>>()
            .join(" ");
        let (text, kept) = summarize_counted(&rules, 1);
        assert!(
            kept > 1,
            "the floor did not engage, so this fixture proves nothing: kept {kept}"
        );
        assert!(
            kept <= MAX_PINNED_CONSTRAINTS,
            "the pinned cap stopped binding: kept {kept}"
        );
        // The number reported is the number of sentences actually returned —
        // not the budget, and not the whole input.
        assert_eq!(
            kept,
            split_sentences(&text).len(),
            "the count does not describe the text it came with"
        );
        assert!(
            kept < split_sentences(&rules).len(),
            "nothing was summarised at all"
        );

        // Where the budget genuinely binds, it binds exactly: prose with no
        // rule in it must not inherit the floor.
        let prose_input = (0..30)
            .map(|i| format!("The subsystem emitted metric {i} during the run."))
            .collect::<Vec<_>>()
            .join(" ");
        let (prose_text, prose_kept) = summarize_counted(&prose_input, 4);
        assert_eq!(prose_kept, 4, "the budget did not bind on plain prose");
        assert_eq!(prose_kept, split_sentences(&prose_text).len());

        // And a turn shorter than the budget reports what it returned, whole.
        let (short_text, short_kept) = summarize_counted("One sentence only.", 5);
        assert_eq!(short_kept, split_sentences(&short_text).len());
        assert!(short_kept <= 1, "a one-sentence turn reported {short_kept}");
    }

    #[test]
    fn split_handles_terminators_and_newlines() {
        let s = split_sentences("First one. Second! Third?\nFourth line");
        assert_eq!(s, vec!["First one.", "Second!", "Third?", "Fourth line"]);
    }

    #[test]
    fn salience_rewards_distinct_content_words() {
        let rich = salience("deterministic extractive summarization preserves ordering");
        let thin = salience("it is a the of to");
        assert!(rich > thin);
        assert_eq!(thin, 0.0);
    }

    #[test]
    fn salience_penalizes_runaway_length() {
        let normal = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let bloated = format!("{normal} {}", "kilo ".repeat(40));
        // The bloated sentence has more words but the penalty caps its score so
        // it can't dominate purely by being long-winded.
        assert!(salience(bloated.trim()) < salience(normal) + 5.0);
    }

    #[test]
    fn short_turn_returned_whole() {
        let t = "Only one sentence here.";
        assert_eq!(summarize(t, 5), "Only one sentence here.");
    }

    #[test]
    fn summarize_keeps_budget_in_document_order() {
        let text = "Padding word filler. \
            The migration corrupted the user balance ledger irrecoverably. \
            Nothing much. \
            We rolled back to the previous schema snapshot immediately. \
            ok.";
        let out = summarize(text, 2);
        let kept = out.matches('.').count();
        assert!(kept <= 2, "must not exceed sentence budget: {out:?}");
        // The two content-rich sentences should be the ones retained, and the
        // ledger sentence must precede the rollback sentence (document order).
        let bal = out.find("ledger");
        let roll = out.find("snapshot");
        assert!(
            bal.is_some() && roll.is_some(),
            "kept the salient sentences: {out:?}"
        );
        assert!(bal < roll, "document order preserved: {out:?}");
    }

    #[test]
    fn constraint_line_pinned_even_when_low_salience() {
        // The rule sentence has fewer distinct content words than the two
        // surrounding sentences; pure salience at budget 1 would drop it.
        let text =
            "The authentication subsystem reads distributed configuration entries from consul. \
            Never delete the production database. \
            The telemetry pipeline aggregates regional collector metrics hourly.";
        let out = summarize(text, 1);
        assert!(
            out.to_lowercase()
                .contains("never delete the production database"),
            "constraint must be pinned verbatim, got: {out:?}"
        );
    }

    #[test]
    fn destructive_command_literal_pinned() {
        let text = "We rearranged the helper modules for clarity and moved things around. \
            Run rm -rf build to clean the tree. \
            The integration matrix exercises linux and macos targets thoroughly.";
        let out = summarize(text, 1);
        assert!(
            out.contains("rm -rf"),
            "destructive literal pinned, got: {out:?}"
        );
    }

    #[test]
    fn constraints_capped_to_avoid_defeating_compaction() {
        // 15 rule sentences; the cap keeps at most MAX_PINNED_CONSTRAINTS.
        let body = (0..15)
            .map(|i| format!("You must always preserve invariant number {i} carefully."))
            .collect::<Vec<_>>()
            .join(" ");
        let out = summarize(&body, 2);
        let kept = out.matches("must always preserve").count();
        assert!(kept <= MAX_PINNED_CONSTRAINTS, "cap respected, kept {kept}");
        assert!(kept >= 2, "still recall-favoured, kept {kept}");
    }

    #[test]
    fn zero_budget_clamps_to_one() {
        let out = summarize("Aaa bbbb cccc. Dddd eeee ffff. Gggg hhhh iiii.", 0);
        assert!(!out.is_empty());
        assert!(out.matches('.').count() <= 1);
    }
}
