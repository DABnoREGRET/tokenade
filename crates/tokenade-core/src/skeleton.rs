//! **Rendering a file by its STRUCTURE rather than by its first lines.**
//!
//! A skeleton keeps the signatures and folds the bodies; a YAML sketch keeps
//! the keys and folds the subtrees; a markdown outline keeps the heading
//! hierarchy. `structural_truncate` picks which of the three answers, and
//! falls back to the head/tail cut when no shape is recognised.
//!
//! **Why this is no longer in `compress`.** It used to be, and that is what
//! made the FOLD layer reach up to the CODE layer: the precise skeleton asks
//! `treesitter` for the spans of a syntax tree, and `index` for its
//! per-language declaration patterns when no grammar knows the extension (73
//! extensions there versus 16 grammars here). It also asks `markdown` to
//! recognise a document. Three levels above the fold, from the fold.
//!
//! Here those calls point downward, and `compress` keeps what really is a
//! primitive: deduplication, cutting, list folding.
//!
//! The fallback stays written: with no grammar AND no language pattern, the
//! skeleton is built line by line. That is what makes this module safe to
//! move — it never assumed the parser would answer.

use crate::tokenizer;

/// Minimum number of elided lines before [`skeleton`] / [`skeleton_markdown`]
/// append their one-line actionable recovery footer. Below this the inline
/// `// … N lines …` markers already signal the loss, the file is small enough
/// that a re-Read is cheap, and a verbose footer would invert the savings.
pub(crate) const SKELETON_FOOTER_MIN_LINES: usize = 12;

/// Hard per-skeleton token budget (#2). A skeleton is a navigation aid,
/// not a payload: past this point a targeted `tokenade query` or an
/// offset/limit Read is cheaper than scrolling more signatures. We stop
/// at the next line boundary once the running estimate crosses this and
/// report how many declarations were dropped so they stay recoverable.
/// 8 files × 500 = 4k < the 5k per-output disclosure threshold, so a
/// multi-file skeleton call still returns inline rather than stashing.
pub(crate) const SKELETON_MAX_TOKENS: usize = 500;

/// Produce a "skeleton" of source code: keep lines that look like declarations,
/// signatures, imports, and braces; replace contiguous body lines with a
/// `// … N lines …` marker. Language-agnostic, heuristic, lossy by design.
pub fn skeleton(text: &str) -> String {
    skeleton_for_ext(text, None)
}

/// The line comment marker for `ext`, used for the `… N lines …` elision.
///
/// The marker was hardcoded to `//`, so every skeleton of a `#`-comment
/// language shipped a line that is not a comment in that language:
/// `    // … 13 lines …` in the middle of a Python file. The AST path never
/// hit it (it has its own renderer); the LINE-BASED fallback is what every
/// language without a registered grammar goes through, which is exactly the
/// set most likely to be `#`-commented. Callers already carry `ext` — it was
/// simply not threaded down to the one place that needed it.
pub(crate) fn line_comment_for(ext: Option<&str>) -> &'static str {
    match ext
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .as_deref()
    {
        Some(
            "py" | "rb" | "sh" | "bash" | "zsh" | "fish" | "pl" | "r" | "yml" | "yaml" | "toml"
            | "cfg" | "ini" | "conf" | "tf" | "nix" | "ex" | "exs" | "jl" | "cmake" | "mk" | "make"
            | "dockerfile" | "gitignore",
        ) => "#",
        Some("lua" | "hs" | "elm" | "sql" | "ada") => "--",
        Some("lisp" | "clj" | "cljs" | "el" | "scm") => ";;",
        Some("vim") => "\"",
        _ => "//",
    }
}

/// [`skeleton`] that knows the language, so the elision marker is a comment
/// in it. `None` keeps the historical `//`.
pub fn skeleton_for_ext(text: &str, ext: Option<&str>) -> String {
    let (entries, hidden_total) = skeleton_entries_linebased(text, line_comment_for(ext));
    render_skeleton(entries, hidden_total)
}

/// AST-driven skeleton (#1). Uses real tree-sitter parse spans
/// ([`crate::treesitter::skeleton_spans`]) to keep exactly the
/// declaration headers + import statements and fold everything else —
/// no `// fn foo` comment or `"class Bar"` string literal is ever kept,
/// and a signature wrapping several physical lines is kept whole. Falls
/// back to the line-based [`skeleton`] heuristic when no grammar is
/// registered for `ext` (or the parse trips its timeout). Either way the
/// result is bounded to [`SKELETON_MAX_TOKENS`].
pub fn skeleton_ext(text: &str, ext: Option<&str>) -> String {
    skeleton_ext_budget(text, ext, SKELETON_MAX_TOKENS)
}

/// [`skeleton_ext`] with an explicit render budget (`usize::MAX` = never
/// truncate). See `render_skeleton_budget` for why the read-view needs it.
pub fn skeleton_ext_budget(text: &str, ext: Option<&str>, budget: usize) -> String {
    if let Some(ext) = ext {
        if let Some((spans, test_bodies)) =
            crate::treesitter::skeleton_spans_and_test_bodies(text, ext)
        {
            if !spans.is_empty() {
                // A skeleton answers "what does this file offer?". On a
                // well-tested source file the test module holds most of the
                // signatures, so the answer arrived buried in the proof:
                // measured on one of ours, 46 of 78 signatures were tests.
                // The module's own header stays, and the lines dropped here
                // land in the existing "… N lines …" marker, so nothing goes
                // missing without saying so.
                let kept: Vec<(usize, usize)> = spans
                    .into_iter()
                    .filter(|(s, _)| !test_bodies.iter().any(|(a, b)| s >= a && s <= b))
                    .collect();
                let (entries, hidden_total) =
                    skeleton_entries_from_spans(text, &kept, line_comment_for(Some(ext)));
                return render_skeleton_budget(entries, hidden_total, budget);
            }
        }
    }
    // No grammar for this extension — but the SYMBOL INDEX may still know the
    // language: 73 extensions there against 16 grammars here. Driving the
    // skeleton from its per-language declaration patterns keeps the real
    // signature lines instead of whatever the language-agnostic heuristic
    // guesses.
    if let Some(ext) = ext {
        if let Some(rows) = crate::index::declaration_rows(ext, text) {
            if !rows.is_empty() {
                let spans: Vec<(usize, usize)> = rows.into_iter().map(|r| (r, r)).collect();
                let (entries, hidden_total) =
                    skeleton_entries_from_spans(text, &spans, line_comment_for(Some(ext)));
                let from_spec = render_skeleton_budget(entries, hidden_total, budget);
                // Take whichever is SHORTER. The spec-driven view is more
                // precise — real declarations, not declaration-looking lines —
                // but a file whose declarations are scattered pays a fold
                // marker per gap, and on Clojure and Lua that came out several
                // times BIGGER than the heuristic it replaced. Measured
                // 2026-08-28 across ten grammar-less languages: the spec wins
                // 257 files to 33, and this comparison is what makes the 33
                // cost nothing. No language can regress.
                let from_heuristic = skeleton_for_ext(text, Some(ext));
                return if from_heuristic.len() < from_spec.len() {
                    from_heuristic
                } else {
                    from_spec
                };
            }
        }
    }
    skeleton(text)
}

/// One skeleton line plus whether it begins a kept declaration (used to
/// count "+N more symbol(s)" when the [`SKELETON_MAX_TOKENS`] cap trims
/// the tail). Fold markers and brace-only lines carry `false`.
type SkelEntry = (String, bool);

/// Build skeleton entries the line-based heuristic way (fallback path
/// and the back-compat [`skeleton`] entrypoint).
/// Lines that belong to the BODY of a type declaration, which the skeleton must
/// keep.
///
/// A function's body is implementation and eliding it is the whole point. A
/// **type's body is its signature**: `pub struct Plan {` followed by
/// `// … 17 lines …` tells the reader a type exists and nothing whatever about
/// it, so anyone who needs the shape has to open the file — which costs far
/// more than the fields would have. Same for `enum` (its variants ARE the type)
/// and `trait`/`interface` (method signatures, not bodies).
///
/// `impl` blocks are excluded: those hold implementations.
///
/// Brace-counted, so it simply never fires on Python — where a `class` body is
/// methods, i.e. implementation, and eliding it is right.
///
/// Capped: a generated 400-variant enum is a listing, not a signature. Past the
/// cap the remaining lines fold as usual.
///
/// Returns `Keep` for a member line, `Swallow` for a doc comment inside the
/// body — dropped SILENTLY, because a `// … 1 lines …` marker between two
/// fields costs tokens to announce a comment nobody asked for — and `Outside`
/// everywhere else.
#[derive(Clone, Copy, PartialEq)]
enum TypeBody {
    Outside,
    Keep,
    Swallow,
}

fn type_body_lines(text: &str) -> Vec<TypeBody> {
    const MAX_KEPT_PER_TYPE: usize = 40;
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = vec![TypeBody::Outside; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let head =
            t.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '(' || c == ')');
        let _ = head;
        let opens_type = ["struct", "enum", "trait", "interface"].iter().any(|kw| {
            t.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == *kw)
        }) && t.trim_end().ends_with('{')
            // `impl Trait for X {` names a trait but declares no type.
            && !t.split_whitespace().next().is_some_and(|w| w == "impl");
        if !opens_type {
            i += 1;
            continue;
        }
        let mut depth = 0i32;
        let mut kept = 0usize;
        let mut j = i;
        while j < lines.len() {
            for c in lines[j].chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if j > i {
                let b = lines[j].trim();
                let is_doc = b.starts_with("///") || b.starts_with("//!") || b.starts_with("//");
                if is_doc {
                    keep[j] = TypeBody::Swallow;
                } else if !b.is_empty() && kept < MAX_KEPT_PER_TYPE {
                    keep[j] = TypeBody::Keep;
                    kept += 1;
                }
            }
            if depth <= 0 {
                break;
            }
            j += 1;
        }
        i = j.max(i + 1);
    }
    keep
}

mod formats;
pub use formats::*;

fn skeleton_entries_linebased(text: &str, comment: &str) -> (Vec<SkelEntry>, usize) {
    let mut out: Vec<SkelEntry> = Vec::new();
    let mut hidden = 0usize;
    let mut hidden_total = 0usize;
    let flush = |out: &mut Vec<SkelEntry>, hidden: &mut usize, total: &mut usize| {
        if *hidden > 0 {
            out.push((crate::elision::body_marker(comment, *hidden), false));
            *total += *hidden;
            *hidden = 0;
        }
    };
    let type_body = type_body_lines(text);
    for (idx, line) in text.lines().enumerate() {
        let body = type_body.get(idx).copied().unwrap_or(TypeBody::Outside);
        if is_structural(line) || body == TypeBody::Keep {
            flush(&mut out, &mut hidden, &mut hidden_total);
            out.push((line.to_string(), is_decl_start(line)));
        } else if line.trim().is_empty() || body == TypeBody::Swallow {
            // swallow
        } else {
            hidden += 1;
        }
    }
    flush(&mut out, &mut hidden, &mut hidden_total);
    (out, hidden_total)
}

/// Build skeleton entries from tree-sitter line spans (1-based, inclusive).
/// Lines inside any span are kept; the first line of each span is flagged
/// as a declaration start for the cap's symbol count. Everything else is
/// folded into `// … N lines …` markers.
fn skeleton_entries_from_spans(
    text: &str,
    spans: &[(usize, usize)],
    comment: &str,
) -> (Vec<SkelEntry>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let mut kept = vec![false; n];
    let mut starts = vec![false; n];
    for &(s, e) in spans {
        if s >= 1 && s <= n {
            starts[s - 1] = true;
        }
        for ln in s..=e {
            if ln >= 1 && ln <= n {
                kept[ln - 1] = true;
            }
        }
    }
    let type_body = type_body_lines(text);
    for (i, k) in kept.iter_mut().enumerate() {
        if type_body.get(i).copied().unwrap_or(TypeBody::Outside) == TypeBody::Keep {
            *k = true;
        }
    }
    let mut out: Vec<SkelEntry> = Vec::new();
    let mut hidden = 0usize;
    let mut hidden_total = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if kept[i] {
            if hidden > 0 {
                out.push((crate::elision::body_marker(comment, hidden), false));
                hidden_total += hidden;
                hidden = 0;
            }
            out.push((line.to_string(), starts[i]));
        } else if line.trim().is_empty()
            || type_body.get(i).copied().unwrap_or(TypeBody::Outside) == TypeBody::Swallow
        {
            // swallow
        } else {
            hidden += 1;
        }
    }
    if hidden > 0 {
        out.push((crate::elision::body_marker(comment, hidden), false));
        hidden_total += hidden;
    }
    (out, hidden_total)
}

/// Render skeleton entries to a string, enforcing the per-skeleton token
/// budget (#2). Accumulates each line's estimate; once the running total
/// would cross [`SKELETON_MAX_TOKENS`] (and at least one line is emitted)
/// it stops at that line boundary and appends a footer naming how many
/// declarations remain unshown. When the cap is never hit it falls back
/// to the signatures-only recovery footer once enough body lines elided.
fn render_skeleton(entries: Vec<SkelEntry>, hidden_total: usize) -> String {
    render_skeleton_budget(entries, hidden_total, SKELETON_MAX_TOKENS)
}

/// Same, with an explicit token budget. The 500-token default fits the CLI
/// `skeleton` command (quick orientation); the read-view needs EVERY symbol
/// (a hidden symbol there = silently unfindable code) so it passes
/// `usize::MAX`.
fn render_skeleton_budget(entries: Vec<SkelEntry>, hidden_total: usize, budget: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut running = 0usize;
    let mut cut_at: Option<usize> = None;
    for (idx, (line, _)) in entries.iter().enumerate() {
        let cost = tokenizer::estimate(line) + 1;
        if !out.is_empty() && running + cost > budget {
            cut_at = Some(idx);
            break;
        }
        running += cost;
        out.push(line.clone());
    }
    if let Some(at) = cut_at {
        let remaining = entries[at..].iter().filter(|(_, decl)| *decl).count();
        out.push(format!(
            "// [{SKELETON_TRUNCATED_MARKER} at ~{} tokens; +{} more symbol(s) hidden; \
             Read the file with offset/limit, or `tokenade query \"<name>\"` for a specific declaration]",
            budget, remaining
        ));
    } else if hidden_total >= SKELETON_FOOTER_MIN_LINES {
        // #1 — actionable footer so the agent knows the bodies are
        // recoverable and how (signatures-only by design). Only once the
        // elided volume amortises the footer's own size.
        out.push(format!(
            "// [tokenade skeleton — {hidden_total} body line(s) replaced by `// … N lines …` markers; \
             Read the file with offset/limit, or `tokenade query \"<name>\"` for a declaration's full body]"
        ));
    }
    out.join("\n")
}

/// Does this kept line begin a declaration (for the cap's symbol count)?
/// True for signature/keyword lines; false for bare braces and the
/// attribute/decorator lines that merely precede a declaration.
fn is_decl_start(line: &str) -> bool {
    let t = line.trim();
    if t == "}" || t.starts_with("} ") || t == "{" {
        return false;
    }
    if t.starts_with(['#', '@']) {
        return false;
    }
    is_structural(line)
}

fn is_structural(line: &str) -> bool {
    let t = line.trim_start();
    // Attribute / decorator lines precede declarations — always keep them.
    if t.starts_with(['#', '@']) {
        return true;
    }
    // Strip leading visibility / modifier keywords (incl. Rust `pub(crate)`)
    // so e.g. `pub(crate) async fn foo` still matches the `fn` keyword below.
    let mut head = t;
    loop {
        let stripped = [
            "pub(crate)",
            "pub(super)",
            "pub(self)",
            "pub",
            "async",
            "static",
            "public",
            "private",
            "protected",
            "export",
            "default",
            "abstract",
            "final",
            "const",
            "unsafe",
            "extern",
        ]
        .iter()
        .find_map(|m| {
            head.strip_prefix(m)
                .map(|r| r.trim_start_matches(['(', ')', ' ']))
        });
        match stripped {
            Some(rest) if rest.len() < head.len() => head = rest,
            _ => break,
        }
    }
    const KEYS: &[&str] = &[
        "fn ",
        "def ",
        "class ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "interface ",
        "type ",
        "func ",
        "function ",
        "import ",
        "from ",
        "use ",
        "#include",
        "package ",
        "module ",
    ];
    KEYS.iter().any(|k| head.starts_with(k)) || t.ends_with('{') || t == "}" || t.starts_with("} ")
}

/// Convenience: compress and report the savings (line-based, no ext).
pub fn skeleton_with_savings(text: &str) -> (String, crate::Savings) {
    skeleton_with_savings_ext(text, None)
}

/// Like [`skeleton_with_savings`] but uses the AST skeleton ([`skeleton_ext`])
/// when an extension with a registered grammar is supplied. This is the
/// entrypoint the CLI uses so real source files get tree-sitter precision
/// and the [`SKELETON_MAX_TOKENS`] bound; unknown extensions fall back to
/// the line-based heuristic.
pub fn skeleton_with_savings_ext(text: &str, ext: Option<&str>) -> (String, crate::Savings) {
    skeleton_with_savings_ext_for(text, ext, 1)
}

/// Opening of the footer a skeleton carries when [`SKELETON_PER_FILE_CEILING`]
/// (or a batch's share of [`SKELETON_CALL_BUDGET`]) cut it short. One
/// spelling, so a caller that has to ASK whether its answer was cut cannot
/// drift from the renderer that writes it — see [`skeleton_was_truncated`].
pub const SKELETON_TRUNCATED_MARKER: &str = "tokenade skeleton — truncated";

/// Was this rendered skeleton cut short? `skeleton`'s `--budget` diagnostic
/// used to state, on stderr, that "the one target you asked for is always
/// shown in full" — printed one line under a footer reading "truncated at
/// ~2500 tokens; +8 more symbol(s) hidden". Two different bounds: `--budget`
/// chooses WHICH targets are shown, and the per-file ceiling bounds how much
/// of one. Conflating them made the tool contradict its own output.
pub fn skeleton_was_truncated(rendered: &str) -> bool {
    rendered.contains(SKELETON_TRUNCATED_MARKER)
}

/// Ceiling a whole `skeleton` call may spend, batch included — the number
/// [`SKELETON_MAX_TOKENS`] was derived from (8 files × 500), kept explicit
/// so the split below cannot drift from it.
pub(crate) const SKELETON_CALL_BUDGET: usize = 4_000;

/// Most a single file may take even when it is the only one asked for. Past
/// this a targeted `query` really is the cheaper move.
pub(crate) const SKELETON_PER_FILE_CEILING: usize = 2_500;

/// [`skeleton_with_savings_ext`] told how many files this ONE call covers.
///
/// The 500-token bound was sized for a batch of eight, then applied to every
/// call — so asking for a single file paid the price of asking for eight, and
/// on this repository 81 of 187 files came back truncated, median 25
/// declarations hidden, under a footer recommending a full Read: the
/// expensive path the command exists to avoid. Splitting the SAME call budget
/// across the files actually requested keeps the batch invariant exactly
/// where it was and stops charging one file for seven it did not ask about.
pub fn skeleton_with_savings_ext_for(
    text: &str,
    ext: Option<&str>,
    files_in_call: usize,
) -> (String, crate::Savings) {
    let budget = (SKELETON_CALL_BUDGET / files_in_call.max(1))
        .clamp(SKELETON_MAX_TOKENS, SKELETON_PER_FILE_CEILING);
    let before = tokenizer::estimate(text);
    let s = skeleton_ext_budget(text, ext, budget);
    let after = tokenizer::estimate(&s);
    (s, crate::Savings::new(before, after))
}

/// Structure-preserving replacement for positional head/tail truncation
/// (#26). Head/tail keeps the first and last N lines and throws the MIDDLE
/// away — which, on a doc or an unrecognised source file, is exactly where
/// the substance lives (the buried section, the declaration past the license
/// header). This picks a shape that survives that loss:
///
///   1. Markdown / prose (`.md`-family extension OR heading-density) →
///      [`crate::markdown::outline`] (heading hierarchy + per-section preview).
///   2. Code-like (high density of declaration lines) → [`skeleton`]
///      (signatures kept, bodies folded as `// … N lines …`).
///   3. Otherwise (genuinely flat logs / unstructured data) → positional
///      head/tail via [`crate::compress::generic`].
///
/// Returns the first candidate that actually shrinks the input; the chosen
/// `Savings` lets the caller decide whether the substitution was worth it.
pub fn structural_truncate(text: &str, file_path: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let doc_ext = matches!(
        ext.as_deref(),
        Some("md" | "markdown" | "mdx" | "rst" | "txt" | "text" | "adoc" | "org")
    );

    // 1. Markdown / prose docs → outline (TOC + per-section preview).
    if doc_ext || crate::markdown::looks_like_markdown(text) {
        let outline = crate::markdown::outline(text);
        let after = tokenizer::estimate(&outline);
        if after < before {
            return (outline, crate::Savings::new(before, after));
        }
    }

    // 2. Code-like → skeleton. Guarded on declaration density so prose
    //    (where almost nothing is "structural") is never skeleton-ised into
    //    near-nothing — that case falls through to head/tail below.
    if structural_density(text) >= 0.15 {
        let (sk, sav) = skeleton_with_savings_ext(text, ext.as_deref());
        if sav.tokens_after < before {
            return (sk, sav);
        }
    }

    // 3. Flat / unstructured → positional head/tail (last resort).
    let trunc = crate::compress::generic(text);
    let after = tokenizer::estimate(&trunc);
    (trunc, crate::Savings::new(before, after))
}

/// Fraction of non-blank lines that look like declarations/structure.
/// Drives the [`skeleton`] guard in [`structural_truncate`]: high on code,
/// near-zero on prose.
fn structural_density(text: &str) -> f32 {
    let mut total = 0usize;
    let mut structural = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if is_structural(line) {
            structural += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    structural as f32 / total as f32
}

/// Configuration knob for the non-code skeleton variants below.
const YAML_ARRAY_CAP: usize = 5;

#[cfg(test)]
include!("skeleton_tests.rs");
