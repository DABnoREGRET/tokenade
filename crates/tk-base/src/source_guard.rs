//! **The source sweep, for the guards that run one: once.**
//!
//! Several tests of this crate ask the same question — "no module
//! spells X on its own" — and each one rewrote its directory
//! walk. MEASURED 2026-09-08: three copies, and a mutation removing
//! `stack.push(p)` SURVIVED in two of them. The guard stopped
//! descending, half of the compactors living under `cmd/` left the
//! perimeter, and nothing said so — a count of swept files does not
//! tell "I descend" from "there are many at the root".
//!
//! The first fix had closed only ONE copy. Hence the class-level fix:
//! a single walk, tested here, and [`rust_sources`] REFUSES to return a
//! result that saw no subdirectory.

use std::path::{Path, PathBuf};

/// Every `.rs` under `root`, with its NON-test part.
///
/// Panics if `root` HAS subdirectories and the walk reported no
/// file from them: a source guard that stopped descending gives a verdict
/// on a perimeter that is no longer its own, and that is how two
/// mutations survived.
///
/// The rule was first "the walk must have seen a nested file",
/// full stop. It then refused a LEGITIMATELY flat directory:
/// `crates/tokenade-bench/src/bin` has no subdirectory, and the first
/// guard placed on it died on that assertion. A rule that
/// forbids a correct case gets defused, which is exactly what it
/// was meant to prevent. What is checked is the AGREEMENT between what the
/// directory contains and what the walk reported from it.
pub fn rust_sources(root: &Path) -> Vec<(PathBuf, String)> {
    // A subdirectory that ITSELF contains a `.rs`. Not "a
    // subdirectory", full stop: `docs/` or an assets folder is one
    // and has nothing to report, so requiring it would refuse a correct tree — and
    // a guard that forbids a correct case gets defused.
    let has_nested_rs = std::fs::read_dir(root)
        .map(|it| {
            it.flatten().filter(|e| e.path().is_dir()).any(|d| {
                std::fs::read_dir(d.path())
                    .map(|inner| {
                        inner
                            .flatten()
                            .any(|f| f.path().extension().and_then(|e| e.to_str()) == Some("rs"))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    let mut stack = vec![root.to_path_buf()];
    let mut out = Vec::new();
    let mut nested = false;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            nested |= p.parent() != Some(root);
            let Ok(src) = std::fs::read_to_string(&p) else {
                continue;
            };
            out.push((p, strip_test_code(&src)));
        }
    }
    assert!(
        nested || !has_nested_rs,
        "{} has a subdirectory carrying `.rs` files and the walk reported none of them: \
         a source guard that no longer descends judges another perimeter",
        root.display()
    );
    assert!(
        !out.is_empty(),
        "no `.rs` under {}: the guard no longer checks anything, and an empty guard \
         always passes",
        root.display()
    );
    out
}

/// The same files, WHOLE — test code included.
///
/// [`rust_sources`] strips the test blocks so that a guard does not match
/// itself in its own expected strings, and that is the right default. But a guard
/// looking for a defect that CAN live in a test then cannot see it:
/// the path-separator guard (`no_source_asks_a_string_whether_a_path_\
/// crosses_a_directory`) was first written on `rust_sources` and its canary
/// stayed SILENT — four of the six sites it had to find, including the one that
/// stopped the 1.1.11 release, are in `#[cfg(test)]` modules.
///
/// The mirror image of an earlier bug: there, the PRODUCTION half of a file was
/// invisible; here, the TEST half. A guard must say which of the two it
/// judges, and the call now says so.
///
/// Whoever uses it carries the burden of not finding itself: skipping
/// comment lines is enough in practice.
pub fn rust_sources_whole(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for (p, _) in rust_sources(root) {
        if let Ok(src) = std::fs::read_to_string(&p) {
            out.push((p, src));
        }
    }
    out
}

/// **Strip the test blocks, without TRUNCATING the file.**
///
/// The rule was long `src.split("#[cfg(test)]").next()`: everything that
/// follows the FIRST test module disappeared, production code included.
/// MEASURED 2026-09-08 on the repo's sources: **1,521,151 bytes of
/// production code out of 10,659,031 — 14 %, spread over 127 files — were
/// invisible to every guard built here**. `filter.rs` alone hid 164,641 of them,
/// including `render_kept`, `note_folded` and `folded_ceiling`, i.e.
/// exactly the mechanism the marker guards check.
///
/// One guard had already paid for this mistake and fixed it IN
/// ITSELF, by looking for `pub fn` at column 0. That was repairing the instance:
/// the seam every guard goes through kept the defect.
///
/// The answer already existed: `srcsweep::production_lines` tracks braces
/// and its own header tells the same lesson, learned on 2026-08-27 on
/// `auto.rs` (ten blocks, the first at line 1041 of 3397). This module
/// wrote a second version twelve days later, and the second one was the
/// wrong one. A single spelling from now on — and `srcsweep` is no longer behind
/// a compile flag, because an answer that cannot be called
/// gets rewritten.
fn strip_test_code(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for (_, line) in crate::srcsweep::production_lines(src) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// **A line that cuts a file at the first `#[cfg(test)]`.**
///
/// The pattern that cost three rewrites: `src.split("#[cfg(test)]").next()`,
/// `body.find("\n#[cfg(test)]")`. It is recognisable — the literal and a
/// cutting verb on the same line — and [`crate::srcsweep`] is the only
/// place in the repo allowed to carry it.
pub fn truncates_at_a_test_block(line: &str) -> bool {
    let l = line.trim_start();
    if l.starts_with("//") {
        return false;
    }
    // The end of the literal, not its start: the second bad rewrite
    // searched for `"\n#[cfg(test)]"`, where the opening quote does not touch
    // the attribute.
    l.contains("#[cfg(test)]\"")
        && (l.contains(".split")
            || l.contains(".find")
            || l.contains(".splitn")
            || l.contains("match_indices")
            || l.contains(".position"))
}

/// **The product's source roots.**
///
/// There have been TWO since 2026-09-13, and it is the split into crates that
/// imposes it: the base layer lives in `tk-base`, the rest in
/// `tokenade-core`. A guard reading only one of the two would say nothing
/// about the 32 modules of the other — and it would not say so: a sweep that
/// finds nothing is green. Six tests of this repo went red on the day of the
/// split, precisely because they were rightly written with a
/// FLOOR on the number of files read.
///
/// The path is computed from this file, not from a current directory:
/// a guard must read the same tree whether it is launched by `cargo test`,
/// by `nextest` or by a bench.
/// The `crates/` directory of this workspace.
///
/// Declared HERE and only once: three guards recomputed it from
/// `CARGO_MANIFEST_DIR`, and a fourth named its files one by one —
/// `every_location_line_asks_the_one_declarer` read three hand-written
/// `include_str!`, and it went red the day the calls moved to another
/// file, accusing the product of having lost call sites.
pub fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// **The only library file carrying `needle`, with its content.**
///
/// Four places in this repo named their subject by a hand-written
/// path — two guards, a `filter` test, and `build.rs`. All four broke
/// on the same day, while splitting the god files, and three did so by ACCUSING the
/// product: "only 2 call site(s) reached", "the sweep reached only 0 arms",
/// and a `build.rs` declaring the dispatch block not found. A test that
/// looks in the wrong place must say it got lost, not that a defect
/// appeared — and a seam quoted VERBATIM in a comment finds
/// itself, as this paragraph checked at its own expense.
///
/// Uniqueness is part of the answer: two files carrying the same seam,
/// and the caller would only talk about one of the two, silently. The sibling
/// `x_tests.rs` files are set aside — a test often SPELLS what it checks.
/// The family of a LIBRARY module, without having to say which
/// crate it lives in.
///
/// `library_module_family("docs")` finds the root carrying `docs.rs` and returns
/// the trunk followed by its branches. That is the call the guards want: they
/// know the module's name, not the crate where the refactoring put it.
pub fn library_module_family(name: &str) -> String {
    let root = crate_srcs()
        .into_iter()
        .find(|r| r.join(format!("{name}.rs")).is_file() || r.join(name).join("mod.rs").is_file())
        .unwrap_or_else(|| panic!("no library root carries `{name}.rs` or `{name}/mod.rs`"));
    module_family(&root, name)
}

/// **A module AND its children, concatenated.**
///
/// `module_family(src, "gain")` returns `gain.rs` followed by everything
/// `gain/` contains — including files that are not Rust, since the
/// dashboard writes part of its logic in JavaScript under `webdash/`.
///
/// Three guards enumerated this family by hand, and two already carried the
/// comment saying why that is fragile — "a split module the
/// sweep does not name is a module the sweep does not see". They
/// broke the day `gain.rs` went from two children to seven; the
/// centralisation guard then looked for a ×5 weighting in the three files
/// it knew, and found none.
///
/// The order is deterministic (the parent first, then the sorted children) so
/// that a match counter is reproducible.
pub fn module_family(src_root: &Path, name: &str) -> String {
    let dir = src_root.join(name);
    // BOTH layouts. In 2018 the trunk is `name.rs` next to `name/`;
    // in 2015 it is called `name/mod.rs` and its branches are its neighbours.
    // Thirty-five directories of this tree are still in the second form,
    // and a caller asking for the family of `hooks` does not know — nor needs to
    // know — which of the two it targets.
    let flat = src_root.join(format!("{name}.rs"));
    let parent = if flat.is_file() {
        flat
    } else {
        dir.join("mod.rs")
    };
    let mut out = std::fs::read_to_string(&parent).unwrap_or_else(|e| {
        panic!("{} not found: {e}", parent.display());
    });
    // RECURSIVE, and not being so has a cost: a child now has
    // children. `install/gemini_hooks/register.rs` carries `gemini_hook_command`,
    // and a one-level read answered "0 call sites" on a
    // helper called three times — the guard accused the product, again.
    let mut kids: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p != parent {
                kids.push(p);
            }
        }
    }
    kids.sort();
    for k in kids {
        if let Ok(body) = std::fs::read_to_string(&k) {
            out.push('\n');
            out.push_str(&body);
        }
    }
    out
}

pub fn the_one_source_carrying(needle: &str) -> (PathBuf, String) {
    one_of(crate_srcs(), needle, "the library")
}

/// The same thing under ONE named root — for the binary crates, which
/// [`crate_srcs`] deliberately does not include.
pub fn the_one_source_under(root: &Path, needle: &str) -> (PathBuf, String) {
    one_of(
        vec![root.to_path_buf()],
        needle,
        &root.display().to_string(),
    )
}

fn one_of(roots: Vec<PathBuf>, needle: &str, where_: &str) -> (PathBuf, String) {
    let mut hits: Vec<(PathBuf, String)> = Vec::new();
    for root in roots {
        for (p, body) in rust_sources_whole(&root) {
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
            {
                continue;
            }
            if body.contains(needle) {
                hits.push((p, body));
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`{needle}` must live in a single file of {where_} — found: {:?}",
        hits.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    hits.pop().unwrap()
}

/// The crate root — library **or binary** — that carries this module.
///
/// [`crate_srcs`] only knows the library, on purpose: what most
/// guards measure is the library. This one is for the guards that
/// also name modules of the CLI or the bench, and which until now wrote the
/// path by hand — `crates/tokenade-core/src/json.rs`, which ceased
/// to exist the day `json` moved into `tk-formats` and which made
/// a test PANIC instead of making it say it was lost.
///
/// Uniqueness is checked: two crates carrying `foo.rs`, and the caller would
/// only talk about one of the two, silently.
pub fn workspace_module_root(name: &str) -> PathBuf {
    let crates = crates_dir();
    let mut hits: Vec<PathBuf> = std::fs::read_dir(crates.as_path())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("src"))
        .filter(|r| r.join(format!("{name}.rs")).is_file() || r.join(name).join("mod.rs").is_file())
        .collect();
    hits.sort();
    assert_eq!(
        hits.len(),
        1,
        "`{name}`: {} crate root(s) carry it — {hits:?}",
        hits.len()
    );
    hits.pop().unwrap_or_default()
}

/// The trunk of a workspace module: `name.rs`, or `name/mod.rs` in 2015.
pub fn workspace_module_trunk(name: &str) -> PathBuf {
    let root = workspace_module_root(name);
    let flat = root.join(format!("{name}.rs"));
    if flat.is_file() {
        flat
    } else {
        root.join(name).join("mod.rs")
    }
}

/// The module AND its children, without having to say which crate it lives in —
/// library or binary. The counterpart of [`library_module_family`] for the
/// guards that cross the whole workspace.
pub fn workspace_module_family(name: &str) -> String {
    module_family(&workspace_module_root(name), name)
}

pub fn crate_srcs() -> Vec<PathBuf> {
    let crates = crates_dir();
    let crates = crates.as_path();
    // DERIVED from the directory, not hand-written. A list of names is
    // exactly what lets the next crate slip through: `tk-base` on the
    // morning of 2026-09-13, `tk-fold` in the afternoon, and a guard that does not read
    // a root does not say so — it is green.
    //
    // The convention IS the rule: a library layer is called
    // `tk-<layer>`, and `tokenade-core` is the facade gathering them. The
    // binaries (`tokenade-cli`, `tokenade-bench`) are not layers: what these
    // guards measure is the library.
    let mut out: Vec<PathBuf> = std::fs::read_dir(crates)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (n.starts_with("tk-") || n == "tokenade-core") && p.join("src").is_dir()
        })
        .map(|p| p.join("src"))
        .collect();
    out.sort();
    assert!(
        out.len() >= 2,
        "no library root found under {}: a guard that reads \
         nothing is green, and that is the worst of verdicts",
        crates.display()
    );
    out
}

/// The PRODUCTION Rust files of both roots, combined.
pub fn all_rust_sources() -> Vec<(PathBuf, String)> {
    crate_srcs().iter().flat_map(|r| rust_sources(r)).collect()
}

/// Same, but the WHOLE file — tests included.
pub fn all_rust_sources_whole() -> Vec<(PathBuf, String)> {
    crate_srcs()
        .iter()
        .flat_map(|r| rust_sources_whole(r))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_descends_and_drops_test_code() {
        let files = all_rust_sources();
        assert!(files.len() > 100, "only {} file(s)", files.len());
        // It DESCENDS: half of the compactors live under `cmd/`.
        assert!(
            files
                .iter()
                .any(|(p, _)| crate::platform::path_has_segment(p, "cmd")),
            "no file under `cmd/`: the walk does not descend"
        );
        // And it cuts at the first test module, so that a guard does not
        // match itself in its own expected strings.
        let (_, code) = files
            .iter()
            .find(|(p, _)| p.file_name().is_some_and(|n| n == "stores.rs"))
            .expect("stores.rs");
        assert!(!code.contains("every_store_override_is_listed"));
    }

    /// Nobody asks a STRING again whether a path goes through a directory.
    ///
    /// `p.to_string_lossy().contains("/cmd/")` is wrong on Windows, where the
    /// separator is `\`. This repo carried SIX of them, and only one turned
    /// a test red — the one just above, which stopped the 1.1.11 release after
    /// three platforms were built. The other five silently answered
    /// NO: the bench declared a release binary `debug`, and its refusal
    /// of a stale instrument was no longer recognised under `cargo test`. A
    /// defect that breaks nothing is the one that stays.
    ///
    /// The remedy is `platform::path_has_segment`, which compares segments and
    /// cannot get the platform wrong.
    #[test]
    fn no_source_asks_a_string_whether_a_path_crosses_a_directory() {
        let mut hits = Vec::new();
        for (path, code) in all_rust_sources_whole() {
            for (n, line) in code.lines().enumerate() {
                let l = line.trim_start();
                // The DOCUMENTATION line explaining the defect is not the
                // defect. The tree already went red once on its own
                // explanation.
                if l.starts_with("//") {
                    continue;
                }
                if !line.contains("to_string_lossy()") {
                    continue;
                }
                // `contains("…/…")` or `contains("…\\…")` on a path's
                // text: both spellings, because fixing one by
                // writing the other would be the same mirrored defect.
                let asks = ["contains(\"", "starts_with(\"", "ends_with(\""]
                    .iter()
                    .filter_map(|k| line.split_once(k))
                    .any(|(_, rest)| {
                        rest.split_once('"')
                            .is_some_and(|(arg, _)| arg.contains('/') || arg.contains('\\'))
                    });
                if asks {
                    hits.push(format!("{}:{}  {}", path.display(), n + 1, l));
                }
            }
        }
        assert!(
            hits.is_empty(),
            "{} site(s) ask a string what `platform::path_has_segment` \
             answers without a separator:\n  {}",
            hits.len(),
            hits.join("\n  ")
        );
    }

    /// A walk that does not descend is a REFUSAL, not an empty result.
    #[test]
    fn a_walk_that_stopped_descending_refuses() {
        // The rule changed and this test with it. It said "a
        // flat tree is refused", which refused
        // `crates/tokenade-bench/src/bin` — flat for good reasons — and
        // killed the first guard placed on it. What is checked is the AGREEMENT:
        // if a subdirectory carries `.rs` files, the walk must report some.
        let root = std::env::temp_dir().join(format!("tokenade-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("sub/b.rs"), "fn b() {}").unwrap();
        assert_eq!(rust_sources(&root).len(), 2, "both levels are reported");
        // A subdirectory WITHOUT `.rs` — assets, documentation — does not
        // make the walk fail: it had nothing to report.
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/x.png"), "pas du rust").unwrap();
        assert_eq!(rust_sources(&root).len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A LEGITIMATELY flat directory is not a broken walk.
    ///
    /// `crates/tokenade-bench/src/bin` has no subdirectory, and the
    /// first version of this rule refused to sweep it. A guard that
    /// forbids a correct case gets defused — exactly what it
    /// was meant to prevent.
    #[test]
    fn a_legitimately_flat_directory_is_accepted() {
        let dir = std::env::temp_dir().join(format!("tokenade-flat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.join("b.rs"), "fn b() {}\n").unwrap();
        assert_eq!(rust_sources(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **Production code that FOLLOWS a test module must be seen.**
    ///
    /// A truncation at the first `#[cfg(test)]` makes everything that
    /// follows invisible, and a guard that does not see what it checks is green.
    ///
    /// This probe named ONE file and ONE function — `filter.rs` and
    /// `folded_ceiling`. The anchor followed the function when `folded_ceiling`
    /// moved into `fold_count`, and the probe went red without a
    /// single defect existing. A hard-coded anchor measures the place it
    /// points to, not the property. So it now sweeps ALL the files
    /// carrying the shape — a test block, then production code — and
    /// requires a REACH FLOOR, without which a tree where the shape
    /// no longer existed would make it green for nothing.
    #[test]
    fn production_code_after_a_test_module_is_still_seen() {
        // ALL the crates, binaries included: what is measured here is the
        // SWEEP, and the guards of `tokenade-cli` as well as those of
        // `tokenade-bench` apply it to their own sources. The layers
        // (`crate_srcs`) are the right scope for an architecture guard,
        // not for this one.
        let mut whole: Vec<(PathBuf, String)> = Vec::new();
        for e in std::fs::read_dir(crates_dir())
            .into_iter()
            .flatten()
            .flatten()
        {
            let src = e.path().join("src");
            if src.is_dir() {
                whole.extend(rust_sources_whole(&src));
            }
        }
        let mut reached = Vec::new();
        let mut lost = Vec::new();
        let mut leaked = Vec::new();
        for (p, src) in whole {
            // A sibling `*_tests.rs` is mounted under `#[cfg(test)]` by its
            // trunk: ALL its content is test, and looking in it for "production
            // code after a test block" accuses a helper function.
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
            {
                continue;
            }
            let out = &crate::srcsweep::production_source(&src);
            // `contains` is not enough here: `srcsweep` and `web` carry the
            // string `"#[cfg(test)]"` as production DATA — one sweeps
            // sources, the other quotes them. Searching for it without telling code
            // from literal accused two correct files, the same mistake as
            // the one below, in the other direction.
            if has_top_level_test_attr(out) {
                leaked.push(p.display().to_string());
            }
            // The file's LAST test block, then a top-level `pub fn`
            // after it: that is exactly what a truncation
            // would take away.
            //
            // `top_level_fns_after` ignores the inside of string literals,
            // and that is not a precaution. Without it, the probe read
            // `pub fn outer` INSIDE the string of a `treesitter.rs` test —
            // `let src = "pub fn outer() { … }"` — then declared lost a
            // function that never existed. A probe that takes a test's data
            // for code measures the test.
            let Some(name) = top_level_fn_after_last_test_block(&src) else {
                continue;
            };
            reached.push(format!("{}::{name}", p.display()));
            if !out.contains(&name) {
                lost.push(format!("{} : `{name}`", p.display()));
            }
        }
        assert!(
            reached.len() >= 4,
            "the probe only reached {} file(s) carrying the shape \"test \
             block then production code\"; it used to find 9, then 6 once \
             widened to all crates. Below \
             this floor it no longer proves anything, and the SYNTHETIC proof is \
             in `srcsweep` — `a_test_module_before_production_code_does_not_\
             hide_it`: {reached:?}",
            reached.len()
        );
        assert!(
            lost.is_empty(),
            "production code that FOLLOWS a test block disappeared from the \
             sweep:\n  {}",
            lost.join("\n  ")
        );
        assert!(
            leaked.is_empty(),
            "a test block survived the sweep, so a guard reading these \
             sources would judge test code:\n  {}",
            leaked.join("\n  ")
        );
    }

    /// A `#[cfg(test)]` that is CODE: at the start of a line, and not
    /// inside a string literal.
    fn has_top_level_test_attr(src: &str) -> bool {
        let mut in_str = false;
        for line in src.lines() {
            let was = in_str;
            let mut esc = false;
            for c in line.chars() {
                if esc {
                    esc = false;
                    continue;
                }
                match c {
                    '\\' => esc = true,
                    '"' => in_str = !in_str,
                    _ => {}
                }
            }
            if !was && line.starts_with("#[cfg(test)]") {
                return true;
            }
        }
        false
    }

    /// The name of the first TOP-LEVEL `pub fn` following the file's last
    /// `#[cfg(test)]` — ignoring what is inside a string
    /// literal, since a test often carries Rust code as data.
    /// The top-level declarations the probe recognises after a test
    /// block.
    ///
    /// It only knew `pub fn`. Fixing the orphaned doc comments
    /// moved five test modules down to the END of their
    /// file — which is their right place — and the shape dropped from 9
    /// files to 3, below the floor. Yet the measured property is not
    /// "a PUBLIC function survives", it is "a production ITEM that
    /// follows a test block survives": a truncated constant is lost all
    /// the same. The five shapes below measure it just as well.
    ///
    /// The shape has become RARE — putting tests at the end of the file is
    /// the best thing to do — so the probe now reads ALL the
    /// crates, binaries included, and finds 6; the floor is 4. What
    /// it proves is that the shape, where it exists in this tree,
    /// goes through the sweep; what proves it IN EVERY CASE is
    /// `srcsweep`'s synthetic fixture, which needs no real file.
    const TOP_LEVEL_ITEM_KINDS: &[&str] = &["pub fn", "pub(crate) fn", "pub struct", "pub const"];

    fn top_level_fn_after_last_test_block(src: &str) -> Option<String> {
        let mut in_str = false;
        let mut seen_test = false;
        let mut attributed = false;
        let mut last: Option<String> = None;
        for line in src.lines() {
            let was_in_str = in_str;
            // Parity of unescaped quotes: enough to tell
            // "line of code" from "line inside a string".
            let mut prev_escape = false;
            for c in line.chars() {
                if prev_escape {
                    prev_escape = false;
                    continue;
                }
                match c {
                    '\\' => prev_escape = true,
                    '"' => in_str = !in_str,
                    _ => {}
                }
            }
            if was_in_str {
                continue;
            }
            if line.starts_with("#[cfg(test)]") {
                seen_test = true;
                last = None;
                // The item FOLLOWING the attribute is the one the attribute carries:
                // it is test, and the sweep is right to remove it. Without
                // this skip, `#[cfg(test)] pub(crate) fn f()` — a test helper
                // declared without a module — was counted as lost
                // production, and the probe accused three correct files.
                attributed = true;
                continue;
            }
            if attributed {
                let t = line.trim_start();
                if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
                    continue;
                }
                attributed = false;
                continue;
            }
            if seen_test && last.is_none() {
                for kind in TOP_LEVEL_ITEM_KINDS {
                    let Some(rest) = line.strip_prefix(kind).and_then(|r| r.strip_prefix(' '))
                    else {
                        continue;
                    };
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        last = Some(format!("{kind} {name}"));
                        break;
                    }
                }
            }
        }
        last
    }

    /// The exact shape: code, a test block, more code.
    #[test]
    fn a_test_block_is_cut_out_and_not_cut_off() {
        let src = "pub fn a() {\n    1\n}\n\n#[cfg(test)]\nmod t {\n    #[test]\n    fn x() {}\n}\n\npub fn b() {\n    2\n}\n";
        let out = strip_test_code(src);
        assert!(out.contains("pub fn a"), "{out}");
        assert!(
            out.contains("pub fn b"),
            "the code after it was lost: {out}"
        );
        assert!(!out.contains("mod t"), "{out}");
    }

    /// But a directory EMPTY of `.rs` is refused: a guard that finds
    /// nothing always passes, and that is the failure mode it cannot
    /// report on its own.
    #[test]
    fn a_walk_that_finds_nothing_refuses() {
        let dir = std::env::temp_dir().join(format!("tokenade-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "rien\n").unwrap();
        let r = std::panic::catch_unwind(|| rust_sources(&dir));
        assert!(r.is_err(), "an empty sweep must refuse");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod one_declarer_tests {
    use super::*;

    /// The detector must REACH the rule it tests: a hand-written
    /// counter-example, otherwise the repo guard below can be green
    /// for having recognised nothing.
    #[test]
    fn the_detector_fires_on_the_shape_that_cost_three_writes() {
        assert!(truncates_at_a_test_block(
            r##"        let code = src.split("#[cfg(test)]").next().unwrap_or(&src);"##
        ));
        assert!(truncates_at_a_test_block(
            r##"    match body.find("\n#[cfg(test)]") {"##
        ));
        // Neither a comment, nor a read that cuts nothing.
        assert!(!truncates_at_a_test_block(
            r##"        // src.split("#[cfg(test)]") est interdit"##
        ));
        assert!(!truncates_at_a_test_block(
            r##"        assert!(!filter.contains("#[cfg(test)]"));"##
        ));
    }

    /// **A single spelling of "which half of this file ships?".**
    ///
    /// MEASURED 2026-09-08: the cut at the first `#[cfg(test)]` made
    /// **1,521,151 of the 10,659,031 bytes of production code — 14 %, over 127
    /// files — invisible** to the guard carrying it. `filter.rs` alone
    /// hid 164,641 of them, including `render_kept`, `note_folded` and
    /// `folded_ceiling`. The rule was learned on 2026-08-27, written in
    /// `srcsweep`, then rewritten wrongly twice: here and in
    /// the `duplication` instrument. It is this guard, not memory, that
    /// prevents the fourth.
    #[test]
    fn only_srcsweep_may_spell_the_split() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let mut swept = 0usize;
        let mut guilty: Vec<String> = Vec::new();
        for (path, code) in rust_sources(&root) {
            if path.file_name().and_then(|f| f.to_str()) == Some("srcsweep.rs") {
                continue;
            }
            swept += 1;
            for (n, line) in code.lines().enumerate() {
                if truncates_at_a_test_block(line) {
                    guilty.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(
            swept > 400,
            "only {swept} file(s) swept: the guard stopped descending"
        );
        assert!(
            guilty.is_empty(),
            "\"which half of this file ships?\" has ONE declarer, \
             `srcsweep::production_lines`, which tracks braces. Cutting at the first \
             `#[cfg(test)]` makes everything after it invisible — 14 % of this repo's \
             production code, measured:\n  {}",
            guilty.join("\n  ")
        );
    }
}

/// **The repo's capture corpus, named once.**
///
/// `crates/tokenade-core/tests/fixtures/` holds the REAL outputs of
/// commands (`ps aux`, `npm build`, `ioreg`, `top`…) against which the
/// folds are measured. It is not a crate's asset: it is the
/// repo's asset, and tests of several layers read it.
///
/// Before the split, each reached it through
/// `env!("CARGO_MANIFEST_DIR")/tests/fixtures/…` — an expression whose
/// value changes with the crate compiling the file. Five
/// `tk-fold` tests therefore looked for the corpus in `crates/tk-fold/tests/`, which
/// does not exist, and failed on `No such file or directory`: loud,
/// hence honest, but it is the same class of mistake as a path relative to the
/// current directory.
pub fn fixtures_dir() -> PathBuf {
    let d = crates_dir()
        .join("tokenade-core")
        .join("tests")
        .join("fixtures");
    assert!(
        d.is_dir(),
        "the capture corpus is not found at {}: a test that reads nothing \
         measures nothing",
        d.display()
    );
    d
}
