//! What the READ view ladder delivers on real code, and what it
//! could have delivered.
//!
//! `read_code_strip_lite` is 7th in the ranking: **350,715 bytes still
//! delivered, −43 %, 9,742 per call**, and over the eight detailable calls the
//! median is **15,933 bytes returned out of 18,268 — −12 %**, spread over nine
//! days and several sessions. So it is not a probe: it is a big
//! source file returned almost whole.
//!
//! The ladder of `read_diffs.rs` is: comments (≥ 10 %) → lite (≥ 5 %) →
//! skeleton (size ≥ 6,000). The first two RETURN immediately, so
//! on an 18 KB file whose comments weigh 12 %, the skeleton —
//! which exists precisely for big files — is never reached.
//!
//! It is the same lesson on another ladder: a compactor that "succeeded" by
//! returning almost everything prevents the one that would have done better.
//!
//! `cargo run -q -p tokenade-bench --bin read_view_headroom [--limit N]`

use std::path::{Path, PathBuf};
use tokenade_core::code_strip::{language_for_extension, strip, StripMode};

/// The product's threshold: below it, the skeleton is not offered.
const SKELETON_MIN_SIZE: u64 = 6_000;

/// Beyond this, it is no longer a file an agent READS: it is a minified bundle
/// or generated code, and the skeleton spends seconds on it for an answer
/// nobody uses. The bound is STATED rather than suffered — a first
/// version without it swept 281 competitor repos without ever finishing, and
/// **a probe that returns no verdict lies by omission**.
const MAX_BYTES: u64 = 512 * 1024;

/// **A skeleton is a legitimate view only on code.**
///
/// `language_for_extension` accepts `html` and `css`, rightly: their
/// comments are removed like the others. But "the signatures with line
/// anchors" means nothing for an HTML page, and the skeleton returns
/// **50 tokens out of 110,366** there — it is not a fold, it is destruction.
/// Adding those files up with the code would announce a headroom the
/// product must not take.
fn a_skeleton_is_a_real_view(ext: &str) -> bool {
    !matches!(
        ext.to_ascii_lowercase().as_str(),
        "html" | "htm" | "css" | "scss" | "sass" | "less"
    )
}

struct Row {
    path: String,
    /// Does the skeleton make sense on this file? The two populations are
    /// counted SEPARATELY, never added together.
    code: bool,
    before: usize,
    /// What the ladder delivers TODAY.
    delivered: usize,
    mode: &'static str,
    /// What the skeleton would have delivered.
    skeleton: usize,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if matches!(name.as_ref(), "target" | "node_modules" | ".git") {
                continue;
            }
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn pct(before: usize, after: usize) -> usize {
    before
        .saturating_sub(after)
        .saturating_mul(100)
        .checked_div(before)
        .unwrap_or(0)
}

/// What the product would deliver on this file, through the SHARED decision.
///
/// It used to be reproduced here — a `ladder()` "identical" plus a separate
/// `delivered_under_the_rule()` — and the two had already diverged: the
/// final fallback returned `min(comments, lite)` LABELLED "skeleton", without
/// the 10 % bar the product imposes on it. A bench that models a rule
/// the product does not have prices something other than the product.
fn delivered(text: &str, ext: &str, skeleton: Option<usize>) -> Option<(usize, &'static str)> {
    let lang = language_for_extension(ext)?;
    let before = tokenade_core::tokenizer::estimate(text);
    let comments = tokenade_core::tokenizer::estimate(&strip(text, lang, StripMode::Comments));
    let lite = tokenade_core::tokenizer::estimate(&strip(text, lang, StripMode::Lite));
    match tokenade_core::read_view::choose(before, comments, lite, skeleton) {
        Some((view, tokens)) => Some((tokens, view.as_str())),
        // No view bites: the reader gets the WHOLE file, and it is
        // the heaviest residue there is. Counting it as "delivered = before"
        // is the only honest reading.
        None => Some((before, "none")),
    }
}

/// A slice of the corpus, measured. The skeleton is only offered above
/// `SKELETON_MIN_SIZE`: below it the question does not arise, and measuring it
/// would inflate the denominator.
fn measure_slice(paths: &[PathBuf], root: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    for p in paths {
        let Ok(meta) = std::fs::metadata(p) else {
            continue;
        };
        if meta.len() < SKELETON_MIN_SIZE || meta.len() > MAX_BYTES {
            continue;
        }
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let Ok(body) = std::fs::read_to_string(p) else {
            continue;
        };
        let before = tokenade_core::tokenizer::estimate(&body);
        let sk = tokenade_core::skeleton::skeleton_ext_budget(&body, Some(ext), usize::MAX);
        let sk_tokens = tokenade_core::tokenizer::estimate(&sk);
        // The skeleton is only a view on CODE: on markup it returns
        // 50 tokens out of 110,366, which is destruction and not a fold.
        let offered = a_skeleton_is_a_real_view(ext).then_some(sk_tokens);
        let Some((delivered_tokens, mode)) = delivered(&body, ext, offered) else {
            continue;
        };
        rows.push(Row {
            path: p
                .strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned(),
            code: a_skeleton_is_a_real_view(ext),
            before,
            delivered: delivered_tokens,
            mode,
            skeleton: sk_tokens,
        });
    }
    rows
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--limit", "--delivered"], &args);
    let limit: usize = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);

    // `--delivered` VERIFIES the top of the ranking through the real CLI. Without it this
    // report only knows one layer, and layer 2 stashes big files:
    // MEASURED 2026-09-08, the top three entries deliver 1,386, 1,651 and
    // 1,370 bytes where this report counts them at 270,568, 206,807 and 254,348
    // tokens. It is the third time an instrument of this bench designates as a
    // target what the product already folds best.
    let delivered_check = args.iter().any(|a| a == "--delivered");
    let bench = match delivered_check
        .then(tokenade_bench::Delivery::open)
        .transpose()
    {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();

    // Parallelised by slices, like `compactor_review`: the skeleton
    // (tree-sitter) is the dominant cost and it is independent file by
    // file.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunk = paths.len().div_ceil(threads.max(1)).max(1);
    let root_ref = &root;
    let mut rows: Vec<Row> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|slice| scope.spawn(move || measure_slice(slice, root_ref)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .flatten()
            .collect()
    });
    if rows.is_empty() {
        eprintln!(
            "read_view_headroom: NO source file over {SKELETON_MIN_SIZE} bytes \
             under {}.\n The instrument measured nothing — this is not \"no headroom\".",
            root.display()
        );
        std::process::exit(2);
    }

    let markup = rows.iter().filter(|r| !r.code).count();
    rows.retain(|r| r.code);
    let before: usize = rows.iter().map(|r| r.before).sum();
    let delivered: usize = rows.iter().map(|r| r.delivered).sum();
    println!(
        "{} CODE file(s) ≥ {SKELETON_MIN_SIZE} B · {} tokens\n\
         \x20 delivered under the product's rule   {} (−{} %)\n\
         \x20 ({markup} markup or style file(s) EXCLUDED: a skeleton is not \
         a view there, it destroys)",
        rows.len(),
        thousands(before),
        thousands(delivered),
        pct(before, delivered),
    );

    // **What REMAINS**, ranked by bytes still delivered. This report used to rank
    // the files the skeleton-first rule had ALREADY fixed — it
    // published its own gain, and reading it top to bottom was reading a list
    // of solved cases.
    let mut worst: Vec<&Row> = rows.iter().collect();
    worst.sort_by_key(|r| std::cmp::Reverse(r.delivered));
    println!(
        "\nwhat REMAINS, by tokens still delivered — {}:",
        if delivered_check {
            "VERIFIED on the delivery path below"
        } else {
            "ONE LAYER ONLY, not verified; add `--delivered`"
        }
    );
    for r in worst.iter().take(limit) {
        println!(
            "  {:>7} → {:>7} ({:<8}) · skeleton {:>7}  {}",
            thousands(r.before),
            thousands(r.delivered),
            r.mode,
            thousands(r.skeleton),
            r.path
        );
    }

    if let Some(d) = bench.as_ref() {
        println!("\n{}", tokenade_bench::Stages::verification_header());
        let (mut here, mut real) = (0usize, 0usize);
        for r in worst.iter().take(limit) {
            let full = root.join(&r.path);
            let input = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
            match d.measure(&["read", &full.to_string_lossy()]) {
                Ok(out) => {
                    here += input as usize;
                    real += out;
                    println!(
                        "  input {:>9} B   delivered {:>7} B   {}",
                        thousands(input as usize),
                        thousands(out),
                        r.path
                    );
                }
                Err(note) => {
                    eprintln!("{note}");
                    std::process::exit(2);
                }
            }
        }
        println!(
            "  ── {} B in, {} B REALLY delivered\n",
            thousands(here),
            thousands(real)
        );
    }

    // And by MODE: a view that returns a lot on many files is a
    // class, whereas a heavy file is only an artifact.
    let mut by_mode: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for r in &rows {
        let e = by_mode.entry(r.mode).or_default();
        e.0 += 1;
        e.1 += r.delivered;
    }
    let mut modes: Vec<_> = by_mode.into_iter().collect();
    modes.sort_by_key(|(_, (_, d))| std::cmp::Reverse(*d));
    println!("\nby chosen MODE:");
    for (mode, (n, d)) in modes {
        println!(
            "  {:<8} {:>6} file(s)  {:>12} tokens delivered",
            mode,
            n,
            thousands(d)
        );
    }
}
