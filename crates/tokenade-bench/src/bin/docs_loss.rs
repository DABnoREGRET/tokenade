//! **What extracting a DOCUMENT loses of its content, needle by needle.**
//!
//! ```text
//! docs_loss                  # both corpora
//! docs_loss <file>           # a single document
//! docs_loss --missing        # list only the documents that lose something
//! docs_loss --canary         # validates the instrument
//! ```
//!
//! The MEASUREMENT lives in [`tokenade_bench::docs_corpus`], not here: the
//! `docs_recall_ratchet` ratchet needs it too, and two implementations of the same
//! oracle drift apart. This binary only makes it readable.
//!
//! **The corpus is not new, the instrument is.** See [`tokenade_bench::docs_corpus`] for what it
//! contains, why nothing walked it, and the two ways the neighbouring
//! Python harness measures wrong.

use tokenade_bench::docs_corpus::{canary, measure_all, NOT_A_DOCUMENT};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(
        &[
            "--canary",
            "--missing",
            "--kind",
            "--ask",
            "--pairs",
            "--text",
        ],
        &args,
    );
    if args.iter().any(|a| a == "--canary") {
        std::process::exit(canary(true));
    }
    if canary(false) != 0 {
        eprintln!("INSTRUMENT NOT VALIDATED — the canary no longer reproduces the known defect");
        std::process::exit(1);
    }
    let only_missing = args.iter().any(|a| a == "--missing");
    // **What the SUMMARISER thinks it is reading.** The Kind decides the extension of the
    // stash file and the advice in the recovery footer: an article served as
    // `api.rest.chain.txt` sends the agent off parsing REST.
    // **The cheapest path the product calls, measured.** See
    // `docs_corpus::ask_all`.
    if args.iter().any(|a| a == "--ask") {
        let rows = tokenade_bench::docs_corpus::ask_all(None);
        let w = rows.iter().map(|r| r.file.len()).max().unwrap_or(10);
        let (mut ok, mut reachable) = (0usize, 0usize);
        for r in &rows {
            reachable += usize::from(r.in_extract);
            ok += usize::from(r.answered);
            let verdict = match (r.in_extract, r.answered) {
                (false, _) => "ABSENT FROM THE EXTRACTION",
                (true, true) => "answered",
                (true, false) => "LOST BY THE RETRIEVER",
            };
            println!(
                "{:<w$} {:>6}/{:<6} chars  {verdict:<24} {} · « {} »",
                r.file,
                r.chars,
                r.extract_chars,
                r.token,
                r.question,
                w = w
            );
        }
        println!(
            "\nTOTAL {}/{} answered · {}/{} reachable (the token is in the extraction)",
            ok,
            rows.len(),
            reachable,
            rows.len()
        );
        return;
    }
    // **The delivered TEXT, as is.** The `tokenade read` path summarises, so it
    // cannot be used to compare two extractions line by line — which is what is
    // needed to READ a change instead of only measuring it.
    if args.iter().any(|a| a == "--text") {
        let Some(target) = args.iter().find(|a| !a.starts_with('-')) else {
            eprintln!("docs_loss --text <file>");
            std::process::exit(2);
        };
        let bytes = std::fs::read(target).unwrap_or_else(|e| panic!("{target}: {e}"));
        let Some(fmt) = tokenade_core::docs::detect_format(Some(target), &bytes) else {
            eprintln!("unrecognised format: {target}");
            std::process::exit(1);
        };
        print!("{}", tokenade_core::docs::extract(&bytes, fmt));
        return;
    }

    // **The same content, written differently.** See `docs_corpus::pairs`.
    if args.iter().any(|a| a == "--pairs") {
        let target = args.iter().find(|a| !a.starts_with('-')).cloned();
        let rows = tokenade_bench::docs_corpus::pairs(target.as_deref());
        let mut stem = String::new();
        for r in &rows {
            if r.stem != stem {
                println!("\n{}", r.stem);
                stem = r.stem.clone();
            }
            println!(
                "  {:<28} served by {:<40} ABSENT from {}",
                r.word,
                r.served.join(" "),
                r.absent.join(" ")
            );
        }
        println!("\n{} divergence(s)", rows.len());
        return;
    }
    if args.iter().any(|a| a == "--kind") {
        let rows = measure_all(None);
        let w = rows.iter().map(|r| r.file.len()).max().unwrap_or(10);
        let mut tally: std::collections::BTreeMap<&str, usize> = Default::default();
        for r in &rows {
            if NOT_A_DOCUMENT.contains(&r.format.as_str()) {
                continue;
            }
            *tally.entry(r.kind.as_str()).or_default() += 1;
            println!(
                "{:<w$} {:<6} {:>7} {}",
                r.file,
                r.format,
                r.tokens,
                r.kind,
                w = w
            );
        }
        println!();
        for (k, n) in &tally {
            println!("  {k:<16} {n}");
        }
        return;
    }
    let target = args.iter().find(|a| !a.starts_with('-')).cloned();
    let rows = measure_all(target.as_deref());

    let w = rows.iter().map(|r| r.file.len()).max().unwrap_or(10);
    println!(
        "{:<w$} {:<6} {:<3} {:>7} {:>10}  lost",
        "file",
        "fmt",
        "lang",
        "tokens",
        "needles",
        w = w
    );
    let (mut scoped, mut kept, mut wanted, mut tokens) = (0usize, 0usize, 0usize, 0usize);
    for r in &rows {
        if NOT_A_DOCUMENT.contains(&r.format.as_str()) {
            if !only_missing {
                println!(
                    "{:<w$} {:<6} {:<3} {:>7} {:>10}  {}",
                    r.file,
                    r.format,
                    r.lang,
                    "-",
                    "-",
                    r.note.unwrap_or(""),
                    w = w
                );
            }
            continue;
        }
        scoped += 1;
        kept += r.found;
        wanted += r.total;
        tokens += r.tokens;
        if only_missing && r.missing.is_empty() && r.served.is_empty() && r.note.is_none() {
            continue;
        }
        let note = r.note.unwrap_or("");
        println!(
            "{:<w$} {:<6} {:<3} {:>7} {:>4}/{:<5}  {note}{}{}",
            r.file,
            r.format,
            r.lang,
            r.tokens,
            r.found,
            r.total,
            if note.is_empty() { "" } else { " · " },
            if r.served.is_empty() {
                r.missing.join(" · ")
            } else {
                format!(
                    "{} · WRONGLY SERVED: {}",
                    r.missing.join(" · "),
                    r.served.join(" · ")
                )
            },
            w = w
        );
    }
    let pct = if wanted == 0 {
        0.0
    } else {
        100.0 * kept as f64 / wanted as f64
    };
    println!(
        "\nTOTAL {scoped} document(s) in scope · {kept}/{wanted} needles kept \
         ({pct:.1} %) · {tokens} tokens delivered · {} out of scope",
        rows.len() - scoped
    );
}
