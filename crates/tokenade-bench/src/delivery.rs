//! **What is DELIVERED, and through which stages.**
//!
//! A delivery is the output as the agent receives it — banners included,
//! since a banner is paid for in bytes like the rest. `Stages` says which
//! fold stages did work, and `argv_for` builds the invocation that
//! turns them on, because a stage we think we measure and that no argument switches on
//! returns a "0" that nothing distinguishes from a real zero.

use super::*;

/// **The first `.rs` under `root` newer than `built`**, if there is one.
///
/// Separate from [`Bench`] so it can be tested on a fabricated tree: a guard
/// that can only be exercised on the real repo is never exercised.
/// **A DELIVERY bench**: what an agent really receives, both stages
/// included.
///
/// It exists because measuring ONE stage names targets that are not targets.
/// MEASURED 2026-09-08: `json_headroom` ranked a third-party `…/ta.json` at the top
/// of its pool at **1,145,860 bytes still delivered (−9 %)**; the real
/// `tokenade read` delivers **1,891 (−99.9 %)** of it, because stage 2 stashes and
/// summarises what stage 1 let through. The instrument overestimated by a
/// factor of 600 and named as a target what the product already folds best.
///
/// The two guards live HERE and not in the caller, because each one
/// lied once when it lived in only one instrument:
///
/// * a binary older than the tree measures code that no longer exists
///   ([`Bench::source_newer_than_exe`]);
/// * the state left by the previous measurement makes the dedup answer instead
///   of the compactor, intermittently ([`Bench::forget`]).
pub struct Delivery {
    bench: Bench,
}

impl Delivery {
    /// Opens the bench, or returns the sentence to print when it refuses to measure.
    pub fn open() -> Result<Delivery, String> {
        let bench = Bench::new().map_err(|e| format!("the bench does not open: {e}"))?;
        if let Some(src) = bench.source_newer_than_exe() {
            return Err(stale_exe_note(bench.exe(), &src));
        }
        Ok(Delivery { bench })
    }

    pub fn bench(&self) -> &Bench {
        &self.bench
    }

    /// ONE independent measurement: the state is forgotten first.
    pub fn measure(&self, args: &[&str]) -> Result<usize, String> {
        self.measure_timed(args).map(|(b, _)| b)
    }

    /// The same measurement, but it returns the TEXT.
    ///
    /// `render` already exists for a capture served on standard input; a
    /// page is served by its FILE NAME (`web <file>`), so it had no
    /// door to its own rendering. That is what deprived `--pages` of the
    /// ranking by shape for as long as it existed: a
    /// byte census names the heavy pages, only a census of
    /// SHAPES names a class.
    pub fn render_args(&self, args: &[&str]) -> Result<String, String> {
        self.bench.forget();
        let run = self.bench.run(args).map_err(|e| e.to_string())?;
        Ok(run.text())
    }

    /// The same measurement, with the TIME — see [`Delivery::measure_stages_timed`]
    /// for the caveat that a debug time carries.
    pub fn measure_timed(&self, args: &[&str]) -> Result<(usize, u128), String> {
        self.bench.forget();
        let run = self.bench.run(args).map_err(|e| e.to_string())?;
        Ok((run.bytes(), run.ms_ratio_only()))
    }

    /// Writes a file into the bench's bin — see [`Bench::fixture`].
    ///
    /// The READ path is not measured through stdin: `read` routes on
    /// the EXTENSION, and content passed without a file name reaches no
    /// reader. It is the same trap as "measuring a capture under `cat`",
    /// one door further.
    pub fn fixture(&self, stem: &str, ext: &str, body: &str) -> Result<std::path::PathBuf, String> {
        self.bench
            .fixture(stem, ext, body)
            .map_err(|e| e.to_string())
    }

    /// **Measure a capture under ITS program**, and not under `cat`.
    ///
    /// The product routes on the command line. Writing the capture into a
    /// file and running `wrap "cat <file>"` therefore measures what `cat`
    /// triggers, and reports "nothing is folded" for every capture whose
    /// compactor is specific to its program. MEASURED 2026-09-08 on the 265
    /// captures: **284,036 bytes announced versus 156,503 actually delivered**,
    /// a residue overestimated by 81 %, and eight captures given at 100 % while
    /// their compactor exists and folds.
    ///
    /// `tokenade filter <prog>` compacts stdin "as if" that program
    /// had produced it. It is the command compactor ALONE — one stage: it
    /// stashes but does not summarise. `wrap` offers no way to attribute stdin
    /// to a program, so both stages under the right program are not
    /// reachable without fabricating a fake executable, which this repo has already
    /// ruled out. The caller MUST say so in its report rather than announce
    /// a delivery.
    pub fn measure_as_program(&self, prog: &str, body: &str) -> Result<usize, String> {
        self.measure_stages(prog, body, Stages::CommandOnly)
    }

    /// The same measurement, choosing HOW MANY stages it goes through.
    ///
    /// `--delivered` was built to get the figure for BOTH
    /// stages — the only one a patch note is allowed to quote. A later fix
    /// corrected its routing (`wrap "cat <capture>"` measured the compactor of
    /// `cat`) by going through `filter <prog>`, which has only one stage: the
    /// capability disappeared in a correct fix, and nothing said so — the
    /// module comment kept announcing two stages for eight
    /// iterations. The number of stages is now ASKED for, and the report says
    /// which one it measured.
    pub fn measure_stages(&self, prog: &str, body: &str, stages: Stages) -> Result<usize, String> {
        self.measure_stages_timed(prog, body, stages)
            .map(|(b, _)| b)
    }

    /// The same measurement, with the TIME the child took.
    ///
    /// In debug this time does not compare to a release time — but it does
    /// compare to ITSELF on an input twice as large, and that is all
    /// it takes to answer "does the cost grow faster than the
    /// input?". Hence `ms_ratio_only`, which carries that caveat in its
    /// name.
    pub fn measure_stages_timed(
        &self,
        prog: &str,
        body: &str,
        stages: Stages,
    ) -> Result<(usize, u128), String> {
        self.bench.forget();
        let argv = argv_for(stages, prog);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let run = self
            .bench
            .run_with_stdin(&argv, Some(body))
            .map_err(|e| e.to_string())?;
        Ok((run.bytes(), run.ms_ratio_only()))
    }

    /// The TEXT the agent would receive, not just its size.
    ///
    /// A byte census says that a fold returns; it does not say what it
    /// returns. Three structural corruptions went through a green total before
    /// someone read the first line.
    pub fn render(&self, prog: &str, body: &str, stages: Stages) -> Result<String, String> {
        self.bench.forget();
        let argv = argv_for(stages, prog);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let run = self
            .bench
            .run_with_stdin(&argv, Some(body))
            .map_err(|e| e.to_string())?;
        Ok(run.text())
    }
}

/// How many delivery stages a measurement goes through.
///
/// `wrap` applies TWO — the command compactor, then the stash
/// and the retrieval pointer — and they interact: folding at the first
/// can bring the document below the second's threshold, which stops cutting,
/// and the DELIVERED output grows (measured 16,122 → 20,397).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stages {
    /// The command compactor alone. Names targets; does not put a figure on a gain.
    CommandOnly,
    /// What the agent really receives.
    Delivered,
    /// The EXTRACTION of a page, through `web <file>`: one stage, and it is not
    /// the same as a command's.
    ///
    /// Written here because the guard `no_instrument_spells_a_stage_claim_of_its_own`
    /// demanded it — the page census announced its stages itself, which
    /// is exactly the earlier fault redone in ten-minute code.
    PageOnly,
}

impl Stages {
    /// **All** the variants, so that a guard can walk them.
    ///
    /// Without it, `the_banner_says_which_path_was_taken` listed two
    /// variants by hand: a third arrived silent, and the rule "a
    /// stage does not present itself as deliverable" did not hold it. It is the
    /// same fault one level below the one this type fixes.
    pub const ALL: &'static [Stages] = &[Stages::CommandOnly, Stages::Delivered, Stages::PageOnly];

    /// Does this announcement hold for what the agent REALLY receives?
    pub fn is_delivered(self) -> bool {
        matches!(self, Stages::Delivered)
    }

    /// The sentence that ANNOUNCES a partial verification: the top of a
    /// ranking re-measured on both stages, when the total is not.
    ///
    /// It exists because two instruments spelled it, and the one that
    /// spelled it twice used it once for a stage-1 total.
    pub fn verification_header() -> &'static str {
        "VERIFICATION on the delivery path (both stages, real CLI) —\n         \x20 the TOTAL below is not:"
    }

    /// How a report must present itself. Written here so that the sentence follows
    /// the path taken, and not a module comment one forgets to
    /// update — which is exactly what happened before.
    pub fn banner(self) -> &'static str {
        match self {
            Stages::CommandOnly => {
                "ONE stage ONLY: this report names targets, it does not put a figure on a \
                 deliverable gain."
            }
            Stages::Delivered => "BOTH stages: this is what the agent receives.",
            Stages::PageOnly => {
                "ONE stage ONLY: the extraction of a page. This report names targets, it \
                 does not put a figure on a deliverable gain."
            }
        }
    }
}

/// The argv that asks for `stages` stages for the output of `prog`.
///
/// Extracted to be checkable: a flag that does not reach the CLI returns the
/// same number under another name, and that is precisely what happened
/// before.
pub fn argv_for(stages: Stages, prog: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["filter".to_string()];
    if stages == Stages::Delivered {
        argv.push("--deliver".to_string());
    }
    argv.extend(prog.split_whitespace().map(str::to_string));
    argv
}
