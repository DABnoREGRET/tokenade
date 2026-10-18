//! `npm_list` — npm list.
const HEAD: usize = 80;


pub fn detect(text: &str) -> bool {
    text.contains('@')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    if let Some(tree) = tree(text) {
        return Some(tree);
    }
    // Priority, not plain head: `npm ls` on a healthy tree is 200 identical
    // lines and the ONLY reason to read it is the one that says
    // `UNMET PEER DEPENDENCY` or `invalid`. The plain fold kept the first
    // eighty alphabetically and cut exactly that (MEASURED).
    crate::table_fold::fold_table_priority(text, HEAD, "npm-list", "npm dep(s)")
}

/// A line of the tree: its depth and what it says, the drawing stripped.
fn node(line: &str) -> Option<(usize, &str)> {
    let content = line.trim_start_matches(['│', '├', '└', '─', '┬', ' ']);
    if content.len() == line.len() {
        return None;
    }
    let prefix = &line[..line.len() - content.len()];
    // `├── a` (4 chars) is depth 1, `│ ├── b` depth 2: two chars per level
    // after the first connector's four.
    Some(((prefix.chars().count() / 2).saturating_sub(1).max(1), content.trim_end()))
}

/// What npm ITSELF says is wrong with a line — read from the words npm puts
/// there, never from a word that can be part of a package NAME.
fn problem(content: &str) -> bool {
    (content.starts_with("UNMET ") && !content.starts_with("UNMET OPTIONAL DEPENDENCY "))
        || content.contains(" invalid: ")
        || content.ends_with(" invalid")
        || content.ends_with(" extraneous")
}

/// **`npm ls --all`: what is wrong, what the project depends on, and counts.**
///
/// The priority fold looked for problem WORDS anywhere on a line, so a
/// healthy tree surfaced `error-ex`, `json-parse-even-better-errors`,
/// `@smithy/invalid-dependency` and `es-errors deduped` as "problems", next
/// to 129 `UNMET OPTIONAL DEPENDENCY` lines — optional packages for other
/// platforms, absent by design — and announced "170 problem(s)" on a tree
/// that had none (measured on a real 3 341-line `npm ls --all`). npm marks a
/// real problem in its own words: `UNMET DEPENDENCY`, `UNMET PEER
/// DEPENDENCY`, `invalid: "…" from …`, `extraneous`, `npm error …`. Those are
/// shown with the chain that requires them; the rest is counted.
fn tree(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if !lines.iter().any(|l| l.contains("├── ") || l.contains("└── ") || l.contains("├─┬ ") || l.contains("└─┬ ")) {
        return None;
    }
    let mut chain: Vec<&str> = Vec::new();
    let (mut problems, mut direct, mut optional, mut other) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut deduped = 0usize;
    let mut nodes = 0usize;
    let root = lines.first().copied().unwrap_or("");
    for line in &lines[1..] {
        let Some((depth, content)) = node(line) else {
            if !line.trim().is_empty() {
                other.push(line.trim_end().to_string());
            }
            continue;
        };
        nodes += 1;
        chain.truncate(depth.saturating_sub(1));
        if problem(content) {
            let path: Vec<&str> = chain.iter().map(|c| c.split(' ').next().unwrap_or(c)).collect();
            let via = if path.is_empty() { String::new() } else { format!("   (via {})", path.join(" > ")) };
            problems.push(format!("  {content}{via}"));
        } else if let Some(name) = content.strip_prefix("UNMET OPTIONAL DEPENDENCY ") {
            if !optional.iter().any(|o| o == name) {
                optional.push(name.to_string());
            }
        } else if content.ends_with(" deduped") {
            deduped += 1;
        } else if depth <= 1 {
            direct.push(content.to_string());
        }
        chain.push(content);
    }
    let mut out = format!(
        // The tree's own marks: npm's trailing `npm error …` lines restate them.
        "[tokenade:npm-ls] {root} · {nodes} entries · {deduped} deduped · {} problem(s)\n",
        problems.len()
    );
    if !problems.is_empty() {
        out.push_str(&format!("problems ({}):\n{}\n", problems.len(), problems.join("\n")));
    }
    if !other.is_empty() {
        out.push_str(&format!("{}\n", other.join("\n")));
    }
    out.push_str(&format!("direct dependencies ({}):\n  {}\n", direct.len(), direct.join(", ")));
    if !optional.is_empty() {
        let shown: Vec<&str> = optional.iter().take(12).map(String::as_str).collect();
        let more = if optional.len() > 12 { format!(", … (+{})", optional.len() - 12) } else { String::new() };
        out.push_str(&format!(
            "optional dependencies not installed ({}, normal for other platforms): {}{more}\n",
            optional.len(),
            shown.join(", ")
        ));
    }
    Some(out.trim_end().to_string())
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "app@1.0.0   /path\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("app@1.0.0   /path")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(85)).unwrap();
        assert!(out.contains("[tokenade:npm-list]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(85)).unwrap();
        assert!(out.contains("app@"));
        assert!(out.contains('@'));
    }

    /// A healthy package whose NAME holds a problem word is not a problem; an
    /// optional dependency absent by design is counted, not listed as one;
    /// npm's own marks are, with what requires them.
    #[test]
    fn a_tree_reports_what_npm_marks_not_what_a_name_says() {
        let raw = "demo@1.0.0 /p\n├─┬ @aws-sdk/client@3.0.0\n│ ├── @smithy/invalid-dependency@4.2.8\n│ ├── error-ex@1.3.4 deduped\n│ └── UNMET OPTIONAL DEPENDENCY fsevents@~2.3.2\n├── extra-pkg@0.1.0 extraneous\n├─┬ vite@7.3.1\n│ └── left-pad-x@1.0.0 invalid: \"^2.0.0\" from vite@7.3.1\n├── UNMET DEPENDENCY missing-pkg@^1.0.0\n└── error-ex@1.3.4\n\nnpm error code ELSPROBLEMS\n";
        let out = try_compact(raw).unwrap();
        assert!(out.starts_with("[tokenade:npm-ls] demo@1.0.0 /p · 9 entries · 1 deduped · 3 problem(s)\n"), "{out}");
        assert!(out.contains("  left-pad-x@1.0.0 invalid: \"^2.0.0\" from vite@7.3.1   (via vite@7.3.1)"), "{out}");
        assert!(out.contains("  UNMET DEPENDENCY missing-pkg@^1.0.0\n"), "{out}");
        assert!(!out.lines().any(|l| l.starts_with("  @smithy/invalid-dependency")), "{out}");
        assert!(out.contains("direct dependencies (3):\n  @aws-sdk/client@3.0.0, vite@7.3.1, error-ex@1.3.4\n"), "{out}");
        assert!(out.contains("optional dependencies not installed (1, normal for other platforms): fsevents@~2.3.2"), "{out}");
    }
}
