//! **A long phrase repeated in the MIDDLE of lines, said once.**
//!
//! The existing folds catch repetition by LINE: the template fold
//! requires lines to share their whole shape, deduplication
//! requires them to be identical, and path grouping
//! requires the repetition to be at the START. A phrase recurring in the middle of
//! otherwise different lines met none of the three.
//!
//! MEASURED 2026-09-09 on the delivery path of the corpus captures:
//! `lspci` carries `Advanced Micro Devices, Inc. [AMD]` on **23 lines, 770 of the
//! 3,228 bytes delivered (23 %)**; `file /usr/lib/*` carries `symbolic link to`
//! **34 times (561 B, 15 %)**; `tasklist /v` and `whoami /all` each have
//! theirs. None of those bytes was reachable.
//!
//! The phrase is replaced by a short reference and **declared once at the top**,
//! as `[paths from …]` and `[same on every row: …]` already declare
//! what they pulled out of the lines. It therefore stays findable in the
//! output — "nothing is lost" and "it can still be
//! found" are two different properties.

/// Does this phrase cross a STRUCTURE rather than carry text?
///
/// The value separator comes from `template_fold`, which declares it: two
/// spellings would make the fold and the refusal diverge. This fold's references
/// are its own.
fn crosses_a_structure(s: &str) -> bool {
    if s.contains(crate::template_fold::VALUE_SEPARATOR.trim())
        || s.contains('«')
        || s.contains('»')
    {
        return true;
    }
    // **Two spaces in a row are a COLUMN.** Prose does not use them;
    // an aligned table puts them everywhere, and a reference placed over them shifts everything
    // that follows on the line. MEASURED: `dir` declared
    // `10/02/2026  20:16                 0` and `tasklist /svc`
    // `N/A                                         ` — padding, not
    // text, and the product already has `collapse_alignment_padding` for that.
    if s.contains("  ") {
        return true;
    }
    // **A rule is not a phrase.** `schtasks` declared
    // `======================================== ====== ===============`, the
    // line that SEPARATES the header from the data. Replacing it with a reference
    // takes the table's boundary away from the reader.
    let alnum = s.chars().filter(|c| c.is_alphanumeric()).count();
    alnum * 2 < s.chars().count()
}

/// Minimum length of a phrase for it to be worth a reference.
///
/// Below this threshold the reference costs almost what the phrase costs, and the reader
/// pays for an indirection for nothing.
const MIN_PHRASE: usize = 16;

/// Minimum number of LINES carrying the phrase.
///
/// Two occurrences never pay for the declaration; it is the same reasoning
/// as the template fold's `MIN_RUN`.
const MIN_LINES: usize = 5;

/// What the reference means, said in the output and not only here.
///
/// **A single line, in brackets, declarations included.** Written on
/// their own lines, the declarations were counted as delivered
/// CONTENT: the `e2e_a_two_stage_fold_states_the_original_magnitude` guard counts
/// what the answer returns while skipping lines starting with `[`, and one more
/// declaration shifted the announced number of missing lines by one.
/// Every annotation of this product is in brackets; this one only
/// half was.
const LEGEND: &str = "[tokenade:said-once] each «N» below stands for the quoted phrase after \
                      its `=`, written once here instead of on every line";

/// The reference replacing the phrase, by rank.
fn tag(i: usize) -> String {
    format!("«{}»", i + 1)
}

/// The candidate phrases: runs of words bounded by spaces, present
/// on at least [`MIN_LINES`] distinct lines.
///
/// Bounded by spaces on BOTH sides, because a cut in the middle of a
/// word makes the phrase unfindable in the output — this was paid for once on
/// `ioreg` identifiers.
fn candidates(lines: &[&str]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for line in lines {
        let words: Vec<&str> = line.split(' ').collect();
        let mut offs = Vec::with_capacity(words.len() + 1);
        let mut o = 0usize;
        for w in &words {
            offs.push(o);
            o += w.len() + 1;
        }
        offs.push(o);
        let mut here: std::collections::HashSet<&str> = Default::default();
        for i in 0..words.len() {
            // A SINGLE word counts too, as soon as it is long enough: a repeated path,
            // identifier or package name is exactly what
            // this fold exists to say once. Requiring two words left
            // `/tmp/validate-Demo-App-0.4.21-x86_64.pkg` out of reach, and the
            // only candidate enclosing it crossed the template fold's
            // separator — so refusing the structure also closed the door
            // that really paid.
            for j in (i + 1)..=words.len() {
                let (a, b) = (offs[i], offs[j] - 1);
                if b <= a || b > line.len() {
                    break;
                }
                let s = &line[a..b];
                if s.len() < MIN_PHRASE {
                    continue;
                }
                if s.len() > 200 {
                    break;
                }
                // **Never across another fold's structure.** The template
                // fold separates its values with ` · `, and a phrase that
                // encloses this separator turns `A · B · C` into `A «1» C`:
                // nothing is lost, but the list no longer counts the same
                // number of values, and its legend promises the opposite.
                // MEASURED on `ps aux`, where the three phrases kept were
                // all `· /tmp/…-x86_64.pkg` fragments.
                if crosses_a_structure(s) {
                    continue;
                }
                here.insert(s);
            }
        }
        for s in here {
            *seen.entry(s).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = seen
        .into_iter()
        .filter(|(_, n)| *n >= MIN_LINES)
        .map(|(s, n)| (s.to_string(), n))
        .collect();
    // The most profitable first: what it removes is its length minus the
    // reference, times the number of lines.
    v.sort_by_key(|(s, n)| std::cmp::Reverse((s.len().saturating_sub(4)) * n));
    v
}

/// Replaces the phrase **only where it is a whole word**.
///
/// The candidates are bounded by spaces on both sides; the replacement
/// was not, and `str::replace` bit inside words. The
/// `e2e_a_fold_does_not_hide_a_failure_it_has_no_keyword_for` guard
/// caught it: on an 800-line log, `18d39f34e2c31923` appears alone on
/// dozens of lines AND in `bundle-18d39f34e2c31923.tar.gz`, the ONLY
/// line saying the run failed. It came back as
/// `checksum mismatch for artifact bundle-«1».tar.gz` — unfindable for anyone
/// searching for the artifact name. The same lesson as for candidates, on the
/// replacement side this time.
fn replace_whole(body: &str, phrase: &str, tag: &str) -> String {
    // **Never inside a code block.** Ten lines of Rust folded
    // into `«1»  (x3)` give an excerpt that no longer compiles, and nothing in the
    // output says the fence is lying. Phrase detection, for its part, stays
    // on the WHOLE document: a phrase repeated on both sides of a block
    // must still be seen, which a split would take away from it.
    //
    // Accepted consequence: a phrase that exists ONLY in code is
    // still a candidate, is replaced nowhere, and the profitability gate
    // refuses it by itself. A round for nothing, never a corruption.
    let fenced = crate::fold_gate::fenced_ranges(body);
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    let mut at = 0usize;
    while let Some(i) = rest.find(phrase) {
        let before_ok = i == 0
            || rest[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        let end = i + phrase.len();
        let after_ok =
            end == rest.len() || rest[end..].chars().next().is_some_and(char::is_whitespace);
        let abs = at + i;
        let in_code = fenced.iter().any(|r| r.contains(&abs));
        if before_ok && after_ok && !in_code {
            out.push_str(&rest[..i]);
            out.push_str(tag);
        } else {
            out.push_str(&rest[..end]);
        }
        rest = &rest[end..];
        at += end;
    }
    out.push_str(rest);
    out
}

/// Replaces the heaviest phrases with a reference, and declares them at the top.
///
/// `None` when nothing pays: the text comes back intact rather than rewritten for
/// two bytes.
pub fn fold_if_it_pays(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_LINES {
        return None;
    }
    let mut body = text.to_string();
    let mut declared: Vec<String> = Vec::new();
    let mut phrases: Vec<String> = Vec::new();
    for (phrase, _) in candidates(&lines).into_iter().take(6) {
        if declared.len() == 3 {
            break;
        }
        // A phrase included in one already pulled out no longer exists.
        if !body.contains(&phrase) {
            continue;
        }
        // **And never a fragment of an already declared phrase.** Without this `lspci`
        // output `«1» = Advanced Micro Devices, Inc. [AMD]` THEN
        // `«2» = Advanced Micro Devices, Inc.`: two almost identical
        // declarations, which the reader has to tell apart for nothing.
        if phrases
            .iter()
            .any(|p: &String| p.contains(&phrase) || phrase.contains(p))
        {
            continue;
        }
        let t = tag(declared.len());
        let before = body.len();
        let candidate = replace_whole(&body, &phrase, &t);
        // **The phrase is in backticks.** Without them, a declaration
        // whose phrase itself contains the separator — measured on
        // `ps aux`, where the template fold leaves values starting with
        // `·` — reads `«1» = · /tmp/x · «2» = …` and the reader no longer knows
        // where the first one ends.
        let head = format!("{t} = `{phrase}`");
        if before <= candidate.len() + head.len() + 3 {
            continue;
        }
        body = candidate;
        declared.push(head);
        phrases.push(phrase);
    }
    if declared.is_empty() {
        return None;
    }
    let folded = format!("{LEGEND} · {}\n{body}", declared.join(" · "));
    // The shared profitability gate: changing an output's shape for
    // less than a tenth is a cost the bytes do not measure.
    if crate::fold_gate::pays(text.len(), folded.len(), 0) {
        Some(folded)
    } else {
        None
    }
}

#[cfg(test)]
mod a_phrase_is_never_replaced_inside_code {
    use super::fold_if_it_pays;

    /// The phrase recurs in PROSE and in a code block. The prose occurrences
    /// fold; those of the block stay, otherwise the excerpt no longer
    /// compiles and nothing in the output says the fence is lying.
    ///
    /// The fixture must REACH the rule: more than five lines, a
    /// phrase of at least sixteen characters, and a gain clear enough for the
    /// profitability gate to accept — otherwise the gate is what refuses, and the green
    /// test would say nothing about the code block.
    #[test]
    fn the_fenced_occurrences_survive() {
        // The REPEATED run must be identical on both sides, otherwise the one
        // the fold picks exists only in the prose and the block is never a
        // candidate — the fixture then does not reach the rule. Bounded by
        // spaces on both sides, otherwise `replace_whole` refuses it before
        // reaching the fence (the two other refusals the harness names).
        let shared = "configure le ASUS XG-C100C 10G Network Adapter puis sort";
        let mut doc = String::new();
        for i in 0..8 {
            doc.push_str(&format!("Note {i} : {shared} et rien de plus\n"));
        }
        doc.push_str("```rust\n");
        for _ in 0..3 {
            doc.push_str(&format!("// {shared} et rien de plus\n"));
        }
        doc.push_str("```\n");
        let out = fold_if_it_pays(&doc).expect("the fixture does not reach the rule: nothing paid");

        assert!(out.contains('«'), "no reference placed:\n{out}");
        let fenced: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.trim_start().starts_with("```"))
            .skip(1)
            .take_while(|l| !l.trim_start().starts_with("```"))
            .collect();
        assert_eq!(
            fenced.len(),
            3,
            "the block does not have three lines:\n{out}"
        );
        assert!(
            fenced.iter().all(|l| l.contains(shared)),
            "the fold entered the code block:\n{out}"
        );
        assert!(
            !fenced.iter().any(|l| l.contains('«')),
            "a reference was placed in a code block:\n{out}"
        );
    }

    /// Same phrase in prose and in a CommonMark indented run.
    /// The indented occurrences stay: `fenced_ranges` names them.
    #[test]
    fn the_indented_occurrences_survive() {
        let shared = "configure le ASUS XG-C100C 10G Network Adapter puis sort";
        let mut doc = String::new();
        for i in 0..8 {
            doc.push_str(&format!("Note {i} : {shared} et rien de plus\n"));
        }
        doc.push('\n');
        for _ in 0..3 {
            doc.push_str(&format!("    // {shared} et rien de plus\n"));
        }
        let out = fold_if_it_pays(&doc).expect("the fixture does not reach the rule: nothing paid");

        assert!(out.contains('«'), "no reference placed:\n{out}");
        let indented: Vec<&str> = out.lines().filter(|l| l.starts_with("    //")).collect();
        assert_eq!(
            indented.len(),
            3,
            "the indented run does not have three lines:\n{out}"
        );
        assert!(
            indented.iter().all(|l| l.contains(shared)),
            "the fold entered the indented code:\n{out}"
        );
        assert!(
            !indented.iter().any(|l| l.contains('«')),
            "a reference was placed in an indented run:\n{out}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `lspci` capture: a vendor name on 23 lines.
    #[test]
    fn a_phrase_repeated_mid_line_is_said_once() {
        let classes = [
            "Host bridge",
            "IOMMU",
            "PCI bridge",
            "SMBus",
            "ISA bridge",
            "Encryption controller",
            "USB controller",
            "Audio device",
            "Non-Essential Instrumentation",
            "SATA controller",
            "Signal processing controller",
            "Network controller",
        ];
        let text: String = classes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!("00:{i:02}.0 {c}: Advanced Micro Devices, Inc. [AMD] Raphael Ridge {c}\n")
            })
            .collect();
        let out = fold_if_it_pays(&text).expect("the phrase pays for its reference");
        assert!(out.len() < text.len(), "{} vs {}", out.len(), text.len());
        // Declared ONCE, and therefore still findable in the output.
        assert_eq!(
            out.matches("Advanced Micro Devices, Inc. [AMD]").count(),
            1,
            "{out}"
        );
        assert!(out.contains("«1» = "), "the declaration is missing: {out}");
        assert!(out.contains(LEGEND), "the legend is missing: {out}");
        // **The declaration fits on the legend LINE**, in brackets
        // like every annotation of this product: output on its own line,
        // it was counted as delivered content and shifted by one the
        // number of missing lines a stage-2 summary announces.
        let first = out.lines().next().unwrap_or_default();
        assert!(
            first.starts_with('['),
            "the legend does not open with `[`: {first}"
        );
        assert!(
            first.contains("«1» = "),
            "the declaration left the legend: {first}"
        );
        // Every line recomposes: the reference is where the phrase was, and
        // the rest of the line is intact.
        for (i, c) in classes.iter().enumerate() {
            assert!(
                out.contains(&format!("00:{i:02}.0 {c}: «1» {c}")),
                "line {i} damaged: {out}"
            );
        }
    }

    /// **A reference never crosses another fold's structure.**
    ///
    /// The template fold separates its values with ` · `. A phrase enclosing
    /// that separator turns `A · B` into `A«1»`: nothing is lost, and
    /// yet the list no longer shows the same number of values, while the
    /// template's legend promises exactly the opposite. MEASURED on `ps aux`, where
    /// the three phrases kept were all
    /// `· /tmp/…-x86_64.pkg` fragments.
    #[test]
    fn a_tag_never_swallows_the_separator_of_another_fold() {
        let text: String = (0..8)
            .map(|i| format!("  valeur{i} · /tmp/validate-Demo-App-0.4.21-x86_64.pkg · autre{i}\n"))
            .collect();
        let out = fold_if_it_pays(&text).expect("the repeated path pays for its reference");
        assert!(
            !out.contains("`·") && !out.contains("· `"),
            "a reference swallowed the template fold's separator: {out}"
        );
        // And every line still shows its TWO separators.
        for line in out.lines().skip(1) {
            assert_eq!(
                line.matches(" · ").count(),
                2,
                "the arity of the value list changed: {line}"
            );
        }
        assert!(
            crosses_a_structure("a · b"),
            "the separator is not recognised"
        );
        assert!(
            crosses_a_structure("x «1» y"),
            "the reference is not recognised"
        );
        // A column's padding and a table rule are
        // structure, not text.
        // One fixture per disjunct, and each must get past the OTHERS:
        // this one is rich in alphanumeric characters, so only the
        // two-spaces rule can refuse it. Written `10/02/2026  20:16      0`,
        // it also fell under the rule-line rule and the mutant survived
        // (three iterations in a row).
        assert!(
            crosses_a_structure("explorer.exe  1234  Services"),
            "column padding is not recognised"
        );
        assert!(
            !crosses_a_structure("explorer.exe 1234 Services"),
            "a single space is not a column"
        );
        // And this one carries NO double space, so only the rule-line
        // rule can refuse it.
        assert!(
            crosses_a_structure("======================== ============="),
            "a table rule is not recognised"
        );
        // And the real phrases of the corpus all pass.
        for ok in [
            "Advanced Micro Devices, Inc. [AMD]",
            "RootDomainUserClient",
            "/tmp/validate-Demo-App-0.4.21-x86_64.pkg",
            "fe80::1/64 (proto kernel_ll)",
            "symbolic link to",
        ] {
            assert!(!crosses_a_structure(ok), "legitimate phrase refused: {ok}");
        }
        assert!(!crosses_a_structure("un texte ordinaire"));
    }

    /// **The reference never bites inside a word.**
    ///
    /// The candidates are bounded by spaces; the replacement was
    /// not. On a real log, `18d39f34e2c31923` appears alone on
    /// dozens of lines AND in `bundle-18d39f34e2c31923.tar.gz`, the only
    /// line saying the run failed: it came back as
    /// `bundle-«1».tar.gz`.
    #[test]
    fn a_tag_never_bites_inside_a_word() {
        let mut text = String::new();
        for i in 0..9 {
            text.push_str(&format!(
                "commit {i} touche 18d39f34e2c31923 dans le module\n"
            ));
        }
        text.push_str("checksum mismatch for artifact bundle-18d39f34e2c31923.tar.gz\n");
        let out = fold_if_it_pays(&text).expect("the phrase pays for its reference");
        assert!(
            out.contains("bundle-18d39f34e2c31923.tar.gz"),
            "the reference bit into a word: {out}"
        );
        assert!(out.contains("«1»"), "nothing was folded: {out}");
        assert_eq!(
            out.matches("18d39f34e2c31923").count(),
            2,
            "the identifier must stay in the declaration AND in the artifact name: {out}"
        );
    }

    /// Too rare, too short, or not paying: the text comes back intact.
    #[test]
    fn a_phrase_that_does_not_pay_leaves_the_text_alone() {
        // **Only four lines**, and nothing else refuses: the phrase
        // is long, it dominates its line, and the fold would pay handsomely if the
        // bound gave way. The fixture must get past every other bound,
        // otherwise the mutation of this one survives (twice already).
        let long = "une phrase commune vraiment tres longue qui domine largement chacune \
                    des lignes ou elle apparait et qui paierait son renvoi sans peine";
        let mut rare = String::new();
        for i in 0..4 {
            rare.push_str(&format!("{long} {i}\n"));
        }
        rare.push_str("une autre ligne\nencore une autre\n");
        assert!(
            fold_if_it_pays(&rare).is_none(),
            "four lines are not enough, and nothing else was supposed to refuse"
        );
        // Enough lines, but the phrase is too SHORT.
        let short: String = (0..9).map(|i| format!("aaa bb cc {i}\n")).collect();
        assert!(
            fold_if_it_pays(&short).is_none(),
            "a short phrase does not pay"
        );
        // Enough lines and long enough, but it does not give back a tenth:
        // the line is almost entirely unique next to it.
        let words = [
            "alpha bravo charlie delta echo foxtrot golf hotel india juliett kilo lima",
            "mike november oscar papa quebec romeo sierra tango uniform victor whiskey",
            "xray yankee zulu aleph beth gimel daleth he waw zayin heth teth yodh kaph",
            "lamedh mem nun samekh ayin pe sadhe qoph resh shin taw alif ba ta tha jim",
            "ha kha dal dhal ra zay sin shin sad dad tah zah ayn ghayn fa qaf kaf lam",
            "mim nun ha waw ya hamza alpha beta gamma delta epsilon zeta eta theta iota",
        ];
        let thin: String = words
            .iter()
            .map(|m| format!("phrase commune ici, puis {m}\n"))
            .collect();
        assert!(
            fold_if_it_pays(&thin).is_none(),
            "a tenth not reached: must refuse"
        );
    }
}
