//! What is LEFT in the markdown a web page compacts to, priced in tokens.
//!
//! The web path is where most of what this product saves comes from, and also
//! where most of what it still delivers goes. Before writing another pass, price
//! the candidates: characters lie (indentation is a fifth of the characters and
//! a thirtieth of the tokens), so every number here is `tokenizer::estimate`.
//!
//! `#[ignore]`d — a measurement to decide from, not a gate.
//!
//! `cargo test -p tokenade-core --test web_headroom -- --ignored --nocapture`

// `corpus` is mounted by `main.rs` for the whole binary: a single compiled
// copy instead of one per subject.
use crate::corpus;

use tokenade_core::tokenizer::estimate;

fn pages() -> Vec<(String, String)> {
    corpus::pages()
}

fn link_re() -> regex::Regex {
    regex::Regex::new(r"\[([^\]]*)\]\(([^)]*)\)").expect("static")
}

/// Candidate reductions. Each rewrites the COMPACTED markdown; the delta is what
/// the transformation would be worth if it joined the pipeline.
fn candidates() -> Vec<(&'static str, fn(&str) -> String)> {
    vec![
        ("drop_all_link_targets", |s: &str| {
            link_re().replace_all(s, "$1").into_owned()
        }),
        ("drop_anchor_only_targets", |s: &str| {
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    if url.starts_with('#') {
                        text.to_string()
                    } else {
                        c[0].to_string()
                    }
                })
                .into_owned()
        }),
        // The LOSSLESS half of `drop_anchor_only_targets`: an anchor that only
        // repeats the link text (`[Configuration](#configuration)`) teaches
        // nobody anything, whereas an anchor that NAMES another section
        // (`[voir ici](#limites-connues)`) says where it leads.
        ("drop_anchor_only_when_it_repeats_the_text", |s: &str| {
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    match url.strip_prefix('#') {
                        Some(frag) if tokenade_core::web::says_the_same_words(text, frag) => {
                            text.to_string()
                        }
                        _ => c[0].to_string(),
                    }
                })
                .into_owned()
        }),
        // Collapse blanks, but NOT in a code block: a blank line there is
        // content. `drop_blank_line_runs` below does not make the
        // difference, so its number is a ceiling, not a proposal.
        ("collapse_blank_runs_outside_fences", |s: &str| {
            let mut out = String::with_capacity(s.len());
            let mut in_fence = false;
            let mut blanks = 0usize;
            for line in s.lines() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                }
                if !in_fence && line.trim().is_empty() {
                    blanks += 1;
                    if blanks > 1 {
                        continue;
                    }
                } else {
                    blanks = 0;
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        }),
        // A REPEATED anchor: the second time, the target has already been
        // stated in the same document. That is what `compact_citations`
        // already does for URLs (`text⟨N⟩` + a References section), and its
        // pattern does not cover `#anchor`s.
        ("fold_repeated_anchor_targets", |s: &str| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    if !url.starts_with('#') {
                        return c[0].to_string();
                    }
                    if seen.insert(url.to_string()) {
                        c[0].to_string()
                    } else {
                        text.to_string()
                    }
                })
                .into_owned()
        }),
        // What `compact_citations` LEAVES: its pattern only takes
        // `https?://…` and `/…`, so a relative link (`../guide/x`,
        // `guide/x`) never enters the references and is repeated in full at
        // every occurrence.
        ("fold_repeated_relative_targets", |s: &str| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    let relative = !url.starts_with('#')
                        && !url.starts_with('/')
                        && !url.starts_with("http://")
                        && !url.starts_with("https://");
                    if !relative {
                        return c[0].to_string();
                    }
                    if seen.insert(url.to_string()) {
                        c[0].to_string()
                    } else {
                        text.to_string()
                    }
                })
                .into_owned()
        }),
        ("strip_url_query", |s: &str| {
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    match url.split_once('?') {
                        Some((head, _)) => format!("[{text}]({head})"),
                        None => c[0].to_string(),
                    }
                })
                .into_owned()
        }),
        ("fold_repeated_targets", |s: &str| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    if seen.insert(url.to_string()) {
                        c[0].to_string()
                    } else {
                        text.to_string()
                    }
                })
                .into_owned()
        }),
        ("drop_target_when_it_repeats_the_text", |s: &str| {
            link_re()
                .replace_all(s, |c: &regex::Captures| {
                    let (text, url) = (&c[1], &c[2]);
                    let norm = |x: &str| {
                        x.chars()
                            .filter(|c| c.is_alphanumeric())
                            .flat_map(char::to_lowercase)
                            .collect::<String>()
                    };
                    let last = url.rsplit('/').next().unwrap_or("");
                    if !last.is_empty() && norm(last) == norm(text) {
                        text.to_string()
                    } else {
                        c[0].to_string()
                    }
                })
                .into_owned()
        }),
        // The LOSSLESS variant of `drop_duplicate_lines`: a run of identical
        // lines becomes one line and its count, a mechanism the product
        // already has for logs (`[×3 identical lines]`). Priced next to the
        // lossy version to know what caution costs — and if it costs almost
        // nothing, it is the one we write.
        ("fold_identical_runs_losslessly", |s: &str| {
            let mut out = String::with_capacity(s.len());
            let mut lines = s.lines().peekable();
            while let Some(l) = lines.next() {
                let mut n = 1usize;
                while lines.peek() == Some(&l) && !l.trim().is_empty() {
                    lines.next();
                    n += 1;
                }
                if n > 1 {
                    out.push_str(&format!("{l}  (×{n})\n"));
                } else {
                    out.push_str(l);
                    out.push('\n');
                }
            }
            out
        }),
        // And the same on NON-adjacent identical lines, still lossless: the
        // second occurrence points back to the first by its number.
        ("fold_repeated_lines_by_reference", |s: &str| {
            let mut first: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            let mut out = String::with_capacity(s.len());
            for (i, l) in s.lines().enumerate() {
                let t = l.trim();
                if t.len() < 24 {
                    out.push_str(l);
                    out.push('\n');
                    continue;
                }
                match first.get(t) {
                    Some(n) => out.push_str(&format!("(= ligne {n})\n")),
                    None => {
                        first.insert(t, i + 1);
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
            out
        }),
        ("drop_duplicate_lines", |s: &str| {
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            s.lines()
                .filter(|l| l.trim().len() < 12 || seen.insert(l.trim()))
                .map(|l| format!("{l}\n"))
                .collect()
        }),
        ("drop_blank_line_runs", |s: &str| {
            let mut out = String::with_capacity(s.len());
            let mut blank = false;
            for l in s.lines() {
                if l.trim().is_empty() {
                    if blank {
                        continue;
                    }
                    blank = true;
                } else {
                    blank = false;
                }
                out.push_str(l);
                out.push('\n');
            }
            out
        }),
        ("drop_all_indent", |s: &str| {
            s.lines().map(|l| format!("{}\n", l.trim_start())).collect()
        }),
        ("drop_image_targets", |s: &str| {
            regex::Regex::new(r"!\[([^\]]*)\]\(([^)]*)\)")
                .expect("static")
                .replace_all(s, "[image: $1]")
                .into_owned()
        }),
    ]
}

#[test]
#[ignore = "measurement: what is LEFT in a compacted page"]
fn what_is_left_in_a_compacted_web_page() {
    let pages = pages();
    assert!(pages.len() > 20, "only {} pages", pages.len());
    let cands = candidates();
    let mut totals = vec![0usize; cands.len()];
    let mut base_total = 0usize;
    let mut raw_total = 0usize;

    for (name, html) in &pages {
        let md = tokenade_core::web::html_to_markdown(html);
        let base = estimate(&md);
        raw_total += estimate(html);
        base_total += base;
        for (k, (_cname, f)) in cands.iter().enumerate() {
            totals[k] += estimate(&f(&md));
        }
        let _ = name;
    }

    println!(
        "corpus: {raw_total} raw -> {base_total} delivered ({:.1}% saved today)",
        100.0 * (raw_total - base_total) as f64 / raw_total as f64
    );
    for (k, (cname, _)) in cands.iter().enumerate() {
        // A candidate may COST — that is a result, not an impossible case. An
        // unsigned subtraction made this measurement PANIC the day a rule
        // entered the pipeline: the matching candidate became a no-op that
        // added a line ending, hence one token more than the baseline. An
        // instrument must be able to say "worse".
        let delta = base_total as i64 - totals[k] as i64;
        println!(
            "  {cname:<36} {:>8}  ({:+.1}% further)",
            totals[k],
            100.0 * delta as f64 / base_total as f64
        );
    }
}

/// The web path must keep COMPRESSING, not just keep being faithful.
///
/// Every other floor in this corpus guards the opposite direction — headings
/// kept, minimum markdown length, no heading swallowed into a link label. All
/// of them PASS when the pipeline degrades toward passthrough, because keeping
/// more text reads as better fidelity. Nothing failed if `html_to_markdown`
/// stopped stripping boilerplate; the only signal would have been the bill.
///
/// That matters here more than anywhere else: `html-to-markdown` is the second
/// largest earner in the production ledger (45.7 M tokens net over 1 076 calls,
/// delivering about a tenth of what it is handed).
///
/// Floors sit well below the measured corpus (2026-08-03: 95.8 % corpus-wide,
/// worst page 60.6 %) so a genuinely less-compressible page can join the corpus
/// without a false alarm. They are regression detectors, not targets.
#[test]
fn web_compaction_keeps_earning_its_keep() {
    const PER_PAGE_FLOOR: f64 = 45.0;
    const CORPUS_FLOOR: f64 = 90.0;

    let pages = pages();
    assert!(pages.len() > 20, "corpus shrank to {} pages", pages.len());

    let (mut raw_total, mut md_total) = (0usize, 0usize);
    let mut weak: Vec<String> = Vec::new();
    for (name, html) in &pages {
        let md = tokenade_core::web::html_to_markdown(html);
        let (raw, out) = (estimate(html), estimate(&md));
        raw_total += raw;
        md_total += out;
        if raw == 0 {
            continue;
        }
        let saved = 100.0 * (raw - out) as f64 / raw as f64;
        if saved < PER_PAGE_FLOOR {
            weak.push(format!("{name} {saved:.1}%"));
        }
    }

    assert!(
        weak.is_empty(),
        "page(s) below the {PER_PAGE_FLOOR}% floor: {}",
        weak.join(", ")
    );
    let corpus = 100.0 * (raw_total - md_total) as f64 / raw_total as f64;
    assert!(
        corpus >= CORPUS_FLOOR,
        "corpus compaction fell to {corpus:.1}%, floor is {CORPUS_FLOOR}% \
         ({raw_total} -> {md_total} tokens) — the pipeline is drifting toward \
         passthrough while every fidelity test still passes"
    );
    println!("web corpus: {raw_total} -> {md_total} tokens ({corpus:.1}% saved)");
}

/// Probe: the compression ratio of the web path, per page and corpus-wide.
#[test]
#[ignore = "probe: the compression ratio of the web route"]
fn probe_web_compression_ratio() {
    let pages = pages();
    let (mut rt, mut mt) = (0usize, 0usize);
    let mut worst: Vec<(f64, String)> = Vec::new();
    for (name, html) in &pages {
        let md = tokenade_core::web::html_to_markdown(html);
        let (r, m) = (estimate(html), estimate(&md));
        rt += r;
        mt += m;
        let saved = if r > 0 {
            100.0 * (r - m) as f64 / r as f64
        } else {
            0.0
        };
        worst.push((saved, name.clone()));
    }
    worst.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!(
        "corpus: {rt} -> {mt} tokens ({:.1}% saved) over {} pages",
        100.0 * (rt - mt) as f64 / rt as f64,
        pages.len()
    );
    for (s, n) in worst.iter().take(6) {
        println!("  worst {s:5.1}%  {n}");
    }
}

/// Writes the delivered markdown of each corpus page into a directory, to be
/// able to DIFF two states of the pipeline.
///
/// It was rewritten by hand three times before being put here. A question
/// asked three times belongs to the instrument.
///
/// `WEB_DUMP=/path cargo test -p tokenade-core --test web_headroom -- --ignored dump_delivered_markdown --nocapture`
#[test]
#[ignore = "dump gated on `$WEB_DUMP`, to DIFF two states of the pipeline"]
fn dump_delivered_markdown() {
    // `--ignored` runs ALL the binary's ignored tests, including when one only
    // came to read the headroom numbers. A diagnostic without a target must
    // not fail that reading: it SAYS so and stops.
    let Ok(dir) = std::env::var("WEB_DUMP") else {
        println!("dump_delivered_markdown: nothing to do without WEB_DUMP=<directory>");
        return;
    };
    let out = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&out).expect("output directory");
    let mut n = 0;
    for (name, html) in pages() {
        let md = tokenade_core::web::html_to_markdown(&html);
        let stem = name.trim_end_matches(".html");
        std::fs::write(out.join(format!("{stem}.md")), md).expect("write");
        n += 1;
    }
    println!("wrote {n} page(s) into {}", out.display());
}
