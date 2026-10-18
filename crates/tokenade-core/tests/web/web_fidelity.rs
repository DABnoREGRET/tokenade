//! Battery 4 — PAGE FIDELITY (hermetic).
//!
//! Decides D6 (adopt essence's extraction techniques) and D7 (keep our
//! DOM-based negative-class stripping). For every fixture page: what share of
//! the source `<p>` text survives into `distill_page(...).md`, and what share
//! of the non-`<p>` structural text (`td,th,dt,dd,h1-6,caption`) does.
//!
//! Fidelity is measured as MULTISET BIGRAM RECALL: each source element's text
//! (one `<p>`, one `<td>`, ...) is tokenized (Unicode `\w+`, lower-cased) into
//! adjacent WORD-PAIRS (a lone-word element keeps its single word); a
//! frequency bag is built from the distilled markdown's own adjacent pairs;
//! each markdown pair covers at most one source occurrence. Plain unigram
//! (single-word) recall was tried first and read 98%/95% — far above the
//! known baseline — because short structural text (headings, `dt`/`dd`,
//! table cells) is dominated by common short words ("the", "and", a bare
//! number) that coincidentally survive via UNRELATED text elsewhere on the
//! page, crediting fidelity for content that was actually dropped. Requiring
//! adjacent pairs to survive together removes that false-positive path and is
//! what reproduces the known baseline (Stage 1 gate: aggregate `<p>` ~94%,
//! non-`<p>` ~82%). See `battery4_page_fidelity`'s printed aggregate.
//!
//!     cargo test -p tokenade-core --test web_fidelity -- --nocapture
//!
//! To refresh the ratchet after an ACCEPTED change to `distill_page`/`web.rs`:
//!     cargo test -p tokenade-core --test web_fidelity -- --ignored --nocapture regen_fidelity_baseline

// `ab` is mounted by `main.rs` for the whole binary: a single copy
// compiled instead of one per subject.
use crate::ab;

use std::collections::HashMap;
use tokenade_core::web;
use tokenade_core::web_pipeline;

#[derive(serde::Deserialize, Clone)]
struct ManifestEntry {
    file: String,
    class: String,
    // Read by `serde` from the baseline and never consulted here: it is
    // part of the file's SHAPE, and removing it would change what the
    // deserializer accepts.
    #[allow(dead_code)]
    lang: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy)]
struct Baseline {
    p_fidelity: f64,
    nonp_fidelity: f64,
    md_chars: usize,
    /// On HOW MUCH the fidelity was computed. Without this number, `0.057` and
    /// `0.891` read the same although the first is over 35 trigrams and
    /// the second over thousands. `default` because a baseline written before
    /// this column must still parse.
    #[serde(default)]
    p_grams: usize,
    #[serde(default)]
    nonp_grams: usize,
}

/// Below this, a fidelity rate distinguishes nothing.
///
/// MEASURED 2026-09-10 on the 39 pages: the denominators go from **0** —
/// `guardian_sec` and `hn_thread` do not have a single `<p>` trigram, so their
/// "fidelity" is a `0/0` — to several thousand, and the gap in the ranking
/// is between 44 and 69. Under this floor, losing ONE trigram drops the rate
/// by tens of points: the ratchet would cry over noise, and one iteration was
/// lost chasing a `p=0.057` computed on 35 trigrams of a newsletter
/// banner.
///
/// These pages are not left unguarded for all that: the count of delivered
/// characters applies to all of them.
const SIGNIFICANT_GRAMS: usize = 50;

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages")
}

fn baseline_path() -> std::path::PathBuf {
    fixtures_dir().join("fidelity_baseline.json")
}

fn manifest() -> Vec<ManifestEntry> {
    let p = fixtures_dir().join("manifest.json");
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing {p:?}: {e}"));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("manifest.json does not parse: {e}"))
}

fn load_baseline() -> std::collections::BTreeMap<String, Baseline> {
    let p = baseline_path();
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "missing {p:?}: {e}. Generate it once with `cargo test -p tokenade-core \
             --test web_fidelity -- --ignored --nocapture regen_fidelity_baseline`."
        )
    });
    serde_json::from_str(&s)
        .unwrap_or_else(|e| panic!("fidelity_baseline.json does not parse: {e}"))
}

// ---------------------------------------------------------------------------
// text extraction — independent of `web.rs`, on purpose: this measures what
// `web.rs` DID to the source, so it must not share its own tag-stripping code.
// ---------------------------------------------------------------------------

fn strip_script_style(html: &str) -> String {
    // Rust's `regex` has no backreferences (this bites more than
    // once), so script/style each get their own literal alternative rather
    // than a `<(script|style)>...</\1>` pattern.
    let re =
        regex::Regex::new(r"(?is)<script\b[^>]*>.*?</script>|<style\b[^>]*>.*?</style>").unwrap();
    re.replace_all(html, " ").into_owned()
}

/// Removes the microdata the browser paints FOR NOBODY, before
/// measuring fidelity.
///
/// The reference of this battery was "the page minus its scripts and its
/// styles". It therefore counted Stack Exchange's `<div class="d-none" itemprop="name">`,
/// which repeat for robots the author and date that the page
/// already displays. `bag_recall` counts OCCURRENCES: a word written twice
/// in the reference and once in the output reads as a loss, so much so
/// that the ratchet refused the deduplication that fixed precisely this
/// duplicate — 0.375 against 0.417, on a page where NOTHING readable had
/// disappeared.
///
/// The question is asked of `web_tree::is_machine_only_markup`, the same function
/// as the shipped path: two spellings would make the rule and its judge
/// diverge.
fn strip_machine_only_microdata(html: &str) -> String {
    // No backreference in `regex` (the module notes it above): the
    // closing tag is searched by hand, on the captured name.
    let open = regex::Regex::new(r#"(?is)<(\w+)((?:[^>]*\s)?itemprop\s*=[^>]*)>"#).unwrap();
    let attr = |tag: &str, key: &str| -> Option<String> {
        regex::Regex::new(&format!(r#"(?is){key}\s*=\s*"([^"]*)""#))
            .ok()
            .and_then(|r| r.captures(tag).map(|c| c[1].to_string()))
    };
    let hidden_attr = regex::Regex::new(r"(?is)\shidden(\s|=|/|$)").unwrap();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(m) = open.find_at(html, cursor) {
        let c = open.captures(&html[m.start()..]).unwrap();
        let name = c[1].to_string();
        let tag = c[2].to_string();
        let machine = tokenade_core::web_tree::is_machine_only_markup(
            attr(&tag, "class").as_deref(),
            attr(&tag, "style").as_deref(),
            hidden_attr.is_match(&tag),
            true,
        );
        let close = format!("</{name}>");
        let end = html[m.end()..]
            .to_ascii_lowercase()
            .find(&close.to_ascii_lowercase())
            .map(|k| m.end() + k + close.len());
        match (machine, end) {
            (true, Some(end)) => {
                out.push_str(&html[cursor..m.start()]);
                out.push(' ');
                cursor = end;
            }
            _ => {
                out.push_str(&html[cursor..m.end()]);
                cursor = m.end();
            }
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// **A formula is written THREE times in the source and delivered once.**
///
/// Wikipedia puts, for each equation, the presentation MathML (hidden,
/// `display: none`, for screen readers), the `<annotation
/// encoding="application/x-tex">` that carries the TeX, and a fallback `<img>` whose
/// `alt` is the same TeX. A browser paints ONE. `bag_recall` counts
/// occurrences, so a reference that keeps all three reads as a LOSS the
/// rendering that delivers only one: measured on `wikipedia_math`, 0.909 → 0.797 on
/// 9,271 trigrams, for a page where every changed line is a formula.
/// It is exactly the duplicate that [`strip_machine_only_microdata`] handles
/// above, on another page and in another form.
///
/// The formula is therefore reduced to the TeX the page DECLARES, through the function of the
/// shipped path: two spellings would make the rule and its judge diverge.
/// A `<math>` that declares no TeX is left whole — it is then the
/// presentation that the rendering delivers, and the reference must ask for it.
fn fold_math_to_its_tex(html: &str) -> String {
    let open = regex::Regex::new(r"(?is)<math\b").unwrap();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(m) = open.find_at(html, cursor) {
        let Some(k) = html[m.end()..].to_ascii_lowercase().find("</math>") else {
            break;
        };
        let end = m.end() + k + "</math>".len();
        out.push_str(&html[cursor..m.start()]);
        match tokenade_core::web_tree::tex_of_math_fragment(&html[m.start()..end]) {
            Some(tex) => {
                out.push(' ');
                out.push_str(&tex);
                out.push(' ');
            }
            None => out.push_str(&html[m.start()..end]),
        }
        cursor = end;
    }
    out.push_str(&html[cursor..]);
    out
}

fn strip_tags(s: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]+>").unwrap();
    re.replace_all(s, " ").into_owned()
}

/// Decoded, tag-stripped text of every `<tag>...</tag>` block for the given
/// tag names, ONE STRING PER ELEMENT (not concatenated) so n-grams below
/// never spuriously bridge two unrelated elements. One regex per tag name —
/// Rust's `regex` has no backreferences.
fn extract_tag_blocks(cleaned_html: &str, tags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for tag in tags {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}>")).unwrap();
        for cap in re.captures_iter(cleaned_html) {
            let inner = strip_tags(&cap[1]);
            out.push(web::decode_entities(&inner));
        }
    }
    out
}

fn words(s: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\w+").unwrap();
    re.find_iter(s).map(|m| m.as_str().to_lowercase()).collect()
}

/// N-grams per element, falling back to the longest shorter window an
/// element actually has (so a one- or two-word element — a table cell, a
/// short heading — still contributes something instead of being silently
/// excluded). See the module doc for why n-grams, not unigrams.
fn ngrams_per_element(blocks: &[String], n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        let ws = words(b);
        let k = n.min(ws.len());
        if k == 0 {
            continue;
        }
        out.extend(ws.windows(k).map(|w| w.join(" ")));
    }
    out
}

/// N-grams over a continuous text stream (the distilled markdown).
fn ngrams_stream(text: &str, n: usize) -> Vec<String> {
    let ws = words(text);
    if ws.is_empty() {
        return Vec::new();
    }
    // **All sizes up to `n`, not only `n`.**
    //
    // `ngrams_per_element` emits a k-gram when a block carries fewer than `n`
    // words (`k = n.min(ws.len())`) — a two-word `<h2>` gives a bigram.
    // The markdown stream, for its part, carried ONLY trigrams, so a
    // short block could NEVER be found, even copied word for word.
    //
    // Measured: `stackexch_q` carries 19 non-`<p>` trigrams in all, including the
    // `<h2>` "9 Answers 9". Removing the hidden microdata brings it down to
    // "9 Answers", two words — a bigram that the output contains and that the
    // bag could not contain. Fidelity fell from 0.417 to 0.375 for a
    // block the fold had made MORE accurate.
    let mut out = Vec::new();
    for k in 1..=n.min(ws.len()) {
        out.extend(ws.windows(k).map(|w| w.join(" ")));
    }
    out
}

/// Multiset recall: `(survived, total)` where `survived` is the count of
/// `source` word occurrences also found in a frequency bag of `target`, each
/// `target` occurrence covering at most one `source` occurrence.
fn bag_recall(source: &[String], target: &[String]) -> (usize, usize) {
    let mut pool: HashMap<&str, i64> = HashMap::new();
    for w in target {
        *pool.entry(w.as_str()).or_insert(0) += 1;
    }
    let mut survived = 0usize;
    for w in source {
        if let Some(c) = pool.get_mut(w.as_str()) {
            if *c > 0 {
                *c -= 1;
                survived += 1;
            }
        }
    }
    (survived, source.len())
}

fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        100.0
    } else {
        num as f64 / den as f64 * 100.0
    }
}

struct PageResult {
    file: String,
    class: String,
    p_survived: usize,
    p_words: usize,
    nonp_survived: usize,
    nonp_words: usize,
    md: String,
    md_chars: usize,
    raw_visible_chars: usize,
}

const NONP_TAGS: &[&str] = &[
    "td", "th", "dt", "dd", "h1", "h2", "h3", "h4", "h5", "h6", "caption",
];

/// N-gram size for the recall measure — tuned so the aggregate reproduces the
/// known baseline (see the module doc): unigrams read 98.2%/94.8%, bigrams
/// 96.3%/89.1%, trigrams land near the gate.
const FIDELITY_NGRAM: usize = 3;

/// **A fence's language is METADATA, not page text.**
///
/// This rate is a STREAM trigram coverage: inserting a word in the middle
/// of the sequence destroys the trigram that spanned it, and so returns a
/// lower number although nothing was lost. MEASURED when giving their language
/// to 95 fences: `py_glossary` lost 41 trigrams out of 9,994 and
/// `django_leaf` 4 out of 808, for pages where **every changed line was
/// a fence** and not a line of text.
///
/// The info string is therefore reduced to the bare fence before counting. That is the
/// judge corrected before the baseline, not the baseline blessed on a drop.
fn without_fence_info_strings(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("```") && t.len() > 3 {
                "```"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        // Same rule for a cell's line break: `a<br>b` read as `a br b`
        // inserted the word "br" between two lines of the cell (measured:
        // newegg 1.000 → 0.903, a specifications table where no
        // line was lost).
        .replace("<br>", " ")
}

fn analyze(entry: &ManifestEntry) -> PageResult {
    let path = fixtures_dir().join(&entry.file);
    let html =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"));
    // Deep, 2-segment synthetic URL — never trips `site_readers::is_site_index`,
    // so the shell-recovery branch (which does a LIVE fetch) can never fire and
    // this battery stays hermetic regardless of how thin any one page distils.
    let url = format!(
        "https://fixture.test/pages/{}",
        entry.file.trim_end_matches(".html")
    );
    let d = web_pipeline::distill_page(&html, &url, Some("text/html; charset=utf-8"));

    let cleaned = fold_math_to_its_tex(&strip_script_style(&strip_machine_only_microdata(&html)));
    let p_blocks = extract_tag_blocks(&cleaned, &["p"]);
    let nonp_blocks = extract_tag_blocks(&cleaned, NONP_TAGS);
    let md_ngrams = ngrams_stream(&without_fence_info_strings(&d.md), FIDELITY_NGRAM);
    let (p_survived, p_words) =
        bag_recall(&ngrams_per_element(&p_blocks, FIDELITY_NGRAM), &md_ngrams);
    let (nonp_survived, nonp_words) = bag_recall(
        &ngrams_per_element(&nonp_blocks, FIDELITY_NGRAM),
        &md_ngrams,
    );

    let raw_visible = web::decode_entities(&strip_tags(&cleaned));

    PageResult {
        file: entry.file.clone(),
        class: entry.class.clone(),
        p_survived,
        p_words,
        nonp_survived,
        nonp_words,
        md_chars: d.md.chars().count(),
        raw_visible_chars: raw_visible.chars().count(),
        md: d.md,
    }
}

/// Battery 4. Prints the aggregate + per-class A/B table, then enforces the
/// hard floors below — each a real shipped bug.
#[test]
fn battery4_page_fidelity() {
    let entries = manifest();
    assert!(
        entries.len() >= 30,
        "fixture corpus too small: {} pages, expected ~40",
        entries.len()
    );
    let results: Vec<PageResult> = entries.iter().map(analyze).collect();

    let (tot_p_s, tot_p_n) = results.iter().fold((0usize, 0usize), |a, r| {
        (a.0 + r.p_survived, a.1 + r.p_words)
    });
    let (tot_np_s, tot_np_n) = results.iter().fold((0usize, 0usize), |a, r| {
        (a.0 + r.nonp_survived, a.1 + r.nonp_words)
    });
    let agg_p = pct(tot_p_s, tot_p_n);
    let agg_np = pct(tot_np_s, tot_np_n);

    println!(
        "\nBattery 4 aggregate: <p> fidelity = {agg_p:.1}% (gate: near 94%), \
         non-<p> fidelity = {agg_np:.1}% (gate: near 82%)\n"
    );

    let mut metrics = vec![
        ab::Metric::new(
            "<p> fidelity (aggregate)",
            agg_p,
            agg_p,
            ab::Unit::Pct,
            true,
        ),
        ab::Metric::new(
            "non-<p> fidelity (aggregate)",
            agg_np,
            agg_np,
            ab::Unit::Pct,
            true,
        ),
    ];

    let mut classes: Vec<&str> = entries.iter().map(|e| e.class.as_str()).collect();
    classes.sort_unstable();
    classes.dedup();
    for c in &classes {
        let sub: Vec<&PageResult> = results.iter().filter(|r| r.class == *c).collect();
        let (ps, pn) = sub
            .iter()
            .fold((0, 0), |a, r| (a.0 + r.p_survived, a.1 + r.p_words));
        let (nps, npn) = sub
            .iter()
            .fold((0, 0), |a, r| (a.0 + r.nonp_survived, a.1 + r.nonp_words));
        let pf = pct(ps, pn);
        let npf = pct(nps, npn);
        metrics.push(ab::Metric::new(
            format!("<p> fidelity ({c})"),
            pf,
            pf,
            ab::Unit::Pct,
            true,
        ));
        metrics.push(ab::Metric::new(
            format!("non-<p> fidelity ({c})"),
            npf,
            npf,
            ab::Unit::Pct,
            true,
        ));
    }
    ab::print_table(&metrics);

    // Floor (a): a news-front page keeps at least this many headings — the
    // shipped bug this guards was a front page distilling to a wall of
    // unstructured prose, its section headings gone entirely.
    const NEWS_HEADING_FLOOR: usize = 3;
    for r in results.iter().filter(|r| r.class == "news-front") {
        let n =
            r.md.lines()
                .filter(|l| l.trim_start().starts_with('#'))
                .count();
        ab::assert_floor(
            n >= NEWS_HEADING_FLOOR,
            format!(
                "{}: news-front page kept only {n} headings, floor is {NEWS_HEADING_FLOOR}",
                r.file
            ),
        );
    }

    // Floor (b): a heading must never end up inside a link label — the
    // `[### Title ###](url)` inversion is a real shipped bug.
    let wrapped = regex::Regex::new(r"^\[#{1,6}[^\]]*\]\([^)]+\)\s*$").unwrap();
    for r in &results {
        for line in r.md.lines() {
            ab::assert_floor(
                !wrapped.is_match(line.trim()),
                format!("{}: heading inverted into a link label: {line:?}", r.file),
            );
        }
    }

    // Floor (c): a page never distils below this many chars when its raw
    // visible text is comfortably longer — essence's accept ladder
    // constants: sufficient=1000, floor=1000/5=200.
    const RAW_SUFFICIENT_CHARS: usize = 1000;
    const MIN_MD_CHARS: usize = 200;
    for r in &results {
        if r.raw_visible_chars > RAW_SUFFICIENT_CHARS {
            ab::assert_floor(
                r.md_chars >= MIN_MD_CHARS,
                format!(
                    "{}: {} raw visible chars distilled to only {} md chars (floor {MIN_MD_CHARS})",
                    r.file, r.raw_visible_chars, r.md_chars
                ),
            );
        }
    }

    // Floor (d): the RATCHET — no page distils to strictly fewer chars than
    // the committed baseline. On Stage 1 this compares today's shipped path
    // against itself, so it is trivially satisfied; it is the guard for every
    // later stage.
    let baseline = load_baseline();
    // The 39 pages are reported TOGETHER. One `assert!` per page stops at
    // the first and hides the 38 others: a deliberate reduction on one
    // page then looked like an isolated regression, while the question
    // is always "and the others?".
    let mut problems: Vec<String> = Vec::new();
    // The rates we refuse to compare, and why. Printed, never silenced.
    let mut insignificant: Vec<String> = Vec::new();
    for r in &results {
        let b = baseline.get(&r.file).unwrap_or_else(|| {
            panic!(
                "{}: no fidelity_baseline.json entry — regenerate it",
                r.file
            )
        });
        if r.md_chars < b.md_chars {
            problems.push(format!(
                "{}: RATCHET REGRESSION — {} md chars now vs {} recorded in fidelity_baseline.json",
                r.file, r.md_chars, b.md_chars
            ));
        }
        // Floor (e): FIDELITY itself. The baseline had always carried these
        // two numbers and NOTHING read them — only the character count
        // was ratcheted. Yet that is the wrong way round: a count
        // that drops can be a wanted compaction, and it is then
        // fidelity that must answer. Without this floor, the only way to
        // get a deliberate reduction through was to regenerate the baseline,
        // which ALSO ERASES fidelity — the proof and its subject crushed
        // in one move.
        //
        // One-point tolerance: bigram recall moves by a hair when
        // an empty line disappears between two paragraphs.
        for (what, now, was, grams) in [
            (
                "<p>",
                pct(r.p_survived, r.p_words) / 100.0,
                b.p_fidelity,
                r.p_words,
            ),
            (
                "structural",
                pct(r.nonp_survived, r.nonp_words) / 100.0,
                b.nonp_fidelity,
                r.nonp_words,
            ),
        ] {
            // A rate computed on next to nothing is not compared: see
            // `SIGNIFICANT_GRAMS`. The page is NAMED rather than silenced, so that
            // nobody reads its rate as a measurement.
            if !rate_is_comparable(grams) {
                insignificant.push(format!("{} {what} on {grams} trigram(s)", r.file));
                continue;
            }
            if fidelity_regressed(now, was) {
                problems.push(format!(
                    "{}: {what} FIDELITY DROPPED — {now:.3} against {was:.3} in baseline \
                     (on {grams} trigrams)",
                    r.file
                ));
            }
        }
    }
    if !insignificant.is_empty() {
        println!(
            "\n{} rate(s) not compared, denominator under {SIGNIFICANT_GRAMS} trigrams — \
             a rate computed on that distinguishes nothing (the character count still \
             guards these pages):\n  {}\n",
            insignificant.len(),
            insignificant.join("\n  ")
        );
    }
    ab::assert_floor(
        problems.is_empty(),
        format!(
            "{} failing page(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ),
    );
}

/// A page's baseline, as written in the file.
///
/// A separate function so it is TESTABLE: regeneration is `#[ignore]` and
/// writes into a versioned file, so no test can exercise it — the
/// mutation that replaced the denominators with `0` survived everything, including
/// the battery (move the decision out of the place no test
/// reaches).
fn baseline_of(r: &PageResult) -> Baseline {
    Baseline {
        p_fidelity: pct(r.p_survived, r.p_words) / 100.0,
        nonp_fidelity: pct(r.nonp_survived, r.nonp_words) / 100.0,
        md_chars: r.md_chars,
        p_grams: r.p_words,
        nonp_grams: r.nonp_words,
    }
}

/// Is this rate computed on enough material to be compared?
///
/// A PURE decision taken out of the battery, for the same reason as
/// `fidelity_regressed`: in the battery it only runs on the corpus
/// pages, so mutating it would change almost nothing there.
fn rate_is_comparable(grams: usize) -> bool {
    grams >= SIGNIFICANT_GRAMS
}

/// A fidelity that drops, give or take a hair.
///
/// Taken out of the battery to be TESTABLE: in the battery, it only
/// runs on pages that are fine, so mutating it changes nothing —
/// a clause that no case can make fail is not guarded.
///
/// The one-point slack absorbs the quiver of bigram recall when an
/// empty line disappears between two paragraphs; below that, it is text
/// that is missing.
fn fidelity_regressed(now: f64, was: f64) -> bool {
    const FIDELITY_SLACK: f64 = 0.01;
    now < was - FIDELITY_SLACK
}

/// Regenerates `fixtures/fidelity_baseline.json` from the CURRENT shipped
/// `distill_page`. Not run by default — this is a deliberate, reviewed act
/// (an accepted fidelity change), not something `cargo test` should do on its
/// own. See the module doc for the invocation.
#[test]
#[ignore = "regenerates the fidelity baseline"]
fn regen_fidelity_baseline() {
    let entries = manifest();
    // BTreeMap, not HashMap: the file is committed, and a HashMap reshuffles all
    // 39 entries on every regeneration — 256 lines of diff with zero changed
    // values, which is exactly the noise a real regression hides in.
    let mut out: std::collections::BTreeMap<String, Baseline> = std::collections::BTreeMap::new();
    for e in &entries {
        let r = analyze(e);
        out.insert(r.file.clone(), baseline_of(&r));
    }
    let json = serde_json::to_string_pretty(&out).unwrap();
    std::fs::write(baseline_path(), &json).unwrap();
    println!("wrote {} entries to {:?}", out.len(), baseline_path());
}

/// The DENOMINATOR floor, on both sides.
///
/// `guardian_sec` and `hn_thread` do not have a single `<p>` trigram: their
/// "fidelity" is a `0/0`, and the baseline recorded it as a hard number.
/// What the baseline RECORDS carries the denominator, otherwise a `0/0` reads back
/// as perfect fidelity — that is the case of `hn_thread`, `p_fidelity: 1.0`
/// on zero trigrams.
#[test]
fn the_baseline_records_what_the_rate_was_computed_on() {
    let r = PageResult {
        file: "x.html".into(),
        class: "article".into(),
        p_survived: 0,
        p_words: 0,
        nonp_survived: 70,
        nonp_words: 140,
        md: String::new(),
        md_chars: 1_000,
        raw_visible_chars: 5_000,
    };
    let b = baseline_of(&r);
    assert_eq!(b.p_grams, 0, "the denominator of a 0/0 is not recorded");
    assert_eq!(
        b.nonp_grams, 140,
        "the structural denominator is not recorded"
    );
    assert!(
        (b.nonp_fidelity - 0.5).abs() < 1e-9,
        "the rate itself changed"
    );
}

#[test]
fn a_rate_on_almost_nothing_is_not_compared() {
    assert!(!rate_is_comparable(0), "a 0/0 was compared");
    assert!(!rate_is_comparable(SIGNIFICANT_GRAMS - 1));
    // The smallest ACCEPTED case: without it, a floor that refuses everything passes.
    assert!(rate_is_comparable(SIGNIFICANT_GRAMS));
    assert!(rate_is_comparable(4_000));
}

#[test]
fn a_fidelity_that_drops_is_seen_and_a_hair_is_not() {
    // Missing text: seen.
    assert!(fidelity_regressed(0.80, 0.95));
    assert!(fidelity_regressed(0.939, 0.95));
    // A hair: ignored, otherwise the floor cries at every removed empty line.
    assert!(!fidelity_regressed(0.945, 0.95));
    assert!(!fidelity_regressed(0.95, 0.95));
    // A fidelity that RISES is never a regression.
    assert!(!fidelity_regressed(0.99, 0.95));
}
