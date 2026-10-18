//! `grep -A/-B/-C` (context) — matches printed with surrounding context lines.
//!
//! Real command-specific compactor (was: generic `line_cap(text, 80, "context
//! match(es)")` behind a `!text.trim().is_empty()` detector that matched
//! anything). The cap silently dropped every line past 80 — and on a recursive
//! `grep -rn -C3` the hits are spread through the tree, so truncating at 80
//! threw away most of the matches you ran the search for.
//!
//! Output shape (recursive, with `-n`):
//! ```text
//! src/api/client.rs-21-    // build the request
//! src/api/client.rs-22-    let url = endpoint();
//! src/api/client.rs:23:    let resp = fetch(url).await?;     ← match (`:` sep)
//! src/api/client.rs-24-    parse(resp)
//! --
//! src/api/server.rs-40-    fn route() {
//! src/api/server.rs:41:        fetch_handler();
//! src/api/server.rs-42-    }
//! ```
//!
//! grep separates the leading `path` from the rest with `:` on a MATCH line and
//! `-` on a CONTEXT line, and prints `--` between non-adjacent groups. Where the
//! tokens go: in a recursive search every line of a group repeats the SAME file
//! path. On a deep tree that repeated path is the bulk of the bytes — the
//! line numbers, the `:`/`-` match marker, and the code are the signal.
//!
//! Strategy: within each `--`-delimited group, fold the repeated leading path
//! into a single header line and strip it from every member line, keeping the
//! `:`/`-` separator (so match-vs-context stays visible), the line number, and
//! the content byte-for-byte. The path is moved to the header, never dropped;
//! `--` group boundaries are preserved; a banner states how many groups/files
//! were folded. Groups whose shared prefix can't be pinned to a grep separator
//! on EVERY line (single-file context with no path, or anything that doesn't
//! parse) are emitted verbatim — content is never mangled. Small inputs pass
//! through.
//!
//! Detection/parsing are STRUCTURAL on grep's ASCII delimiters (`:` match-sep,
//! `-` context-sep, `--` group-sep) and the *byte-identical repeated prefix* —
//! never on an English keyword. The locale-/charset-variable surface (paths,
//! which may carry accents/CJK, and the matched source content) is preserved
//! exactly: the fold only ever removes a prefix proven identical across every
//! line of the group, computed on UTF-8 char boundaries.

const PASSTHROUGH_LINES: usize = 8;

/// A `<lead-token><sep>…` line: a non-empty lead token followed by a `:` or `-`
/// separator with more text after it. Used only by the structural detector.
fn line_has_separator(line: &str) -> bool {
    let t = line.trim_start();
    match t.find([':', '-']) {
        Some(idx) => idx > 0 && idx + 1 < t.len(),
        None => false,
    }
}

pub fn detect(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let nonempty: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if nonempty.is_empty() {
        return false;
    }
    // A grep-context stream either carries `--` group separators, or a strong
    // majority of its lines carry the `<token><sep>` structure. Arbitrary prose
    // (no separators, no `--`) is left alone.
    let has_group_sep = nonempty.iter().any(|l| l.trim_end() == "--");
    let grepish = nonempty.iter().filter(|l| line_has_separator(l)).count();
    has_group_sep || grepish * 10 >= nonempty.len() * 7
}

crate::archetypes::detect_gated_try_compact!();

/// Byte length of the longest common prefix of `lines`, backed off to a UTF-8
/// char boundary of `lines[0]` so the result is always a valid `&str` split.
fn common_prefix_len(lines: &[&str]) -> usize {
    let first = lines[0].as_bytes();
    let mut end = first.len();
    for l in &lines[1..] {
        let lb = l.as_bytes();
        let max = end.min(lb.len());
        let mut i = 0;
        while i < max && first[i] == lb[i] {
            i += 1;
        }
        end = i;
        if end == 0 {
            break;
        }
    }
    while end > 0 && !lines[0].is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Fold one match group whose lines all share grep's repeated `path` prefix.
///
/// Returns `(path, residuals)` where each residual is the line with `path`
/// removed — the leading `:`/`-` separator (match vs context), line number, and
/// content all kept. Returns `None` (→ caller emits the group verbatim) unless
/// the shared prefix is immediately followed by a `:` or `-` separator on EVERY
/// line AND at least one line is a match line (`:`). That guarantees we only
/// ever strip a prefix that is byte-identical across the group and consistent
/// with grep's format, so content is never mangled.
fn try_fold_group<'a>(group: &[&'a str]) -> Option<(&'a str, Vec<&'a str>)> {
    if group.len() < 2 {
        return None;
    }
    let plen = common_prefix_len(group);
    if plen == 0 {
        return None;
    }
    let prefix = &group[0][..plen];
    let mut has_match = false;
    let mut residuals: Vec<&str> = Vec::with_capacity(group.len());
    for &line in group {
        let rest = &line[plen..];
        match rest.as_bytes().first().copied() {
            Some(b':') => has_match = true,
            Some(b'-') => {}
            _ => return None,
        }
        residuals.push(rest);
    }
    if !has_match {
        return None;
    }
    Some((prefix, residuals))
}

pub fn compact(text: &str) -> String {
    let s = crate::ansi::strip_ansi(text);
    let passthrough = || s.trim_end_matches('\n').to_string();

    let lines: Vec<&str> = s.lines().collect();
    if lines.iter().filter(|l| !l.trim().is_empty()).count() < PASSTHROUGH_LINES {
        return passthrough();
    }

    // Split into `--`-delimited groups, keeping the separators as their own
    // single-element marker groups so boundaries survive verbatim.
    let mut groups: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for &l in &lines {
        if l.trim_end() == "--" {
            groups.push(std::mem::take(&mut cur));
            groups.push(vec!["--"]);
        } else {
            cur.push(l);
        }
    }
    groups.push(cur);

    let mut rendered: Vec<String> = Vec::new();
    let mut folded_groups = 0usize;
    let mut folded_lines = 0usize;
    // A SET, not a Vec. Only `.len()` is ever read from it (the banner below),
    // so the Vec's order was never observed — but its `contains` was a linear
    // scan run once per foldable group, which is quadratic in the number of
    // distinct files a grep touches. `grep -r` across a repo is exactly the
    // call that produces thousands of them. Same shape as the `roots.contains`
    // that made `archive.rs` quadratic.
    let mut distinct_paths: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for g in &groups {
        if g.is_empty() {
            continue;
        }
        if g.len() == 1 && g[0] == "--" {
            rendered.push("--".to_string());
            continue;
        }
        match try_fold_group(g) {
            Some((prefix, residuals)) => {
                folded_groups += 1;
                folded_lines += g.len();
                distinct_paths.insert(prefix);
                let mut block = String::with_capacity(prefix.len() + s.len() / groups.len().max(1));
                block.push_str(prefix);
                block.push('\n');
                block.push_str(&residuals.join("\n"));
                rendered.push(block);
            }
            None => {
                rendered.push(
                    g.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n"),
                );
            }
        }
    }

    if folded_groups == 0 {
        // Nothing foldable (e.g. single-file context with no repeated path) —
        // keeping every line beats line_cap's silent truncation.
        return passthrough();
    }

    let banner = format!(
        "[tokenade:grep-context] repeated path folded to per-group header in {folded_groups} group(s) across {} file(s) · {folded_lines} line(s) reattributed",
        distinct_paths.len()
    );
    let out = format!("{banner}\n{}", rendered.join("\n"));
    let out = out.trim_end_matches('\n').to_string();

    // Never emit a larger result than the input.
    if crate::tokenizer::estimate(&out) >= crate::tokenizer::estimate(&passthrough()) {
        return passthrough();
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// Recursive `grep -rn -C<ctx>`: `n_files` groups, each `ctx` context lines
    /// before a match, the match, then `ctx` context lines after; groups joined
    /// by `--`. Match lines use `:`, context lines use `-` (grep's own format).
    fn build_grep_ctx(n_files: usize, ctx: usize) -> String {
        let mut s = String::new();
        for i in 0..n_files {
            let path = format!("src/module/submodule/component_{i:03}.rs");
            for b in 0..ctx {
                let ln = 10 + b;
                let _ = writeln!(s, "{path}-{ln}-    let before_{b} = compute();");
            }
            let mln = 10 + ctx;
            let _ = writeln!(s, "{path}:{mln}:    let needle = target_value();");
            for a in 0..ctx {
                let ln = 11 + ctx + a;
                let _ = writeln!(s, "{path}-{ln}-    let after_{a} = compute();");
            }
            if i + 1 < n_files {
                s.push_str("--\n");
            }
        }
        s
    }

    /// The banner counts DISTINCT files, and must still count them exactly
    /// now that the tally is a set rather than a linearly-scanned Vec.
    ///
    /// Sized past the point where the old `contains` mattered: 400 distinct
    /// files meant ~80 000 string compares to answer a question a hash answers
    /// once. `grep -r` over a repo is the call that produces that shape, and
    /// it is one of the most frequent commands an agent runs.
    #[test]
    fn distinct_file_count_is_exact_on_a_wide_grep() {
        let input = build_grep_ctx(400, 2);
        let out = compact(&input);
        assert!(
            out.contains("across 400 file(s)"),
            "banner miscounted: {}",
            out.lines().next().unwrap_or_default()
        );
        // The same path repeated must count once, not once per group.
        let repeated = format!("{0}--\n{0}--\n{0}", build_grep_ctx(1, 2));
        let out = compact(&repeated);
        if out.contains("[tokenade:grep-context]") {
            assert!(
                out.contains("across 1 file(s)"),
                "a repeated path counted more than once: {}",
                out.lines().next().unwrap_or_default()
            );
        }
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_is_structural() {
        assert!(detect(&build_grep_ctx(4, 3)));
        // A single-line `path:lineno:` form is recognised by its separators.
        assert!(detect("src/a.rs:12:let x = 1;\nsrc/a.rs-13-let y = 2;\n"));
        // Free prose with no separators / `--` is left alone.
        assert!(!detect("building the project\nall went well today\n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = build_grep_ctx(6, 2);
        let _ = try_compact(&s);
        let _ = compact(&s);
        let _ = compact_with_savings(&s);
    }

    #[test]
    fn quality_paths_folded_content_and_linenos_kept_verbatim() {
        let input = build_grep_ctx(20, 3);
        let out = compact(&input);
        assert!(out.contains("[tokenade:grep-context]"), "banner missing: {out}");
        // True file count announced.
        assert!(out.contains("20 file(s)"), "file count wrong: {out}");
        // Each path appears EXACTLY once — as a header, no longer on every line.
        let occ = out.matches("src/module/submodule/component_000.rs").count();
        assert_eq!(occ, 1, "path should be a single header, not repeated per line: {out}");
        // The match content + its separator/line-number survive verbatim.
        assert!(out.contains("let needle = target_value();"), "match content dropped: {out}");
        assert!(out.contains(":13:"), "match separator+lineno dropped: {out}");
        // Context content + its separator/line-number survive verbatim.
        assert!(out.contains("let before_0 = compute();"), "leading context dropped: {out}");
        assert!(out.contains("let after_2 = compute();"), "trailing context dropped: {out}");
        assert!(out.contains("-10-"), "context separator+lineno dropped: {out}");
        // grep's group boundaries are preserved.
        assert!(out.contains("--"), "group separators dropped: {out}");
        // Every file's match is still present (line_cap used to drop the tail).
        assert!(out.contains("src/module/submodule/component_019.rs"), "last file dropped: {out}");
    }

    #[test]
    fn quality_small_run_passthrough() {
        // 3 lines (< PASSTHROUGH_LINES) → verbatim, no banner.
        let input = build_grep_ctx(1, 1);
        let out = compact(&input);
        assert!(!out.contains("[tokenade:grep-context]"), "small run got a banner: {out}");
        assert!(out.contains("let needle = target_value();"), "small-run match lost: {out}");
        assert!(out.contains("src/module/submodule/component_000.rs"), "small-run path lost: {out}");
    }

    #[test]
    fn single_file_no_repeated_path_passthrough() {
        // `grep -C2 pat file` (single file, with -n) has NO repeated path — the
        // prefix can't be pinned to a separator on every line, so it stays
        // verbatim (and nothing is silently dropped).
        let mut input = String::new();
        for ln in 10..30 {
            let sep = if ln == 20 { ':' } else { '-' };
            let _ = writeln!(input, "{ln}{sep}    line content number {ln}");
        }
        let out = compact(&input);
        // No path to fold → passthrough → every match/context line kept.
        assert!(!out.contains("[tokenade:grep-context]"), "got a fold banner with no path: {out}");
        assert!(out.contains("20:    line content number 20"), "match line lost: {out}");
        assert!(out.contains("29-    line content number 29"), "tail context lost: {out}");
    }

    #[test]
    fn multilingual_non_ascii_paths_and_content_preserved() {
        // grep's separators stay ASCII; paths + matched source may carry
        // accents/CJK and must survive byte-for-byte through the prefix fold.
        let mut input = String::new();
        for i in 0..12 {
            let path = format!("src/modèles/café/composant_{i:02}.rs");
            let _ = writeln!(input, "{path}-9-    // contexte précédent");
            let _ = writeln!(input, "{path}:10:    let résultat = « succès attendu »;");
            let _ = writeln!(input, "{path}-11-    // contexte suivant");
            input.push_str("--\n");
        }
        // A CJK-path group too.
        let cjk = "src/コンポーネント/ボタン.rs";
        let _ = writeln!(input, "{cjk}-1-    // 前のコンテキスト");
        let _ = writeln!(input, "{cjk}:2:    let 値 = 「成功」;");
        let _ = writeln!(input, "{cjk}-3-    // 後のコンテキスト");

        assert!(detect(&input), "non-ASCII grep context not detected structurally");
        let out = compact(&input);
        assert!(out.contains("[tokenade:grep-context]"), "banner missing under non-ASCII: {out}");
        // Accented path folded to a header (present, exactly once for file 00).
        assert_eq!(out.matches("src/modèles/café/composant_00.rs").count(), 1,
            "accented path not folded to single header: {out}");
        // Accented + CJK content preserved verbatim.
        assert!(out.contains("let résultat = « succès attendu »;"), "accented match lost: {out}");
        assert!(out.contains("// contexte précédent"), "accented context lost: {out}");
        assert!(out.contains("src/コンポーネント/ボタン.rs"), "CJK path lost: {out}");
        assert!(out.contains("let 値 = 「成功」;"), "CJK match lost: {out}");
        assert!(out.contains("// 後のコンテキスト"), "CJK context lost: {out}");
    }

    #[test]
    fn savings_on_large_recursive_context() {
        // 60 files × (3 before + match + 3 after) = the repeated path dominates
        // and folds to one header per group.
        let (_, sav) = compact_with_savings(&build_grep_ctx(60, 3));
        assert!(sav.tokens_before > 0);
        let pct = 100 * (sav.tokens_before - sav.tokens_after) / sav.tokens_before;
        assert!(pct >= 35, "large recursive grep -C only saved {pct}%");
    }
}
