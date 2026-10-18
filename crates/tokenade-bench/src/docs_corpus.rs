//! **The DOCUMENTS corpus and what it loses.**
//!
//! Written here rather than in the binary because two callers need it and
//! a copied measurement drifts: the `docs_loss` probe, which REPORTS it, and the
//! `docs_recall_ratchet` ratchet, which refuses to let it drop. Experience showed what
//! an oracle nobody consults costs; an oracle consulted by two
//! different implementations is the next defect.
//!
//! The corpus is `testdata/docs-eval/corpus/`, tracked by git since
//! 2026-07-18: **28 entries in the manifest**, of which **20 documents** (odt, pdf, ods,
//! xlsx, docx, four of each, in four languages) and eight `.csv` that do not go
//! through `docs::extract`. Each entry carries its NEEDLES — the exact values
//! a reader would look for.

use std::path::PathBuf;

/// The manifest formats that are not DOCUMENTS: `.csv` and `.html`
/// do not go through `docs::extract` at all. They are not silently ignored —
/// the report counts and names them, because a corpus part of which is set aside
/// without saying so suggests a coverage it does not have.
pub const NOT_A_DOCUMENT: &[&str] = &["csv", "html"];

pub fn corpus_dir() -> PathBuf {
    crate::Bench::workspace_root().join("testdata/docs-eval/corpus")
}

/// **The SECOND corpus, and why it matters.**
///
/// `testdata/docs-bench/fixture/docs-audit/` holds ten documents and
/// `truth/docs-audit.json` gives each of them an injected `ZK-XXXX` marker. It
/// lay dormant like the first: five test lines cite it, all on the same
/// `docx.docx` or the same `pdf_mid.pdf`.
///
/// It brings what `docs-eval` lacks: **epub, pptx and odp** — three formats that
/// the product advertises and that nothing measured — and **three real PDFs** of 81 KB,
/// 500 KB and 2.6 MB, where `docs-eval` only has documents made by
/// LibreOffice for the occasion.
///
/// Its oracle is THIN, and that is said rather than hidden: **one ASCII needle per
/// document**. Experience showed exactly what such a list lets through — a
/// PDF lost all its text there while scoring 50 %. A marker found proves that
/// the extraction reaches the document, not that it renders it well. The `EMPTY
/// EXTRACTION` note remains the second safeguard.
pub fn audit_dir() -> PathBuf {
    crate::Bench::workspace_root().join("testdata/docs-bench/fixture/docs-audit")
}

/// The text as a needle is searched in it: blanks squashed, table bars
/// removed. Written ONCE, used by the measurement and by the canary.
pub fn normalise(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_whitespace() || c == '|' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        out.push(c);
        last_space = false;
    }
    out.trim().to_string()
}

pub struct Row {
    pub source: &'static str,
    pub file: String,
    pub format: String,
    pub lang: String,
    pub tokens: usize,
    /// The Kind that `auto::detect` claims on the EXTRACTION — it is what sets
    /// the stash file's extension and the hints in the recovery footer.
    pub kind: String,
    pub found: usize,
    pub total: usize,
    pub missing: Vec<String>,
    /// **What the document does NOT say and the extraction served anyway.**
    ///
    /// The corpus could only measure a LOSS. A document under revision carries
    /// its deleted text and its reviewer's name; serving them is not a
    /// loss, it is a FABRICATION, which ranks above it. The
    /// manifest can therefore name `absent` strings, and this column says
    /// which ones came out.
    pub served: Vec<String>,
    pub note: Option<&'static str>,
}

pub fn measure(
    dir: &std::path::Path,
    file: &str,
    format: &str,
    lang: &str,
    needles: &[String],
    absent: &[String],
) -> Row {
    let mut row = Row {
        source: "",
        file: file.to_string(),
        format: format.to_string(),
        lang: lang.to_string(),
        tokens: 0,
        kind: String::new(),
        found: 0,
        total: needles.len(),
        missing: Vec::new(),
        served: Vec::new(),
        note: None,
    };
    if NOT_A_DOCUMENT.contains(&format) {
        row.note = Some("out of scope (does not go through docs::extract)");
        return row;
    }
    let path = dir.join(file);
    let Ok(bytes) = std::fs::read(&path) else {
        row.note = Some("UNREADABLE");
        return row;
    };
    let Some(fmt) = tokenade_core::docs::detect_format(Some(file), &bytes) else {
        row.note = Some("FORMAT NOT RECOGNISED");
        return row;
    };
    let text = tokenade_core::docs::extract(&bytes, fmt);
    row.tokens = tokenade_core::tokenizer::estimate(&text);
    row.kind = format!("{:?}", tokenade_core::auto::detect(&text));
    if text.trim().is_empty() {
        row.note = Some("EMPTY EXTRACTION");
    }
    let hay = normalise(&text);
    for n in needles {
        if hay.contains(&normalise(n)) {
            row.found += 1;
        } else {
            row.missing.push(n.clone());
        }
    }
    for a in absent {
        if hay.contains(&normalise(a)) {
            row.served.push(a.clone());
        }
    }
    row
}

/// **CANARY**: a KNOWN defect, which the instrument must see.
///
/// A minimal RTF — not a zip, so nothing to fabricate — that carries three things:
///
/// * a TWO-word needle that the document splits with a `\par`, so it can
///   only be found if [`normalise`] squashes blanks. It is the defect the probe
///   would commit without it, and it was paid for on the other corpus: a value
///   perfectly delivered, declared lost because the comparison bore on the
///   wrong form;
/// * a word that the format stores in its FONT table, outside the text, so it
///   must be lost. If it is found, the probe reads the raw bytes and not
///   the extraction;
/// * the requirement that both verdicts land TOGETHER: "two found" and "two
///   lost" are both broken instruments, in opposite
///   directions.
pub fn canary(verbose: bool) -> i32 {
    let rtf = br"{\rtf1\ansi{\fonttbl{\f0 ABSENTFONT;}}\f0 CANARI\par AIGUILLE ici.}";
    let fmt = match tokenade_core::docs::detect_format(Some("c.rtf"), rtf) {
        Some(f) => f,
        None => {
            eprintln!("INSTRUMENT NOT VALIDATED — the canary is not even recognised as RTF");
            return 1;
        }
    };
    let text = tokenade_core::docs::extract(rtf, fmt);
    let hay = normalise(&text);
    let present = hay.contains(&normalise("CANARI AIGUILLE"));
    let absent = !hay.contains("ABSENTFONT");
    if present && absent {
        if verbose {
            println!("canary: 1 found, 1 lost — instrument valid");
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"CANARI AIGUILLE found (across a \
         line break), ABSENTFONT lost\", got present={present} absent={absent}\n\
         extract: {hay:?}"
    );
    1
}

/// All the documents of BOTH corpora, measured, sorted by format then by name.
///
/// `only` filters by substring of the file name; `None` takes everything.
pub fn measure_all(only: Option<&str>) -> Vec<Row> {
    let mut rows = Vec::new();
    let keep = |file: &str| only.is_none_or(|t| file.contains(t));

    // --- docs-eval: manifest with several needles per document ---
    let manifest_path = corpus_dir().join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("{}: {e}", manifest_path.display()));
    let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("manifest JSON");
    for e in &entries {
        let file = e["file"].as_str().unwrap_or_default().to_string();
        if !keep(&file) {
            continue;
        }
        let needles: Vec<String> = e["needles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let absent: Vec<String> = e["absent"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut row = measure(
            &corpus_dir(),
            &file,
            e["format"].as_str().unwrap_or("?"),
            e["lang"].as_str().unwrap_or("?"),
            &needles,
            &absent,
        );
        row.source = "docs-eval";
        rows.push(row);
    }

    // --- docs-audit: one marker per document, and the format comes from the NAME ---
    let truth_path =
        crate::Bench::workspace_root().join("testdata/docs-bench/truth/docs-audit.json");
    let raw = std::fs::read_to_string(&truth_path)
        .unwrap_or_else(|e| panic!("{}: {e}", truth_path.display()));
    let truth: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&raw).expect("truth JSON");
    // **What this corpus could not say.** `docs-audit.json` carries only one
    // marker per document, so it only measures a LOSS — and FABRICATION
    // ranks above it. The forbidden strings live in a file
    // next to it rather than in that one: three Python harnesses already read its
    // `{key: marker}` shape, and changing it would break them for nothing.
    let absent: std::collections::BTreeMap<String, Vec<String>> = std::fs::read_to_string(
        crate::Bench::workspace_root().join("testdata/docs-bench/truth/docs-audit-absent.json"),
    )
    .ok()
    .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
    .map(|v| {
        v.as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, x)| {
                (
                    k.clone(),
                    x.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect(),
                )
            })
            .collect()
    })
    .unwrap_or_default();
    for (stem, marker) in &truth {
        // The file name is not in the truth: it is `<key>.<extension>`, and
        // the extension is read from disk rather than guessed — `pdf_small` does not have
        // `pdf_small` as its extension.
        let Some(file) = std::fs::read_dir(audit_dir()).ok().and_then(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .find(|n| n.rsplit_once('.').is_some_and(|(s, _)| s == stem))
        }) else {
            continue;
        };
        if !keep(&file) {
            continue;
        }
        let ext = file
            .rsplit_once('.')
            .map(|(_, e)| e)
            .unwrap_or("?")
            .to_string();
        let mut row = measure(
            &audit_dir(),
            &file,
            &ext,
            "?",
            std::slice::from_ref(marker),
            absent.get(stem).map(Vec::as_slice).unwrap_or_default(),
        );
        row.source = "docs-audit";
        rows.push(row);
    }

    rows.sort_by(|a, b| (&a.format, &a.file).cmp(&(&b.format, &b.file)));
    rows
}

/// **The question a document must be able to answer.**
///
/// `testdata/docs-bench/truth/questions.json` gives, for each of the ten
/// `docs-audit` documents, a natural-language question and the exact token
/// that answers it. It is the THIRD oracle of this axis that nothing read, and the
/// closest to real usage: the product documentation calls
/// `tokenade read <file> --prompt "…"` "the CHEAPEST option in tokens spent"
/// — and it says why, a single round-trip where N questions asked separately
/// would cost N, each re-paying the context of the previous one. Nothing
/// measured whether the answer comes back.
pub struct Answer {
    pub file: String,
    pub question: String,
    pub token: String,
    /// Is the token even IN the extraction? Without this column, a
    /// document whose extraction loses the token would blame the retriever for a
    /// loss made one stage lower — exactly what happened before.
    pub in_extract: bool,
    /// Is the token in the returned PASSAGE?
    pub answered: bool,
    /// What the passage costs, in characters. A correct and huge answer is
    /// not the same answer as a correct and short one.
    pub chars: usize,
    /// What the whole extraction weighs. Without this denominator, "answered" does not
    /// distinguish a retriever that FINDS from a retriever that returns everything —
    /// and a mutant replacing the search with `Some(the whole document)`
    /// survived the first version of this ratchet, which proved it.
    pub extract_chars: usize,
}

pub fn ask_all(only: Option<&str>) -> Vec<Answer> {
    let truth_path =
        crate::Bench::workspace_root().join("testdata/docs-bench/truth/questions.json");
    let raw = std::fs::read_to_string(&truth_path)
        .unwrap_or_else(|e| panic!("{}: {e}", truth_path.display()));
    let truth: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&raw).expect("questions JSON");
    let mut out = Vec::new();
    for (stem, v) in &truth {
        let Some(file) = std::fs::read_dir(audit_dir()).ok().and_then(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .find(|n| n.rsplit_once('.').is_some_and(|(s, _)| s == stem))
        }) else {
            continue;
        };
        if only.is_some_and(|t| !file.contains(t)) {
            continue;
        }
        let question = v["question"].as_str().unwrap_or_default().to_string();
        let token = v["token"].as_str().unwrap_or_default().to_string();
        let Ok(bytes) = std::fs::read(audit_dir().join(&file)) else {
            continue;
        };
        let Some(fmt) = tokenade_core::docs::detect_format(Some(&file), &bytes) else {
            continue;
        };
        let text = tokenade_core::docs::extract(&bytes, fmt);
        let in_extract = normalise(&text).contains(&normalise(&token));
        // `answer_over_blob` is "the ONE retrieval core behind every ask a
        // stash/ref entry point" — so we measure the product, not a variant
        // we would have tuned ourselves.
        let passage = tokenade_core::rag::answer_over_blob(&text, &question);
        let chars = passage.as_deref().map(|p| p.chars().count()).unwrap_or(0);
        let answered = passage
            .as_deref()
            .is_some_and(|p| normalise(p).contains(&normalise(&token)));
        out.push(Answer {
            file,
            question,
            token,
            in_extract,
            answered,
            chars,
            extract_chars: text.chars().count(),
        });
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// A divergence: a word one spelling delivers and another does not.
pub struct PairRow {
    pub stem: String,
    pub word: String,
    /// The files of the family that deliver the word.
    pub served: Vec<String>,
    /// Those that do not deliver it.
    pub absent: Vec<String>,
}

/// The word as it is compared: without the punctuation the RENDERING adds around it
/// (`**bold**`, `- item`, `« quoted »`). A word that differs only by its markdown
/// decoration is not a reader divergence.
fn compare_word(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// **What a family compares.** `ar.odt` is a report, `ar.ods` a
/// spreadsheet: two different documents bearing the same name, and the manifest already
/// says so by giving them different needles. Comparing them would measure the
/// writing of the corpus, not a reader. The class comes from the FORMAT, not from a list
/// of names encountered; an unknown format compares to nothing.
fn document_class(format: &str) -> Option<&'static str> {
    Some(match format {
        "odt" | "fodt" | "docx" | "rtf" | "fb2" | "epub" => "text",
        "ods" | "fods" | "xlsx" | "xls" | "xlsb" => "spreadsheet",
        "odp" | "fodp" | "pptx" => "slides",
        "odg" | "fodg" => "drawing",
        _ => return None,
    })
}

/// The corpus families: one entry per (name without extension, class), carrying
/// the files that belong to it. READS no document — the ratchet needs the
/// denominator without paying for the extraction.
fn families() -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let raw = std::fs::read_to_string(corpus_dir().join("manifest.json")).expect("manifest");
    let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("manifest JSON");
    for e in &entries {
        let file = e["file"].as_str().unwrap_or_default().to_string();
        let format = e["format"].as_str().unwrap_or("?");
        // The PDF is not another spelling of the same document, it is a
        // layout: headers, hyphenation, column order. Comparing it there would measure the
        // pagination, not a reader.
        if NOT_A_DOCUMENT.contains(&format) || format == "pdf" {
            continue;
        }
        let (Some(stem), Some(class)) = (
            file.rsplit_once('.').map(|(s, _)| s.to_string()),
            document_class(format),
        ) else {
            continue;
        };
        out.entry(format!("{stem} [{class}]"))
            .or_default()
            .push(file);
    }
    out
}

/// The number of families that [`pairs`] actually compares — the ratchet's
/// DENOMINATOR, without which "zero divergence" could mean "nothing to
/// compare".
pub fn comparable_families() -> usize {
    families().values().filter(|m| m.len() >= 2).count()
}

/// **The same content, written differently: what one spelling returns and not the other.**
///
/// The corpus holds sixteen families where a document exists in several formats — the
/// same text out of the same spreadsheet (`merge.fods` / `merge.ods` / `merge.xlsx`),
/// the same note in three spellings. The manifest's needles are already
/// comparable from one spelling to another, and that is what `measure_all` measures.
/// They only cover what someone THOUGHT of declaring.
///
/// This probe asks the question without a list: a word that one of the spellings
/// delivers and another does not is, by construction, a reader that loses
/// (or that invents). It is exactly the question that found the reviewer's
/// name served with the prose and the empty cell that swallowed its
/// neighbour — asked by hand, one file at a time.
///
/// The PDF is EXCLUDED and that is said: it is not another spelling of the same
/// document, it is a layout, with its headers, its hyphenation and its column
/// order. Comparing it there would measure the pagination, not a reader.
pub fn pairs(only: Option<&str>) -> Vec<PairRow> {
    use std::collections::BTreeSet;
    let mut out = Vec::new();
    for (stem, files) in families() {
        if files.len() < 2 || only.is_some_and(|t| !stem.contains(t)) {
            continue;
        }
        let sets: Vec<(String, BTreeSet<String>)> = files
            .iter()
            .filter_map(|file| {
                let bytes = std::fs::read(corpus_dir().join(file)).ok()?;
                let fmt = tokenade_core::docs::detect_format(Some(file), &bytes)?;
                let words = normalise(&tokenade_core::docs::extract(&bytes, fmt))
                    .split_whitespace()
                    .map(compare_word)
                    .filter(|w| !w.is_empty())
                    .collect();
                Some((file.clone(), words))
            })
            .collect();
        if sets.len() < 2 {
            continue;
        }
        let union: BTreeSet<&String> = sets.iter().flat_map(|(_, s)| s.iter()).collect();
        for word in union {
            let served: Vec<String> = sets
                .iter()
                .filter(|(_, s)| s.contains(word))
                .map(|(f, _)| f.clone())
                .collect();
            if served.len() == sets.len() {
                continue;
            }
            out.push(PairRow {
                stem: stem.clone(),
                word: word.clone(),
                absent: sets
                    .iter()
                    .filter(|(_, s)| !s.contains(word))
                    .map(|(f, _)| f.clone())
                    .collect(),
                served,
            });
        }
    }
    out
}
