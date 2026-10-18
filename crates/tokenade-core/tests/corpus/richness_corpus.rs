//! Every compactor must keep as much of its input's rare information as it
//! keeps today. A ratchet, not a threshold.
//!
//! A single global floor cannot work here and the corpus says so: 77% of
//! fixtures round-trip at 1.00, but `tasklist` legitimately scores 0.29 and
//! `sc_query` 0.10 — dropping per-process noise IS their job. Any floor loose
//! enough to admit those two admits a compactor that has started eating the
//! payload. So the gate is per-fixture and relative: whatever a compactor
//! retains today is its floor tomorrow.
//!
//! What this catches that nothing else does: a cap tightened from 30 to 8, a
//! column dropped, a fold widened. Byte-level snapshots
//! (`snapshot_compactors.rs`) cover a dozen hand-written cases, not this
//! corpus, and would fail on every intentional change too — a diff nobody
//! reads. Token-savings numbers move the WRONG way on this class of bug: a
//! compactor that deletes the answer looks like an improvement.
//!
//! Refresh after an intentional change:
//!     TOKENADE_RICHNESS_BLESS=1 cargo test -p tokenade-core --test corpus richness_corpus
//! then read the diff on `tests/richness_baseline.tsv` before committing it —
//! a drop there is the whole point of the file.

// `corpus` is mounted by `main.rs` for the whole binary: a single copy
// compiled instead of one per subject.
use crate::corpus;
use corpus::Route;

use std::collections::BTreeMap;
use std::path::PathBuf;

/// How far a fixture may drift below its recorded value before failing.
/// Not zero: the estimator is a heuristic and atom counts move by a hair when
/// unrelated formatting changes. 0.02 is far below any real information loss
/// (the perturbation that motivated this gate moved `tasklist` by 0.13).
const TOLERANCE: f64 = 0.02;

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/richness_baseline.tsv")
}

/// Fixtures are named after the command that produced them. Longest known
/// prefix wins, so `tasklist_svc_full` routes to `tasklist_svc`, not `tasklist`.
fn prog_for_stem(stem: &str) -> Option<String> {
    corpus::program_of(stem)
}

/// Per fixture: what it KEEPS (the richness) and BY WHOM it was folded
/// (the compactor's name). Both, because the first alone is blind to the
/// second — see the routing floor below.
fn measure() -> BTreeMap<String, (f64, String)> {
    let mut out = BTreeMap::new();
    for f in corpus::fixtures() {
        let (stem, body) = (f.key.clone(), f.body.clone());
        if f.route == Route::Format {
            // Routed by content + extension, like a `tokenade read`.
            let (kind, compacted, _sav) =
                tokenade_core::auto::compact_with_path_hint(&body, &f.file_name());
            out.insert(
                stem,
                (
                    tokenade_core::richness::richness(&body, &compacted),
                    format!("auto:{}", kind.as_str()),
                ),
            );
            continue;
        }
        // `knows_program` answers "is there a NAMED compactor", and that is not
        // the question this gate asks. `filter_command` also folds through the
        // generic and table passes, which claim plenty that no name does — so
        // selecting by `knows_program` left the heaviest compaction in the
        // corpus unwatched. MEASURED: `windows/driverquery` folds 28 388 bytes
        // to 2 671 with a richness of **0.1454**, and was in no baseline row at
        // all, so tightening it further would have gone unnoticed. The command
        // is whatever the fixture is named after; if no longer prefix is known,
        // the first token is what a user would have typed.
        let prog = prog_for_stem(&stem).unwrap_or_else(|| {
            stem.split('/')
                .next_back()
                .unwrap_or(&stem)
                .split('_')
                .next()
                .unwrap_or("")
                .to_string()
        });
        if prog.is_empty() {
            continue;
        }
        let r = tokenade_core::filter::filter_command(&[prog], &body);
        out.insert(
            stem,
            (
                tokenade_core::richness::richness(&body, &r.output),
                r.filter,
            ),
        );
    }
    out
}

/// `key\trichness[\tcompactor]`. The third column is optional on
/// READ: a baseline written before it existed stays readable, and its rows
/// then impose no routing — rather than turning the whole suite red
/// over a missing column.
fn load_baseline() -> BTreeMap<String, (f64, Option<String>)> {
    let Ok(text) = std::fs::read_to_string(baseline_path()) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut cols = line.split('\t');
        let (Some(k), Some(v)) = (cols.next(), cols.next()) else {
            continue;
        };
        let Ok(v) = v.trim().parse::<f64>() else {
            continue;
        };
        let filter = cols
            .next()
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty());
        out.insert(k.to_string(), (v, filter));
    }
    out
}

fn write_baseline(m: &BTreeMap<String, (f64, String)>) {
    let body: String = m
        .iter()
        .map(|(k, (v, filter))| format!("{k}\t{v:.4}\t{filter}\n"))
        .collect();
    std::fs::write(baseline_path(), body).expect("write baseline");
}

#[test]
fn no_compactor_retains_less_rare_information_than_it_did() {
    let current = measure();
    // No fixture is silently skipped. The corpus used to choose what to measure
    // with `knows_program`, which answers "is there a NAMED compactor" — but
    // `filter_command` also folds through the generic and table passes, so a
    // fixture could be compacted hard and measured by nothing. MEASURED before
    // this assertion existed: 57 fixtures routed nowhere, and among them
    // `windows/driverquery` folded 28 388 bytes to 2 671 at a richness of
    // 0.1454 — the heaviest information loss in the corpus, with no floor
    // under it. A gate that quietly measures a subset of its corpus reports
    // the health of the part it happened to look at.
    // Removing this assertion cannot be caught on its own: with the fallback above
    // in place every fixture routes, so there is nothing for it to catch. It is a
    // regression guard whose subject is exactly the selector above — narrow that
    // back to `prog_for_stem` alone and this line is what fires, naming 57 fixtures
    // sitting in the corpus measured by nothing.
    let fixture_count = corpus::fixtures().len();
    assert_eq!(
        current.len(),
        fixture_count,
        "{} of {fixture_count} fixtures were not measured at all — a fixture that \
         routes nowhere is not a pass, it is a blind spot",
        fixture_count - current.len()
    );
    // The ROUTING floor — see below, after the baseline is read.
    // Two hand-picked ceilings used to live here (`windows/driverquery_full`
    // and `windows/schtasks_full`, pinned under 0.30) because the ratchet
    // only fails on a DROP: a compactor that stops firing leaves
    // more information, so richness GOES UP and the row passes silently,
    // turned into decoration. Pinning two fixtures answered the instance; the
    // baseline now records BY WHOM each fixture was folded, which
    // answers the class for all 354.
    // LOCALE PARITY. For every capture that has an `_en` twin, the same
    // compactor must fire AND fold as much.
    //
    // `cmd/mod.rs` explicitly asks that parsers hold up in at least
    // seven languages, and nothing checked it. MEASURED 2026-09-07:
    // `lscpu` on this machine under `fr_FR.UTF-8` folded **4 062 bytes to
    // 4 061** while the same output in English fell to 1 442 (−63 %) — the
    // `Drapeaux :` and `Vulnérabilité …` lines start with neither of the two
    // English prefixes the compactor looked for. An earlier fix had corrected
    // DETECTION and left COMPACTION English-only: a defect proven on one
    // consumer and not on its sibling, once again.
    //
    // The tolerance is wide on purpose: the two outputs are not the same
    // bytes (a translation is longer or shorter), so only a POLICY gap
    // should turn red, not translation noise.
    const LOCALE_SLACK: f64 = 0.15;
    let mut lopsided = Vec::new();
    for (name, (rich, filter)) in &current {
        let Some(base) = name.strip_suffix("_en") else {
            continue;
        };
        let Some((rich_other, filter_other)) = current.get(base) else {
            continue;
        };
        if filter != filter_other {
            lopsided.push(format!(
                "{base}: the locale changes the compactor ({filter_other} vs {filter})"
            ));
        } else if (rich - rich_other).abs() > LOCALE_SLACK {
            lopsided.push(format!(
                "{base}: folds {rich_other:.4} in one locale and {rich:.4} in the other"
            ));
        }
    }
    assert!(
        lopsided.is_empty(),
        "{} command(s) are not compacted the same way depending on the machine's \
         language — English is one case, not the rule:\n  {}",
        lopsided.len(),
        lopsided.join("\n  ")
    );

    // What the corpus KEEPS, stated on every run. A ratchet that only reports
    // "no drop" does not say what it watches over: MEASURED 2026-09-07,
    // 121 of the 354 fixtures reach a compactor written for a command,
    // and they exercise only 29 of them — the product ships 935 (672 linux,
    // 129 macos, 134 windows). The rest falls to `generic` (147) or is not
    // folded at all (`passthrough`, 45). That is the number to raise, and
    // until now nobody could compute it.
    let specific: Vec<&String> = current
        .values()
        .map(|(_, f)| f)
        .filter(|f| tokenade_core::filter::filter_name_is_command_specific(f))
        .collect();
    let distinct: std::collections::BTreeSet<&&String> = specific.iter().collect();
    println!(
        "richness corpus: {} fixtures, {} on a command compactor, {} distinct compactors",
        current.len(),
        specific.len(),
        distinct.len()
    );
    assert!(
        current.len() > 100,
        "only {} fixtures routed — the corpus or the stem→prog mapping moved",
        current.len()
    );
    if std::env::var("TOKENADE_RICHNESS_BLESS").is_ok() {
        write_baseline(&current);
        eprintln!("richness baseline rewritten ({} fixtures)", current.len());
        return;
    }
    let baseline = load_baseline();
    assert!(
        !baseline.is_empty(),
        "no baseline at {} — create it with TOKENADE_RICHNESS_BLESS=1",
        baseline_path().display()
    );

    let mut regressions = Vec::new();
    let mut unrecorded = Vec::new();
    let mut rerouted = Vec::new();
    let mut weakened = Vec::new();
    for (name, (now, filter)) in &current {
        match baseline.get(name) {
            Some((was, _)) if *now < was - TOLERANCE => {
                regressions.push(format!("{name}: {was:.4} → {now:.4} ({:+.4})", now - was));
            }
            Some((was, _)) if *now > was + TOLERANCE => {
                // THE OTHER SIDE. Richness that GOES UP means we fold
                // LESS: the information is not lost, the savings are, and
                // the user receives more bytes. For a long time the ratchet only
                // failed downwards, and it was assumed that the ROUTING floor
                // was enough to cover the other direction — two mutations
                // refuted that: neutralising the recognition of `lscpu` lines
                // leaves the compactor with the same name while it folds
                // nothing anymore.
                weakened.push(format!("{name}: {was:.4} → {now:.4} ({:+.4})", now - was));
            }
            Some((_, recorded)) => {
                // The ROUTING floor. A fixture whose compactor changes
                // name no longer keeps the same thing: either it was renamed
                // (and the baseline must say so), or it no longer fires and the
                // fixture fell back to `generic` — in which case richness GOES UP
                // and the ratchet does not turn red. Rows of a baseline written
                // before this column impose nothing.
                if let Some(was_filter) = recorded {
                    if was_filter != filter {
                        rerouted.push(format!("{name}: {was_filter} → {filter}"));
                    }
                }
            }
            // A new fixture with no recorded value is not a pass. Silence here
            // is how a corpus grows a blind spot.
            None => unrecorded.push(name.clone()),
        }
    }
    assert!(
        regressions.is_empty(),
        "compaction now discards rare information it used to keep — token savings \
         will have gone UP on these, which is exactly why this gate exists:\n  {}\n\
         If the change is intended, re-bless with TOKENADE_RICHNESS_BLESS=1 and \
         justify the drop in the commit.",
        regressions.join("\n  ")
    );
    assert!(
        unrecorded.is_empty(),
        "fixtures with no recorded richness: {unrecorded:?}\n\
         Re-bless with TOKENADE_RICHNESS_BLESS=1."
    );
    assert!(
        weakened.is_empty(),
        "these fixtures are folded LESS than before — the information is not \
         lost, the savings are, and nothing else says so:\n  {}\n\
         If intended, re-bless with TOKENADE_RICHNESS_BLESS=1 and \
         justify it in the commit.",
        weakened.join("\n  ")
    );
    assert!(
        rerouted.is_empty(),
        "{} fixture(s) no longer go through the same compactor — a fixture that \
         falls back to `generic` keeps MORE information, so the richness floor \
         says nothing and the row becomes decoration:\n  {}\n\
         If the change is intended (rename, revised route), re-bless with \
         TOKENADE_RICHNESS_BLESS=1 and justify it in the commit.",
        rerouted.len(),
        rerouted.join("\n  ")
    );

    // And the MIRROR: a baseline row that is no longer measured at all.
    //
    // The loop above walks `current` and queries `baseline`: it
    // sees a NEW fixture with no value, never a value whose fixture
    // disappeared. So the corpus can shrink silently — a deleted
    // fixture, a directory dropped from the enumerator, a broken route — and
    // the `current.len() > 100` floor is far too loose to tell:
    // losing the 138 locale fixtures would leave 216.
    //
    // This is the mirror of the hole closed earlier (measured but no baseline
    // row), left open on the other side, and it is the same lesson: a gate that
    // can only fail on a VALUE is blind to the loss of a ROW.
    let vanished: Vec<&String> = baseline
        .keys()
        .filter(|k| !current.contains_key(*k))
        .collect();
    assert!(
        vanished.is_empty(),
        "{} baseline fixture(s) are no longer measured — a fixture that stops \
         being measured is indistinguishable from one that never existed, and the \
         corpus shrinks without anything turning red:\n  {vanished:?}\n\
         If the removal is intended, re-bless with TOKENADE_RICHNESS_BLESS=1 \
         and justify it in the commit.",
        vanished.len()
    );
}

/// **An output that keeps NOTHING does not get blessed.**
///
/// The ratchet above compares to the past: whatever it blessed once becomes the
/// norm. Measured 2026-10-02: three fixtures in the repo carried a defect and
/// the ratchet defended it — `macos/nslookup_apple` without its address,
/// `windows/tasklist` without the biggest processes, and `windows/wmic_os`
/// blessed at **0.0000**: every `wmic` line ends with `\r\r\n` and all
/// of them were erased. Zero retention is not savings, it is an empty
/// output; this floor depends on no baseline and cannot be re-blessed.
/// An exception is declared here, with its reason.
#[test]
fn no_fixture_keeps_nothing_of_its_rare_information() {
    const FLOOR: f64 = 0.01;
    const DECLARED: &[(&str, &str)] = &[];
    let current = measure();
    assert!(
        current.len() > 100,
        "only {} fixtures routed",
        current.len()
    );
    let empty: Vec<String> = current
        .iter()
        .filter(|(name, (r, _))| *r < FLOOR && !DECLARED.iter().any(|(d, _)| d == name))
        .map(|(name, (r, by))| format!("{name}: {r:.4} ({by})"))
        .collect();
    assert!(
        empty.is_empty(),
        "these fixtures deliver almost none of their rare information — an emptied output, not a folded one:\n  {}",
        empty.join("\n  ")
    );
}

/// How many compactors stop firing when their words are taken away?
///
/// `#[ignore]`d: it is the measurement that sizes the work, not yet a
/// gate — the list it prints is what must be made structural.
#[test]
#[ignore = "measurement: how many compactors stop firing without the words"]
fn how_many_compactors_depend_on_the_language() {
    let mut lost: Vec<String> = Vec::new();
    let mut kept = 0usize;
    for f in corpus::fixtures() {
        if f.route != Route::Command {
            continue;
        }
        let Some(prog) = prog_for_stem(&f.key) else {
            continue;
        };
        let before = tokenade_core::filter::filter_command(std::slice::from_ref(&prog), &f.body);
        if !tokenade_core::filter::filter_name_is_command_specific(&before.filter) {
            continue;
        }
        let after = tokenade_core::filter::filter_command(&[prog], &corpus::mangle_labels(&f.body));
        if after.filter == before.filter {
            kept += 1;
        } else {
            lost.push(format!("{} : {} → {}", f.key, before.filter, after.filter));
        }
    }
    lost.sort();
    println!(
        "dedicated compactors that SURVIVE label scrambling: {kept}; that fall: {}",
        lost.len()
    );
    for l in &lost {
        println!("  {l}");
    }
}
