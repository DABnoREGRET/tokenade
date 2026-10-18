//! **The same trace, twenty times.**
//!
//! A test loop or a server replaying a request produces the same block
//! verbatim. Counting them is useful; delivering them all is not.
//!
//! Canonicalisation is what makes two blocks "the same": line numbers,
//! addresses, request ids change from one occurrence to the next without
//! changing anything about the defect. Over-canonicalising merges two
//! distinct defects — the only mistake that costs information.

use super::*;

/// Detect repeated stacktrace blocks in `text` and collapse identical groups
/// to a single example tagged `(×N)`.
///
/// Block boundaries are heuristic: pytest uses `____` separators, cargo test
/// uses `---- test_<name> stdout ----`, jest/mocha use `●` and indentation.
/// We split on any of those AND on lines starting a new error header
/// (`Traceback`, `thread '...' panicked at`, `Error:`). Each block is
/// canonicalised (strip line numbers, hex addresses, `/tmp/.../` paths, test
/// names) and hashed; identical canonical hashes are folded.
///
/// When no separators are present (i.e. there's only one block), the input is
/// returned unchanged so per-language compactors see the original frames.
pub fn dedup_blocks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let separators: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_block_separator(l))
        .map(|(i, _)| i)
        .collect();
    // Need at least 2 separators to form 2+ blocks worth deduping.
    if separators.len() < 2 {
        return text.to_string();
    }

    // Preamble = everything up to the first separator (the "FAILURES" header
    // and friends). Keep it verbatim.
    let preamble: String = lines[..separators[0]].join("\n");

    let mut groups: Vec<(String, Vec<(usize, String)>)> = Vec::new(); // (canonical, [(orig_idx, header+body)])
    for w in 0..separators.len() {
        let start = separators[w];
        let end = if w + 1 < separators.len() {
            separators[w + 1]
        } else {
            lines.len()
        };
        let block: String = lines[start..end].join("\n");
        let canon = canonicalise(&block);
        if let Some(g) = groups.iter_mut().find(|(c, _)| *c == canon) {
            g.1.push((w, block));
        } else {
            groups.push((canon, vec![(w, block)]));
        }
    }

    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(preamble.trim_end());
        out.push('\n');
    }
    for (_canon, blocks) in groups {
        let count = blocks.len();
        // Emit the FIRST occurrence verbatim — its test name / line numbers
        // give the agent a real entry point to investigate.
        out.push_str(blocks[0].1.trim_end());
        if count > 1 {
            // List up to 3 sibling identifiers so the agent knows which OTHER
            // tests share this failure.
            let siblings: Vec<String> = blocks
                .iter()
                .skip(1)
                .take(3)
                .filter_map(|(_, b)| extract_block_label(b))
                .collect();
            let extra = if siblings.is_empty() {
                String::new()
            } else {
                format!(
                    " (also: {}{})",
                    siblings.join(", "),
                    if count - 1 > siblings.len() {
                        format!(", +{} more", count - 1 - siblings.len())
                    } else {
                        String::new()
                    }
                )
            };
            let _ = write!(out, "\n… identical traceback ×{count}{extra} …");
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub(crate) fn is_block_separator(line: &str) -> bool {
    let t = line.trim_start();
    // pytest:   `_____ test_foo _____`
    // cargo:    `---- test_foo stdout ----`
    // mocha:    `  ●  test name` (jest)
    // jest:     `●  describe › test`
    (t.starts_with("____") && t.contains("____"))
        || (t.starts_with("---- ") && t.contains("stdout"))
        || t.starts_with('●')
        || (t.starts_with("=====") && t.contains("FAIL"))
}

/// Strip volatile bits so two structurally identical tracebacks share a key:
/// line numbers, hex addresses, `/tmp/...` temp paths, test-case names in the
/// block separator. Best-effort; collisions are fine (just over-merges).
pub(crate) fn canonicalise(block: &str) -> String {
    let mut s = block.to_string();
    // Strip hex addresses: 0xdeadbeef -> 0xN. The spelling is shared with
    // the template fold (`value_shapes`): the one that used to live here
    // missed `0XDEADBEEF` and bit into the end of `foo0x1f`.
    s = re(crate::regexes::HEX_LITERAL)
        .replace_all(&s, "0xN")
        .into_owned();
    // Strip line:column suffixes: foo.py:42, src/foo.rs:42:7 -> foo.py:N
    s = re(r":\d+(?::\d+)?").replace_all(&s, ":N").into_owned();
    // Strip `line N` / `lineno=N`
    s = re(r"\bline\s+\d+").replace_all(&s, "line N").into_owned();
    // Strip /tmp/pytest-of-... / /var/folders/... / target/tmp/...
    s = re(r"/tmp/[\w\-./]+").replace_all(&s, "/tmp/X").into_owned();
    s = re(r"/var/folders/[\w\-./]+")
        .replace_all(&s, "/var/folders/X")
        .into_owned();
    // Strip the test name from separator headers. pytest: `___ test_foo ___`
    s = re(r"_{3,}\s*[\w:.\[\]\-]+\s*_{3,}")
        .replace_all(&s, "___ T ___")
        .into_owned();
    s = re(r"----\s*[\w:.\[\]\-]+\s*stdout\s*----")
        .replace_all(&s, "---- T stdout ----")
        .into_owned();
    // Final, aggressive pass: collapse any remaining digit run to `N` so
    // parametrised identifiers like `test_case_0` / `test_case_1` / `[case42]`
    // hash to the same canonical form. This is safe for our use case (we're
    // grouping by structural equivalence, not preserving values).
    s = re(r"\d+").replace_all(&s, "N").into_owned();
    // Drop trailing whitespace per line.
    s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

/// Pull the test identifier out of a block's separator (used to list siblings
/// of a folded group). Returns `None` if there's no recognisable header.
pub(crate) fn extract_block_label(block: &str) -> Option<String> {
    let first = block.lines().next()?;
    let t = first.trim();
    if let Some(stripped) = t.strip_prefix("---- ") {
        return stripped.split(" stdout").next().map(str::to_string);
    }
    if let Some(rest) = t.strip_prefix('_') {
        return Some(rest.trim_matches('_').trim().to_string());
    }
    if let Some(stripped) = t.strip_prefix("● ") {
        return Some(stripped.trim().to_string());
    }
    None
}
