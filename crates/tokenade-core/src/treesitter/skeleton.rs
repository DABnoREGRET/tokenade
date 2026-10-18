//! **Keep the signatures, drop the bodies.**
//!
//! A file's skeleton is what an agent reads to know what it contains without
//! reading it. The difficulty is not finding the signatures, it is knowing
//! where the header ends and the body begins — and recognising a test module,
//! whose signatures answer none of the questions one asks a skeleton.

use super::*;

/// Line spans (1-based, inclusive) to KEEP for a structural skeleton:
/// the *header* of each declaration (decorators + signature, up to but
/// not including the body) plus whole import statements. Everything
/// outside the returned spans is body/noise the caller folds into
/// `// … N lines …` markers.
///
/// This is the AST-driven replacement for the line-based
/// `compress::is_structural` heuristic. Because spans come from real
/// parse nodes, a `// fn foo` in a comment or `"class Bar"` in a string
/// literal is never kept, and a signature wrapping several physical
/// lines is kept *whole* — the two failure modes of the regex skeleton.
///
/// Returns `None` when no grammar is registered for `ext` (caller falls
/// back to the regex skeleton) or the parse trips its timeout.
pub fn skeleton_spans_and_test_bodies(
    source: &str,
    ext: &str,
) -> Option<(Vec<(usize, usize)>, Vec<(usize, usize)>)> {
    let spec = lang_for_ext(ext)?;
    let lang = (spec.language)();
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_micros(parse_timeout_micros(source.len()));
    let mut on_progress = |_state: &tree_sitter::ParseState| {
        if std::time::Instant::now() > deadline {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut on_progress);
    let src_bytes = source.as_bytes();
    let mut feed = |offset: usize, _pt: tree_sitter::Point| -> &[u8] {
        src_bytes.get(offset..).unwrap_or(&[])
    };
    let tree = parser.parse_with_options(&mut feed, None, Some(options))?;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    collect_skeleton_spans(tree.root_node(), src_bytes, spec, &mut spans, 0);
    spans.sort_unstable();
    let mut tests: Vec<(usize, usize)> = Vec::new();
    collect_test_module_bodies(tree.root_node(), src_bytes, spec, &mut tests, 0);
    tests.sort_unstable();
    Some((spans, tests))
}

/// Declaration header spans only — what a stack frame resolves against.
pub fn skeleton_spans(source: &str, ext: &str) -> Option<Vec<(usize, usize)>> {
    skeleton_spans_and_test_bodies(source, ext).map(|(spans, _)| spans)
}

/// Body row ranges of test-only modules (Rust `#[cfg(test)] mod …`), header
/// excluded. The skeleton folds these away: on a well-tested source file the
/// tests are the majority of the signatures, and someone asking what a file
/// OFFERS is not asking what proves it. They stay in `skeleton_spans` so a
/// panic inside a test still resolves to the test's own signature, and stay
/// in the symbol index so a test remains findable by name.
///
/// Empty for every language whose grammar has no such marker, which is the
/// honest answer rather than a guess about naming conventions.
/// How deep the AST walks descend before they stop.
///
/// Every walk here is recursive, one frame per AST node, and tree-sitter's
/// own parser is NOT — it handles nesting our stack cannot. A generated
/// TypeScript union of 18 000 members (`"a4b:ApproveSkill" | "a4b:…" | …`,
/// 690 KB, in `alchemy-effect/packages/alchemy/src/AWS/IAM/actions.generated.ts`)
/// parses into ~18 000 nested `union_type` nodes, and `tokenade map` on a
/// repository holding that ONE file died with SIGSEGV — not a missed symbol,
/// a crash of the whole command, which no `Result` upstream can catch.
///
/// Generated files of that shape are ordinary in TypeScript: SDK action
/// unions, GraphQL codegen, protobuf output.
///
/// 500 is far past anything hand-written — measured over the corpus, the
/// deepest tree in a non-generated file is well inside it — and far inside
/// the smallest stack these walks run on (a rayon worker's 2 MiB, during a
/// parallel index build, in a debug build where the frames are largest).
/// What is lost past the cap is a symbol or call nested 500 levels deep;
/// the declaration that CONTAINS such a monster sits at depth 2 and is kept.
pub(super) const MAX_AST_DEPTH: usize = 500;

pub(super) fn collect_test_module_bodies(
    node: Node<'_>,
    src: &[u8],
    spec: &LangSpec,
    out: &mut Vec<(usize, usize)>,
    depth: usize,
) {
    if depth >= MAX_AST_DEPTH {
        return;
    }
    if spec.modules.contains(&node.kind()) && is_test_only_module(node, src) {
        let header_end = header_end_row(node, src);
        let end = node.end_position().row;
        if end > header_end {
            out.push((header_end + 2, end + 1));
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_test_module_bodies(child, src, spec, out, depth + 1);
    }
}

/// Is this module gated on `cfg(test)`? The attributes precede the module as
/// sibling `attribute_item` nodes in tree-sitter-rust.
pub(super) fn is_test_only_module(node: Node<'_>, src: &[u8]) -> bool {
    let mut prev = node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let attr: String = text(p, src).split_whitespace().collect();
                if attr.contains("cfg(test)") {
                    return true;
                }
                prev = p.prev_sibling();
            }
            "line_comment" | "block_comment" => prev = p.prev_sibling(),
            _ => return false,
        }
    }
    false
}

/// Is this node declared at module scope — outside every type and function?
///
/// The index asks the same question with `current_caller == "<module>"`;
/// here the tree is walked directly because the skeleton visits nodes rather
/// than tracking a caller.
pub(super) fn at_module_scope(node: Node<'_>, spec: &LangSpec) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        let k = p.kind();
        if spec.functions.contains(&k) || spec.types.contains(&k) {
            return false;
        }
        cur = p.parent();
    }
    true
}

/// Best-effort one-line signature of the declaration that *encloses* the
/// 1-indexed `line` in `source` (`ext` selects the language). Used by the
/// stack-trace compactor to inline the signature of a throwing frame's
/// function so the agent doesn't have to open the file.
///
/// Heuristic: among the header spans (see [`skeleton_spans`], which are the
/// signature lines of each declaration — body excluded), pick the one whose
/// start row is the GREATEST that is still `<= line`. For a frame line (which
/// points inside a function body) that resolves to the nearest preceding
/// declaration header, i.e. the enclosing function — top-level data/imports
/// sit above it and later functions start after the line, so neither wins.
///
/// Returns `None` for unsupported languages, an unparseable file, or when no
/// declaration precedes the line. Multi-line signatures are collapsed to one
/// line and length-capped.
pub fn signature_enclosing_line(source: &str, ext: &str, line: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    let spans = skeleton_spans(source, ext)?;
    let (start, end) = spans
        .into_iter()
        .filter(|(s, _)| *s <= line)
        .max_by_key(|(s, _)| *s)?;
    let lines: Vec<&str> = source.lines().collect();
    if start == 0 || end > lines.len() || start > end {
        return None;
    }
    // Collapse the (possibly multi-line) header into a single squeezed line.
    let sig = lines[start - 1..end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let sig = sig.split_whitespace().collect::<Vec<_>>().join(" ");
    // Strip a trailing opening brace so `fn foo() -> T {` reads `fn foo() -> T`.
    let sig = sig.trim_end_matches('{').trim_end().to_string();
    if sig.is_empty() {
        return None;
    }
    const CAP: usize = 200;
    if sig.chars().count() > CAP {
        let cut: String = sig.chars().take(CAP).collect();
        Some(format!("{cut}…"))
    } else {
        Some(sig)
    }
}

pub(super) fn collect_skeleton_spans(
    node: Node<'_>,
    src: &[u8],
    spec: &LangSpec,
    spans: &mut Vec<(usize, usize)>,
    depth: usize,
) {
    if depth >= MAX_AST_DEPTH {
        return;
    }
    let kind = node.kind();
    if spec.imports.contains(&kind) {
        // Imports are short — keep the whole statement (handles multi-line
        // `use a::{\n  b,\n  c,\n};` Rust groups too).
        spans.push((node.start_position().row + 1, node.end_position().row + 1));
    } else if spec.functions.contains(&kind)
        || spec.types.contains(&kind)
        || spec.modules.contains(&kind)
    {
        let start = header_start_row(node);
        let end = header_end_row(node, src);
        spans.push((start + 1, end + 1));
    } else if spec.members.contains(&kind) && at_module_scope(node, spec) {
        // A module-scope `const` / `static` / `#define` declares a name and
        // has no body to elide — it IS a signature. The index has recorded
        // them all along, so `query MIN_INPUT` answered while `skeleton` on
        // the same file did not show the line: two doors, one file, two
        // answers. Measured 2026-08-27 over real sources: 36 of 324
        // top-level Rust declarations and 60 of 86 C ones were missing from
        // their skeleton, `semantic.c` losing 21 of its 22 `#define`.
        //
        // MODULE SCOPE only: a struct field and an enum variant are the same
        // `members` kinds, and pulling those out would reprint the body of
        // every type the skeleton deliberately elides to its header.
        spans.push((node.start_position().row + 1, node.end_position().row + 1));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_skeleton_spans(child, src, spec, spans, depth + 1);
    }
}

/// Row where a declaration's kept header begins. Decorators/annotations
/// that precede a `def`/`class` live in a wrapping `decorated_definition`
/// parent (Python) — pull the span start up to include them. TS/Java/etc.
/// attach decorators as children of the node itself, so `node.start_row`
/// already covers them.
pub(super) fn header_start_row(node: Node<'_>) -> usize {
    if let Some(parent) = node.parent() {
        if parent.kind() == "decorated_definition" {
            return parent.start_position().row;
        }
    }
    node.start_position().row
}

/// Row where a declaration's kept header ends — i.e. the last line that
/// is signature rather than body. For brace-delimited languages this is
/// the line bearing the opening `{`; for colon/indent bodies (Python)
/// it's the line *before* the first body statement. Declarations with no
/// body node (type aliases, abstract/trait signatures, field-less
/// structs) keep their whole (usually single-line) span.
pub(super) fn header_end_row(node: Node<'_>, src: &[u8]) -> usize {
    let start = node.start_position().row;
    match body_node(node) {
        Some(body) => {
            let body_row = body.start_position().row;
            let opens_brace = src.get(body.start_byte()).copied() == Some(b'{');
            if opens_brace {
                body_row
            } else {
                body_row.saturating_sub(1).max(start)
            }
        }
        None => node.end_position().row,
    }
}

/// Find the node that holds a declaration's body, so the header can be
/// kept and the body folded. Prefers the grammar's `body` field; falls
/// back to the first child whose kind names a block / member list.
pub(super) fn body_node(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(b) = node.child_by_field_name("body") {
        return Some(b);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let k = child.kind();
        if k == "block"
            || k == "statement_block"
            || k == "field_declaration_list"
            || k == "declaration_list"
            || k == "enum_variant_list"
            || k == "class_body"
            || k == "enum_body"
            || k == "interface_body"
            || k == "struct_body"
            || k.ends_with("_body")
        {
            return Some(child);
        }
    }
    None
}
