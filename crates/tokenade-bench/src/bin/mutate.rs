//! `cargo run -q -p tokenade-bench --bin mutate -- --plan <file.json>`
//!
//! The project's mutation harness, with its guards written once. See
//! [`tokenade_bench::mutate`] for what each guard prevents and what it
//! cost to learn.
//!
//! The plan is JSON:
//!
//! ```json
//! {
//!   "filter": "test(~lockfile)",
//!   "mutants": [
//!     {"name": "the npm flavour disappears",
//!      "file": "crates/tk-formats/src/lockfile.rs",
//!      "from": "return Flavour::PackageLock;",
//!      "to":   "return Flavour::Unknown;"}
//!   ]
//! }
//! ```
//!
//! `backdrop` (optional) takes the same shape: the DEFECT set before each
//! mutation, to make a non-regression floor killable — and the verdict
//! is then inverted, which the report states in so many words.

use std::path::Path;
use tokenade_bench::mutate::{run_plan, Plan, Verdict};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--plan"], &args);
    let Some(plan_path) = args
        .iter()
        .position(|a| a == "--plan")
        .and_then(|i| args.get(i + 1))
    else {
        eprintln!("usage: mutate --plan <file.json>");
        std::process::exit(2);
    };
    let text = match std::fs::read_to_string(plan_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("mutate: {plan_path}: {e}");
            std::process::exit(2);
        }
    };
    let plan: Plan = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mutate: unreadable plan: {e}");
            std::process::exit(2);
        }
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();

    // ONE harness at a time: two parallel runs mutate and restore the
    // same files, and one overwrites the other's restoration. Taken BEFORE
    // any write, and released by `Drop`.
    let _lock = match tokenade_bench::mutate::RunLock::acquire(&root) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mutate: {e}");
            std::process::exit(2);
        }
    };

    if !plan.backdrop.is_empty() {
        println!(
            "backdrop: {} defect(s) set — THE VERDICT IS INVERTED, \
             the suite must turn GREEN for a mutation to count as killed",
            plan.backdrop.len()
        );
    }
    match run_plan(&root, &plan) {
        Ok(verdicts) => {
            let killed = verdicts
                .iter()
                .filter(|(_, v)| *v == Verdict::Killed)
                .count();
            for (name, v) in &verdicts {
                println!(
                    "{}  {name}",
                    if *v == Verdict::Killed {
                        "KILLED  "
                    } else {
                        "SURVIVED"
                    }
                );
                if *v == Verdict::Killed {
                    continue;
                }
                // **A survivor must say WHO could have caught the fixture.**
                // This trap bit four iterations in a row despite
                // a memory entry, a section of the project rules and a recipe:
                // writing the rule a fifth time would have changed nothing, so
                // the harness carries it. The other refusals of the same function
                // are named, with their line, so that the question is asked
                // in front of a short list rather than in general.
                let Some(m) = plan.mutants.iter().find(|m| &m.name == name) else {
                    continue;
                };
                let Ok(src) = std::fs::read_to_string(root.join(&m.file)) else {
                    continue;
                };
                let others = tokenade_bench::mutate::competing_guards(&src, &m.from);
                if others.is_empty() {
                    continue;
                }
                println!(
                    "          {} other refusal(s) in the same function — which one catches \
                     the fixture?",
                    others.len()
                );
                for (line, text) in others.iter().take(6) {
                    println!("            {}:{line}  {text}", m.file);
                }
            }
            println!("{killed}/{} killed · TREE CLEAN OK", verdicts.len());
        }
        Err(halt) => {
            // A HALT is not a verdict, and it must not read as
            // "nothing found". The tree is restored: the guards undo it in
            // their `Drop`, including on this path.
            eprintln!("{halt}");
            std::process::exit(2);
        }
    }
}
