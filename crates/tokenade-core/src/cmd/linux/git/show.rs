//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.

pub fn compact(output: &str) -> String {
    // `git show <ref>` has the same shape as `git log -p` for a
    // single commit (header + full diff). Reuse the log compactor.
    let body = super::log::with_patch(output);
    // …but give back the header the log compactor drops. Across forty
    // commits, `Author:`/`Date:` are the price of density; on the ONE commit
    // `git show` is asked about, "who, when, which full hash" is often the
    // question — measured 2026-10-03, a real `git show --stat HEAD~3` came
    // back with an 8-char hash and neither. Three lines.
    let mut lines = output.lines();
    let first = lines.next().unwrap_or("");
    let one_commit = output.lines().filter(|l| l.starts_with("commit ")).count() == 1;
    let Some(full) = first.strip_prefix("commit ").filter(|_| one_commit) else {
        return body;
    };
    let header: Vec<&str> = std::iter::once(first)
        .chain(lines.take_while(|l| !l.trim().is_empty()))
        .collect();
    let short = full.split_whitespace().next().unwrap_or(full);
    let rest = match body.split_once('\n') {
        Some((head, rest)) if !head.is_empty() && short.starts_with(head.trim()) => rest,
        _ => body.as_str(),
    };
    format!("{}\n{rest}", header.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_commit_keeps_its_full_hash_author_and_date() {
        let raw = "commit 98c8e93fda0deb6d8c5c4fe952b2f1d5e68882ab\nAuthor: A Person <a@example.org>\nDate:   Sat Oct 3 00:06:47 2026 +0200\n\n    Subject line\n\n PATCHNOTES.md | 1 +\n 1 file changed, 1 insertion(+)\n";
        let out = compact(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(&lines[..3], [
            "commit 98c8e93fda0deb6d8c5c4fe952b2f1d5e68882ab",
            "Author: A Person <a@example.org>",
            "Date:   Sat Oct 3 00:06:47 2026 +0200",
        ]);
        assert!(!lines.contains(&"98c8e93f"), "the short hash is not repeated:\n{out}");
        assert!(out.contains("Subject line") && out.contains("PATCHNOTES.md"), "{out}");
    }
}
