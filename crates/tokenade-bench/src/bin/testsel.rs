//! Which tests exercise what I just changed?
//!
//! `cargo run -q -p tokenade-bench --bin testsel` — reads the working diff.
//! `cargo run -q -p tokenade-bench --bin testsel -- <file…>` — explicit.
//!
//! The full suite costs 132 seconds for 9,203 tests. That is the right price
//! BEFORE a commit, and a bad price while groping around a function:
//! a compactor iteration does ten or twenty of them, and twenty times 132 seconds
//! is half an hour spent rerunning 9,190 tests that could not see anything.
//!
//! What it prints is a `-E` expression for nextest, built from
//! the touched files:
//!
//! * the file's MODULE PATH — `cmd/linux/df_table.rs` gives
//!   `cmd::linux::df_table`, which catches its unit tests;
//! * the DEPENDENTS `tokenade impact` names, through the same path, because
//!   a compactor that delegates to an archetype is broken by a change
//!   that no test of its own module will see;
//! * the corpus-driven GUARD BINARIES, as soon as a compactor or a
//!   routing moves: they are the ones that see a ROUTING change, which
//!   no test named after a module can see, and they cost two
//!   seconds.
//!
//! ## What it is not
//!
//! It is not a replacement for the suite. A selector cannot see a
//! test that exercises the code through a path nothing names — and this repo
//! has some: the guard that caught the `locale_census` bench looks for files that
//! spawn a process, by sweeping the SOURCES, from an end-to-end test
//! of another crate. No dependency analysis links those two.
//! So: this for the short loop, the whole suite before committing.

use std::collections::BTreeSet;
use std::process::Command;

/// `crates/tokenade-core/src/cmd/linux/df_table.rs` → `cmd::linux::df_table`.
/// Returns `None` for anything that is not a Rust source of a crate.
fn module_path(rel: &str) -> Option<String> {
    let after = rel.split("/src/").nth(1)?;
    let stem = after.strip_suffix(".rs")?;
    // `mod.rs` and `lib.rs` name their DIRECTORY, not one more module.
    let stem = match stem.rsplit_once('/') {
        Some((dir, "mod")) => dir,
        _ => match stem {
            "lib" | "main" => return None,
            other => other,
        },
    };
    Some(stem.replace('/', "::"))
}

fn changed_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        // `XY path`, and `R  old -> new`: the new one is what counts.
        .filter_map(|l| {
            l.get(3..)
                .map(|p| p.rsplit(" -> ").next().unwrap_or(p).to_string())
        })
        .filter(|p| p.ends_with(".rs"))
        .collect()
}

/// The dependents, asked of the product itself — in JSON, because the text
/// output stops at thirty and a selector that loses dependents
/// SILENTLY errs in the dangerous direction: it returns a green that does not
/// cover what was changed. MEASURED on `archetypes.rs`: 974
/// dependents, 30 shown.
fn dependents(file: &str) -> Vec<String> {
    let exe = std::env::var("TOKENADE_BENCH_EXE").unwrap_or_else(|_| "tokenade".into());
    let Ok(out) = Command::new(&exe).args(["impact", "--json", file]).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // No dependency on a JSON parser for three lines: the paths are
    // the only strings of the document ending in `.rs`, except `"file"`.
    let mut v = Vec::new();
    let mut in_deps = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("\"dependents\"") {
            in_deps = true;
            continue;
        }
        if in_deps {
            if t.starts_with(']') {
                break;
            }
            if let Some(p) = t.trim_matches([',', '"']).strip_suffix(".rs") {
                v.push(format!("{p}.rs"));
            }
        }
    }
    v
}

/// The corpus-driven subjects: the only ones that see a ROUTING
/// change, which no test named after a module can see.
///
/// They used to be named `binary(<subject>)`, which was true when every
/// `tests/*.rs` was its own binary. Since the 87 targets were
/// regrouped into 8, these four live in `corpus`, `fold` and `guards`:
/// `binary(richness_corpus)` no longer selected ANYTHING, silently, every
/// time a compactor moved — the instrument named a demolished building.
/// `test(~<subject>::)` targets the MODULE, which has not moved, and returns exactly the
/// same selection as before the regrouping. `the_corpus_guards_still_exist`
/// keeps the list true.
const CORPUS_GUARDS: [&str; 4] = [
    "richness_corpus",
    "language_free_fold",
    "routing_guards",
    "corpus_error_survival",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&[], &args);
    let files = if args.is_empty() {
        changed_files()
    } else {
        args
    };
    if files.is_empty() {
        eprintln!("no modified .rs file: nothing to select");
        std::process::exit(1);
    }

    let mut mods: BTreeSet<String> = BTreeSet::new();
    let mut touches_a_compactor = false;
    for f in &files {
        if f.contains("/cmd/") || f.contains("auto.rs") || f.contains("filter.rs") {
            touches_a_compactor = true;
        }
        if let Some(m) = module_path(f) {
            mods.insert(m);
        }
        for d in dependents(f) {
            if let Some(m) = module_path(&d) {
                mods.insert(m);
            }
        }
    }

    // Beyond a certain number of modules, the selection is no longer a
    // selection: `archetypes.rs` has 974 dependents, i.e. the whole crate, and
    // a 974-term filter costs more to write than not to write. Saying so
    // and returning the whole suite is the honest answer — a hub module
    // is not tested on the cheap.

    const TOO_WIDE: usize = 60;
    if mods.len() > TOO_WIDE {
        println!(
            // **Without `--features tokenade-core/srcsweep`.** That
            // flag no longer gates anything, and NAMING it resolves a distinct
            // feature set — hence a full rebuild of the workspace. The
            // project rules say so; the instrument printing the command still said
            // otherwise, which put the cost back in the hands of whoever copied it.
            "cargo nextest run --workspace"
        );
        eprintln!(
            "{} module(s) touched — this is a hub, not a leaf. The whole suite.",
            mods.len()
        );
        return;
    }

    let mut terms: Vec<String> = mods.iter().map(|m| format!("test(~{m})")).collect();
    if touches_a_compactor {
        // A ROUTING change is not visible in any test named after a
        // module: the corpus sees it, and only the corpus. Two seconds.
        for b in CORPUS_GUARDS {
            terms.push(format!("test(~{b}::)"));
        }
    }
    if terms.is_empty() {
        eprintln!("the touched files carry no testable module");
        std::process::exit(1);
    }

    println!("cargo nextest run --workspace -E '{}'", terms.join(" or "));
    eprintln!(
        "{} file(s) → {} module(s){}. The WHOLE suite is still due before committing.",
        files.len(),
        mods.len(),
        if touches_a_compactor {
            " + the corpus guards"
        } else {
            ""
        }
    );
}

#[cfg(test)]
mod tests {
    use super::{module_path, CORPUS_GUARDS};

    /// An instrument that names a subject must name a subject that EXISTS.
    ///
    /// The class, not the instance: regrouping the 87 test targets into 8
    /// silenced four `binary(...)` here without a single line going red,
    /// because a nextest filter that selects nothing is not an
    /// error. This test fails the day a subject is renamed or moved.
    #[test]
    fn the_corpus_guards_still_exist() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("tokenade-core")
            .join("tests");
        for subject in CORPUS_GUARDS {
            let found = std::fs::read_dir(&root)
                .expect("tests/")
                .flatten()
                .any(|e| e.path().join(format!("{subject}.rs")).is_file());
            assert!(
                found,
                "`{subject}` is named by testsel and exists under no target of \
                 `tokenade-core/tests/`: the selection for it is SILENT"
            );
        }
    }

    #[test]
    fn a_source_path_becomes_the_module_it_declares() {
        assert_eq!(
            module_path("crates/tokenade-core/src/cmd/linux/df_table.rs").as_deref(),
            Some("cmd::linux::df_table")
        );
        assert_eq!(
            module_path("crates/tokenade-core/src/auto.rs").as_deref(),
            Some("auto")
        );
        // `mod.rs` names its directory: `cmd/linux/mod.rs` is `cmd::linux`,
        // not `cmd::linux::mod`, which would catch no test.
        assert_eq!(
            module_path("crates/tokenade-core/src/cmd/linux/mod.rs").as_deref(),
            Some("cmd::linux")
        );
        // A crate root restricts nothing: a `test(~lib)` filter
        // would randomly select everything carrying those three letters.
        assert_eq!(module_path("crates/tokenade-core/src/lib.rs"), None);
        assert_eq!(module_path("crates/tokenade-cli/src/main.rs"), None);
        // An integration test does not live under `src/`: nothing to get from it here,
        // it is selected by its binary.
        assert_eq!(
            module_path("crates/tokenade-core/tests/richness_corpus.rs"),
            None
        );
    }
}
