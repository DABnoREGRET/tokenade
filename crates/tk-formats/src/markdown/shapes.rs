//! **Recognising what a line IS.**
//!
//! A YAML header, a reference definition, a code-block fence,
//! a bullet, a line of badges. None of these judgements changes anything
//! at all: they say what the next line inherits, and getting it wrong here makes
//! a code block fold like prose.

/// Heuristic: ≥2 markdown markers in the first 40 lines → likely
/// markdown. Real markdown files always have multiple markers (a
/// heading + at least one list/code-block/link, or several headings).
/// #551 — was a single-line trigger which caught bash outputs that
/// happened to contain ONE matching line (a `# comment`, an `ls`-style
/// `- entry`, a URL in error text formatted as `[text](url)`).
/// Mirror of the same density-not-presence pattern as
/// #391/#409/#481/#550.
pub fn looks_like_markdown(text: &str) -> bool {
    // Two-class scoring: `# ` (single-hash header) is the ONLY marker
    // that overlaps with shell / Python / .gitignore / requirements.txt
    // comments. Files with only `# ` markers are usually not markdown —
    // they're comment-dominated config files. Require ≥2 total markers
    // AND at least one stronger signal (`## `, code fence, list bullet,
    // link) to confirm. A typical README (`# Title\n## Install\n…`) still
    // passes via the `## ` marker; a typical requirements.txt with two
    // `# header` lines no longer falsely matches.
    //
    // The WINDOW, not the rule, is what made this disagree with the detector
    // that routes here. Both stopped at 40 lines, and a document whose first
    // strong marker sits below that — MDX front-matter and imports, a licence
    // header, a long intro paragraph, a title followed straight by a big table
    // — failed here after `auto::detect` had already called it Markdown. The
    // dispatcher routed it in and the first line of `compact` handed it back.
    // MEASURED 2026-08-26 over 9 001 documents of 4-40 KB: **381 rejected,
    // 4.99 MB returned whole, and 377 of the 381 carry the marker further
    // down**. Only 4 were genuinely marker-less.
    //
    // 400 is the bound `detectors::looks_strongly_like_markdown` already uses
    // for the same question, so this is its window and not a new number. The
    // loop returns the moment it is convinced, so the wider bound is paid only
    // by documents that never qualify — the ones about to be handed back
    // anyway.
    let mut total = 0usize;
    let mut strong = 0usize;
    for l in text.lines().take(400) {
        let t = l.trim_start();
        let is_weak = t.starts_with("# ");
        let is_strong = t.starts_with("## ")
            || crate::fences::fence_marker(t).is_some()
            || t.starts_with("- ")
            || t.starts_with("* ")
            || (t.starts_with('[') && t.contains("](") && t.contains(')'));
        if is_weak || is_strong {
            total += 1;
        }
        if is_strong {
            strong += 1;
        }
        if total >= 2 && strong >= 1 {
            return true;
        }
    }
    false
}

pub(super) fn front_matter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().map(|l| l.trim()) != Some("---") {
        return None;
    }
    lines[1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|p| p + 1)
}

/// Find the line index where a contiguous run of `[id]: url …` reference
/// definitions starts at the end of the document. Returns `None` if there
/// is no such block.
pub(super) fn link_ref_block_start(lines: &[&str]) -> Option<usize> {
    let mut start: Option<usize> = None;
    for (i, l) in lines.iter().enumerate().rev() {
        if l.trim().is_empty() {
            continue;
        }
        if is_link_ref_def(l) {
            start = Some(i);
        } else {
            break;
        }
    }
    // Only return a block of ≥3 to avoid stripping a single inline-defined
    // reference that lives in the middle of the doc.
    start.filter(|s| lines.len() - s >= 3)
}

pub(super) fn is_link_ref_def(line: &str) -> bool {
    // `  [id]: https://… "optional title"`
    let t = line.trim_start();
    if !t.starts_with('[') {
        return false;
    }
    let Some(rb) = t.find(']') else { return false };
    let after = &t[rb + 1..];
    after.starts_with(": ") || after.starts_with(":\t")
}

/// If `lines[i]` is a fenced block opener (``` or `~~~`), return
/// (marker, lang, end-line-index, number of body lines between the fences).
pub(super) fn fenced_code_at<'a>(
    lines: &'a [&'a str],
    i: usize,
) -> Option<(&'a str, &'a str, usize, usize)> {
    let opener = lines[i].trim_start();
    let marker = crate::fences::fence_marker(opener)?;
    let ch = marker.as_bytes()[0];
    let n = opener.bytes().take_while(|&b| b == ch).count();
    let lang = opener[n..].trim();
    for (j, l) in lines.iter().enumerate().skip(i + 1) {
        if l.trim_start().starts_with(marker) {
            return Some((marker, lang, j, j - i - 1));
        }
    }
    // No closer — treat the whole tail as the block (defensive).
    Some((marker, lang, lines.len() - 1, lines.len() - i - 1))
}

/// CommonMark indented code: a run of lines each starting with 4 spaces or a
/// tab. Blank lines inside the run stay in the block until a non-indented
/// non-blank line. Nested list items (`    - foo`) are not code.
///
/// Returns `(end_index, body_line_count)` if `lines[i]` opens such a run.
pub(super) fn indented_code_at(lines: &[&str], i: usize) -> Option<(usize, usize)> {
    if !is_indented_code_line(lines[i]) {
        return None;
    }
    if list_marker(strip_one_indent(lines[i])).is_some() {
        return None;
    }
    let mut j = i;
    let mut body = 0usize;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            let continues = lines[j + 1..]
                .iter()
                .find(|x| !x.trim().is_empty())
                .is_some_and(|n| {
                    is_indented_code_line(n) && list_marker(strip_one_indent(n)).is_none()
                });
            if continues {
                j += 1;
                continue;
            }
            break;
        }
        if !is_indented_code_line(l) || list_marker(strip_one_indent(l)).is_some() {
            break;
        }
        body += 1;
        j += 1;
    }
    if body == 0 {
        return None;
    }
    Some((j - 1, body))
}

fn is_indented_code_line(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn strip_one_indent(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('\t') {
        rest
    } else if let Some(rest) = line.strip_prefix("    ") {
        rest
    } else {
        line
    }
}

/// ATX heading: 1–6 `#` then a space or end of line. Leaf block, so the
/// next indented run is code even without a blank line.
pub(super) fn is_atx_heading(line: &str) -> bool {
    let t = line.trim_start();
    let n = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&n) && (t.len() == n || t.as_bytes().get(n) == Some(&b' '))
}

/// Return the indent (in spaces) if `line` looks like a list-item start.
pub(super) fn list_marker(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return Some(indent);
    }
    // Ordered: `1. ` `42. `
    if let Some(dot) = t.find(". ") {
        let prefix = &t[..dot];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            return Some(indent);
        }
    }
    None
}

/// A line is a "badge line" if ≥60 % of its non-whitespace characters belong
/// to badge-shaped patterns: `[![…](shields/badge URL)](link)`, raw
/// `<img src=…/>` for shields.io, or pure repeats of those constructs.
pub(super) fn is_badge_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // Quick reject: must contain one of the badge signatures.
    let has_badge_signature = t.contains("[![")
        || t.contains("shields.io")
        || t.contains("img.shields.io")
        || (t.starts_with("<a ") && t.contains("<img"))
        || (t.starts_with("<img") && t.contains("badge"));
    if !has_badge_signature {
        return false;
    }
    // If the line is mostly badges + small connectors, treat as decorative.
    // Count "badge characters" — chars inside [![..]](..) and shields URLs.
    //
    // The cursor walks BYTES but slices a `&str`, so it has to land on char
    // boundaries: stepping one byte at a time panicked on the first multibyte
    // character in the line (`start byte index N is not a char boundary`), which
    // is any README with a CJK, accented or emoji character before its badges.
    // Advance by the width of the char instead, and count characters on both
    // sides of the ratio — counting badge BYTES against total CHARS declared
    // every non-Latin line a badge line.
    let mut badge_chars = 0usize;
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if rest.starts_with("[!") {
            // Scan to matching `)` after `]` then after the link's `)`.
            if let Some(rb) = rest.find(')') {
                badge_chars += rest[..rb + 1].chars().count();
                i += rb + 1;
                continue;
            }
        }
        if rest.starts_with("<img") || rest.starts_with("<a ") {
            if let Some(close) = rest.find('>') {
                badge_chars += rest[..close + 1].chars().count();
                i += close + 1;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    let total = t.chars().filter(|c| !c.is_whitespace()).count();
    total > 0 && badge_chars * 100 / total >= 60
}

/// Strip inline non-structural HTML tags (`<p>`, `<a>`, `<br>`, `<sub>`,
/// `<sup>`, `<img>`, `<span>`, …) while keeping the text content between
/// them. Block-level tags handled by the main pass are NOT removed here.
/// Map a fenced-code-block language label to its canonical short form.
/// Ported from nadimtuhin/claude-token-optimizer's `compress.js` rule
/// set. We collapse known-long synonyms (`javascript` → `js`,
/// `typescript` → `ts`, `python` → `py`, etc.) so a doc with dozens of
/// fences shaves a measurable handful of tokens per fence. Unknown
/// labels pass through unchanged (no false-aliasing risk).
pub(super) fn canonical_fence_lang(lang: &str) -> &str {
    match lang.trim() {
        "javascript" | "Javascript" | "JavaScript" => "js",
        "typescript" | "Typescript" | "TypeScript" => "ts",
        "python" | "Python" => "py",
        "ruby" | "Ruby" => "rb",
        "rust" | "Rust" => "rs",
        "shell" | "Shell" | "bash" | "Bash" | "zsh" => "sh",
        "yaml" | "Yaml" | "YAML" => "yml",
        "markdown" | "Markdown" => "md",
        "html" | "Html" | "HTML" => "html", // already short
        "json" | "Json" | "JSON" => "json",
        "kotlin" | "Kotlin" => "kt",
        "swift" | "Swift" => "swift",
        "csharp" | "CSharp" | "C#" | "c#" => "cs",
        "objective-c" | "Objective-C" | "objectivec" => "objc",
        "powershell" | "Powershell" | "PowerShell" => "ps1",
        "dockerfile" | "Dockerfile" => "dockerfile",
        other => other,
    }
}
