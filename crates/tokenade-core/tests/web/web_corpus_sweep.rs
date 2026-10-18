//! Real-data regression + speed sweep over the FULL 147-page corpus (not the
//! 40-page Battery-4 subsample). Hunts panics, empty distills, token blowups and
//! slow pages across the whole thing. `#[ignore]` — run on demand:
//!
//!     cargo test -p tokenade-core --test web_corpus_sweep -- --ignored --nocapture

use std::time::Instant;

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../experiments/cf-clearance/corpus")
}

#[test]
#[ignore = "demande le corpus local `experiments/cf-clearance/corpus`"]
fn distill_the_whole_corpus_without_panic_or_blowup() {
    let dir = corpus_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("corpus dir {dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "no corpus pages found");

    let mut rows: Vec<(String, usize, usize, u128)> = Vec::new(); // name, in_tok, out_tok, ms
    let mut empties = Vec::new();
    let mut blowups = Vec::new();
    let mut slow = Vec::new();
    let total_start = Instant::now();

    for p in &entries {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(p).unwrap();
        let (body, _enc) = tokenade_core::text_encoding::decode(&bytes)
            .unwrap_or_else(|| (String::from_utf8_lossy(&bytes).into_owned(), "utf-8"));
        let in_tok = tokenade_core::tokenizer::estimate(&body);
        let t = Instant::now();
        // The real shipped entry point.
        let d = tokenade_core::web_pipeline::distill_page(
            &body,
            "https://sweep.test/page",
            Some("text/html"),
        );
        let ms = t.elapsed().as_millis();
        let out_tok = tokenade_core::tokenizer::estimate(&d.md);

        // A page whose RAW visible text was substantial but distilled to nearly
        // nothing is a suspicious drop (some are legitimately JS shells).
        if body.len() > 4000 && d.md.trim().chars().count() < 30 {
            empties.push(name.clone());
        }
        // The pipeline promises tokens DOWN. A distill that produced MORE tokens
        // than the raw input is a real regression (barring tiny pages).
        if out_tok > in_tok && in_tok > 200 {
            blowups.push((name.clone(), in_tok, out_tok));
        }
        if ms > 750 {
            slow.push((name.clone(), ms));
        }
        rows.push((name, in_tok, out_tok, ms));
    }

    let total_ms = total_start.elapsed().as_millis();
    let mut outs: Vec<usize> = rows.iter().map(|r| r.2).collect();
    outs.sort_unstable();
    let p = |q: f64| outs[((outs.len() as f64 * q) as usize).min(outs.len() - 1)];
    let mut times: Vec<u128> = rows.iter().map(|r| r.3).collect();
    times.sort_unstable();
    let tp = |q: f64| times[((times.len() as f64 * q) as usize).min(times.len() - 1)];

    println!(
        "\n=== corpus sweep: {} pages in {total_ms} ms ===",
        rows.len()
    );
    println!(
        "out tokens  p50={} p95={} max={}",
        p(0.50),
        p(0.95),
        outs.last().copied().unwrap_or(0)
    );
    println!(
        "distill ms  p50={} p95={} max={}",
        tp(0.50),
        tp(0.95),
        times.last().copied().unwrap_or(0)
    );
    println!(
        "suspicious-empty: {} · token-blowups: {} · slow(>750ms): {}",
        empties.len(),
        blowups.len(),
        slow.len()
    );
    for (n, i, o) in &blowups {
        println!("  BLOWUP {n}: {i} -> {o} tokens");
    }
    for (n, ms) in &slow {
        println!("  SLOW {n}: {ms} ms");
    }
    for n in empties.iter().take(20) {
        println!("  EMPTY {n}");
    }

    // Hard floor: the pipeline must never turn a real page into MORE tokens than
    // it started with. That defeats the product's entire purpose.
    assert!(
        blowups.is_empty(),
        "{} page(s) distilled to MORE tokens than the raw input — a token regression",
        blowups.len()
    );
}

/// Dump the distilled markdown of every corpus page into `$SWEEP_DUMP`, so a
/// speed change can be proven output-neutral by diffing two dumps (the F1/F2/F4
/// speed guardrail).
///
///     SWEEP_DUMP=/tmp/after cargo test -p tokenade-core \
///         --test web_corpus_sweep dump_distilled -- --ignored --nocapture
#[test]
#[ignore = "dump gated on `$SWEEP_DUMP`"]
fn dump_distilled_corpus() {
    let out = match std::env::var("SWEEP_DUMP") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => return,
    };
    std::fs::create_dir_all(&out).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(corpus_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    entries.sort();
    for p in &entries {
        let bytes = std::fs::read(p).unwrap();
        let (body, _enc) = tokenade_core::text_encoding::decode(&bytes)
            .unwrap_or_else(|| (String::from_utf8_lossy(&bytes).into_owned(), "utf-8"));
        let d = tokenade_core::web_pipeline::distill_page(
            &body,
            "https://sweep.test/page",
            Some("text/html"),
        );
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(out.join(format!("{name}.md")), d.md).unwrap();
    }
    println!("dumped {} pages to {out:?}", entries.len());
}
