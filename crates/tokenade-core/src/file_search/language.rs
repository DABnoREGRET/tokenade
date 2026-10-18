//! **Which language is this?** — four questions, in this order.
//!
//! The extension first, then the shebang, then the file NAME (`Makefile`,
//! `Dockerfile`), and as a last resort the PARSE trial: several grammars are
//! tried and the one yielding the most structure wins.
//!
//! The order is the heart of the matter. A parse trial is expensive and it
//! always succeeds a little — almost any file "parses" into something — so it
//! must only decide where the three cheap proofs are silent.

use super::*;

/// Detect the tree-sitter language tag from a file path's extension
/// (e.g. `"foo.rs"` → `Some("rust")`). Returns `None` when no
/// extension or unrecognised extension — callers should fall back to
/// line chunking.
pub fn language_from_path(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "rb" | "rake" | "gemspec" => "ruby",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "html" | "htm" => "html",
        "css" => "css",
        _ => return None,
    })
}

/// Detect the language from a `#!` line. Cheap and unambiguous — a
/// shebang is an explicit declaration by the file's author, so it wins
/// over every heuristic below and we return the instant it matches.
///
/// Only interpreters we have a *grammar* for are listed. `sh`, `bash`,
/// `perl`, `awk` etc. deliberately return `None`: naming them would
/// produce a tag `parser_for_language` rejects, so `chunk_ast` would
/// land back on `chunk_lines` having burnt a parse for nothing. `None`
/// reaches the same fallback one step sooner and does not lie about
/// what we support.
pub fn language_from_shebang(content: &str) -> Option<&'static str> {
    let first = content.lines().next()?;
    let rest = first.strip_prefix("#!")?;
    // Two shapes: `#!/usr/bin/python3` and `#!/usr/bin/env python3`.
    // Take the last whitespace-separated token that is not `env` and
    // not a `-flag`, then keep only the basename and strip a trailing
    // version suffix (`python3.11` → `python`).
    let interp = rest
        .split_whitespace()
        .rfind(|t| !t.starts_with('-') && !t.ends_with("env"))?;
    let base = interp.rsplit('/').next()?;
    let stem: String = base
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '_')
        .collect();
    Some(match stem.as_str() {
        "python" => "python",
        "node" | "nodejs" | "bun" | "deno" => "javascript",
        "ruby" | "jruby" => "ruby",
        "php" => "php",
        "swift" => "swift",
        _ => return None,
    })
}

/// Detect the language from a well-known *filename* (no usable
/// extension). Same rule as the shebang table: a name is only listed
/// when a vendored grammar actually parses that file's contents.
///
/// Notably absent, and on purpose: `Makefile`, `Dockerfile`,
/// `CMakeLists.txt`, `go.mod`. We vendor no grammar for make, docker,
/// cmake, or the go.mod format (`tree_sitter_go` parses Go *source*,
/// not module manifests — feeding it a go.mod yields an error-dense
/// tree and chunk boundaries worse than the line window). They stay on
/// the line fallback until a grammar exists.
///
/// The Ruby entries carry their weight: Gemfile/Rakefile/Podfile and
/// friends are plain Ruby DSL files that `tree_sitter_ruby` parses
/// exactly. The Python entries are build scripts written in Python or
/// in Starlark, a Python subset the Python grammar handles cleanly.
pub fn language_from_file_name(path: &std::path::Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    Some(match name {
        "Gemfile" | "Rakefile" | "Podfile" | "Vagrantfile" | "Brewfile" | "Guardfile"
        | "Capfile" | "Berksfile" | "Fastfile" | "Appfile" | "Dangerfile" => "ruby",
        "SConstruct" | "SConscript" | "wscript" | "BUILD" | "BUILD.bazel" | "WORKSPACE"
        | "WORKSPACE.bazel" | "Snakefile" => "python",
        _ => return None,
    })
}

/// Grammars entered into the parse trial, in tie-break order.
///
/// Bounded at seven — the brief allows eight, and each entry has to
/// earn its slot because every one costs a full parse:
///   * python / javascript / ruby dominate the extensionless scripts
///     and the code blocks lifted out of web pages and PDFs, which is
///     exactly the population that reaches this function;
///   * rust / go / java / c cover the rest of the common snippet mass.
///
/// Deliberately excluded to keep the trial *decisive* rather than
/// merely broad:
///   * `typescript` — a near-superset of JavaScript, so on plain JS the
///     two tie and the "clearly better" rule below would return `None`,
///     losing a detection we would otherwise make. JavaScript alone is
///     the safer bet; a real `.ts` file has an extension anyway.
///   * `cpp` — same near-tie against `c`.
///   * `html` / `css` — trivially identified by their own markers, and
///     both are permissive enough to score suspiciously well on junk.
///   * `csharp` / `swift` / `kotlin` / `scala` — rare in extensionless
///     or embedded form; not worth a parse each on every call.
#[rustfmt::skip]
pub(crate) const TRIAL_LANGUAGES: &[&str] = &["python", "javascript", "ruby", "rust", "go", "java", "c"];

/// Upper bound on input the parse trial will touch.
///
/// 64 KiB. The trial parses the input once per candidate grammar, so
/// its cost is ~7 full parses — bounded, but not free, and it sits on
/// an indexing path that walks whole trees. 64 KiB is far above any
/// realistic extensionless script or extracted code block (the cases
/// this feature exists for) while capping the worst case at a fixed,
/// small multiple of a single parse. Anything larger keeps today's
/// behaviour and goes straight to the line window.
///
/// Cost note: this is a reasoned bound on parse count and input size,
/// not a timing measurement — no benchmark was run for this change.
pub(crate) const TRIAL_MAX_BYTES: usize = 64 * 1024;

/// Node kinds that signal real *structure* rather than an incidental
/// run of expressions. Prose and config files can parse with few
/// errors under a permissive grammar, but they never produce a
/// function, class, or import node — so this count, not the error
/// count, is what separates code from not-code.
///
/// The union is safe across grammars: kinds are near-disjoint between
/// them, and the overlaps that do exist (`function_definition` is both
/// Python and C) mean the same thing in both.
pub(crate) const STRUCTURE_KINDS: &[&str] = &[
    // python
    "function_definition",
    "class_definition",
    "import_statement",
    "import_from_statement",
    "decorated_definition",
    // javascript
    "function_declaration",
    "class_declaration",
    "method_definition",
    "export_statement",
    // ruby
    "method",
    "class",
    "module",
    // rust
    "function_item",
    "struct_item",
    "impl_item",
    "enum_item",
    "use_declaration",
    "mod_item",
    // go
    "method_declaration",
    "import_declaration",
    "type_declaration",
    "package_clause",
    // java
    "class_body",
    "interface_declaration",
    // c
    "preproc_include",
    "struct_specifier",
    "declaration_list",
];

/// Result of parsing one candidate grammar over the trial input.
pub(crate) struct TrialScore {
    errors: usize,
    structure: usize,
    nodes: usize,
}

/// Parse `content` with one grammar and tally errors, structural
/// nodes, and total nodes in a single cursor walk.
pub(crate) fn score_language(content: &str, language: &str) -> Option<TrialScore> {
    let lang = parser_for_language(language)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(content.as_bytes(), None)?;

    let mut errors = 0usize;
    let mut structure = 0usize;
    let mut nodes = 0usize;
    let mut cursor = tree.walk();
    let mut is_root = true;
    // Iterative pre-order walk — no recursion, so a pathological tree
    // cannot blow the stack the way `merge_node` guards against with
    // AST_RECURSION_LIMIT.
    'walk: loop {
        let node = cursor.node();
        nodes += 1;
        if node.is_error() || node.is_missing() {
            errors += 1;
        } else if !is_root && STRUCTURE_KINDS.contains(&node.kind()) {
            // The root is never evidence of structure. It matters:
            // Python's root node kind is literally `module`, which also
            // names Ruby's `module X ... end` — counting it handed every
            // Python parse a free structural point and made non-code
            // (INI, prose) resolve to Python instead of `None`.
            structure += 1;
        }
        is_root = false;
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Some(TrialScore {
        errors,
        structure,
        nodes,
    })
}

/// Last resort: identify the language by *trying* to parse the content
/// with each candidate grammar and keeping the best fit.
///
/// Ranking is errors ascending, then structural richness descending,
/// then node count descending — a grammar that chokes is wrong, and
/// among grammars that cope the one that recovers the most real
/// structure is the one that actually understands the file.
///
/// The winner is returned **only when it clearly beats the runner-up**.
/// A near-tie means we genuinely cannot tell, and in that case `None`
/// — today's behaviour, the line window — is the right answer: a wrong
/// grammar produces confidently wrong chunk boundaries, which is worse
/// for retrieval than honest fixed windows.
pub fn language_by_parse_trial(content: &str) -> Option<&'static str> {
    // Guard the cost. Also skip inputs too small to judge: a couple of
    // lines score identically under every grammar and would always
    // near-tie anyway.
    if content.len() > TRIAL_MAX_BYTES || content.trim().len() < 24 {
        return None;
    }

    let mut scored: Vec<(&'static str, TrialScore)> = TRIAL_LANGUAGES
        .iter()
        .filter_map(|lang| score_language(content, lang).map(|s| (*lang, s)))
        .collect();
    if scored.len() < 2 {
        return None;
    }
    scored.sort_by(|a, b| {
        a.1.errors
            .cmp(&b.1.errors)
            .then(b.1.structure.cmp(&a.1.structure))
            .then(b.1.nodes.cmp(&a.1.nodes))
    });

    let (best_lang, best) = &scored[0];
    let (_, second) = &scored[1];

    // Hard floor: no structure means no evidence this is code at all.
    // English prose parses as a chain of bare expressions under several
    // grammars with few or zero errors — this is the check that keeps
    // us from labelling a README as Python.
    if best.structure == 0 {
        return None;
    }
    // A winner that is itself error-dense is not a fit, just the least
    // bad of a bad set.
    if best.errors * 10 > best.nodes {
        return None;
    }

    // "Clearly better" — either it is the only grammar that parsed the
    // file cleanly, or it ties on errors but recovers substantially
    // more structure than the next candidate.
    let decisive = best.errors < second.errors
        || (best.errors == second.errors && best.structure >= second.structure * 2);
    if decisive {
        Some(best_lang)
    } else {
        None
    }
}

/// Full language detection, cheapest signal first: extension, then
/// shebang, then well-known filename, then — only if all of those came
/// up empty — the parse trial. Returns `None` when nothing is
/// conclusive, and the caller falls back to line chunking.
pub fn detect_language(path: &std::path::Path, content: &str) -> Option<&'static str> {
    language_from_path(path)
        .or_else(|| language_from_shebang(content))
        .or_else(|| language_from_file_name(path))
        .or_else(|| language_by_parse_trial(content))
}
