//! **A fold that costs a banner must gain more than its banner.**
//!
//! Written once because it was written twice. `template_fold::fold_run_if_it_pays`
//! carried the rule — folded + legend must fit under 90 % of the original — and
//! `searchout` wrote a second, PARTIAL one: it checked that a repeated
//! line gives back bytes, and never that the banner announcing the fold was
//! earned. MEASURED while writing it: a 20-byte line repeated twice gives back
//! 10 bytes, the banner costs 95, and the output GROWS by 85.
//!
//! The 10 % margin is not a comfort setting: a fold that gives back 2 % changes
//! the shape of the output for nothing, and a changed shape is a cost the
//! bytes do not measure.

/// Is the fold worth delivering? `legend` is what the announcement will cost on top
/// of `folded` — zero if the fold already announces itself in the measured body.
pub fn pays(original: usize, folded: usize, legend: usize) -> bool {
    (folded + legend) * 10 <= original * 9
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case that got this module written: the fold gives back 10 bytes, the banner
    /// costs 95.
    #[test]
    fn a_fold_that_does_not_pay_for_its_own_banner_is_refused() {
        assert!(!pays(1_000, 990, 95), "10 bytes given back, 95 spent");
        assert!(pays(1_000, 500, 95));
    }

    /// Giving back exactly the margin is enough; below it, no.
    #[test]
    fn the_margin_is_ten_percent() {
        assert!(pays(1_000, 900, 0));
        assert!(!pays(1_000, 901, 0));
        // An empty output always pays; an empty input never pays.
        assert!(pays(100, 0, 0));
        assert!(!pays(0, 0, 1));
    }
}

/// **The byte ranges a fenced code block occupies.**
///
/// A fold that rewrites text has no business inside: ten lines of
/// Rust folded into `«1»  (x3)` give an excerpt that no longer compiles, and the
/// reader has no way to guess that the fence is lying. MEASURED 2026-09-09,
/// that is exactly what `phrase_fold` did as soon as it was offered to the web
/// route, and what a canary refused before shipping.
///
/// `template_fold::fold_if_it_pays_outside_code` answers a NEIGHBOURING
/// and different question: it needs the PROSE portions, one by one, to
/// fold each separately. `phrase_fold` looks for its phrases across the
/// whole document — a phrase repeated on both sides of a block must
/// stay seen — so it needs the FORBIDDEN ranges, not a split. The
/// two shapes coexist for that reason, written here so that a future
/// "unification" knows what it would break.
///
/// The opening and closing fences are included in the range: their text
/// is syntax, not prose.
///
/// A CommonMark indented run (4 spaces / tab) counts too — same gate as
/// `markdown::indented_code_at` (after blank / ATX / start of doc, never
/// inside a list). fold sits below formats, so the helper cannot be shared.
pub fn fenced_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut at = 0usize;
    for line in text.split('\n') {
        lines.push((at, line));
        at += line.len() + 1;
    }
    let slice: Vec<&str> = lines.iter().map(|(_, l)| *l).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut blank_run = 1usize;
    let mut list_run = 0usize;
    while i < lines.len() {
        let (start, line) = lines[i];
        let trimmed = line.trim_start();
        // Same two markers as `tk_formats::fences::fence_marker`.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            let mut j = i + 1;
            while j < lines.len() {
                if lines[j].1.trim_start().starts_with(marker) {
                    let close_end = lines[j].0 + lines[j].1.len();
                    out.push(start..close_end);
                    i = j + 1;
                    blank_run = 1;
                    list_run = 0;
                    break;
                }
                j += 1;
            }
            if j == lines.len() {
                // A fence never closed runs to the end: the document
                // is truncated or malformed, and in both cases what follows
                // is code.
                out.push(start..text.len());
                break;
            }
            continue;
        }
        if let Some(end) = indented_code_run_end(&slice, i, list_run, blank_run) {
            let last = end - 1;
            let last_end = lines[last].0 + lines[last].1.len();
            out.push(start..last_end);
            i = end;
            blank_run = 1;
            list_run = 0;
            continue;
        }
        if line.trim().is_empty() {
            blank_run += 1;
        } else if is_list_item(line) {
            list_run = 1;
            blank_run = 0;
        } else {
            list_run = 0;
            blank_run = usize::from(is_atx_heading(line));
        }
        i += 1;
    }
    out
}

/// CommonMark indented code: 4 spaces or a tab, and not a nested list
/// item (`    - foo`). Same gate as `markdown::indented_code_at`. One
/// declarer: `rag::split` used to carry an exact copy.
pub fn is_indented_code_line(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    let rest = if let Some(r) = line.strip_prefix('\t') {
        r
    } else if let Some(r) = line.strip_prefix("    ") {
        r
    } else {
        return false;
    };
    let t = rest.trim_start();
    !(t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || (t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains(". ")))
}

pub(crate) fn is_list_item(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return true;
    }
    if let Some(dot) = t.find(". ") {
        let prefix = &t[..dot];
        !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

pub(crate) fn is_atx_heading(line: &str) -> bool {
    let t = line.trim_start();
    let n = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&n) && (t.len() == n || t.as_bytes().get(n) == Some(&b' '))
}

/// Exclusive end index of an indented code run opening at `i`, or `None`.
///
/// Gate: `list_run == 0` and `blank_run > 0` (blank / ATX / start of doc).
pub(crate) fn indented_code_run_end(
    lines: &[&str],
    i: usize,
    list_run: usize,
    blank_run: usize,
) -> Option<usize> {
    if list_run != 0 || blank_run == 0 || i >= lines.len() || !is_indented_code_line(lines[i]) {
        return None;
    }
    let mut j = i;
    let mut body = 0usize;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            let continues = lines[j + 1..]
                .iter()
                .find(|x| !x.trim().is_empty())
                .is_some_and(|n| is_indented_code_line(n));
            if continues {
                j += 1;
                continue;
            }
            break;
        }
        if !is_indented_code_line(l) {
            break;
        }
        body += 1;
        j += 1;
    }
    if body == 0 {
        None
    } else {
        Some(j)
    }
}

#[cfg(test)]
mod fenced_range_tests {
    use super::fenced_ranges;

    fn inside(text: &str, needle: &str) -> bool {
        let i = text.find(needle).expect("needle");
        fenced_ranges(text).iter().any(|r| r.contains(&i))
    }

    #[test]
    fn a_fenced_block_is_named_and_the_prose_around_it_is_not() {
        let t = "avant\n```rust\nlet x = 1;\n```\naprès";
        assert!(inside(t, "let x = 1;"));
        assert!(!inside(t, "avant"));
        assert!(!inside(t, "après"));
        // The fence itself is syntax, not prose.
        assert!(inside(t, "```rust"));
    }

    #[test]
    fn the_tilde_marker_counts_too() {
        let t = "a\n~~~\ncode\n~~~\nb";
        assert!(inside(t, "code"));
        assert!(!inside(t, "b"));
    }

    /// A fence never closed runs to the end: otherwise a truncated
    /// document — which stashing produces — would reopen its folds on
    /// code.
    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let t = "prose\n```\ncode ici\nencore du code";
        assert!(inside(t, "code ici"));
        assert!(inside(t, "encore du code"));
        assert!(!inside(t, "prose"));
    }

    #[test]
    fn a_document_without_a_fence_has_no_range() {
        assert!(fenced_ranges("juste de la prose\net une autre ligne").is_empty());
    }

    /// CommonMark indented code is the third fence form. Same hole `~~~`
    /// had before `fence_marker`: phrase_fold / template_fold treated a
    /// 4-space sample as prose and rewrote it.
    #[test]
    fn an_indented_code_block_is_named_and_the_prose_around_it_is_not() {
        let t = "avant\n\n    let x = 1;\n    let y = 2;\n\naprès";
        assert!(inside(t, "let x = 1;"));
        assert!(inside(t, "let y = 2;"));
        assert!(!inside(t, "avant"));
        assert!(!inside(t, "après"));
    }

    #[test]
    fn nested_list_items_are_not_indented_code() {
        let t = "- top\n    - nested\n";
        assert!(!inside(t, "nested"));
        assert!(fenced_ranges(t).is_empty());
    }

    #[test]
    fn list_item_continuation_is_not_indented_code() {
        let t = "- intro\n    continuation\n";
        assert!(!inside(t, "continuation"));
        assert!(fenced_ranges(t).is_empty());
    }

    #[test]
    fn an_indented_run_after_an_atx_heading_still_counts() {
        let t = "# Title\n    fn f() {}\nprose";
        assert!(inside(t, "fn f()"));
        assert!(!inside(t, "prose"));
    }
}

/// **A fold must never be what makes a threshold get crossed DOWNWARDS.**
///
/// Written once, for ALL thresholds. Stashing paid for it twice: folding in
/// the first stage brought the view under the second stage's
/// threshold, which stopped summarising, and the DELIVERED output grew from
/// 16,122 to 20,397 bytes. The rule was then written in `wrap` — for that
/// threshold only.
///
/// A third case followed: a fold applied at the distiller's exit returned
/// a page short enough for the REPLAY MEMORY to stop recognising it,
/// and the page went out whole again on the second call. The threshold was different, the
/// rule identical, and the `wrap` version could not carry it.
///
/// Returns `true` when the fold is safe: for each threshold, the folded view stays
/// above, or it was already below before.
pub fn crosses_no_threshold_downwards(before: &str, after: &str, thresholds: &[usize]) -> bool {
    let b = crate::tokenizer::estimate(before);
    let a = crate::tokenizer::estimate(after);
    thresholds.iter().all(|&t| !(b >= t && a < t))
}

#[cfg(test)]
mod threshold_tests {
    use super::crosses_no_threshold_downwards as ok;

    fn words(n: usize) -> String {
        vec!["mot"; n].join(" ")
    }

    /// Thresholds are DERIVED from what the estimator answers, never guessed:
    /// a fixture assuming "two words per token" tests the assumption and
    /// not the rule.
    fn est(s: &str) -> usize {
        crate::tokenizer::estimate(s)
    }

    #[test]
    fn a_fold_that_stays_above_is_allowed() {
        let (b, a) = (words(400), words(300));
        let t = est(&a) - 1; // both stay above
        assert!(ok(&b, &a, &[t]));
    }

    #[test]
    fn a_fold_that_crosses_downwards_is_refused() {
        let (b, a) = (words(400), words(20));
        let t = est(&a) + 1; // `b` above, `a` below
        assert!(
            est(&b) >= t && est(&a) < t,
            "the fixture does not bracket the threshold"
        );
        assert!(!ok(&b, &a, &[t]));
    }

    /// Already below before: the fold cannot defuse anything, it costs nothing.
    #[test]
    fn a_view_already_below_is_free_to_fold() {
        let (b, a) = (words(20), words(5));
        let t = est(&b) + 1;
        assert!(ok(&b, &a, &[t]));
    }

    /// **All thresholds, not the first.** That is the exact defect of the replay-memory
    /// case: the `wrap` version only knew one, and the fold defused the other.
    #[test]
    fn every_threshold_counts_not_just_one() {
        let (b, a) = (words(400), words(200));
        let low = est(&a) - 1; // crossed by neither
        let mid = est(&a) + 1; // crossed downwards by the fold
        assert!(
            ok(&b, &a, &[low]),
            "the low threshold should refuse nothing"
        );
        assert!(
            !ok(&b, &a, &[low, mid]),
            "one threshold out of TWO is enough to refuse: the replay-memory case"
        );
    }
}
