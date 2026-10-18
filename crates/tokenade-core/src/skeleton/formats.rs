//! **The skeleton of formats that are not CODE.**
//!
//! YAML, Markdown, Terraform: their "declaration" is not a function signature
//! but a key, a heading, a resource block.
//!
//! They share the same promise with code — render the SHAPE and keep quiet
//! about the content — and none of its mechanics. Mixing them would suggest a
//! common parser is possible, whereas what matters here is precisely what each
//! format calls a declaration.

use super::*;

/// Depth beyond which a YAML file no longer renders its keys.
const YAML_DEPTH_CAP: usize = 2;

pub(crate) const MARKDOWN_BLURB_CHARS: usize = 200;

/// Skeleton of a YAML / k8s manifest: keep keys up to depth [`YAML_DEPTH_CAP`],
/// collapse long arrays to the first [`YAML_ARRAY_CAP`] items + a count,
/// strip inline comments and trailing whitespace.
///
/// Heuristic, line-based — no YAML parser dependency. Indent is measured in
/// leading spaces (tabs are normalised to 2 spaces). Bodies of deeper subtrees
/// are folded as `… N lines hidden …`.
pub fn skeleton_yaml(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hidden: usize = 0;
    let mut array_run: Option<(usize, usize)> = None; // (indent_of_dash, count_so_far)
    let flush_hidden = |out: &mut Vec<String>, hidden: &mut usize, indent: usize| {
        if *hidden > 0 {
            let pad = " ".repeat(indent);
            out.push(format!("{pad}# … {} line(s) hidden …", hidden));
            *hidden = 0;
        }
    };
    for raw in text.lines() {
        let line = raw.replace('\t', "  ");
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        // Truncate huge inline string scalars (e.g. base64 secrets).
        let mut display = line.clone();
        if let Some((k, v)) = trimmed.split_once(':') {
            if v.trim().len() > 240 {
                let prefix = " ".repeat(indent);
                display = format!("{prefix}{k}: <{} chars elided>", v.trim().len());
            }
        }
        // Array item handling — `  - foo` runs.
        if trimmed.starts_with("- ") || trimmed == "-" {
            match array_run {
                Some((dash_indent, ref mut count)) if dash_indent == indent => {
                    *count += 1;
                    if *count <= YAML_ARRAY_CAP {
                        out.push(display);
                    } else {
                        hidden += 1;
                    }
                    continue;
                }
                _ => {
                    flush_hidden(&mut out, &mut hidden, indent);
                    array_run = Some((indent, 1));
                    out.push(display);
                    continue;
                }
            }
        } else {
            // Ending an array run — emit the elision marker if any.
            if let Some((dash_indent, count)) = array_run {
                if count > YAML_ARRAY_CAP {
                    let pad = " ".repeat(dash_indent);
                    out.push(format!(
                        "{pad}# … {} more item(s) elided …",
                        count - YAML_ARRAY_CAP
                    ));
                }
                array_run = None;
                hidden = 0;
            }
        }
        // Depth-based folding for nested keys.
        let level = indent / 2;
        if level > YAML_DEPTH_CAP {
            hidden += 1;
        } else {
            flush_hidden(&mut out, &mut hidden, indent);
            out.push(display);
        }
    }
    // Tail flush.
    if let Some((dash_indent, count)) = array_run {
        if count > YAML_ARRAY_CAP {
            let pad = " ".repeat(dash_indent);
            out.push(format!(
                "{pad}# … {} more item(s) elided …",
                count - YAML_ARRAY_CAP
            ));
        }
    }
    if hidden > 0 {
        out.push(format!("# … {hidden} line(s) hidden …"));
    }
    out.join("\n")
}

/// **DECLARED RESERVE**: the variant that COUNTS. The dispatcher calls the
/// bare form; this one exists for the caller that must report, and all three
/// formats carry it together so that none is the exception.
#[allow(dead_code)]
pub(crate) fn skeleton_yaml_with_savings(text: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let s = skeleton_yaml(text);
    let after = tokenizer::estimate(&s);
    (s, crate::Savings::new(before, after))
}

/// Skeleton of a markdown doc: keep ATX headings (`# … ###`), the first
/// [`MARKDOWN_BLURB_CHARS`] characters of each section's body, then fold the
/// rest as `… N more line(s) elided …`. Fenced code blocks are summarised
/// as `code: \`fence-info\` (N lines)` so the agent knows they exist without
/// reading them.
pub fn skeleton_markdown(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut section_chars = 0usize;
    let mut hidden_in_section = 0usize;
    let mut hidden_total = 0usize;
    let flush = |out: &mut Vec<String>, hidden: &mut usize| {
        if *hidden > 0 {
            out.push(format!("  … {} more line(s) elided …", hidden));
            *hidden = 0;
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let trim = line.trim_start();
        // Heading: reset blurb budget.
        if trim.starts_with('#') {
            flush(&mut out, &mut hidden_in_section);
            out.push(line.to_string());
            section_chars = 0;
            i += 1;
            continue;
        }
        // Fenced code block.
        if trim.starts_with("```") {
            let fence = trim.trim_start_matches('`');
            let mut count = 0;
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with("```") {
                count += 1;
                j += 1;
            }
            flush(&mut out, &mut hidden_in_section);
            out.push(format!("```{fence} ({} lines hidden)```", count));
            hidden_total += count;
            i = j + 1;
            continue;
        }
        // Section blurb up to N chars.
        if line.trim().is_empty() {
            // Skip blank-line runs after we've emitted a blurb.
            i += 1;
            continue;
        }
        if section_chars < MARKDOWN_BLURB_CHARS {
            out.push(line.to_string());
            section_chars += line.len();
        } else {
            hidden_in_section += 1;
            hidden_total += 1;
        }
        i += 1;
    }
    flush(&mut out, &mut hidden_in_section);
    // #1 — actionable footer: a markdown skeleton drops blurb tails and code
    // block bodies. Tell the agent how to get the dropped prose/code back.
    // Gated on the same amortisation threshold as `skeleton`.
    if hidden_total >= SKELETON_FOOTER_MIN_LINES {
        out.push(format!(
            "[tokenade markdown skeleton — {hidden_total} line(s) of prose/code elided; \
             re-Read the file with offset/limit for a section, or `tokenade expand-ref <hash>` \
             (hash printed in this output's banner) for the verbatim document]"
        ));
    }
    out.join("\n")
}

/// **DECLARED RESERVE**: see [`skeleton_yaml_with_savings`].
#[allow(dead_code)]
pub(crate) fn skeleton_markdown_with_savings(text: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let s = skeleton_markdown(text);
    let after = tokenizer::estimate(&s);
    (s, crate::Savings::new(before, after))
}

/// Skeleton of a Terraform / HCL file: keep `resource`/`module`/`variable`/
/// `output`/`data`/`provider`/`terraform` block headers + the closing brace,
/// fold body contents as `# … N line(s) elided …`.
pub fn skeleton_terraform(text: &str) -> String {
    let block_keywords = [
        "resource",
        "module",
        "variable",
        "output",
        "data",
        "provider",
        "terraform",
        "locals",
        "ephemeral",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut hidden = 0usize;
    let flush = |out: &mut Vec<String>, hidden: &mut usize| {
        if *hidden > 0 {
            out.push(format!("  # … {} line(s) elided …", hidden));
            *hidden = 0;
        }
    };
    for line in text.lines() {
        let trim = line.trim_start();
        if trim.is_empty() || trim.starts_with('#') || trim.starts_with("//") {
            continue;
        }
        let is_header = block_keywords
            .iter()
            .any(|k| trim.starts_with(&format!("{k} ")) || trim.starts_with(&format!("{k}\t")))
            || trim.starts_with("locals {");
        if is_header && trim.ends_with('{') {
            flush(&mut out, &mut hidden);
            out.push(line.to_string());
            depth += 1;
            continue;
        }
        if trim == "}" {
            flush(&mut out, &mut hidden);
            out.push(line.to_string());
            depth = (depth - 1).max(0);
            continue;
        }
        if depth > 0 {
            hidden += 1;
        } else {
            out.push(line.to_string());
        }
    }
    flush(&mut out, &mut hidden);
    out.join("\n")
}

/// **DECLARED RESERVE**: see [`skeleton_yaml_with_savings`].
#[allow(dead_code)]
pub(crate) fn skeleton_terraform_with_savings(text: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let s = skeleton_terraform(text);
    let after = tokenizer::estimate(&s);
    (s, crate::Savings::new(before, after))
}
