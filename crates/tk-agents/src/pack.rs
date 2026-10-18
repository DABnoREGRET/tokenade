//! `tokenade pack` core: budget-bounded repo export planning + rendering.
//!
//! Pure logic only — the CLI wrapper (`cmd_pack`) does the IO (git scoping,
//! file reads, tokenization) and feeds the results here. Splitting it this way
//! keeps the budget/fit math and the markdown rendering unit-testable without
//! touching a filesystem or shelling out to git.
//!
//! The contract with the caller: it hands us files already in *priority order*
//! (most-wanted first). With a token budget we walk that order, including each
//! file that still fits and marking the rest omitted — but we never stop early,
//! so a small high-value file after a big skipped one still lands (a bounded
//! knapsack approximation, mirroring [`crate::review_context::greedy_fit`]).
//! Crucially we never truncate SILENTLY: when the budget cannot pay for the
//! whole omitted manifest, what is left out is counted rather than dropped —
//! and the totals (`omitted_count`, `omitted_tokens`) always cover every file.

/// One file considered for the bundle, with its token cost and whether it made
/// the budget cut. `lang` is the markdown fence language (empty = plain fence).
#[derive(Debug, Clone)]
pub struct PackEntry {
    pub path: String,
    pub tokens: usize,
    pub lang: String,
    pub body: String,
    pub included: bool,
}

/// The outcome of fitting a candidate set into a budget.
#[derive(Debug, Clone, Default)]
pub struct PackPlan {
    pub budget: Option<usize>,
    pub entries: Vec<PackEntry>,
    pub included_tokens: usize,
    pub included_count: usize,
    pub omitted_count: usize,
    pub omitted_tokens: usize,
}

impl PackPlan {
    /// Tokens the full candidate set would have cost — the baseline a naive
    /// "dump every file" export would pay.
    pub fn candidate_tokens(&self) -> usize {
        self.included_tokens + self.omitted_tokens
    }
}

/// Fit `files` (caller's priority order, most-wanted first) into an optional
/// token budget. `None` budget → include everything. Order is preserved in
/// `entries` so the rendered bundle reads predictably.
pub fn plan_pack(files: Vec<PackEntry>, budget: Option<usize>) -> PackPlan {
    let mut entries = Vec::with_capacity(files.len());
    let mut included_tokens = 0usize;
    let mut included_count = 0usize;
    let mut omitted_count = 0usize;
    let mut omitted_tokens = 0usize;

    // What the BUNDLE has spent, which is not the same as the content it
    // carries: `included_tokens` stays the files' own text, because that is
    // what the savings arithmetic compares against, while the fit decision
    // pays for the sections too.
    let mut spent = 0usize;
    for mut e in files {
        let cost = entry_cost(&e);
        let fits = match budget {
            Some(b) => spent + cost <= b,
            None => true,
        };
        if fits {
            spent += cost;
            included_tokens += e.tokens;
            included_count += 1;
        } else {
            omitted_count += 1;
            omitted_tokens += e.tokens;
        }
        e.included = fits;
        entries.push(e);
    }

    PackPlan {
        budget,
        entries,
        included_tokens,
        included_count,
        omitted_count,
        omitted_tokens,
    }
}

/// Pick a fence that won't collide with the file body: the shortest run of
/// backticks not appearing in `body`, at least 3. Prevents a file that itself
/// contains ```` ``` ```` from breaking out of its code block.
fn fence_for(body: &str) -> String {
    let mut n = 3usize;
    loop {
        let fence = "`".repeat(n);
        if !body.contains(&fence) {
            return fence;
        }
        n += 1;
    }
}

/// The markdown ONE included file occupies in the bundle: its heading, its
/// fence, the fence language and the blank lines that separate it from the
/// next file.
///
/// One function because the planner and the renderer have to agree on it, and
/// they did not: see [`entry_cost`].
fn section(path: &str, lang: &str, fence: &str, body: &str) -> String {
    let tail = if body.ends_with('\n') { "" } else { "\n" };
    format!("## {path}\n\n{fence}{lang}\n{body}{tail}{fence}\n\n")
}

/// What including this file really costs the bundle: its content AND the
/// section that carries it.
///
/// The plan charged the CONTENT alone. A file with no content therefore cost
/// nothing to the budget and about nineteen tokens to the reader, so every
/// empty file in the repository always fitted, however small the budget.
/// Measured 2026-09-03, `tokenade pack --budget 50` on this repository:
/// 23 files reported as "included, ~50 tokens" — 22 of them empty test
/// fixtures — rendered as 700 tokens, fourteen times the budget, of which
/// about 430 were headings wrapped around empty fences. The help says a very
/// small budget is exceeded by the ten-name floor "and by nothing else"; this
/// was the something else.
///
/// The scaffolding is priced on an EMPTY body, so the file's own text is
/// tokenized once by the caller and never again here — a bundle can carry
/// thousands of candidates.
pub fn entry_cost(e: &PackEntry) -> usize {
    e.tokens + crate::tokenizer::estimate(&section(&e.path, &e.lang, &fence_for(&e.body), ""))
}

/// Fewest omitted files to name even when the budget has nothing left. A
/// manifest of two lines says nothing about the shape of what is missing.
const MIN_OMITTED_LISTED: usize = 10;

/// How many of `lines` fit in `room` tokens, never fewer than
/// [`MIN_OMITTED_LISTED`].
///
/// Shared because this rule has TWO publishers — the markdown manifest and
/// `--json` — and was written into one. Measured 2026-09-03: `pack --budget
/// 200 --json` over `tokenade-cli` returned 4 198 tokens listing all 165
/// omitted files, 21x the cap the help calls a cap, while the markdown path
/// returned 438. A budget honoured on one surface and ignored on the other is
/// not a budget.
pub fn omitted_fit(lines: &[String], room: Option<usize>) -> usize {
    let Some(room) = room else {
        return lines.len();
    };
    let mut used = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let cost = crate::tokenizer::estimate(line);
        if i >= MIN_OMITTED_LISTED && used + cost > room {
            return i;
        }
        used += cost;
    }
    lines.len()
}

/// Render the bundle to a single markdown document. The header is an honest
/// manifest: scope, budget, totals, and — when anything was dropped — the full
/// omitted list with sizes, so the agent knows exactly what is NOT here.
pub fn render_markdown(
    plan: &PackPlan,
    scope_desc: &str,
    root_desc: &str,
    skipped_binary_or_large: usize,
) -> String {
    let mut out = String::with_capacity(plan.included_tokens * 4 + 512);
    out.push_str("# tokenade pack\n\n");
    out.push_str(&format!("- root: `{root_desc}`\n"));
    out.push_str(&format!("- scope: {scope_desc}\n"));
    match plan.budget {
        Some(b) => out.push_str(&format!("- budget: {b} tokens\n")),
        None => out.push_str("- budget: unbounded\n"),
    }
    out.push_str(&format!(
        "- included: {} file(s), ~{} tokens\n",
        plan.included_count, plan.included_tokens
    ));
    if plan.omitted_count > 0 {
        out.push_str(&format!(
            "- omitted (over budget): {} file(s), ~{} tokens\n",
            plan.omitted_count, plan.omitted_tokens
        ));
    }
    // The third category, and the one that used to be missing. A file the
    // reader could not take as text is in NEITHER list above, so `included +
    // omitted` did not add up to the scope and nothing said why: on this repo
    // that was 29 + 2241 against 2320 in scope, with 50 files unaccounted.
    // The count was computed and handed to the JSON wire only.
    if skipped_binary_or_large > 0 {
        out.push_str(&format!(
            "- skipped (binary, or larger than the per-file cap): {skipped_binary_or_large} file(s)\n"
        ));
    }
    out.push('\n');

    // Omitted manifest first — surface the gap before the content, never bury
    // it. No silent truncation: what does not fit is COUNTED, which is the
    // half of "never dropped silently" that matters.
    //
    // The list used to be unbounded, and it swamped the very budget it
    // documents: measured 2026-09-03, `pack --budget 200` over this crate
    // returned 16 923 tokens — one included file of ~199 and 1 233 lines of
    // manifest, 84x the cap the help calls a cap. An agent that asks for two
    // hundred tokens and receives seventeen thousand of file names has been
    // handed the opposite of what it asked for.
    if plan.omitted_count > 0 {
        out.push_str("## omitted (did not fit the budget)\n\n");
        let omitted: Vec<_> = plan.entries.iter().filter(|e| !e.included).collect();
        // Unbounded pack lists everything, as before. A bounded one spends
        // what the budget has left after the header and the content, and
        // never fewer than [`MIN_OMITTED_LISTED`] — a manifest of two lines
        // tells the agent nothing about the shape of what it is missing.
        let room = plan.budget.map(|b| {
            let spent = crate::tokenizer::estimate(&out) + plan.included_tokens;
            b.saturating_sub(spent)
        });
        let lines: Vec<String> = omitted
            .iter()
            .map(|e| format!("- `{}` (~{} tokens)\n", e.path, e.tokens))
            .collect();
        let listed = omitted_fit(&lines, room);
        for line in &lines[..listed] {
            out.push_str(line);
        }
        if listed < omitted.len() {
            let rest: usize = omitted[listed..].iter().map(|e| e.tokens).sum();
            out.push_str(&format!(
                "- … and {} more file(s) (~{} tokens) — raise `--budget`, or `Read` them directly\n",
                omitted.len() - listed,
                rest
            ));
        }
        out.push('\n');
    }

    for e in plan.entries.iter().filter(|e| e.included) {
        out.push_str(&section(&e.path, &e.lang, &fence_for(&e.body), &e.body));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with no content is not free: the section that carries it costs
    /// the reader about nineteen tokens.
    ///
    /// The plan charged content alone, so every empty file in a repository
    /// always fitted. Measured 2026-09-03 on a fixture of 60 empty files and
    /// one small one, `pack --budget 50` included all 61 and rendered 714
    /// tokens — fourteen times the budget, nearly all of it headings around
    /// empty fences.
    ///
    /// The second assertion is the one that keeps the fix from being "include
    /// less": a file that fits must still get in.
    #[test]
    fn an_empty_file_is_charged_for_the_section_that_carries_it() {
        let empty = |i: usize| PackEntry {
            path: format!("empty_{i:02}.txt"),
            tokens: 0,
            lang: String::new(),
            body: String::new(),
            included: false,
        };
        let mut files: Vec<PackEntry> = (0..60).map(empty).collect();
        files.push(PackEntry {
            path: "small.txt".into(),
            tokens: 12,
            lang: String::new(),
            body: "alpha beta gamma\n".repeat(3),
            included: false,
        });

        let budget = 50usize;
        let plan = plan_pack(files, Some(budget));
        assert!(
            plan.included_count < 10,
            "60 empty files all fitted a {budget}-token budget: {} included",
            plan.included_count
        );
        assert!(
            plan.included_count > 0,
            "the budget must still buy something"
        );

        // What the reader actually pays for the file sections — the part of the
        // document the budget governs — must not exceed it.
        let md = render_markdown(&plan, "scope", "root", 0);
        // Slice the REAL document at the first file heading: everything above
        // it is the summary header and the omitted manifest, which the help
        // says sit outside the cap.
        let first = plan
            .entries
            .iter()
            .find(|e| e.included)
            .map(|e| format!("## {}\n", e.path))
            .expect("something was included");
        let at = md.find(&first).expect("the included file must be rendered");
        let spent = crate::tokenizer::estimate(&md[at..]);
        assert!(
            spent <= budget,
            "the file sections spent {spent} tokens of a {budget}-token budget"
        );

        // And the charge is not an under-estimate: what was billed covers what
        // was rendered, so the two cannot drift apart again.
        let billed: usize = plan
            .entries
            .iter()
            .filter(|e| e.included)
            .map(entry_cost)
            .sum();
        assert!(
            billed >= spent,
            "billed {billed} tokens for sections that render as {spent}"
        );
    }

    /// A budget big enough for content still buys the content.
    #[test]
    fn the_section_charge_does_not_starve_a_real_budget() {
        let files = vec![
            PackEntry {
                path: "a.rs".into(),
                tokens: 100,
                lang: "rust".into(),
                body: "fn a() {}\n".repeat(20),
                included: false,
            },
            PackEntry {
                path: "b.rs".into(),
                tokens: 100,
                lang: "rust".into(),
                body: "fn b() {}\n".repeat(20),
                included: false,
            },
        ];
        let plan = plan_pack(files, Some(400));
        assert_eq!(
            plan.included_count, 2,
            "400 tokens must still pay for two 100-token files"
        );

        // And content is still charged: a file too big for the budget must not
        // ride in on a cheap section. Charging the section INSTEAD of the
        // content would let it.
        let big = vec![PackEntry {
            path: "big.rs".into(),
            tokens: 1000,
            lang: "rust".into(),
            body: "fn big() {}\n".repeat(200),
            included: false,
        }];
        assert_eq!(
            plan_pack(big, Some(400)).included_count,
            0,
            "a 1000-token file must not fit a 400-token budget"
        );
    }

    /// The planner and the renderer spell one section the same way.
    ///
    /// They were two spellings of it: the plan priced the content and the
    /// renderer emitted a heading, a fence and its language around it. The
    /// fixture below is the shape where hand-rolled variants differ — a body
    /// that ends without a newline and carries a fence of its own.
    #[test]
    fn the_renderer_emits_exactly_the_section_the_plan_paid_for() {
        let e = PackEntry {
            path: "src/readme.md".into(),
            tokens: 9,
            lang: "markdown".into(),
            body: "one\n```\ninner fence\n```\nlast line without newline".into(),
            included: false,
        };
        let plan = plan_pack(vec![e.clone()], None);
        let md = render_markdown(&plan, "scope", "root", 0);
        let expected = section(&e.path, &e.lang, &fence_for(&e.body), &e.body);
        assert!(
            md.contains(&expected),
            "the rendered section is not the one the plan priced:\n{md}"
        );
        assert!(
            entry_cost(&e) >= crate::tokenizer::estimate(&expected) / 2,
            "the charge must be in the same order as what is rendered"
        );
    }

    /// The bounding rule itself, which has two publishers and was written
    /// into one. It must respect the floor, spend the room, and list
    /// everything when there is no budget at all.
    #[test]
    fn omitted_fit_holds_floor_room_and_unbounded() {
        let lines: Vec<String> = (0..80)
            .map(|i| format!("- `src/module_number_{i}/implementation.rs` (~1234 tokens)\n"))
            .collect();
        // No budget: everything, or an unbounded pack silently loses files.
        assert_eq!(omitted_fit(&lines, None), lines.len());
        // No room left still names the floor — a manifest of two lines says
        // nothing about the shape of what is missing.
        assert_eq!(omitted_fit(&lines, Some(0)), MIN_OMITTED_LISTED);
        // Room is spent, not ignored: more room lists strictly more, and
        // never more than there are. 600 and not 200 — ten of these lines
        // cost about two hundred tokens, so at 200 the FLOOR and the room
        // give the same answer and the fixture would be measuring neither.
        let small = omitted_fit(&lines, Some(600));
        let big = omitted_fit(&lines, Some(2000));
        assert!(
            small > MIN_OMITTED_LISTED && big > small,
            "room is not being spent: {small} then {big}"
        );
        assert!(big <= lines.len());
        // What it listed actually fits: the floor is the ONLY thing allowed
        // to exceed the room.
        let cost: usize = lines[..small]
            .iter()
            .map(|l| crate::tokenizer::estimate(l))
            .sum();
        assert!(
            cost <= 600,
            "listed {small} entries costing {cost} against a room of 600"
        );
    }

    /// The manifest of what did NOT fit must itself fit.
    ///
    /// `pack` lists every over-budget file so nothing is dropped in
    /// silence, and that list was unbounded: measured 2026-09-03,
    /// `pack --budget 200` over one crate returned 16 923 tokens — one
    /// included file of ~199 and 1 233 lines of manifest, eighty-four times
    /// the cap the help calls a cap. Counting the remainder keeps the
    /// promise the list was written for and the promise the flag makes.
    ///
    /// The fixtures oppose: a bounded plan must truncate and SAY so, an
    /// unbounded one must list everything. Either alone passes with the
    /// wrong half of the rule.
    fn plan_with(omitted: usize, budget: Option<usize>) -> PackPlan {
        let mut entries: Vec<PackEntry> = vec![PackEntry {
            path: "kept.rs".into(),
            tokens: 199,
            body: "fn main() {}\n".into(),
            lang: "rust".into(),
            included: true,
        }];
        for i in 0..omitted {
            entries.push(PackEntry {
                path: format!("crates/tokenade-core/src/module_with_a_realistic_name_{i}.rs"),
                tokens: 2000 + i,
                body: String::new(),
                lang: "rust".into(),
                included: false,
            });
        }
        PackPlan {
            included_count: 1,
            included_tokens: 199,
            omitted_count: omitted,
            omitted_tokens: entries
                .iter()
                .filter(|e| !e.included)
                .map(|e| e.tokens)
                .sum(),
            budget,
            entries,
        }
    }

    #[test]
    fn the_omitted_manifest_fits_the_budget_it_documents() {
        let bounded = render_markdown(&plan_with(1233, Some(2000)), "scope", "root", 0);
        let listed = bounded
            .lines()
            .filter(|l| l.starts_with("- `crates/"))
            .count();
        assert!(
            listed < 1233,
            "the manifest listed every one of 1233 omitted files inside a 2000-token budget"
        );
        assert!(
            listed >= MIN_OMITTED_LISTED,
            "the manifest kept {listed} names, below the floor of {MIN_OMITTED_LISTED}"
        );
        assert!(
            bounded.contains(&format!("and {} more file(s)", 1233 - listed)),
            "the truncation was silent — nothing counted the remainder:\n{}",
            bounded.lines().take(30).collect::<Vec<_>>().join("\n")
        );
        let n = crate::tokenizer::estimate(&bounded);
        assert!(n <= 2000, "a 2000-token budget produced {n} tokens");

        // The floor only bites when the budget has NOTHING left, which a
        // 2000-token fixture never reaches: at 100 the header and the
        // included file have already spent it, so without a floor the
        // manifest would be empty and the agent would learn nothing about
        // the shape of what it is missing.
        let tiny = render_markdown(&plan_with(1233, Some(100)), "scope", "root", 0);
        let listed = tiny.lines().filter(|l| l.starts_with("- `crates/")).count();
        assert_eq!(
            listed, MIN_OMITTED_LISTED,
            "with no budget left the manifest kept {listed} names, not the floor"
        );

        // And the remainder's token sum must be the tokens of the files
        // actually left out, not of every omitted file.
        let plan = plan_with(1233, Some(2000));
        let bounded2 = render_markdown(&plan, "scope", "root", 0);
        let listed2 = bounded2
            .lines()
            .filter(|l| l.starts_with("- `crates/"))
            .count();
        let expected: usize = plan
            .entries
            .iter()
            .filter(|e| !e.included)
            .skip(listed2)
            .map(|e| e.tokens)
            .sum();
        assert!(
            bounded2.contains(&format!("(~{expected} tokens) — raise")),
            "the remainder counted the wrong files: expected ~{expected} tokens"
        );

        // Unbounded still lists everything: the cap is the budget's doing,
        // not a new ceiling on what `pack` will show.
        let unbounded = render_markdown(&plan_with(1233, None), "scope", "root", 0);
        let listed = unbounded
            .lines()
            .filter(|l| l.starts_with("- `crates/"))
            .count();
        assert_eq!(listed, 1233, "an unbounded pack truncated its manifest");
        assert!(
            !unbounded.contains("more file(s)"),
            "an unbounded pack claimed a remainder it did not omit"
        );
    }

    fn entry(path: &str, tokens: usize) -> PackEntry {
        PackEntry {
            path: path.into(),
            tokens,
            lang: "rust".into(),
            body: format!("// {path}\n"),
            included: false,
        }
    }

    #[test]
    fn unbounded_budget_includes_everything() {
        let plan = plan_pack(vec![entry("a", 10), entry("b", 20)], None);
        assert_eq!(plan.included_count, 2);
        assert_eq!(plan.omitted_count, 0);
        assert_eq!(plan.included_tokens, 30);
        assert!(plan.entries.iter().all(|e| e.included));
    }

    #[test]
    fn budget_skips_overflow_but_keeps_scanning() {
        // a(60) fits, b(60) would overflow 130 → skipped, c(30) still fits.
        //
        // The budget was 100 when a file cost only its content. It buys the
        // sections now too — a heading, a fence and the blank lines around
        // them, about a dozen tokens each — so the same three files need a
        // little more room to exercise the same property, which is that the
        // walk does not STOP at the first file that does not fit.
        let plan = plan_pack(
            vec![entry("a", 60), entry("b", 60), entry("c", 30)],
            Some(130),
        );
        assert_eq!(plan.included_tokens, 90);
        assert_eq!(plan.included_count, 2);
        assert_eq!(plan.omitted_count, 1);
        assert!(plan.entries[0].included); // a
        assert!(!plan.entries[1].included); // b dropped
        assert!(plan.entries[2].included); // c still lands
        assert_eq!(plan.candidate_tokens(), 150);
    }

    #[test]
    fn order_is_preserved() {
        let plan = plan_pack(vec![entry("z", 1), entry("a", 1)], None);
        assert_eq!(plan.entries[0].path, "z");
        assert_eq!(plan.entries[1].path, "a");
    }

    #[test]
    fn render_reports_omitted_and_fences_bodies() {
        let mut a = entry("src/a.rs", 5);
        a.body = "fn main() {}".into();
        let mut b = entry("big.rs", 999);
        b.body = "x".into();
        // 40, not 10: a 5-token file now pays for the section that carries it,
        // so a budget of 10 no longer buys the one file this test renders.
        let plan = plan_pack(vec![a, b], Some(40));
        let md = render_markdown(&plan, "all tracked files", "/repo", 0);
        assert!(md.contains("## src/a.rs"));
        assert!(md.contains("```rust"));
        assert!(md.contains("fn main() {}"));
        // big.rs omitted → listed, body absent.
        assert!(md.contains("omitted (did not fit the budget)"));
        assert!(md.contains("`big.rs`"));
        assert!(!md.contains("\nx\n```"));
    }

    #[test]
    fn fence_escapes_embedded_backticks() {
        let body = "here is a fence:\n```\ncode\n```\n";
        let f = fence_for(body);
        assert!(f.len() >= 4, "must grow past the embedded triple fence");
        assert!(!body.contains(&f));
    }
}
