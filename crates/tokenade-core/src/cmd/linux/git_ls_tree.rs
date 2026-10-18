//! `git_ls_tree` — git ls-tree.

pub fn detect(text: &str) -> bool {
    text.contains("blob ") || text.contains("tree ")
}

/// `100644 blob <40 hex>\tpath` → `<8 hex> path`, `040000 tree …` →
/// `<8 hex> path/`; the mode is written only when it is not the ordinary
/// file/dir one (`100755` executable, `120000` symlink, `160000` submodule).
/// `None` for a line of another shape (`--name-only`, `-l`), which the fold
/// then takes as it is.
fn dense(line: &str) -> Option<String> {
    let (meta, path) = line.split_once('\t')?;
    let mut m = meta.split_whitespace();
    let (mode, kind, sha) = (m.next()?, m.next()?, m.next()?);
    if m.next().is_some() || sha.len() < 8 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let short = &sha[..8];
    let slash = if kind == "tree" { "/" } else { "" };
    Some(match mode {
        "100644" | "040000" => format!("{short} {path}{slash}"),
        _ => format!("{short} {mode} {path}{slash}"),
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    // **Every entry, while that fits.** The fold kept 20 entries at any size:
    // measured 2026-10-03, `git ls-tree HEAD crates/tokenade-core/src/` — 157
    // entries, 14 KB — delivered 20. The 40-hex object id is most of each
    // line; at 8 it still names the object for `git show`.
    let rows: Option<Vec<String>> = text.lines().filter(|l| !l.trim().is_empty()).map(dense).collect();
    if let Some(rows) = rows {
        let all = format!(
            "[tokenade:git-ls-tree] {} tree entry(s) — object id cut to 8 hex, mode only when not 100644/040000\n{}",
            rows.len(),
            rows.join("\n")
        );
        if tk_base::delivery::fits(&all) {
            return Some(all);
        }
        return super::git_listing::fold(&rows.join("\n"), 20, "git-ls-tree", "tree entry(s)");
    }
    super::git_listing::fold(text, 20, "git-ls-tree", "tree entry(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn short_tree_passes_through() {
        let s = "100644 blob abc1234\tREADME.md\n040000 tree def5678\tsrc\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_tree_folds() {
        // Past the delivery threshold; under it every entry is listed.
        let _g = tk_base::delivery::threshold_on_this_thread(40);
        let text: String = (0..30)
            .map(|i| format!("100644 blob {:07x}\tfile{i}.rs\n", i))
            .collect();
        let out = try_compact(&text).unwrap();
        assert!(out.contains("[tokenade:git-ls-tree] 30 tree entry(s)"));
        assert!(out.contains("(+10 more)"));
    }

    #[test]
    fn every_entry_is_listed_while_it_fits() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let mut text = String::from("040000 tree 3b18e512dba79e4c8300dd08aeb37f8e728b8dad\tcmd\n");
        text.push_str("100755 blob 1f2e3d4c5b6a79881726354a5b6c7d8e9f0a1b2c\trun.sh\n");
        for i in 0..40 {
            text.push_str(&format!("100644 blob {:040x}\tmodule_{i}.rs\n", i * 7919 + 1));
        }
        let out = try_compact(&text).unwrap();
        assert!(out.contains("\n3b18e512 cmd/\n1f2e3d4c 100755 run.sh\n"), "{out}");
        for i in 0..40 {
            assert!(out.contains(&format!(" module_{i}.rs")), "lost module_{i}:\n{out}");
        }
    }
}
