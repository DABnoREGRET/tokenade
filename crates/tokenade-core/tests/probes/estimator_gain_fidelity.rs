//! Does the heuristic estimator report the gain the model actually sees?
//!
//! Every savings number the user reads is `1 - after/before` computed with
//! `tokenizer::estimate`. An absolute CPT error is harmless there — it cancels.
//! What does NOT cancel is an *asymmetric* error: when `before` is structured
//! (an a11y tree, a build log, HTML) and `after` is prose-shaped markdown, the
//! two sides land in different chars-per-token tiers and the ratio drifts.
//!
//! Measured once by hand on the a11y corpus: 46 % announced, 29,6 % real. This
//! test is that measurement, automated, against cl100k_base.
//!
//! Run with: `cargo test -p tokenade-core --features bpe --test
//! estimator_gain_fidelity -- --nocapture`
#![cfg(feature = "bpe")]

use tokenade_core::tokenizer::{estimate, estimate_exact};

/// Gain as the user is told it is, and as cl100k says it is.
struct Pair {
    name: String,
    before: String,
    after: String,
}

impl Pair {
    fn reported(&self) -> f64 {
        pct(estimate(&self.before), estimate(&self.after))
    }
    fn real(&self) -> f64 {
        pct(estimate_exact(&self.before), estimate_exact(&self.after))
    }
}

fn pct(before: usize, after: usize) -> f64 {
    if before == 0 {
        return 0.0;
    }
    100.0 * (before as f64 - after as f64) / before as f64
}

fn fixture_dir(sub: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(sub)
}

fn read_all(sub: &str, suffix: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(fixture_dir(sub))
        .unwrap_or_else(|e| panic!("fixtures/{sub}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(suffix))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (name, std::fs::read_to_string(&p).expect("read fixture"))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Where the calibration aids read and write. Override with `TOKENADE_CALIB_DIR`.
pub fn scratch_dir() -> std::path::PathBuf {
    std::env::var_os("TOKENADE_CALIB_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("tokenade-calib"))
}

fn pairs() -> Vec<Pair> {
    let mut pairs = Vec::new();

    for (name, snap) in read_all("a11y", ".a11y.txt") {
        let (out, _) = tokenade_core::web::compact_a11y_tree_with_savings(&snap);
        pairs.push(Pair {
            name: format!("a11y/{name}"),
            before: snap,
            after: out,
        });
    }

    for (name, raw) in read_all("build", ".log") {
        let (out, _) = tokenade_core::cmd::linux::npm_run_script::compact_with_savings(&raw);
        pairs.push(Pair {
            name: format!("build/{name}"),
            before: raw,
            after: out,
        });
    }

    assert!(!pairs.is_empty(), "no corpus to calibrate against");
    pairs
}

/// The headline: per pair, how far the reported gain is from the real one.
#[test]
fn the_reported_gain_matches_what_cl100k_sees() {
    let mut worst: Option<(String, f64)> = None;
    let (mut tb_h, mut ta_h, mut tb_e, mut ta_e) = (0usize, 0usize, 0usize, 0usize);

    println!(
        "{:<44} {:>9} {:>9} {:>8}",
        "fixture", "reported", "cl100k", "drift"
    );
    for p in pairs() {
        let (r, e) = (p.reported(), p.real());
        let drift = r - e;
        println!("{:<44} {r:>8.1}% {e:>8.1}% {drift:>+7.1}pt", p.name);
        tb_h += estimate(&p.before);
        ta_h += estimate(&p.after);
        tb_e += estimate_exact(&p.before);
        ta_e += estimate_exact(&p.after);
        if worst.as_ref().is_none_or(|(_, d)| drift.abs() > d.abs()) {
            worst = Some((p.name.clone(), drift));
        }
    }

    let (corpus_r, corpus_e) = (pct(tb_h, ta_h), pct(tb_e, ta_e));
    println!(
        "corpus: reported {corpus_r:.1}% vs cl100k {corpus_e:.1}% ({:+.1}pt)",
        corpus_r - corpus_e
    );

    let (name, drift) = worst.expect("at least one pair");
    assert!(
        drift.abs() <= 6.0,
        "{name}: reported gain drifts {drift:+.1}pt from cl100k (budget ±6pt)"
    );
    assert!(
        (corpus_r - corpus_e).abs() <= 4.0,
        "corpus gain drifts {:+.1}pt from cl100k (budget ±4pt)",
        corpus_r - corpus_e
    );
}

/// The absolute counts, per side. A gain can be right for the wrong reason —
/// two errors that happen to cancel. This is the diagnostic that tells which
/// side is off and by how much, so a CPT retune has something to aim at.
#[test]
fn each_side_of_the_ratio_is_counted_within_budget() {
    let mut bad = Vec::new();
    println!(
        "{:<44} {:>7} {:>10} {:>9} {:>8}",
        "side", "chars", "heuristic", "cl100k", "err"
    );
    for p in pairs() {
        for (side, text) in [("before", &p.before), ("after", &p.after)] {
            let (h, e) = (estimate(text), estimate_exact(text));
            let err = 100.0 * (h as f64 - e as f64) / e as f64;
            println!(
                "{:<44} {:>7} {h:>10} {e:>9} {err:>+7.1}%",
                format!("{}:{side}", p.name),
                text.chars().count()
            );
            if err.abs() > 20.0 {
                bad.push(format!("{}:{side} {err:+.1}%", p.name));
            }
        }
    }
    assert!(bad.is_empty(), "sides counted outside ±20 %: {bad:?}");
}

/// The shapes a flat chars-per-token divisor cannot price, on REAL captures
/// rather than an invented sample: whitespace-padded tabular output (cl100k has
/// multi-space tokens, so a gutter is nearly free — the flat model read
/// `Get-PSDrive` at +194 %), opaque value dumps with no word boundaries to merge
/// (`nvram`, read at −52 %), and deeply indented trees (every a11y snapshot).
///
/// Getting two of those wrong in OPPOSITE directions is what let a compactor
/// that strips padding claim savings it had not made.
#[test]
fn no_real_capture_is_mispriced_by_more_than_a_third() {
    let mut samples: Vec<(String, String)> = Vec::new();
    for sub in ["windows", "macos", "a11y", "build"] {
        for suffix in [".txt", ".log"] {
            for (name, body) in read_all(sub, suffix) {
                if body.len() >= 400 {
                    samples.push((format!("{sub}/{name}"), body));
                }
            }
        }
    }
    assert!(samples.len() > 40, "only {} captures found", samples.len());

    let mut bad = Vec::new();
    let mut errs = Vec::new();
    for (name, body) in &samples {
        let (h, e) = (estimate(body), estimate_exact(body));
        let err = 100.0 * (h as f64 - e as f64) / e as f64;
        errs.push(err);
        if err.abs() > 30.0 {
            bad.push(format!("{name} {err:+.1}%"));
        }
    }
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = errs[errs.len() / 2];
    println!("{} captures, median err {median:+.1}%", samples.len());
    // 30 % and not 25 % for exactly one capture: `od -c` prints ONE character
    // per four columns, and a piece model that prices a whitespace run once
    // cannot see that cl100k spends a token on each of those cells. It reads
    // -29 %. Every other capture is inside 25 %.
    assert!(bad.is_empty(), "captures mispriced beyond ±30 %: {bad:?}");
    assert!(
        median.abs() <= 12.0,
        "median err {median:+.1}% (budget ±12 %)"
    );
}

/// Diagnostic dump: writes each side to the scratchpad for offline inspection.
/// `#[ignore]` — a calibration aid, not a gate.
#[test]
#[ignore = "diagnostic dump: writes each side into the scratchpad"]
fn dump_sides() {
    let dir = scratch_dir().join("sides");
    std::fs::create_dir_all(&dir).unwrap();
    for p in pairs() {
        let stem = p.name.replace('/', "_");
        std::fs::write(dir.join(format!("{stem}.after")), &p.after).unwrap();
        std::fs::write(dir.join(format!("{stem}.before")), &p.before).unwrap();
    }
    println!("wrote to {}", dir.display());
}
