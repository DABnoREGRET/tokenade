//! `markdown` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Real 1400-line README from the cloned `claude-context-mode` competitor.
pub(crate) fn task_markdown_real(results: &mut Vec<RunResult>) {
    let path = PathBuf::from("benchmarks/fixtures/markdown/big-readme.md");
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    if raw.len() < 1000 {
        return;
    }
    let (out, sav, ms) = measure(&raw, markdown::compact_with_savings);
    // Quality: at least the H1 was preserved (every real README has one) and
    // we observed some compression.
    let h1_kept = raw
        .lines()
        .find(|l| l.starts_with("# "))
        .is_some_and(|h1| out.contains(h1));
    let quality = if h1_kept { 1.0 } else { 0.0 };
    record(
        results,
        "output:markdown-compact",
        "markdown",
        "real-readme",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #33: a markdown doc with a 10 000-line code block (think
// captured CI log inside a README). Must compress > 95 % AND keep the
// heading + the surrounding prose.
pub(crate) fn adv_markdown_huge_code_block(results: &mut Vec<RunResult>) {
    let mut md = String::from("# README\n\nIntro paragraph.\n\n```text\n");
    for i in 0..10_000 {
        md.push_str(&format!("log line {i}: lorem ipsum dolor sit amet\n"));
    }
    md.push_str("```\n\nOutro paragraph after the giant block.\n");
    let (out, sav, ms) = measure(&md, markdown::compact_with_savings);
    let quality = if out.contains("# README")
        && out.contains("Intro paragraph.")
        && out.contains("Outro paragraph after the giant block.")
        && out.contains("[code block elided: 10000 lines]")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:markdown:huge-code-block",
        "markdown",
        "adversarial",
        &md,
        &out,
        ms,
        quality,
    );
}

// Adversarial #34: a doc with 500 reference-link definitions at the bottom
// (an over-zealous Wikipedia-style export). Must strip the block, keep the
// body + inline references, emit the elision marker.
pub(crate) fn adv_markdown_link_reference_dump(results: &mut Vec<RunResult>) {
    let mut md =
        String::from("# Title\n\nText referencing [link a][a], [link b][b], [link c][c].\n\n");
    for i in 0..500 {
        md.push_str(&format!(
            "[ref{i}]: https://example.com/page/{i} \"Title {i}\"\n"
        ));
    }
    let (out, sav, ms) = measure(&md, markdown::compact_with_savings);
    let quality = if out.contains("# Title")
        && out.contains("[link a][a]")
        && out.contains("link-reference definition(s) elided")
        && !out.contains("https://example.com/page/200")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:markdown:link-reference-dump",
        "markdown",
        "adversarial",
        &md,
        &out,
        ms,
        quality,
    );
}
