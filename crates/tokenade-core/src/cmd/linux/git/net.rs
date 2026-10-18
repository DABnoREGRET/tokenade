//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::generic;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut keep = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Drop common progress lines.
        //
        // The `remote: ` prefix is stripped FIRST rather than spelled out per
        // phase: git emits every counter both locally and echoed from the
        // server, and listing the prefixed forms one by one is how
        // `remote: Resolving deltas` came to be missing while
        // `remote: Counting` was handled. A push whose server echoed 300
        // delta-resolution lines came back at four fifths of its size.
        let t = t.strip_prefix("remote: ").unwrap_or(t);
        if t.starts_with("Counting")
            || t.starts_with("Compressing")
            || t.starts_with("Enumerating")
            || t.starts_with("Receiving objects:")
            || t.starts_with("Resolving deltas:")
            || t.starts_with("Writing objects:")
            || t.starts_with("Unpacking objects:")
            || t.starts_with("Total ")
        {
            continue;
        }
        keep.push(line);
    }
    if keep.is_empty() {
        return generic(output);
    }
    keep.join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_echoed_progress_folds_like_the_local_kind() {
        let mut input: String = (0..300)
            .map(|i| format!("remote: Resolving deltas:  {i}% (1/300)\n"))
            .collect();
        input.push_str("! [rejected]        main -> main (non-fast-forward)\n");
        input.push_str("error: failed to push some refs to 'origin'\n");
        let out = super::compact(&input);
        assert!(!out.contains("Resolving deltas"), "progress folds:\n{out}");
        assert!(out.contains("! [rejected]"), "the rejection survives:\n{out}");
        assert!(out.contains("failed to push some refs"), "and why:\n{out}");
    }
}
