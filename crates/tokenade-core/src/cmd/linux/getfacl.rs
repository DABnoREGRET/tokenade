//! `getfacl <path>` — POSIX ACLs.
//!
//! ```text
//! # file: foo
//! # owner: alice
//! # group: alice
//! user::rw-
//! user:bob:rw-
//! group::r--
//! mask::rw-
//! other::r--
//!
//! # file: bar
//! ...
//! ```
//!
//! Compaction: with `-R`, group the paths by the ACL they carry — every
//! distinct ACL once, commonest first, with its count and a few example
//! paths. A tree's ACLs are the same almost everywhere and the reason to run
//! `-R` is to find where they are not, so a single differing file survives a
//! tree of thousands. A single path has nothing to group and keeps the older
//! fold: drop the `# owner:` / `# group:` boilerplate.


pub fn detect(text: &str) -> bool {
    text.contains("# file:")
        && (text.contains("user::") || text.contains("group::") || text.contains("mask::"))
}

crate::archetypes::detect_gated_try_compact!();

/// Distinct ACLs shown in full before the listing tallies the rest, and how
/// many example paths each one carries.
const MAX_GROUPS: usize = 40;
const EXAMPLES: usize = 3;

/// Fold a `getfacl -R` tree by the ACL itself, not by position.
///
/// Dropping two boilerplate lines out of seven was the whole fold, for one
/// path and for a tree alike: measured 2026-09-04 on `getfacl -R crates`,
/// 265 874 bytes came back as 123 928, while the same bytes under a program
/// name tokenade does not know came back as 7 702. Claiming the output and
/// then doing 16x worse than no compactor at all is the worse of the two
/// outcomes, because `dispatch_named` returns as soon as it shrank anything
/// and the generic pass never runs.
///
/// Grouping rather than truncating, because of what the command is FOR. A
/// tree's ACLs are identical almost everywhere and the reason to run `-R` is
/// to find the paths where they are not. A head-and-tally fold keeps the
/// first N records — an arbitrary set — and drops exactly the outliers the
/// question was about. Here every distinct ACL is shown once, rarest last,
/// with the count and a few example paths, so a single differing file
/// survives a tree of thousands.
fn group_by_acl(text: &str) -> Option<String> {
    use std::fmt::Write;

    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut records = 0usize;

    for record in text.split("\n\n") {
        let record = record.trim_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut lines = record.lines();
        let Some(first) = lines.next() else { continue };
        let Some(path) = first.strip_prefix("# file: ") else {
            // A record that does not open with a file header is not one of
            // ours; refuse the whole fold rather than silently dropping it.
            return None;
        };
        records += 1;
        let acl = lines.collect::<Vec<_>>().join("\n");
        let entry = groups.entry(acl.clone()).or_insert_with(|| {
            order.push(acl);
            Vec::new()
        });
        entry.push(path.to_string());
    }

    // One ACL for one path is what a non-recursive call prints, and there is
    // nothing to group.
    if records < 2 || order.len() == records {
        return None;
    }

    // Rarest FIRST, and the cap therefore drops the commonest. Sorting the
    // other way round reads better and defeats the purpose: with more distinct
    // ACLs than the cap, it is the outliers that fall off the end, and the
    // outliers are what `-R` was run to find. What the cap drops is tallied by
    // paths as well as by groups, so the bulk of the tree is still accounted
    // for when it is the bulk that got elided.
    order.sort_by_key(|acl| groups[acl].len());
    let distinct = order.len();
    let mut out = format!("[tokenade:getfacl] {records} path(s), {distinct} distinct ACL(s)\n");
    for acl in order.iter().take(MAX_GROUPS) {
        let paths = &groups[acl];
        let _ = write!(out, "\n## {} path(s)\n{acl}\n  e.g. ", paths.len());
        let shown = paths.len().min(EXAMPLES);
        out.push_str(&paths[..shown].join(", "));
        if paths.len() > shown {
            let _ = write!(out, ", … (+{})", paths.len() - shown);
        }
        out.push('\n');
    }
    if distinct > MAX_GROUPS {
        let covered: usize = order[MAX_GROUPS..].iter().map(|a| groups[a].len()).sum();
        let _ = write!(
            out,
            "\n  ... (+{} more distinct ACL(s), the commonest, covering {covered} path(s))",
            distinct - MAX_GROUPS
        );
    }
    Some(out.trim_end().to_string())
}

pub fn compact(text: &str) -> String {
    if let Some(grouped) = group_by_acl(text) {
        return grouped;
    }
    crate::archetypes::elide_prefixed(
        text,
        &["# owner:", "# group:"],
        true,
        "owner/group boilerplate line(s)",
    )
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# file: foo\n# owner: alice\n# group: alice\nuser::rw-\nuser:bob:rw-\ngroup::r--\nmask::rw-\nother::r--\n";

    #[test]
    fn detects_getfacl() {
        assert!(detect(SAMPLE));
    }

    /// One record per file. `odd` paths get an extra ACL entry, which is the
    /// thing a `-R` listing is run to find.
    fn tree(n: usize, odd: &[usize]) -> String {
        (0..n)
            .map(|i| {
                let extra = if odd.contains(&i) {
                    "user:deploy:rwx\nmask::rwx\n"
                } else {
                    ""
                };
                format!(
                    "# file: src/mod{i:04}.rs\n# owner: alice\n# group: alice\n\
                     user::rw-\n{extra}group::r--\nother::r--\n"
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The recursive form folds by ACL, and the rare one survives.
    ///
    /// Both halves are the point. Eliding two boilerplate lines out of seven
    /// is what the module used to do, and on a tree it left 16x more than the
    /// generic pass would have — a compactor that claims the output has to
    /// beat the one that would have run instead. And a head-and-tally fold
    /// would have passed the size assertion while dropping record 399, which
    /// is the only one worth reading.
    #[test]
    fn a_tree_folds_by_acl_and_the_outlier_survives() {
        let many = tree(400, &[399]);
        let boilerplate_only = crate::archetypes::elide_prefixed(
            &many,
            &["# owner:", "# group:"],
            true,
            "owner/group boilerplate line(s)",
        );
        let out = compact(&many);

        assert!(
            out.starts_with("[tokenade:getfacl] 400 path(s), 2 distinct ACL(s)"),
            "{out:.90}"
        );
        assert!(
            out.len() * 10 < boilerplate_only.len(),
            "the ACL fold must be far smaller than the boilerplate pass ({} vs {})",
            out.len(),
            boilerplate_only.len()
        );
        assert!(
            out.contains("user:deploy:rwx") && out.contains("src/mod0399.rs"),
            "the one path whose ACL differs must be named: {out}"
        );
        assert!(out.contains("399 path(s)"), "the common group carries its count");
    }

    /// Every distinct ACL is shown, and the examples are bounded.
    #[test]
    fn each_group_names_a_few_paths_and_counts_the_rest() {
        let out = compact(&tree(400, &[7, 8, 9]));
        assert!(out.contains("## 397 path(s)"), "{out:.120}");
        assert!(out.contains("## 3 path(s)"));
        assert!(
            out.contains("src/mod0007.rs, src/mod0008.rs, src/mod0009.rs"),
            "a group smaller than the example cap lists all of it: {out}"
        );
        assert!(out.contains("… (+394)"), "the big group counts what it did not name");
    }

    /// More distinct ACLs than the cap: the RARE ones are what survive.
    #[test]
    fn the_cap_drops_the_commonest_not_the_outliers() {
        // 45 distinct ACLs. The bigger the index, the more paths carry it, so
        // the commonest are the ones the cap must drop.
        // Written commonest FIRST, so insertion order is not the answer: a
        // fold that skips the sort keeps whatever came first.
        let mut text = String::new();
        for g in (0..45).rev() {
            for p in 0..=g {
                text.push_str(&format!(
                    "# file: src/g{g:02}_{p:02}.rs\n# owner: alice\n# group: alice\n\
                     user::rw-\nuser:svc{g:02}:r--\ngroup::r--\nother::r--\n\n"
                ));
            }
        }
        let out = compact(&text);
        assert!(
            out.contains("user:svc00:r--"),
            "the rarest ACL (one path) must survive the cap: {out:.200}"
        );
        assert!(
            !out.contains("user:svc44:r--"),
            "the commonest is what the cap drops"
        );
        // 45 groups of 1..45 paths; the cap keeps the 40 rarest (1..40), so
        // the five dropped cover 41+42+43+44+45.
        assert!(
            out.contains("(+5 more distinct ACL(s), the commonest, covering 215 path(s))"),
            "what was dropped is tallied by paths too: {out}"
        );
    }

    /// A block that does not open with `# file:` is not a record of ours, and
    /// grouping it would attribute an ACL to a path that is not there. The
    /// whole fold is refused and the older one runs.
    #[test]
    fn a_block_without_a_file_header_refuses_the_grouping() {
        // Three real records so the grouping would otherwise fire: with a
        // single one, it is declined for having nothing to group and the
        // header rule is never the reason.
        let mut text = String::new();
        for i in 0..3 {
            text.push_str(&format!(
                "# file: src/f{i}.rs\n# owner: alice\n# group: alice\n\
                 user::rw-\ngroup::r--\nother::r--\n\n"
            ));
        }
        text.push_str("some trailing note\nwith two lines\n");
        let out = compact(&text);
        assert!(!out.contains("[tokenade:getfacl]"), "{out}");
        assert!(out.contains("some trailing note"), "nothing is dropped: {out}");
    }

    /// A single path is one record with nothing to group. The fold stays what
    /// it always was.
    #[test]
    fn one_path_keeps_the_boilerplate_fold() {
        let out = compact(SAMPLE);
        assert!(!out.contains("[tokenade:getfacl]"), "{out}");
        assert!(out.contains("user:bob:rw-"), "the ACL itself is untouched");
        assert!(!out.contains("# owner:"));
    }

    #[test]
    fn drops_owner_group_boilerplate() {
        let out = compact(SAMPLE);
        assert!(out.contains("user::rw-"));
        assert!(out.contains("user:bob:rw-"));
        assert!(!out.contains("# owner:"));
        assert!(!out.contains("# group:"));
    }
}
