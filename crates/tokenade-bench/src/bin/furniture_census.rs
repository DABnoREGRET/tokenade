//! **What share of what we deliver is FURNITURE we wrote ourselves?**
//!
//! This repo's ratchets count what SURVIVES from the source. None of them
//! counts what the product ADDS: banners, recovery footers, notes,
//! section previews, elision markers, references. It had already been shown
//! that one of these markers could be the heaviest form of the residue — it
//! was shown on the COMMANDS route, and nobody had asked the question on
//! the web route.
//!
//! First survey, 2026-09-14, on the 27 real pages from the transcripts:
//! **36,245 characters of furniture out of 215,869 delivered, i.e. 16.8 %**, of which
//! 23,642 for the section previews alone (×318) and 5,810 for twelve
//! `[stashed: …]` lines — 484 characters each.
//!
//! Measured through the REAL binary, not the library: what counts is what
//! the agent receives, after both layers.
//!
//! ## What "furniture" means, and what the first version wrongly counted
//!
//! The first survey gave 15.0 % by billing the WHOLE section preview —
//! `(16 lines · "- Random session colors …")`. Read on a changelog page, where
//! the heading is a version number and the preview the release note, this
//! count announced **84 % furniture on a page that delivers its content in
//! its previews**. The quote is from the SOURCE; only the frame is ours.
//!
//! A shape can therefore declare what source it CONTAINS (`inner`): those
//! characters are deducted. Without this rule, the instrument measures the
//! content it claims to tell apart from furniture.
//!
//! `furniture_census <directory|file…> [--prompt "q"]`
//! `TOKENADE_BENCH_EXE=target/release/tokenade` picks the binary.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// The shapes that WE write. Each is a literal opening and
/// closing pattern: one more regex would be one more dependency, and these
/// markers are all bracketed — which is even what makes them recoverable.
/// The shapes that WE write: name, opening, closing, and — when the
/// mark FRAMES some source — the pair that delimits that source, to
/// deduct. `("", "")` means "all of it is ours".
const SHAPES: &[(&str, &str, &str, (&str, &str))] = &[
    // The quote is from the source; `(N lines · "` and `")` are ours.
    ("section preview", " lines · \"", "\")", ("\"", "\"")),
    ("[stashed: …]", "[stashed:", "]", ("", "")),
    ("[full … stashed: …]", "[full ", "]", ("", "")),
    ("[disclose:…]", "[disclose:", "]", ("", "")),
    ("banner [tokenade:…]", "[tokenade:", "]", ("", "")),
    ("note [tokenade] …", "[tokenade]", "\n", ("", "")),
    // The alt text comes from the page.
    ("[image: …]", "[image:", "]", ("[image:", "]")),
    ("elision", "… [", "] …", ("", "")),
    ("nudge 💡", "💡", "\n", ("", "")),
];

fn spans(text: &str, open: &str, close: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find(open) {
        let pos = i + off;
        let Some(rel) = text[pos + open.len()..].find(close) else {
            break;
        };
        let e = pos + open.len() + rel + close.len();
        // The section preview opens with its line count; the mark
        // starts at the parenthesis that carries it. Widening is done on the reported
        // START, never on the cursor used to look for the closing.
        let s = if open.starts_with(" lines") {
            text[..pos].rfind('(').unwrap_or(pos)
        } else {
            pos
        };
        out.push((s, e));
        i = e;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prompt = args
        .iter()
        .position(|a| a == "--prompt")
        .and_then(|i| args.get(i + 1).cloned());
    // The common seam of all instruments: it refuses an unknown
    // flag — otherwise a typo silently measures something other than
    // what was asked — and refuses to run stale.
    tokenade_bench::reject_unknown_args(&["--prompt"], &args);
    let exe =
        std::env::var("TOKENADE_BENCH_EXE").unwrap_or_else(|_| "target/release/tokenade".into());
    let mut targets: Vec<PathBuf> = Vec::new();
    for a in args.iter().filter(|a| !a.starts_with("--")) {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(&p) else {
                continue;
            };
            targets.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
        } else if p.is_file() {
            targets.push(p);
        }
    }
    targets.sort();
    if targets.is_empty() {
        eprintln!("usage: furniture_census <directory|file…> [--prompt \"q\"]");
        return;
    }

    // **FRESH state, otherwise we measure the replay memory.** First survey done
    // without it: the second pass returned 100,137 characters against 215,869 on the
    // first, because `web` folds a page the SESSION has already delivered and
    // replaces it with a pointer. The number dropped by half without a
    // single line of code changing. It has been the bench's rule ever since.
    let state = std::env::temp_dir().join(format!("tk-furniture-census-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::create_dir_all(&state);

    let (mut total, mut pages) = (0usize, 0usize);
    let mut bytes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut hits: BTreeMap<&str, usize> = BTreeMap::new();
    let mut worst: Vec<(f64, usize, String)> = Vec::new();
    for t in &targets {
        if std::fs::metadata(t).map(|m| m.len()).unwrap_or(0) < 2000 {
            continue;
        }
        let mut cmd = Command::new(&exe);
        cmd.arg("web")
            .arg(t)
            .env("TOKENADE_HOOK_DISABLED", "1")
            .env("TOKENADE_STASH_DIR", state.join("stash"));
        // The persistent stores are redirected, NOT `TOKENADE_CONFIG_DIR`:
        // it carries the user's configuration, which the binary needs to
        // work.
        // Isolating state does not mean isolating configuration.
        for st in tokenade_core::stores::ISOLATABLE {
            cmd.env(st.env, state.join(st.file));
        }
        if let Some(q) = &prompt {
            cmd.arg("--prompt").arg(q);
        }
        let Ok(out) = cmd.output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if text.is_empty() {
            continue;
        }
        pages += 1;
        total += text.chars().count();
        let mut here = 0usize;
        for (name, open, close, inner) in SHAPES {
            for (s, e) in spans(&text, open, close) {
                let span = &text[s..e];
                // The framed source is not furniture: deduct it, otherwise
                // the instrument bills the content it claims to tell apart.
                let borrowed = if inner.0.is_empty() {
                    0
                } else {
                    spans(span, inner.0, inner.1)
                        .iter()
                        .map(|(a, b)| {
                            span[*a..*b]
                                .chars()
                                .count()
                                .saturating_sub(inner.0.chars().count() + inner.1.chars().count())
                        })
                        .sum::<usize>()
                };
                let n = span.chars().count().saturating_sub(borrowed);
                *bytes.entry(name).or_default() += n;
                *hits.entry(name).or_default() += 1;
                here += n;
            }
        }
        let share = here as f64 / text.chars().count().max(1) as f64;
        worst.push((
            share,
            here,
            t.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ));
    }
    let furn: usize = bytes.values().sum();
    println!(
        "{pages} pages · {total} characters delivered · furniture {furn} ({:.1} %)",
        100.0 * furn as f64 / total.max(1) as f64
    );
    let mut rows: Vec<_> = bytes.iter().collect();
    rows.sort_by_key(|(_, b)| std::cmp::Reverse(**b));
    for (name, b) in rows {
        let n = hits.get(*name).copied().unwrap_or(0);
        println!("  {name:24} {b:7} chars  ×{n:<5} avg {:4}", b / n.max(1));
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    println!("  worst pages:");
    for (share, here, name) in worst.iter().take(3) {
        println!("    {name:40} {:.0} % ({here} chars)", 100.0 * share);
    }
    let _ = std::fs::remove_dir_all(&state);
}
