//! Minified / bundled source: one enormous line of machine-written code.
//!
//! Every other code path in this crate reads LINES — a skeleton keeps
//! declaration lines, a diff reads hunks, a log dedups shapes. A bundle has
//! no lines: `dist/index.js` is eight rows of which one is 93 124 characters,
//! so line-based folding has nothing to hold on to and the file arrives whole.
//!
//! Measured 2026-08-28 over the tool outputs this machine actually stashed
//! (149 captures older than three days): the three largest unclaimed ones are
//! minified JavaScript, 332 000 tokens between them, folded by nothing. What
//! the agent received was four 300-character slices of minified soup and a
//! stash pointer — bytes that cannot be read at any length.
//!
//! What a bundle CAN say about itself is what it pulls in and what it hands
//! out: the module specifiers and the declared names. Both survive
//! minification, because a specifier is a string literal and an export name is
//! part of the public contract. Local identifiers do not (`sFA`, `ux`, `YV`)
//! and are counted rather than listed.

use regex::Regex;
use std::sync::OnceLock;

/// Longest line, in bytes, below which nothing here applies.
const MIN_LONGEST_LINE: usize = 1_500;
/// Share of the file that must sit in LONG lines for it to read as bundled.
///
/// An earlier version asked this of the single longest line, which is right for a one-row
/// `dist/index.js` and wrong for the shape right next to it: measured
/// 2026-08-28 on this machine's own captures, the two largest unclaimed
/// outputs are bundles of TWELVE and TWENTY rows, each row tens of thousands
/// of characters. No single line carried 40 % of those files, so neither was
/// recognised — 286 000 tokens, on the exact shape the compactor was written
/// for. What matters is that the file is made of long lines, not that one line
/// dominates it.
const LONG_LINE_SHARE: f64 = 0.60;
/// A line at least this long counts toward that share.
const LONG_LINE_BYTES: usize = 500;
/// Smallest input worth looking at.
const MIN_BYTES: usize = 4_000;

/// Is this machine-written, line-free source?
///
/// Three gates, and the third is what keeps a one-line JSON payload or a
/// single very long log line out: the dominant line must read as CODE.
pub fn detect(text: &str) -> bool {
    if text.len() < MIN_BYTES {
        return false;
    }
    let Some(longest) = text.lines().max_by_key(|l| l.len()) else {
        return false;
    };
    if longest.len() < MIN_LONGEST_LINE {
        return false;
    }
    let long_bytes: usize = text
        .lines()
        .filter(|l| l.len() >= LONG_LINE_BYTES)
        .map(|l| l.len())
        .sum();
    if (long_bytes as f64) < text.len() as f64 * LONG_LINE_SHARE {
        return false;
    }
    // A JSON document is one long line too, and it has a compactor of its own
    // that knows far more about it than this one does. So does a minified HTML
    // page — a Cloudflare interstitial is one row of `<!DOCTYPE html><html…`
    // carrying enough `function` and `=>` to clear the code gate below.
    // Ordering in the detector chain already puts both ahead of this one;
    // saying it HERE too means the answer does not depend on that order.
    let head = text.trim_start();
    if head.starts_with('{') || head.starts_with('[') || head.starts_with('<') {
        return false;
    }
    code_markers(longest) >= 3
}

/// How many distinct code shapes the line carries. Counted on SHAPES rather
/// than keywords: a minifier renames identifiers but cannot rename syntax.
fn code_markers(line: &str) -> usize {
    let marks = [
        "function", "=>", "return", "){", ";}", "var ", "const ", "typeof ",
    ];
    marks.iter().filter(|m| line.contains(**m)).count()
}

fn specifier_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:from|require\(|import\()\s*["']([^"'\s]{1,120})["']"#).expect("static")
    })
}

fn declaration_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:^|[;{}\s])(?:function|class)\s+([A-Za-z_$][A-Za-z0-9_$]*)").expect("static")
    })
}

fn export_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?:export\s+(?:default\s+)?(?:function|class|const|let|var)\s+|exports\.)([A-Za-z_$][A-Za-z0-9_$]*)",
        )
        .expect("static")
    })
}

/// Names in source order, deduplicated, with the total kept separately so the
/// cap never hides how many there were.
fn names(re: &Regex, text: &str, cap: usize) -> (Vec<String>, usize) {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for c in re.captures_iter(text) {
        let Some(m) = c.get(1) else { continue };
        if seen.insert(m.as_str().to_string()) && out.len() < cap {
            out.push(m.as_str().to_string());
        }
    }
    (out, seen.len())
}

fn render_list(label: &str, items: &[String], total: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let shown = items.join(", ");
    if total > items.len() {
        format!("{label} ({total}): {shown}, … (+{})\n", total - items.len())
    } else {
        format!("{label} ({total}): {shown}\n")
    }
}

/// **A window someone cut out of a bundle is read, not summarised.**
///
/// The summary is the answer for a bundle too big to read — imports, exports,
/// declared names. It is no answer for 4.5 KB the agent carved out on purpose:
/// measured over sub-agent transcripts (2026-09-29), `dd if=opencode bs=1
/// skip=… count=4500 | tr …` and `strings -n 6 droid | grep -o '.\{0,600\}A3\$…'`
/// came back as `declares (5): ge, Co, ai, qe, Po`, and the next call was
/// `expand-ref` both times — the agent was reading the CODE around a string.
/// While the text fits under the delivery threshold it is handed over whole.
pub fn compact(text: &str) -> String {
    if tk_base::delivery::fits(text) {
        return text.to_string();
    }
    let lines = text.lines().count();
    let longest = text.lines().map(|l| l.len()).max().unwrap_or(0);
    let (specs, n_specs) = names(specifier_re(), text, 40);
    let (decls, n_decls) = names(declaration_re(), text, 24);
    let (exports, n_exports) = names(export_re(), text, 24);
    let mut out = format!(
        "[tokenade:minified] bundled source — {} bytes, {lines} line(s), longest {longest}\n",
        text.len()
    );
    out.push_str(&render_list("imports", &specs, n_specs));
    out.push_str(&render_list("exports", &exports, n_exports));
    out.push_str(&render_list("declares", &decls, n_decls));
    // Anti-inflation: a summary that costs more than the file is not a
    // summary. Cheap to check and it has bitten every compactor here once.
    if out.len() >= text.len() {
        return text.to_string();
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> String {
        let mut one = String::from(
            "import{a as Q}from\"@scope/pkg\";const R=require(\"node:timers/promises\");",
        );
        for i in 0..300 {
            one.push_str(&format!(
                "function f{i}($,Z){{let Y=$.x??0;if(typeof Z===\"string\")return Y;var W=(q)=>q+{i};return W(Y)}}"
            ));
        }
        one.push_str("export function handleRequest(Q){return f0(Q,\"\")}exports.shutdown=f1;");
        format!("#!/usr/bin/env node\n{one}\n")
    }

    #[test]
    fn a_bundle_is_detected_and_a_source_file_is_not() {
        assert!(detect(&bundle()));
        let ordinary: String = (0..400)
            .map(|i| format!("function f{i}(x) {{\n  return x + {i};\n}}\n"))
            .collect();
        assert!(
            !detect(&ordinary),
            "an ordinary source file is not a bundle"
        );
    }

    /// A bundle is not always ONE row. The two largest unclaimed captures on
    /// this machine are twelve and twenty rows of tens of thousands of
    /// characters each — an esbuild output with a handful of `\n` left in.
    /// Asking that the LONGEST line carry the file misses exactly those.
    #[test]
    fn a_bundle_split_over_a_dozen_rows_is_still_a_bundle() {
        let mut b = String::from("#!/usr/bin/env node\n");
        for row in 0..12 {
            let mut one = String::new();
            for i in 0..120 {
                one.push_str(&format!(
                    "function f{row}_{i}($,Z){{let Y=$.x??0;if(typeof Z===\"string\")return Y;var W=(q)=>q+{i};return W(Y)}}"
                ));
            }
            b.push_str(&one);
            b.push('\n');
        }
        let longest = b.lines().map(|l| l.len()).max().unwrap();
        assert!(
            (longest as f64) < b.len() as f64 * LONG_LINE_SHARE,
            "the fixture must NOT be carried by a single row: {longest} of {}",
            b.len()
        );
        assert!(detect(&b), "a multi-row bundle was missed");
    }

    /// A one-line JSON payload is one long line too, and json knows more about
    /// it than this ever will. The fixture carries JS in its VALUES — an npm
    /// registry document or a CI config does — so the code gate cannot be what
    /// rejects it, and the opening brace has to do the work.
    #[test]
    fn a_json_payload_is_left_to_json() {
        let mut j = String::from("{\"scripts\":[");
        for i in 0..400 {
            if i > 0 {
                j.push(',');
            }
            j.push_str(&format!(
                "{{\"id\":{i},\"src\":\"function f{i}(){{const q=1;var w=(z)=>z;return typeof w}}\"}}"
            ));
        }
        j.push_str("]}");
        assert!(j.len() > MIN_BYTES);
        assert!(
            code_markers(&j) >= 3,
            "the fixture must clear the code gate"
        );
        assert!(!detect(&j), "a JSON payload must reach the json compactor");
    }

    /// A minified HTML page opens on `<` and carries plenty of `function` and
    /// `=>` — a Cloudflare interstitial is exactly that. The html compactor
    /// owns it, and saying so here means the answer does not depend on which
    /// detector the chain asks first.
    #[test]
    fn a_minified_html_page_is_left_to_html() {
        let mut h = String::from(
            "<!DOCTYPE html><html lang=\"en-US\"><head><title>Just a moment</title><script>",
        );
        for i in 0..200 {
            h.push_str(&format!(
                "function f{i}(a){{var b=(c)=>c+{i};if(typeof a===\"string\")return b(1);return a}}"
            ));
        }
        h.push_str("</script></head><body></body></html>");
        assert!(h.len() > MIN_BYTES);
        assert!(
            code_markers(&h) >= 3,
            "the fixture must clear the code gate"
        );
        assert!(
            !detect(&h),
            "a minified HTML page must reach the html compactor"
        );
    }

    /// A very long line that is not CODE is not a bundle: a wrapped-off text
    /// file, a base64 blob, one enormous log entry. The gate is on shape, and
    /// shape is what a minifier cannot rename.
    #[test]
    fn a_long_line_that_is_not_code_is_not_a_bundle() {
        let prose = format!(
            "notes\n{}\n",
            "lorem ipsum dolor sit amet consectetur adipiscing elit sed do ".repeat(120)
        );
        assert!(prose.lines().map(|l| l.len()).max().unwrap() > MIN_LONGEST_LINE);
        assert!(!detect(&prose), "prose is not a bundle");
    }

    /// One long line inside a normal file is an embedded asset, not a bundle:
    /// the rest of the file still has the structure every other path reads.
    #[test]
    fn one_long_line_in_a_normal_file_is_not_a_bundle() {
        // Ordinary source lines, at an ordinary width: this fixture also pins
        // LONG_LINE_BYTES, which only discriminates if the file's normal lines
        // are shorter than it and longer than a mutation would set it.
        let mut src: String = (0..600)
            .map(|i| {
                format!(
                    "export function computeAdjustedTotal{i}(input: number): number {{\n    const scaled = input * {i} + 17;\n    return Math.round(scaled / 3);\n}}\n"
                )
            })
            .collect();
        src.push_str(&format!(
            "const ASSET=\"{}\";function h(){{var q=1;return typeof q}}\n",
            "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo".repeat(60)
        ));
        assert!(src.lines().map(|l| l.len()).max().unwrap() > MIN_LONGEST_LINE);
        assert!(!detect(&src), "an embedded asset does not make a bundle");
    }

    /// What survives minification is what the bundle says about the outside
    /// world: its specifiers and its exported names.
    #[test]
    fn the_summary_names_imports_and_exports() {
        let out = compact(&bundle());
        assert!(
            out.contains("@scope/pkg"),
            "an import specifier is missing: {out}"
        );
        assert!(
            out.contains("node:timers/promises"),
            "a require specifier is missing: {out}"
        );
        assert!(out.contains("handleRequest"), "an export is missing: {out}");
        assert!(
            out.contains("shutdown"),
            "a CommonJS export is missing: {out}"
        );
        assert!(
            out.len() < bundle().len() / 10,
            "barely folded: {}",
            out.len()
        );
    }

    /// `dd … count=4500 | tr` on a binary: a window cut on purpose,
    /// rendered as `declares (5): …` then read back by `expand-ref`.
    #[test]
    fn a_window_that_fits_is_handed_over_whole() {
        let mut win = String::new();
        for i in 0..60 {
            win.push_str(&format!("function g{i}(a,b){{var c=a??0;if(typeof b===\"string\")return c;return(q=>q+{i})(c)}}"));
        }
        win.push('\n');
        assert!(detect(&win), "the window is bundle-shaped");
        assert_eq!(compact(&win), win);
        assert!(
            !tk_base::delivery::fits(&bundle()),
            "the fixture bundle must stay over the threshold"
        );
    }

    /// The cap must never hide the count.
    #[test]
    fn a_capped_list_still_reports_the_total() {
        let out = compact(&bundle());
        let line = out
            .lines()
            .find(|l| l.starts_with("declares"))
            .expect("a declares line");
        assert!(
            line.contains("(301)"),
            "the total is not the shown count: {line}"
        );
        assert!(line.contains("(+"), "the elision is not named: {line}");
    }

    /// A summary that costs more than the file is not a summary.
    #[test]
    fn it_never_inflates() {
        let mut small = String::from("function a(){return 1}");
        while small.len() < MIN_BYTES + 100 {
            small.push_str("function b(){return 2}");
        }
        // Under the delivery threshold the window is handed over whole —
        // never MORE than it.
        assert!(compact(&small).len() <= small.len());
        let big = bundle();
        assert!(compact(&big).len() < big.len());
    }
}
