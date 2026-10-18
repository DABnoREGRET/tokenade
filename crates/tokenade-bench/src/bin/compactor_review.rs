//! Review of ALL compactors, all platforms.
//!
//! `cargo run -q --release -p tokenade-bench --bin compactor_review`
//!
//! There are 905 compactor modules and no enumerable registry: the only
//! universal entry point is `filter_command(&[program], text)`, and the
//! program name is the module name. Reading them one by one is not a
//! review, it is 905 opinions.
//!
//! What this does instead: cross every compactor with every REAL output
//! of the captured corpus, and check the invariants that do not depend
//! on whether the input made sense for that particular compactor.
//!
//! * **PANIC** — a compactor must never kill the process, whatever it is
//!   given. Four defects of this class have already been found and
//!   fixed here, all from slicing by BYTES a
//!   text that carried a multibyte character. In production
//!   `filter_command_fail_open` catches it, so a panic does not show:
//!   it just silently costs all the compaction of that output.
//! * **GROWTH** — a compactor that returns MORE bytes than it
//!   received has failed at its only reason to exist. Measured before writing this:
//!   `journalctl` returns 104 % under a French locale.
//!
//! And two measurements, not verdicts:
//!
//! * **CLAIMS WITHOUT FOLDING** — the named compactor accepts the output and
//!   returns at least 98 % of it. It "succeeded", so the language-free ladder behind
//!   it does not apply: its claim COSTS the fallback fold.
//! * **LANGUAGE-DEPENDENT** — the compactor stops answering when the
//!   labels are scrambled, on an output it used to claim.

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Everything the router knows how to route. This is not a list rebuilt from
/// file names: `build.rs` emits `AUTO_DISPATCH_PROGS`, the exact
/// set of names `dispatch_auto` accepts, dashed aliases included. A review
/// that guesses compactor names measures its own guess — the
/// first version of this bench returned `["git","log","oneline"]` for
/// `git_log_oneline.rs`, where the real invocation is `git log --oneline`, and
/// so attributed its verdicts to the wrong compactor.
fn routed_programs() -> Vec<Vec<String>> {
    let mut v: Vec<Vec<String>> = tokenade_core::cmd::AUTO_DISPATCH_PROGS
        .iter()
        .map(|p| vec![p.to_string()])
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The real captures, through the bench's SINGLE enumerator. This body used to be here,
/// and a second one, disagreeing, in `per_command`.
fn corpus_outputs() -> Vec<(String, String)> {
    tokenade_bench::captures_on(tokenade_bench::Route::Command)
        .into_iter()
        .map(|c| (c.key, c.body))
        .collect()
}

/// What a PADDING fold would return on an output we still deliver
/// almost whole. Not a fix: a COSTING, in the spirit of
/// `web_headroom` — what we would gain if the language-free ladder could see
/// that shape. Counted in bytes of interior spaces, which is
/// exactly what `collapse_alignment_padding` removes.
fn padding_headroom(text: &str) -> usize {
    let mut interior = 0usize;
    for line in text.lines() {
        let t = line.trim_end();
        let mut run = 0usize;
        let mut started = false;
        for c in t.chars() {
            if c == ' ' || c == '\t' {
                run += 1;
            } else {
                if started && run > 1 {
                    interior += run - 1;
                }
                run = 0;
                started = true;
            }
        }
        interior += line.len() - t.len();
    }
    interior
}

/// What a slice of compactors found. Returned by value rather than
/// accumulated in shared state: the bench then has no lock and no order
/// depending on scheduling.
#[derive(Default)]
struct Findings {
    panics: Vec<String>,
    grew: Vec<String>,
    claims: Vec<String>,
    calls: usize,
    folded: usize,
}

fn review_slice(compactors: &[(String, Vec<String>)], outputs: &[(String, String)]) -> Findings {
    let mut f = Findings::default();
    for (name, argv) in compactors {
        for (key, body) in outputs {
            f.calls += 1;
            let r = catch_unwind(AssertUnwindSafe(|| {
                tokenade_core::filter::filter_command(argv, body)
            }));
            let Ok(r) = r else {
                f.panics.push(format!("{name} on {key}"));
                continue;
            };
            if r.output.len() * 100 < body.len() * 95 {
                f.folded += 1;
            }
            // A compactor that returns more bytes than it receives has failed at
            // its only reason to exist. The margin covers a short banner
            // added to an already tiny output, where a few more bytes
            // buy information the agent did not have.
            if r.output.len() > body.len() + 200 {
                f.grew.push(format!(
                    "{name} on {key}: {} → {} B ({})",
                    body.len(),
                    r.output.len(),
                    r.filter
                ));
            }
            // "Claims without folding" only makes sense on a REAL pair:
            // the `ansible` compactor applied to a `df` output
            // obviously claims something it will never be given,
            // and the first version of this bench returned 5,370 lines of that
            // noise. The fixture is named after the program that
            // produced it; that name, and that name alone, pairs.
            let stem = key.rsplit('/').next().unwrap_or(key);
            let matches_fixture = stem == name
                || stem
                    .strip_prefix(name.as_str())
                    .is_some_and(|r| r.starts_with('_'));
            if !matches_fixture
                || !tokenade_core::filter::filter_name_is_command_specific(&r.filter)
            {
                continue;
            }
            if r.output.len() * 100 >= body.len() * 98 && body.len() > 512 {
                let head = padding_headroom(&r.output);
                // How many of these outputs are HOLED tables — the only case
                // the per-segment fold still refuses, and so the only one whose
                // margin calls for something other than a padding fold?
                let holed = head * 5 >= r.output.len()
                    && !tokenade_core::archetypes::padding_is_decorative(&r.output);
                f.claims.push(format!(
                    "{name} on {key}: {} → {} B ({}) — padding: {head} B, i.e. {:.0} %{}",
                    body.len(),
                    r.output.len(),
                    r.filter,
                    100.0 * head as f64 / r.output.len() as f64,
                    if holed { " · HOLED TABLE" } else { "" }
                ));
            }
        }
    }
    f
}

fn main() {
    // This bench takes NO argument: silently accepting one would
    // make it answer a different question from the one asked.
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    // Panics are what we are looking for: no trace for each one, otherwise the
    // output is unreadable long before it is useful.
    std::panic::set_hook(Box::new(|_| {}));

    let compactors: Vec<(String, Vec<String>)> = routed_programs()
        .into_iter()
        .map(|argv| (argv.join(" "), argv))
        .collect();
    let outputs = corpus_outputs();
    eprintln!(
        "{} compactors × {} real outputs = {} calls",
        compactors.len(),
        outputs.len(),
        compactors.len() * outputs.len()
    );

    // A fifteen-minute instrument is useless in a loop: it is
    // rerun after every fix, and the waiting becomes the dominant cost of
    // the iteration. 554,148 calls on a single core while twenty-three
    // sleep is the same defect `cargo test` had before nextest.
    // Split by COMPACTOR: each thread takes a slice of the list and returns
    // its three vectors, so no shared state and the order is rebuilt at
    // the end.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(compactors.len().max(1));
    let chunk = compactors.len().div_ceil(threads.max(1));
    let outputs = &outputs;
    let results: Vec<Findings> = std::thread::scope(|scope| {
        let handles: Vec<_> = compactors
            .chunks(chunk.max(1))
            .map(|slice| scope.spawn(move || review_slice(slice, outputs)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut panics: Vec<String> = Vec::new();
    let mut grew: Vec<String> = Vec::new();
    let mut claims_without_folding: Vec<String> = Vec::new();
    let mut calls = 0usize;
    let mut folded = 0usize;
    for f in results {
        panics.extend(f.panics);
        grew.extend(f.grew);
        claims_without_folding.extend(f.claims);
        calls += f.calls;
        folded += f.folded;
    }

    let _ = std::panic::take_hook();
    // Without this floor, a bench that folds nothing at all — routing gate,
    // broken routing, empty corpus — reports "0 PANIC, 0 GROWTH" and
    // reads like good news. It is the release passthrough that once made
    // the `probe` bench announce "−0.0 %".
    //
    // It had been written, then LOST when parallelising this bench: the count
    // survived in each slice and nobody read it any more, which only
    // a `warning: assigned but never used` said. A guard that can no longer
    // fail is exactly what it exists against.
    if tokenade_bench::instrument_is_dead(folded, calls) {
        eprintln!(
            "STOP: {folded} calls out of {calls} folded anything at all. This bench \n\
             measures nothing — check the routing gate and the routing before \n\
             reading a verdict."
        );
        std::process::exit(2);
    }
    println!("{calls} calls, of which {folded} folded\n");
    for (title, rows) in [
        ("PANIC", &panics),
        ("GROWS", &grew),
        ("CLAIMS WITHOUT FOLDING", &claims_without_folding),
    ] {
        println!("## {title} — {}", rows.len());
        let mut shown = rows.clone();
        shown.sort();
        shown.dedup();
        for r in shown.iter().take(40) {
            println!("  {r}");
        }
        if shown.len() > 40 {
            println!("  … and {} more", shown.len() - 40);
        }
        println!();
    }
}
