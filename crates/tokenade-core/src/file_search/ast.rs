//! **Cut on STRUCTURE rather than on lines.**
//!
//! A function is a unit of meaning, a 40-line slice is not. When a grammar is
//! available, the chunks follow the nodes.
//!
//! Two bounds make this workable: a size floor, below which a node is merged
//! with its neighbour rather than producing a one-line chunk, and a recursion
//! ceiling, because a hostile file is an arbitrarily deep tree.

use super::*;

/// Convenience wrapper: detect language from path *and* content, then
/// chunk. Falls back to [`chunk_lines`] only when the language is
/// genuinely unknown — previously that meant "unknown extension",
/// which silently dropped `Rakefile`, extensionless scripts, and code
/// blocks extracted from web pages or PDFs onto a blind 12-line window.
/// **DECLARED RESERVE**, like this whole module: the `file-search` verb no
/// longer exists and nothing writes to the store (see the `file_search` header).
#[allow(dead_code)]
pub(crate) fn chunk_file(path: &std::path::Path, text: &str) -> Vec<(usize, usize, String)> {
    match detect_language(path, text) {
        Some(lang) => chunk_ast(text, lang),
        None => chunk_lines(text),
    }
}

/// AST-aware chunker (L114). Recursively walks
/// the tree-sitter parse, packing sibling nodes together until they
/// reach `AST_CHUNK_BYTES`. Each resulting chunk respects function /
/// class / module boundaries, so a 200-line function never gets split
/// across chunks A and B.
///
/// Returns `(start_line, end_line, snippet)` tuples — same shape as
/// [`chunk_lines`] so the caller can route either path uniformly.
///
/// Falls back to [`chunk_lines`] when no parser is registered for the
/// requested language.
pub fn chunk_ast(text: &str, language: &str) -> Vec<(usize, usize, String)> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let Some(lang) = parser_for_language(language) else {
        return chunk_lines(text);
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&lang).is_err() {
        return chunk_lines(text);
    }
    let Some(tree) = parser.parse(text.as_bytes(), None) else {
        return chunk_lines(text);
    };
    let root = tree.root_node();
    let raw = merge_node(&root, AST_CHUNK_BYTES, 0);
    let merged = merge_adjacent(&raw, AST_CHUNK_BYTES);
    // Convert byte-offsets back to (start_line, end_line, snippet).
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(merged.len());
    for (start, end) in merged {
        let snippet = std::str::from_utf8(&bytes[start..end])
            .unwrap_or("")
            .to_string();
        let start_line = bytes[..start].iter().filter(|&&b| b == b'\n').count() + 1;
        let end_line = bytes[..end].iter().filter(|&&b| b == b'\n').count() + 1;
        out.push((start_line, end_line, snippet));
    }
    out
}

/// Resolve `language` (lowercase identifier — `"rust"`, `"python"`,
/// `"javascript"`, etc.) to a tree-sitter `Language`. Returns `None`
/// for unsupported languages; the caller falls back to line chunking.
pub(crate) fn parser_for_language(language: &str) -> Option<tree_sitter::Language> {
    match language.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" | "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" | "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "typescript" | "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "c++" | "cc" | "cxx" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "csharp" | "cs" | "c#" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        "ruby" | "rb" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "swift" => Some(tree_sitter_swift::LANGUAGE.into()),
        "kotlin" | "kt" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        "scala" => Some(tree_sitter_scala::LANGUAGE.into()),
        "html" => Some(tree_sitter_html::LANGUAGE.into()),
        "css" => Some(tree_sitter_css::LANGUAGE.into()),
        _ => None,
    }
}

pub(crate) const AST_RECURSION_LIMIT: usize = 500;

/// Recursively turn an AST node into byte-range chunks. Mirrors
/// Merge pass: walks children left-to-right and
/// merges siblings until the running length hits `desired`. Children
/// bigger than `desired` recurse one level deeper.
pub(crate) fn merge_node(
    node: &tree_sitter::Node,
    desired: usize,
    depth: usize,
) -> Vec<(usize, usize)> {
    let start = node.start_byte();
    let end = node.end_byte();
    let length = end.saturating_sub(start);

    // Leaf or recursion guard or tiny node → emit whole.
    if node.child_count() == 0 || depth > AST_RECURSION_LIMIT || length < AST_MIN_NODE_BYTES {
        return vec![(start, end)];
    }

    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
    let mut i = 0usize;
    while i < children.len() {
        let child = children[i];
        let group_start = child.start_byte();
        let mut group_end = child.end_byte();
        let mut group_len = group_end - group_start;
        i += 1;
        if group_len > desired {
            // Child itself is too big — recurse a level.
            groups.extend(merge_node(&child, desired, depth + 1));
            continue;
        }
        // Merge subsequent siblings into the current group while we
        // stay under `desired`.
        while i < children.len() {
            let nxt = children[i];
            let nxt_len = nxt.end_byte() - nxt.start_byte();
            if group_len + nxt_len > desired {
                break;
            }
            group_end = nxt.end_byte();
            group_len += nxt_len;
            i += 1;
        }
        groups.push((group_start, group_end));
    }
    groups
}

/// Second-pass merge for output of [`merge_node`]: glue adjacent
/// chunks until each reaches the desired length. Without this pass
/// the AST tends to over-split at module level (each `use` statement
/// in Rust becomes its own chunk).
pub(crate) fn merge_adjacent(chunks: &[(usize, usize)], desired: usize) -> Vec<(usize, usize)> {
    if chunks.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(chunks.len() / 2 + 1);
    let mut cur_start = chunks[0].0;
    let mut cur_end = chunks[0].1;
    for &(s, e) in chunks.iter().skip(1) {
        let cur_len = cur_end - cur_start;
        let nxt_len = e - s;
        if cur_len + nxt_len > desired {
            out.push((cur_start, cur_end));
            cur_start = s;
            cur_end = e;
        } else {
            cur_end = e;
        }
    }
    out.push((cur_start, cur_end));
    out
}
