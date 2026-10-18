//! `docs/ARCHITECTURE.md` is the map. A map that shows a demolished building
//! costs real work.
//!
//! It did. Until 2026-08-04 the doc's top diagram, a whole crate section, the
//! data-flow walkthrough, the add-a-feature checklist and the file map all
//! described `tokenade-mcp` — a JSON-RPC MCP **server** crate that had been
//! removed months earlier. Two modules were written against that phantom:
//! `cache_align`, to keep "tokenade's own MCP `instructions` string"
//! cache-stable, and `path_safety`, to confine "user-input paths reaching MCP
//! handlers". Both were documented, tested, and called from nowhere; each read
//! like a protection that was not there, and both had to be deleted.
//!
//! So: every source path the map names must exist.

use std::collections::BTreeSet;
use std::path::Path;

/// Illustrative names the doc uses on purpose, with the reason. A path here is
/// a promise that it is a placeholder, not a file.
const PLACEHOLDERS: &[(&str, &str)] = &[(
    "crates/tokenade-core/src/yourformat.rs",
    "the add-a-compactor checklist names the file the READER will create",
)];

fn doc() -> String {
    read_doc("docs/ARCHITECTURE.md")
}

fn read_doc(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{rel} must exist: {e}"));
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// `tokenade install --help` is the first thing a new user reads. It claimed,
/// until 2026-08-04, that install "registers tokenade-mcp with each one" and
/// "writes per-project CLAUDE.md scaffold linking tokenade MCP tools" — a
/// server, and a scaffold, that stopped existing when tokenade became
/// hook-first. Same phantom as `docs/ARCHITECTURE.md`, on the surface with the
/// widest audience.
#[test]
fn no_user_facing_help_promises_the_mcp_server() {
    let src = read_doc("crates/tokenade-cli/src/install_cli.rs");
    // Only the help TEXT matters here; comments about detecting a leftover
    // `tokenade-mcp` process from an old install are legitimate and stay.
    for claim in [
        "registers tokenade-mcp",
        "linking tokenade MCP tools",
        "tokenade-mcp with each",
    ] {
        assert!(
            !src.contains(claim),
            "install --help still promises {claim:?} — there is no MCP server to register"
        );
    }
}

/// Every backticked `*.rs` path in the doc, normalised to repo-relative.
fn named_source_paths(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for chunk in body.split('`').skip(1).step_by(2) {
        // Backticked spans carry things like `tokenade-cli/src/main.rs::cmd_x()`.
        let candidate = chunk.split("::").next().unwrap_or(chunk).trim();
        if !candidate.ends_with(".rs") || !candidate.contains('/') {
            continue;
        }
        let rel = if candidate.starts_with("crates/") {
            candidate.to_string()
        } else {
            format!("crates/{candidate}")
        };
        out.insert(rel);
    }
    out
}

#[test]
fn every_source_path_the_architecture_doc_names_exists() {
    let body = doc();
    let paths = named_source_paths(&body);
    assert!(
        paths.len() >= 3,
        "only {} source paths found — the extractor stopped matching the doc's \
         formatting and this test is asserting nothing",
        paths.len()
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut missing = Vec::new();
    for p in &paths {
        if PLACEHOLDERS.iter().any(|(ph, _)| ph == p) {
            continue;
        }
        if !root.join(p).exists() {
            missing.push(p.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "docs/ARCHITECTURE.md points at source that does not exist:\n  {}\n\
         Fix the doc, or add the path to PLACEHOLDERS with the reason it is not \
         a real file.",
        missing.join("\n  ")
    );
}

/// The crate list in the doc must be the crate list on disk.
///
/// This is the check that would have caught the `tokenade-mcp` rot at its
/// source: the diagram named a crate, and `crates/` had not held it for months.
#[test]
fn the_doc_names_no_crate_that_was_removed() {
    let body = doc();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let on_disk: BTreeSet<String> = std::fs::read_dir(root.join("crates"))
        .expect("crates/ must be readable")
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let mut ghosts = Vec::new();
    for chunk in body.split('`').skip(1).step_by(2) {
        // `tokenade-core::filter` names a MODULE inside a crate, not a crate.
        // Cut at both separators so the crate is what gets checked.
        let name = chunk
            .split('/')
            .next()
            .unwrap_or(chunk)
            .split("::")
            .next()
            .unwrap_or(chunk)
            .trim();
        if !name.starts_with("tokenade-") || name.contains(' ') || on_disk.contains(name) {
            continue;
        }
        // The doc keeps ONE deliberate mention: the section that explains this
        // crate is gone. Naming it there is the point.
        if body.contains(&format!("There is no `{name}` crate")) {
            continue;
        }
        ghosts.push(name.to_string());
    }
    ghosts.sort();
    ghosts.dedup();
    assert!(
        ghosts.is_empty(),
        "docs/ARCHITECTURE.md names crate(s) that are not in crates/: {ghosts:?}\n\
         Crates on disk: {on_disk:?}"
    );
}

/// **The VOLUMES the map announces must be true, not only its paths.**
///
/// `every_source_path_the_architecture_doc_names_exists` checked that every
/// named file exists. That did not stop the map from announcing
/// "`tokenade-core` (~36k LoC, 86 files)" for months while the crate carried
/// 270,730 over 1,241 files, nor "`tokenade-cli` (~5.7k LoC, monolithic —
/// pending split)" for 105,612 lines. A right path in a wrong paragraph reads
/// like a right paragraph.
///
/// 20% tolerance: the map is not a counter, it just must not be wrong by an
/// order of magnitude. When this test fails, it is the map's NUMBER that gets
/// updated.
#[test]
fn the_volumes_the_doc_announces_are_within_twenty_percent() {
    use std::path::Path;
    let body = doc();
    let mut checked = 0usize;
    let mut wrong = Vec::new();
    for crate_name in ["tokenade-core", "tokenade-cli", "tokenade-bench"] {
        // "### `<crate>` — N files, M lines"
        let needle = format!("`{crate_name}` — ");
        let Some(at) = body.find(&needle) else {
            wrong.push(format!("{crate_name}: the map no longer announces it"));
            continue;
        };
        let line: String = body[at..].lines().next().unwrap_or_default().to_string();
        let nums: Vec<usize> = line
            .split(|c: char| !c.is_ascii_digit() && c != ' ')
            .filter_map(|chunk| {
                let d: String = chunk.chars().filter(|c| c.is_ascii_digit()).collect();
                (!d.is_empty()).then(|| d.parse().unwrap_or(0))
            })
            .collect();
        if nums.len() < 2 {
            wrong.push(format!("{crate_name}: not two numbers in \"{line}\""));
            continue;
        }
        let (said_files, said_lines) = (nums[0], nums[1]);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join(crate_name).join("src"))
            .expect("the crates directory");
        let mut files = 0usize;
        let mut lines = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files += 1;
                    lines += std::fs::read_to_string(&p)
                        .map(|t| t.split('\n').count())
                        .unwrap_or(0);
                }
            }
        }
        checked += 1;
        for (what, said, real) in [("files", said_files, files), ("lines", said_lines, lines)] {
            let off = (said as f64 - real as f64).abs() / (real.max(1) as f64);
            if off > 0.20 {
                wrong.push(format!(
                    "{crate_name}: the map says {said} {what}, there are {real} \
                     ({:.0}% off)",
                    off * 100.0
                ));
            }
        }
    }
    assert_eq!(checked, 3, "all three crates must be announced");
    assert!(
        wrong.is_empty(),
        "the map has drifted:\n  {}",
        wrong.join("\n  ")
    );
}

/// The layers the map names are those the GUARD holds.
///
/// Two lists of seven names in two files is one too many: the day one
/// changes, the other describes a structure that no longer exists. This test
/// ties them together.
#[test]
fn the_layers_the_doc_names_are_the_layers_the_guard_holds() {
    let body = doc();
    let missing: Vec<&str> = crate::layers_only_point_downwards::LAYERS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !body.contains(&format!("**{name}**")))
        .collect();
    assert!(
        missing.is_empty(),
        "these layers are held by `layers_only_point_downwards` and missing \
         from the map's table: {missing:?}"
    );
}
