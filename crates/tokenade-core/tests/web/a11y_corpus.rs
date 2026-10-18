//! The a11y compactor, measured on REAL browser snapshots.
//!
//! The unit test in `web.rs` builds its own React-shaped sample and asserts
//! >25%. Production told a different story (~7% on user traffic), which is the
//! > classic teach-to-test gap: proved on the shape it invents, not the shape it
//! > meets. These fixtures are actual `browser_snapshot` output — Playwright
//! > 1.61, `ariaSnapshot({ mode: "ai" })`, the exact format the Playwright MCP
//! > server returns (refs, `[cursor=pointer]`, `/url:` lines and all).
//!
//! Capture recipe (see `fixtures/a11y/README.md`) is reproducible: any new page
//! dropped in that directory is picked up by this test automatically.
//!
//! # Where the remaining bulk goes — three levers MEASURED AND REJECTED
//!
//! At 41,4 % corpus-wide the leftover is profiled (`dump_compacted` below):
//! **63–68 % is leaf text**, i.e. what the page actually says. Compressing that
//! further means dropping content, not redundancy. The three structural levers
//! that look promising were each measured under cl100k on 2026-08-03 — do not
//! re-derive them:
//!
//! 1. **Narrowing the indent** (2 spaces/level → 1): **0,1 %.** cl100k merges
//!    newline+indent into a single token, so indentation is very nearly free.
//!    Stripping it entirely reads as −7,6 %, but that number is a token-boundary
//!    artefact of the first word re-merging with the newline — and it would cost
//!    the parent/child structure, which is the whole point of an a11y tree (see
//!    `a11y_never_reparents_a_node`).
//! 2. **Folding common `/url:` prefixes** behind a legend: ~2 % corpus-wide and
//!    wildly uneven (38 % of URL bulk on MDN, 31 % on GitHub, 2 % on Wikipedia
//!    and HN). Rejected on risk, not on principle: a URL is the one thing on the
//!    page the agent may act on, and a mangled one is worse than the 2 %.
//! 3. **Deduping exact-duplicate `/url:` lines** (nav repeated header/footer):
//!    **0,5 %.** Real pages barely repeat a link verbatim.
//!
//! So: no cheap headroom left here. A future gain has to come from the leaf
//! text, which means a content decision, which is the PO's call — not a
//! compactor tweak.

use std::path::PathBuf;

fn fixtures() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/a11y");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("fixtures/a11y missing")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".a11y.txt"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (name, std::fs::read_to_string(&p).expect("read fixture"))
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no a11y fixtures found");
    out
}

/// Every real page must clear a floor, and the corpus as a whole a higher one.
/// The per-page floor is deliberately the weakest page: a snapshot that is
/// already mostly unique text has little to give, and pretending otherwise is
/// how the 25%/7% gap was born.
///
/// These floors read LOWER than the ones this test shipped with (28 %/35 %) and
/// nothing about the compactor changed. The estimator did: it used to price a
/// tree at a flat 4 chars/token and the compacted output the same way, which
/// over-stated this corpus by +6,1 pt (46 % announced vs 29,6 % under cl100k on
/// the worst page). `tests/estimator_gain_fidelity.rs` is the gate that keeps
/// the two agreeing; these numbers are now the ones the model actually sees.
#[test]
fn a11y_compaction_holds_up_on_real_pages() {
    let mut total_before = 0usize;
    let mut total_after = 0usize;
    let mut weak = Vec::new();
    for (name, snap) in fixtures() {
        let (_out, sav) = tokenade_core::web::compact_a11y_tree_with_savings(&snap);
        println!(
            "{name}: {} -> {} tokens ({:.1}%)",
            sav.tokens_before,
            sav.tokens_after,
            sav.percent()
        );
        if sav.percent() < 30.0 {
            weak.push(format!("{name} {:.1}%", sav.percent()));
        }
        total_before += sav.tokens_before;
        total_after += sav.tokens_after;
    }
    let corpus = 100.0 * (total_before - total_after) as f64 / total_before as f64;
    println!("corpus: {total_before} -> {total_after} tokens ({corpus:.1}%)");
    assert!(weak.is_empty(), "pages below the 30% floor: {weak:?}");
    assert!(
        corpus >= 36.0,
        "corpus savings fell to {corpus:.1}% (floor 36%)"
    );
}

/// Indentation IS the tree. Dropping a wrapper without giving its slot back to
/// its children leaves them stranded one level too deep, where they read as
/// children of whatever sibling came before — the agent is then told a button
/// lives inside a dialog it is not in. Nothing may gain an ancestor it did not
/// have.
#[test]
fn a11y_never_reparents_a_node() {
    fn chains(text: &str) -> std::collections::HashMap<String, Vec<String>> {
        let mut stack: Vec<(usize, String)> = Vec::new();
        let mut out = std::collections::HashMap::new();
        for line in text.lines() {
            let trim = line.trim_start();
            if trim.is_empty() {
                continue;
            }
            let indent = line.len() - trim.len();
            while stack.last().is_some_and(|(i, _)| *i >= indent) {
                stack.pop();
            }
            let Some(r) = line.find("[ref=e") else {
                continue;
            };
            let end = line[r..].find(']').map(|i| r + i).unwrap_or(line.len());
            let reference = line[r + 6..end].to_string();
            out.insert(
                reference.clone(),
                stack.iter().map(|(_, r)| r.clone()).collect(),
            );
            stack.push((indent, reference));
        }
        out
    }
    for (name, snap) in fixtures() {
        let before = chains(&snap);
        let after = chains(&tokenade_core::web::compact_a11y_tree(&snap));
        for (reference, ancestors) in after {
            let Some(was) = before.get(&reference) else {
                continue;
            };
            // The kept ancestors must appear, in order, among the original
            // ones: dropping ancestors is fine, inventing one is not.
            let mut it = was.iter();
            for a in &ancestors {
                assert!(
                    it.any(|w| w == a),
                    "{name}: e{reference} is now under e{a}, which was never its ancestor \
                     (was {was:?}, now {ancestors:?})"
                );
            }
        }
    }
}

/// Compaction may not cost the agent its ability to act on the page: the refs
/// of interactive elements are the only handle `browser_click` has.
#[test]
fn a11y_keeps_the_refs_an_agent_clicks() {
    for (name, snap) in fixtures() {
        let out = tokenade_core::web::compact_a11y_tree(&snap);
        for line in snap.lines() {
            let t = line.trim_start();
            let interactive = t.starts_with("- button")
                || t.starts_with("- textbox")
                || t.starts_with("- combobox")
                || t.starts_with("- checkbox");
            if !interactive {
                continue;
            }
            let Some(r) = line.find("[ref=e") else {
                continue;
            };
            let end = line[r..].find(']').map(|i| r + i + 1).unwrap_or(line.len());
            let reference = &line[r..end];
            assert!(
                out.contains(reference),
                "{name}: dropped the ref of an interactive element: {}",
                t.trim()
            );
        }
    }
}

/// Text the user can read on the page must survive compaction. Headings are the
/// load-bearing case: they are how an agent decides where it is.
#[test]
fn a11y_keeps_every_heading_on_real_pages() {
    for (name, snap) in fixtures() {
        let out = tokenade_core::web::compact_a11y_tree(&snap);
        for line in snap.lines() {
            let t = line.trim_start();
            if !t.starts_with("- heading \"") {
                continue;
            }
            let text = t.trim_start_matches("- heading \"");
            let Some(q) = text.find('"') else { continue };
            let text = &text[..q];
            if text.len() < 4 {
                continue;
            }
            assert!(
                out.contains(text),
                "{name}: lost heading {text:?} in compaction"
            );
        }
    }
}

/// A ref is dropped for the ROLE, never for the node. Two things must hold and
/// neither is obvious from the change that introduced them.
///
/// The snapshot marks with `[cursor=…]` the elements that respond to a click
/// but whose role does not say so — a `listitem` that is really a menu entry, a
/// `cell` that is really a button. Those keep their ref even though the role is
/// on the dropped list.
///
/// And the drop must happen LAST. A ref is what makes two otherwise identical
/// lines distinct; strip them before the folding passes and those passes merge
/// nodes that were never the same, re-attaching children under a node that was
/// never their parent. That was observed, on the Wikipedia fixture, the first
/// time this was written.
#[test]
fn a_node_the_snapshot_marks_clickable_keeps_its_ref() {
    let snap = "\
- main [ref=e1]:
  - paragraph [ref=e2]: Ordinary prose that nobody clicks.
  - listitem [ref=e3] [cursor=pointer]: Really a menu entry
  - listitem [ref=e4]: Really a list item
  - button \"Go\" [ref=e5]
";
    let out = tokenade_core::web::compact_a11y_tree(snap);
    assert!(
        out.contains("[ref=e3]"),
        "lost a marked-clickable ref:\n{out}"
    );
    assert!(out.contains("[ref=e5]"), "lost a button ref:\n{out}");
    assert!(!out.contains("[ref=e2]"), "kept a paragraph ref:\n{out}");
    assert!(
        !out.contains("[ref=e4]"),
        "kept a plain list-item ref:\n{out}"
    );
    assert!(
        out.contains("Ordinary prose") && out.contains("Really a list item"),
        "text must survive the ref drop:\n{out}"
    );
}

/// Not a test: dumps the compacted trees so their remaining bulk can be
/// profiled. `cargo test --test a11y_corpus -- --ignored dump_compacted`.
#[test]
#[ignore = "dump, not a test: used to profile what remains"]
fn dump_compacted() {
    let dir = std::path::Path::new("tests/fixtures/a11y");
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let out = tokenade_core::web::compact_a11y_tree(&raw);
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        std::fs::write(format!("/tmp/a11ydump-{name}"), out).unwrap();
    }
}

/// The same invariant as `a11y_never_reparents_a_node`, on the nodes that have
/// no `[ref=]`.
///
/// That test identifies nodes by their ref, which is exactly the set of nodes a
/// snapshot marks as interactive. Everything else — the text, the headings, the
/// captions, most of what a page actually SAYS — was outside its reach, and a
/// real reparenting bug lived there: a `- listitem:` wrapper glued to its first
/// child left the item's remaining children nested UNDER the sibling they used
/// to sit beside. None of them had a ref.
///
/// Identity here is a quoted name that occurs exactly once on both sides.
/// Ambiguous names are skipped rather than guessed: a wrong pairing would
/// report a reparenting that never happened.
#[test]
fn a11y_never_reparents_a_node_that_has_no_ref() {
    fn unique_named_chains(text: &str) -> std::collections::HashMap<String, Vec<String>> {
        let mut stack: Vec<(usize, String)> = Vec::new();
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut out: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for line in text.lines() {
            let trim = line.trim_start();
            if trim.is_empty() {
                continue;
            }
            let indent = line.len() - trim.len();
            while stack.last().is_some_and(|(i, _)| *i >= indent) {
                stack.pop();
            }
            let Some(open) = trim.find('"') else { continue };
            let Some(len) = trim[open + 1..].find('"') else {
                continue;
            };
            let name = trim[open + 1..open + 1 + len].to_string();
            if name.trim().is_empty() {
                continue;
            }
            *seen.entry(name.clone()).or_insert(0) += 1;
            out.insert(name.clone(), stack.iter().map(|(_, n)| n.clone()).collect());
            stack.push((indent, name));
        }
        out.retain(|k, _| seen.get(k) == Some(&1));
        out
    }
    for (name, snap) in fixtures() {
        let out = tokenade_core::web::compact_a11y_tree(&snap);
        // A wrapper glued to its child prints two bullets on one line. Cheap,
        // and it is the shape the bug actually took.
        for l in out.lines() {
            let t = l.trim_start();
            assert!(
                !t.starts_with("- - "),
                "{name}: a node was glued onto its parent's line: {l:?}"
            );
        }
        let before = unique_named_chains(&snap);
        let after = unique_named_chains(&out);
        let mut checked = 0usize;
        for (node, ancestors) in &after {
            let Some(was) = before.get(node) else {
                continue;
            };
            checked += 1;
            let mut it = was.iter();
            for a in ancestors {
                assert!(
                    it.any(|w| w == a),
                    "{name}: {node:?} is now under {a:?}, which was never its ancestor \
                     (was {was:?}, now {ancestors:?})"
                );
            }
        }
        // A guard that checked nothing would pass silently, which is how a
        // security-shaped test lies.
        assert!(
            checked > 50,
            "{name}: only {checked} unnamed-node chains compared — the identity \
             rule stopped matching and this test proves nothing"
        );
    }
}
