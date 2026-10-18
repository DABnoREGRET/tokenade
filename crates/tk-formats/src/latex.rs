//! LaTeX source compactor for `.tex` (and `.bib` if it ends up here
//! via misclassification — BibTeX has its own simpler shape that
//! the strip-comments-and-blanks pass handles correctly).
//!
//! Academic source documents often carry extensive `% comment`
//! lines (TODO markers, citation-key notes, "fix this paragraph"
//! reminders, citation key explanations), section banners, and
//! whitespace-heavy formatting choices. 20-45% drop on typical
//! commented `.tex` files.
//!
//! What it strips:
//!   * full-line `%` comments (LaTeX's only comment marker)
//!   * inline `%` comment tails — quote-aware about `\%` (literal
//!     percent sign that must NOT be treated as a comment marker)
//!   * blank-line runs → one max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every `\command{}` and `\command[opt]{required}` call
//!   * structural macros (`\section`, `\chapter`, `\paragraph`, …)
//!   * math (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`) — left untouched
//!     because removing intra-math whitespace risks changing
//!     rendering for some math packages
//!   * `\begin{env}…\end{env}` environments (preamble plus
//!     `figure`/`table`/`equation`/`align`/`verbatim`/`lstlisting`,
//!     …)
//!   * the entire `\begin{verbatim}…\end{verbatim}` AND
//!     `\begin{lstlisting}…\end{lstlisting}` body — comment
//!     stripping would corrupt code listings
//!
//! Trade-off note: LaTeX `\%` inside an argument like
//! `\caption{50\% accuracy}` survives the inline-strip pass thanks
//! to the `\\%` look-back.

/// Cheap heuristic detector. Requires ≥1 strong LaTeX signal
/// (`\documentclass`, `\begin{document}`, `\usepackage`, …) AND
/// at least 2 backslash-prefixed commands among first 30 non-blank
/// lines. Rejects HTML / XML start tags.
pub fn detect(text: &str) -> bool {
    let head: String = text.lines().take(40).collect::<Vec<_>>().join("\n");
    let trim = head.trim_start();
    if trim.starts_with("<!DOCTYPE") || trim.starts_with("<?xml") || trim.starts_with("<html") {
        return false;
    }
    let strong = head.contains("\\documentclass")
        || head.contains("\\begin{document}")
        || head.contains("\\usepackage")
        || head.contains("\\chapter{")
        || head.contains("\\section{");
    if !strong {
        return false;
    }
    let cmd_count = head
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(30)
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with('%') && t.contains('\\')
        })
        .count();
    cmd_count >= 2
}

/// Strip `%` comments + collapse blank-runs + trim trailing ws.
/// Code listings inside `verbatim` / `lstlisting` are passed through
/// untouched so the compactor never corrupts an included code block.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut in_verbatim: Option<&'static str> = None; // closing tag we're looking for
    for line in text.lines() {
        // Verbatim / lstlisting passthrough.
        if let Some(closer) = in_verbatim {
            out.push_str(line);
            out.push('\n');
            if line.contains(closer) {
                in_verbatim = None;
            }
            continue;
        }
        if line.contains("\\begin{verbatim}") {
            in_verbatim = Some("\\end{verbatim}");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.contains("\\begin{lstlisting}") {
            in_verbatim = Some("\\end{lstlisting}");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        // Full-line comment.
        if t.starts_with('%') {
            continue;
        }
        // Inline comment tail. Respect `\%` literal escape.
        let stripped = strip_inline_comment(trimmed);
        let stripped_t = stripped.trim_end();
        if stripped_t.is_empty() {
            continue;
        }
        blank_run = 0;
        out.push_str(stripped_t);
        out.push('\n');
    }
    out
}

/// `Savings`-yielding variant for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Strip a `%` comment tail. The character at `i` is a comment
/// start only if it is NOT preceded by an UNESCAPED backslash
/// (`\%` is a literal percent sign in LaTeX).
fn strip_inline_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut cut: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // Check for literal `\%` — but `\\%` (two backslashes)
            // is `\\` followed by `%`, so `%` IS a comment then.
            let mut backslashes = 0usize;
            let mut k = i;
            while k > 0 && bytes[k - 1] == b'\\' {
                backslashes += 1;
                k -= 1;
            }
            if backslashes.is_multiple_of(2) {
                cut = Some(i);
                break;
            }
        }
        i += 1;
    }
    match cut {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_TEX: &str = "% Document preamble\n% Generated 2026-05-29\n\\documentclass[oneside, 11pt]{article}\n\\usepackage[utf8]{inputenc}\n\\usepackage{graphicx}\n\\usepackage{hyperref}\n\n% Begin document\n\\begin{document}\n\n\\title{Foo}\n\\author{Bar}\n\\maketitle\n\n% Introduction section\n\\section{Introduction}\n\nWe achieved 95\\% accuracy on the benchmark.  % final result\nSee Figure~\\ref{fig:plot}.\n\n\\begin{verbatim}\n# This is code, % is not a comment here\nfor i in range(10):\n    print(i)\n\\end{verbatim}\n\n\\end{document}\n";

    #[test]
    fn detect_real_tex() {
        assert!(detect(REAL_TEX));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello world\nthis is just prose\n"));
        assert!(!detect("<!DOCTYPE html>\n"));
        assert!(!detect("just one \\command\\ in a sentence\n"));
    }

    #[test]
    fn compact_strips_full_line_comments() {
        let out = compact(REAL_TEX);
        assert!(!out.contains("Document preamble"));
        assert!(!out.contains("Generated 2026"));
        assert!(!out.contains("Begin document"));
        assert!(!out.contains("Introduction section"));
    }

    #[test]
    fn compact_strips_inline_comment_tails() {
        let out = compact(REAL_TEX);
        assert!(!out.contains("final result"));
        assert!(out.contains("We achieved 95"));
    }

    #[test]
    fn compact_preserves_literal_percent() {
        // `95\%` is "95 percent" in LaTeX — `\%` is an escaped %.
        let out = compact(REAL_TEX);
        assert!(
            out.contains("95\\%"),
            "literal `\\%` must survive the inline-comment pass, got:\n{out}"
        );
    }

    #[test]
    fn compact_preserves_verbatim_block() {
        let out = compact(REAL_TEX);
        // The `# This is code, % is not a comment here` comment
        // sits INSIDE verbatim and must survive verbatim.
        assert!(out.contains("# This is code, % is not a comment here"));
    }

    #[test]
    fn compact_preserves_commands_and_structure() {
        let out = compact(REAL_TEX);
        assert!(out.contains("\\documentclass[oneside, 11pt]{article}"));
        assert!(out.contains("\\usepackage[utf8]{inputenc}"));
        assert!(out.contains("\\begin{document}"));
        assert!(out.contains("\\title{Foo}"));
        assert!(out.contains("\\section{Introduction}"));
        assert!(out.contains("\\begin{verbatim}"));
        assert!(out.contains("\\end{verbatim}"));
        assert!(out.contains("\\end{document}"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "\\section{X}\n\n\n\n\\section{Y}\n";
        let out = compact(input);
        assert_eq!(out, "\\section{X}\n\n\\section{Y}\n");
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        let (_, sav) = compact_with_savings(REAL_TEX);
        assert!(
            sav.percent() >= 15.0,
            "expected ≥15% on commented LaTeX, got {:.1}%",
            sav.percent()
        );
    }
}
