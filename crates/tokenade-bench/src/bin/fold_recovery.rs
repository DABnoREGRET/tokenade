//! **Do our folds send the agent looking for what they removed?**
//!
//! ```text
//! fold_recovery                  # the table per family
//! fold_recovery --canary         # validates the instrument
//! fold_recovery --out <file>     # the detail, written to a FILE
//! ```
//!
//! The MEASUREMENT lives in [`tokenade_bench::transcripts`], not here: a ratchet will
//! need it, and two implementations of the same oracle drift apart.
//!
//! The output goes to a file when asked (an output read from the
//! terminal is compacted before it reaches me).

use tokenade_bench::transcripts::{canary, mine, transcript_root};

use tokenade_bench::transcripts::ExecErrors;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(
        &[
            "--canary",
            "--out",
            "--reruns",
            "--greps",
            "--outcomes",
            "--says",
            "--errors",
            "--sequences",
            "--orphans",
            "--bypass",
            "--sizes",
            "--cross",
            "--resume",
        ],
        &args,
    );
    if args.iter().any(|a| a == "--canary") {
        let a = canary(true);
        let b = tokenade_bench::transcripts::canary_reruns(true);
        let c = tokenade_bench::transcripts::canary_greps(true);
        let d = tokenade_bench::transcripts::canary_outcomes(true);
        let e = tokenade_bench::transcripts::canary_complaints(true);
        let f = tokenade_bench::transcripts::canary_exec_errors(true);
        let g = tokenade_bench::transcripts::canary_sequences(true);
        let h = tokenade_bench::transcripts::canary_bypasses(true);
        let i = tokenade_bench::transcripts::canary_stash_sizes(true);
        std::process::exit(a.max(b).max(c).max(d).max(e).max(f).max(g).max(h).max(i));
    }
    if canary(false) != 0 {
        eprintln!("INSTRUMENT NOT VALIDATED — the canary no longer reproduces the known defect");
        std::process::exit(1);
    }
    let root = transcript_root();
    if args.iter().any(|a| a == "--cross") {
        // **Denominator floor**: a rate over three folds says
        // nothing, and the ranking would put it first.
        const FLOOR: usize = 20;
        let m = tokenade_bench::transcripts::mine(&root);
        let mut v: Vec<_> = m
            .by_family_project
            .iter()
            .filter(|(_, t)| t.folds >= FLOOR)
            .collect();
        v.sort_by(|a, b| {
            let ra = a.1.recovered as f64 / a.1.folds as f64;
            let rb = b.1.recovered as f64 / b.1.folds as f64;
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
        });
        println!("recovery rate per (family × project), at least {FLOOR} folds:");
        for ((fam, project), t) in v.iter().take(15) {
            let project = project.trim_start_matches("-home-username--devworkspace-projects-");
            println!(
                "  {:>5.1} %  {:>5} recovered / {:<5} {fam:<20} {project}",
                t.recovered as f64 * 100.0 / t.folds as f64,
                t.recovered,
                t.folds
            );
        }
        let below = m
            .by_family_project
            .values()
            .filter(|t| t.folds < FLOOR)
            .count();
        println!("({below} pair(s) below the floor, not ranked)");
        return;
    }
    if args.iter().any(|a| a == "--sizes") {
        if tokenade_bench::transcripts::canary_stash_sizes(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the size canary no longer comes out right");
            std::process::exit(1);
        }
        let z = tokenade_bench::transcripts::stash_sizes(&root);
        println!("recovery rate of a `stashed`, per size bucket:");
        for (bucket, (folds, recovered, whole)) in &z.buckets {
            let pct = if *folds == 0 {
                0.0
            } else {
                *recovered as f64 * 100.0 / *folds as f64
            };
            println!(
                "  {bucket:<14} {recovered:>4} / {folds:<5} ({pct:.1} %)  of which {whole} re-requested WHOLE"
            );
        }
        return;
    }
    if args.iter().any(|a| a == "--bypass") {
        if tokenade_bench::transcripts::canary_bypasses(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the bypass canary no longer comes out right");
            std::process::exit(1);
        }
        let b = tokenade_bench::transcripts::bypasses(&root);
        let total = b.after_folded_twin + b.after_plain_twin + b.first_time;
        println!(
            "{total} bypass(es) of the fold\n\
             the SAME command had already run, FOLDED: {} ({:.1} %)\n\
             … already run without a fold             : {}\n\
             never run before in the session          : {}",
            b.after_folded_twin,
            if total == 0 {
                0.0
            } else {
                b.after_folded_twin as f64 * 100.0 / total as f64
            },
            b.after_plain_twin,
            b.first_time
        );
        println!(
            "bytes delivered WITHOUT a fold: {} KB, of which {} KB by preemptive bypass",
            b.bypassed_bytes / 1024,
            b.preemptive_bytes / 1024
        );
        println!(
            "what the fold would have made of them: {} KB → {} KB ({:.0} % removed on the share \
             a compactor recognises)",
            b.would_fold_from / 1024,
            b.would_fold_to / 1024,
            if b.would_fold_from == 0 {
                0.0
            } else {
                100.0 - b.would_fold_to as f64 * 100.0 / b.would_fold_from as f64
            }
        );
        let mut progs: Vec<_> = b.by_program.iter().collect();
        progs.sort_by_key(|(_, (_, o))| std::cmp::Reverse(*o));
        println!("the bypassed programs, by bytes delivered:");
        for (prog, (n, bytes)) in progs.iter().take(12) {
            println!("  {prog:<16} {n:>5} times  {} KB", bytes / 1024);
        }
        for (project, cmd) in &b.after_folded_samples {
            println!("  AFTER A FOLD  ({project})  {cmd}");
        }
        return;
    }
    if args.iter().any(|a| a == "--orphans") {
        let m = tokenade_bench::transcripts::mine(&root);
        println!("{} orphan(s):", m.orphan_recoveries);
        let still_held = m.orphan_samples.iter().filter(|(_, _, s)| *s).count();
        println!(
            "  of which {still_held} whose local store still holds the bytes (the fold did happen)"
        );
        for (project, cmd, held) in &m.orphan_samples {
            let mark = if *held { "STORE" } else { "unknown" };
            println!("  {mark}  ({project})  {cmd}");
        }
        return;
    }
    if args.iter().any(|a| a == "--sequences") {
        if tokenade_bench::transcripts::canary_sequences(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the sequence canary no longer comes out right");
            std::process::exit(1);
        }
        let q = tokenade_bench::transcripts::recovery_sequences(&root);
        println!(
            "hashes recovered ONCE: {} · twice: {} · three times or more: {}",
            q.once, q.twice, q.more
        );
        let mut v: Vec<_> = q.pairs.iter().collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        for ((a, b), n) in v.iter().take(12) {
            println!("  {a:>6} → {b:<6}  {n}");
        }
        let mut r: Vec<_> = q.repeat_after.iter().collect();
        r.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        println!("what the first call had reported, when the second one REPEATS the same shape:");
        for (issue, n) in r {
            println!("  {issue:<14} {n}");
        }
        println!(
            "repeats whose first answer contained a banner: {} \
             (of which a NEW fold, of the answer itself: {})",
            q.repeat_on_folded_answer, q.repeat_on_refolded_answer
        );
        println!(
            "commands replayed WORD FOR WORD: {} (of which {} within 5 blocks)",
            q.identical, q.identical_immediate
        );
        println!("distance between two calls of the same SHAPE:");
        for (b, n) in &q.repeat_gap {
            println!("  {b:<22} {n}");
        }
        for (project, cmd) in &q.immediate_samples {
            println!("  IMMEDIATE ({project})  {cmd}");
        }
        return;
    }
    if args.iter().any(|a| a == "--errors") {
        if tokenade_bench::transcripts::canary_exec_errors(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the execution canary no longer comes out right");
            std::process::exit(1);
        }
        let e = tokenade_bench::transcripts::exec_errors(&root);
        println!(
            "commands run AFTER a folded result that failed  : {}/{} ({:.2} %)\n\
             CONTROL, after an intact result                 : {}/{} ({:.2} %)",
            e.after_fold.0,
            e.after_fold.1,
            e.after_fold_pct(),
            e.after_plain.0,
            e.after_plain.1,
            e.after_plain_pct()
        );
        println!(
            "STRICT definition (banner carrying a hash)      : {}/{} ({:.2} %)\n\
             CONTROL under the same definition               : {}/{} ({:.2} %)",
            e.after_fold_strict.0,
            e.after_fold_strict.1,
            ExecErrors::pct_of(e.after_fold_strict),
            e.after_plain_strict.0,
            e.after_plain_strict.1,
            ExecErrors::pct_of(e.after_plain_strict)
        );
        return;
    }
    if args.iter().any(|a| a == "--says") {
        if tokenade_bench::transcripts::canary_complaints(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the complaint canary no longer comes out right");
            std::process::exit(1);
        }
        let c = tokenade_bench::transcripts::complaint_rates(&root);
        println!(
            "the agent complains of missing material after a FOLDED result : {}/{} ({:.2} %)\n\
             CONTROL, after an intact result                               : {}/{} ({:.2} %)",
            c.folded.0,
            c.folded.1,
            c.folded_pct(),
            c.plain.0,
            c.plain.1,
            c.plain_pct()
        );
        for (project, line) in c.folded_samples.iter().take(30) {
            println!("  AFTER A FOLD  ({project})  {line}");
        }
        return;
    }
    if args.iter().any(|a| a == "--outcomes") {
        if tokenade_bench::transcripts::canary_outcomes(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the outcome canary no longer comes out right");
            std::process::exit(1);
        }
        let o = tokenade_bench::transcripts::recovery_outcomes(&root);
        let empty = o.not_found + o.zero_match + o.refused;
        let total = o.served + empty;
        println!(
            "served {} · hash not found {} · « 0 matching line » {} · refused {}\n\
             → {empty} recovery(ies) out of {total} come back empty-handed\n\
             not found: {} WITHIN the TTL (the store should have had them) · {} after · \
             {} fixture(s) from our sources · {} redirected store(s) · {} undecidable",
            o.served,
            o.not_found,
            o.zero_match,
            o.refused,
            o.not_found_inside_ttl,
            o.not_found_after_ttl,
            o.not_found_fixture,
            o.not_found_redirected_store,
            o.not_found_undatable
        );
        for (project, h, hours) in &o.inside_ttl_samples {
            println!("  WITHIN THE TTL  {h}  recovered {hours} h after its banner  ({project})");
        }
        return;
    }
    if args.iter().any(|a| a == "--greps") {
        if tokenade_bench::transcripts::canary_greps(false) != 0 {
            eprintln!(
                "INSTRUMENT NOT VALIDATED — the --grep join canary no longer comes out right"
            );
            std::process::exit(1);
        }
        let g = tokenade_bench::transcripts::grep_outcomes(&root);
        println!(
            "the fold had REMOVED it: {} · it had KEPT it: {}\n\
             « 0 matching line »: {} true negative(s) · {} FALSE negative(s)\n\
             undecidable: {} pattern(s) too short · {} unknown view(s)",
            g.removed, g.kept, g.zero_true, g.zero_false, g.too_short, g.no_view
        );
        for (fam, pat, line) in &g.kept_patterns {
            println!("  KEPT  {fam:<20} --grep {pat}\n         view: {line}");
        }
        for (pat, hash) in &g.false_negatives {
            println!("  FALSE NEGATIVE  {hash}  --grep {pat}");
        }
        let mut by_fam: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for (fam, pat, hash) in &g.removed_patterns {
            by_fam.entry(fam).or_default().push(pat);
            if let Some(out) = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1))
            {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(out)
                    .expect("--out");
                let _ = writeln!(f, "{hash}\t{fam}\t{pat}");
            }
        }
        let mut fams: Vec<_> = by_fam.into_iter().collect();
        fams.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        for (fam, pats) in fams {
            println!(
                "  REMOVED {fam:<20} ×{} : {}",
                pats.len(),
                pats.iter().take(8).copied().collect::<Vec<_>>().join(" · ")
            );
        }
        return;
    }
    if args.iter().any(|a| a == "--resume") {
        // The resume masks (`elided … on cold resume`): how many, and how many
        // did the agent go back for via `expand-ref` after the resume.
        let mut masks = std::collections::BTreeSet::new();
        let mut asked = std::collections::BTreeSet::new();
        tokenade_bench::transcripts::for_each_block(
            &root,
            |_, _, _, b| match b {
                tokenade_bench::transcripts::Block::Received { text, .. } => {
                    if tokenade_bench::transcripts::is_resume_mask(&text) {
                        if let Some(h) = text.split("expand-ref ").nth(1) {
                            masks.insert(
                                h.chars()
                                    .take_while(|c| c.is_ascii_hexdigit())
                                    .collect::<String>(),
                            );
                        }
                    }
                }
                tokenade_bench::transcripts::Block::Bash { cmd, .. } => {
                    for h in cmd.split("expand-ref ").skip(1) {
                        asked.insert(
                            h.chars()
                                .take_while(|c| c.is_ascii_hexdigit())
                                .collect::<String>(),
                        );
                    }
                }
            },
            || {},
        );
        let back = masks.iter().filter(|h| asked.contains(*h)).count();
        println!(
            "resume masks: {} · recalled by expand-ref: {back}",
            masks.len()
        );
        return;
    }
    if args.iter().any(|a| a == "--reruns") {
        if tokenade_bench::transcripts::canary_reruns(false) != 0 {
            eprintln!("INSTRUMENT NOT VALIDATED — the control canary no longer comes out right");
            std::process::exit(1);
        }
        let r = tokenade_bench::transcripts::rerun_rates(&root);
        println!(
            "commands whose result carried a fold: {}/{} rerun within 3 events ({:.2} %)",
            r.folded.0,
            r.folded.1,
            r.folded_pct()
        );
        println!(
            "CONTROL, all the others              : {}/{} ({:.2} %)",
            r.plain.0,
            r.plain.1,
            r.plain_pct()
        );
        for c in &r.folded_reruns {
            println!(
                "  FOLDED THEN RERUN  {}",
                c.chars().take(160).collect::<String>().replace('\n', "⏎")
            );
        }
        return;
    }
    let m = mine(&root);
    let mut rows: Vec<(&String, &tokenade_bench::transcripts::FamilyTally)> =
        m.by_family.iter().collect();
    rows.sort_by(|a, b| {
        (b.1.recovered, b.1.folds)
            .cmp(&(a.1.recovered, a.1.folds))
            .then(a.0.cmp(b.0))
    });
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(10).max(8);
    println!(
        "{:<w$} {:>8} {:>10} {:>7} {:>8}",
        "family",
        "folds",
        "recovered",
        "rate",
        "calls",
        w = w
    );
    for (fam, t) in &rows {
        let pct = if t.folds == 0 {
            0.0
        } else {
            100.0 * t.recovered as f64 / t.folds as f64
        };
        println!(
            "{:<w$} {:>8} {:>10} {:>6.1}% {:>8}",
            fam,
            t.folds,
            t.recovered,
            pct,
            t.calls,
            w = w
        );
    }
    println!(
        "\nTOTAL {} transcript(s) · {} fold(s) announced · {} recovered ({:.1} %) · \
         {} recovery(ies) from a mention · {} orphan(s)",
        m.files,
        m.folds(),
        m.recovered(),
        100.0 * m.rate(),
        m.recoveries_of_a_mention,
        m.orphan_recoveries
    );
    println!(
        "Escape hatches without a join: {} `tokenade raw` · {} TOKENADE_HOOK_DISABLED=1\n\
         Excluded from the denominator: {} mention(s) of `expand-ref <hash>` outside a banner",
        m.raw_calls, m.hook_disabled, m.mentions_outside_banner
    );
    if let Some(i) = args.iter().position(|a| a == "--out") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("fold_recovery --out <file>");
            std::process::exit(2);
        };
        let mut body = String::new();
        for (proj, n) in &m.projects {
            body.push_str(&format!("{n:>6}  {proj}\n"));
        }
        if std::fs::write(path, &body).is_err() {
            eprintln!("cannot write: {path}");
            std::process::exit(1);
        }
        println!("per-project detail written to {path}");
    }
}
