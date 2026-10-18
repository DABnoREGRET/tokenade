//! **The mutation harness**, with its guards written ONCE.
//!
//! The rule is "no probe in `/tmp`", and this harness is a
//! piece of the bench carrying its guards: unique anchor, same features as the
//! suite, refusal to return a verdict when the filter selects no test.
//! Yet it lived in the scratchpad, rewritten at every iteration — so
//! each of its guards was rewritten too.
//!
//! MEASURED 2026-09-08 on the **32 copies** accumulated:
//!
//! | guard | present |
//! |---|---|
//! | unique anchor (HALT otherwise) | 12 / 32 |
//! | refusal on empty selection | **4 / 32** |
//! | `mtime` reset after restore | 12 / 32 |
//! | restore in a `finally` | 14 / 32 |
//! | healthy tree checked BEFORE | **4 / 32** |
//!
//! Each absence has an incident that names it: without `mtime`, cargo serves the
//! MUTATED binary; without guaranteed restore, a HALT leaves the tree mutated;
//! without a unique anchor, the verdict bears on intact code; and without
//! the empty-selection refusal, the harness returns `KILLED` on a mutation that nothing
//! covers — the textbook fault.
//!
//! ## What this harness CANNOT guarantee
//!
//! A `SIGKILL` mid-round lets no `Drop` run: the file stays
//! MUTATED. The census of process launchers (`every_process_launcher_…`)
//! asks exactly this question, and here is the honest answer — a "reaper"
//! would kill the running suite without putting the file back, so it does not
//! cover that risk. The recovery is `git diff` then `git restore` of the
//! SINGLE file named by the plan. Never `git checkout` the whole tree:
//! nothing is committed between iterations.
//!
//! The empty-selection guard, in three of the four copies that had it,
//! was written `'0 tests run' in out`: a substring that fires on
//! **any count ending in zero**. It returned HALT on a perfectly live selection of 20
//! tests.

use std::path::{Path, PathBuf};

/// A mutation: the exact text to replace, in a file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Mutant {
    /// What the mutation claims to break, in one line.
    pub name: String,
    /// File path, relative to the repo root.
    pub file: String,
    /// The anchor. Must appear EXACTLY once.
    pub from: String,
    /// What replaces it.
    pub to: String,
}

/// The complete plan of a mutation round.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Plan {
    /// The `nextest -E` expression that selects the tests.
    pub filter: String,
    /// **A test command instead of `cargo nextest`.**
    ///
    /// Not everything this repo ships is Rust: the release pipeline
    /// is shell and TypeScript, and what it decides is costly. MEASURED on
    /// 2026-09-08 — its retry ladder waited 37 seconds against an npm registry
    /// propagation of 240, it refused two correct
    /// releases, and nothing could mutate it because the harness could only
    /// launch `nextest`.
    ///
    /// Given as argv (`["node", "--test", "…"]`), never as a shell line:
    /// a string passed to `sh -c` turns the plan's content into a command.
    #[serde(default)]
    pub command: Vec<String>,
    pub mutants: Vec<Mutant>,
    /// A BACKDROP applied before each mutation and removed after:
    /// the DEFECT a non-regression floor is meant to catch. On a
    /// healthy tree such a floor is unkillable, so we make it red
    /// first — **and the verdict inverts**: the suite must then be GREEN
    /// to say the mutated clause was load-bearing.
    #[serde(default)]
    pub backdrop: Vec<Mutant>,
}

/// The verdict of a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Killed,
    Survived,
}

/// Why the harness refuses to return a verdict.
#[derive(Debug)]
pub enum Halt {
    /// The anchor does not appear exactly once. A verdict would then bear
    /// on intact code, or on two places at once.
    AnchorNotUnique {
        name: String,
        found: usize,
    },
    /// The filter selects no test. `KILLED` would mean "nothing
    /// covers this", which is the opposite of what the word says.
    NothingSelected {
        filter: String,
    },
    /// The tree was already red: any verdict is noise.
    TreeAlreadyRed,
    /// A previous run was KILLED and left its mutation in the tree.
    ///
    /// Without this check, the next harness answers `HALT healthy tree already red`
    /// — true, and unusable: it does not say that it is OUR mutation that
    /// turns the tree red. MEASURED 2026-09-08: three runs killed in a row
    /// left `if was_above && !stays_above` replaced by `if false`, and the
    /// manual check missed it because it was looking for a
    /// list of patterns written from memory. The plan already holds the answer.
    LeftoverMutation {
        name: String,
        file: String,
    },
    /// The tree stayed broken after restore.
    TreeBrokenAfter,
    Io(String),
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::AnchorNotUnique { name, found } => {
                write!(f, "HALT {name}: anchor found {found} times")
            }
            Halt::NothingSelected { filter } => {
                write!(f, "HALT the filter selects no test: {filter}")
            }
            Halt::TreeAlreadyRed => write!(f, "HALT healthy tree already red"),
            Halt::LeftoverMutation { name, file } => write!(
                f,
                "HALT a previous run left its mutation in the tree — {file} still \
                 carries \"{name}\". Restore that file before rerunning."
            ),
            Halt::TreeBrokenAfter => write!(f, "TREE BROKEN AFTER RESTORE"),
            Halt::Io(e) => write!(f, "HALT {e}"),
        }
    }
}

/// **Is the selection empty?**
///
/// `out.contains("0 tests run")` is a SUBSTRING and "20 tests run"
/// contains it: the harness returned HALT on a perfectly live selection of twenty
/// tests. The zero must be the whole number, not its last digit.
pub fn nothing_was_selected(out: &str) -> bool {
    // TWO dialects, because the plan carries a `command` and
    // that field lets one choose the runner. Knowing only nextest's
    // returned 0/4 "survivors" on a `cargo test` command whose
    // pattern selected NOTHING: a verdict on zero tests, and the guard
    // that exists exactly for that was looking elsewhere.
    const EMPTY: [&str; 2] = [
        // nextest: "Summary [0.0s] 0 tests run: 0 passed"
        "0 tests run",
        // libtest: "running 0 tests"
        "running 0 tests",
    ];
    out.lines().any(|line| {
        EMPTY.iter().any(|needle| {
            line.match_indices(needle).any(|(i, _)| {
                // The character just before the zero must not be a digit.
                // `20 tests run` contains `0 tests run`, and `running 10 tests`
                // does not contain `running 0 tests` — but the same rule holds
                // for both, and writing it once keeps a third
                // dialect from starting again without it.
                let head = &line[..i + needle.find('0').unwrap_or(0)];
                head.chars().next_back().is_none_or(|c| !c.is_ascii_digit())
            })
        })
    })
}

/// A file restored WHATEVER HAPPENS, including on a `?` or a panic.
///
/// Without it, a HALT exits the process leaving the tree mutated — it
/// happened, and the day's work nearly went through `git checkout`, which the
/// rules of this tree forbid.
struct Restored {
    path: PathBuf,
    original: String,
}

impl Restored {
    fn hold(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            original: std::fs::read_to_string(path)?,
        })
    }
}

impl Drop for Restored {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.path, &self.original);
        // **Move the mtime forward.** `shutil.move` preserved it, so the
        // restored file looked older than the compiled artifact and
        // cargo served the MUTATED binary. Writing is enough here — the
        // system dates the file to now — but we say it, because
        // it is half of the trap.
        let _ = filetime_now(&self.path);
    }
}

fn filetime_now(path: &Path) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.set_len(f.metadata()?.len())?;
    Ok(())
}

/// Applies a mutation, returns the file as it must be written.
///
/// `None` when the anchor is not unique — the decision is here, separate from
/// the write, to be testable without touching the disk.
/// **A single mutation harness at a time on this tree.**
///
/// Two runs in parallel mutate and restore the same files: one
/// overwrites the other's restore. MEASURED on 2026-09-08 — two plans launched
/// together left `crates/tokenade-core/src/elision.rs` MUTATED in the tree
/// (`stack.push(p);` removed), and the canary "TREE BROKEN AFTER RESTORE"
/// said so without being able to repair it. A lock refuses the second rather than
/// return a verdict on a tree someone else is writing.
///
/// The lock carries the PID: a file left by a killed run (SIGKILL runs
/// no `Drop`) does not block forever, it is reported and taken over.
#[derive(Debug)]
pub struct RunLock {
    path: std::path::PathBuf,
}

impl RunLock {
    pub fn acquire(root: &Path) -> Result<RunLock, String> {
        let path = root.join("target/.mutate.lock");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(prev) = std::fs::read_to_string(&path) {
            let pid = prev.trim();
            if pid_is_alive(pid) {
                return Err(format!(
                    "another mutation harness is running (pid {pid}) and writes the same \n                     files. Two parallel runs leave the tree MUTATED: wait, or \n                     kill that process then delete {}",
                    path.display()
                ));
            }
            eprintln!("  ℹ stale lock taken over (pid {pid} gone) — a previous run was killed");
        }
        std::fs::write(&path, std::process::id().to_string())
            .map_err(|e| format!("unreadable lock: {e}"))?;
        Ok(RunLock { path })
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Is this PID running? `None` when the question makes no sense (unreadable pid).
pub fn pid_is_alive(pid: &str) -> bool {
    pid.parse::<u32>()
        .is_ok_and(|p| std::path::Path::new(&format!("/proc/{p}")).exists())
}

pub fn apply(source: &str, m: &Mutant) -> Option<String> {
    (source.matches(&m.from).count() == 1).then(|| source.replace(&m.from, &m.to))
}

/// Run the suite under the plan's filter, with the SAME features as the
/// full suite — otherwise the verdict bears on another program.
fn run(root: &Path, plan: &Plan) -> Result<(bool, String), Halt> {
    let argv: Vec<String> = if plan.command.is_empty() {
        [
            "cargo",
            "nextest",
            "run",
            "--workspace",
            // No more `--features tokenade-core/srcsweep`: this
            // flag no longer gates anything, and NAMING it resolves a distinct
            // feature set — so a full workspace rebuild at every
            // launch. Three runs in a row were killed for memory on
            // 2026-09-08 on that rebuild, on a machine that
            // reported no pressure.
            "-E",
            &plan.filter,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        plan.command.clone()
    };
    // **Cap the build PARALLELISM, not the memory.**
    //
    // MEASURED 2026-09-09: the same mutation of `wrap.rs` — which forces the link of the
    // e2e binary, 160 MB — gets killed by the supervisor "for low
    // memory" with 42 GB available and `/proc/pressure/memory avg10=0.00`, and
    // passes on the first try with `CARGO_BUILD_JOBS=2`. It was not a byte
    // cap that was missing, it was the number of simultaneous links. Four by
    // default; the caller keeps the last word by setting the variable.
    let out = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .env(
            "CARGO_BUILD_JOBS",
            std::env::var("CARGO_BUILD_JOBS")
                .unwrap_or_else(|_| crate::safe_build_jobs().to_string()),
        )
        .output()
        .map_err(|e| Halt::Io(e.to_string()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok((out.status.success(), text))
}

/// **Is the tree in the expected state?**
///
/// Three places ask this question — before, during, after — and the
/// third asked it backwards: the final check returned "TREE BROKEN"
/// on a clean, green tree. The canary said so on the first shot, which is why
/// it is required. A single spelling, because the comparison flips
/// meaning with the backdrop and a human gets it backwards one time in
/// three.
///
/// Without a backdrop, healthy means GREEN. With one, the tree starts red on purpose
/// (a non-regression floor is unkillable on a healthy tree), so
/// healthy means RED and a load-bearing mutation turns it green.
fn is_healthy(green: bool, inverted: bool) -> bool {
    green != inverted
}

/// A full round. Returns one verdict per mutation, in plan order.
///
/// The order of the guards is not decorative: healthy tree first (otherwise every
/// verdict is noise), the selection next (otherwise `KILLED` means
/// "nothing covers this"), the anchor last (otherwise the verdict bears on
/// intact code).
pub fn run_plan(root: &Path, plan: &Plan) -> Result<Vec<(String, Verdict)>, Halt> {
    // A backdrop makes the tree RED on purpose: a
    // non-regression floor is unkillable on a healthy tree. The verdict
    // then inverts — the suite must turn GREEN to say the mutated
    // clause was load-bearing.
    // **What a KILLED run may have left behind.** The plan says exactly
    // what to look for: if the anchor has disappeared and its replacement is there, the
    // file still carries the earlier mutation, and the verdict would bear on
    // code nobody wrote.
    for m in plan.mutants.iter().chain(plan.backdrop.iter()) {
        let src = std::fs::read_to_string(root.join(&m.file))
            .map_err(|e| Halt::Io(format!("{}: {e}", m.file)))?;
        if src.matches(&m.from).count() != 1 && src.contains(&m.to) {
            return Err(Halt::LeftoverMutation {
                name: m.name.clone(),
                file: m.file.clone(),
            });
        }
    }
    let inverted = !plan.backdrop.is_empty();
    let _backdrop: Vec<Restored> = plan
        .backdrop
        .iter()
        .map(|m| write_mutant(root, m))
        .collect::<Result<_, _>>()?;

    let (green, out) = run(root, plan)?;
    if nothing_was_selected(&out) {
        return Err(Halt::NothingSelected {
            filter: plan.filter.clone(),
        });
    }
    if !is_healthy(green, inverted) {
        return Err(Halt::TreeAlreadyRed);
    }

    let mut verdicts = Vec::new();
    for m in &plan.mutants {
        let held = write_mutant(root, m)?;
        let (green, _) = run(root, plan)?;
        drop(held);
        // A mutation is KILLED when the tree stops being healthy.
        let killed = !is_healthy(green, inverted);
        verdicts.push((
            m.name.clone(),
            if killed {
                Verdict::Killed
            } else {
                Verdict::Survived
            },
        ));
    }

    let (green, _) = run(root, plan)?;
    if is_healthy(green, inverted) {
        Ok(verdicts)
    } else {
        Err(Halt::TreeBrokenAfter)
    }
}

/// Writes the mutation and returns the guard that will undo it.
fn write_mutant(root: &Path, m: &Mutant) -> Result<Restored, Halt> {
    let path = root.join(&m.file);
    let held = Restored::hold(&path).map_err(|e| Halt::Io(format!("{}: {e}", m.file)))?;
    let found = held.original.matches(&m.from).count();
    let Some(mutated) = apply(&held.original, m) else {
        return Err(Halt::AnchorNotUnique {
            name: m.name.clone(),
            found,
        });
    };
    std::fs::write(&path, mutated).map_err(|e| Halt::Io(format!("{}: {e}", m.file)))?;
    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(from: &str, to: &str) -> Mutant {
        Mutant {
            name: "essai".into(),
            file: "x.rs".into(),
            from: from.into(),
            to: to.into(),
        }
    }

    /// **The guard that just broke.** `'0 tests run' in out` is a
    /// substring, and "20 tests run" contains it: the harness returned HALT
    /// on a perfectly live selection of twenty tests.
    #[test]
    fn a_count_ending_in_zero_is_not_an_empty_selection() {
        assert!(nothing_was_selected("Summary [0.0s] 0 tests run: 0 passed"));
        assert!(!nothing_was_selected(
            "Summary [0.1s] 20 tests run: 20 passed"
        ));
        assert!(!nothing_was_selected(
            "Summary [0.1s] 100 tests run: 100 passed"
        ));
        assert!(!nothing_was_selected(
            "Summary [0.1s] 9240 tests run: 9240 passed"
        ));
        // And the zero is still caught wherever it is in the line.
        assert!(nothing_was_selected("prefix: 0 tests run\nautre chose"));
        // And libtest's dialect, which the plan's `command` field makes
        // reachable: `cargo test` says "running 0 tests", never "tests
        // run". Without this half, four mutations were declared
        // survivors on an empty selection.
        assert!(nothing_was_selected("\nrunning 0 tests\n"));
        assert!(!nothing_was_selected("\nrunning 10 tests\n"));
        assert!(!nothing_was_selected("\nrunning 260 tests\n"));
        assert!(nothing_was_selected(
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 322 filtered out"
        ));
        assert!(!nothing_was_selected("rien de tel ici"));
    }

    /// The anchor must appear EXACTLY once: at zero the verdict bears
    /// on intact code, at two it bears on two places.
    #[test]
    fn an_anchor_that_is_not_unique_yields_no_mutation() {
        assert_eq!(
            apply("un a deux", &m("a", "b")).as_deref(),
            Some("un b deux")
        );
        assert!(apply("un a deux a trois", &m("a", "b")).is_none(), "twice");
        assert!(apply("rien", &m("a", "b")).is_none(), "zero times");
    }

    /// The guard restores the file in its `Drop`, so a `?`, a `return`
    /// or a panic puts it back. Without it, a HALT exits the process
    /// leaving the tree mutated — it happened.
    #[test]
    fn a_file_is_restored_even_when_the_run_bails_out() {
        let dir = std::env::temp_dir().join(format!("tk-mutate-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.rs");
        std::fs::write(&path, "original").unwrap();
        {
            let _held = Restored::hold(&path).unwrap();
            std::fs::write(&path, "muté").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "muté");
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "original",
            "the Drop must undo the mutation"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The final check returned "TREE BROKEN" on a clean, green tree:
    /// three sites spelled the same comparison and the third was
    /// inverted. The canary said so on the first shot.
    #[test]
    fn healthy_means_green_unless_a_backdrop_inverts_it() {
        assert!(
            is_healthy(true, false),
            "without a backdrop, healthy = green"
        );
        assert!(!is_healthy(false, false));
        // With a backdrop the tree starts RED on purpose: that state
        // is the healthy one, and a load-bearing clause turns it green.
        assert!(is_healthy(false, true), "with a backdrop, healthy = red");
        assert!(!is_healthy(true, true));
    }

    /// The plan reads as the iterations write it, `backdrop` included —
    /// and its absence is not an error.
    #[test]
    fn a_plan_without_a_backdrop_reads_fine() {
        let p: Plan = serde_json::from_str(
            r#"{"filter":"test(~x)","mutants":[{"name":"n","file":"f.rs","from":"a","to":"b"}]}"#,
        )
        .expect("readable plan");
        assert!(p.backdrop.is_empty());
        assert_eq!(p.mutants.len(), 1);
    }

    /// **A plan without `command` stays a `nextest` plan.**
    ///
    /// The clause was added to mutate the release pipeline, which is
    /// TypeScript; it must change nothing for the plans that exist. And when
    /// it is given, it is an argv — never a shell line, otherwise the
    /// plan's content becomes a command.
    #[test]
    fn a_plan_without_a_command_still_runs_nextest() {
        let bare: Plan = serde_json::from_str(
            r#"{"filter":"test(x)","mutants":[{"name":"m","file":"f","from":"a","to":"b"}]}"#,
        )
        .expect("a plan without `command`");
        assert!(
            bare.command.is_empty(),
            "`command` must be empty by default"
        );

        let with: Plan = serde_json::from_str(
            r#"{"filter":"x","command":["node","--test","t.mjs"],
                "mutants":[{"name":"m","file":"f","from":"a","to":"b"}]}"#,
        )
        .expect("a plan with `command`");
        assert_eq!(with.command, vec!["node", "--test", "t.mjs"]);
        // An argv, so the first element is an EXECUTABLE and not `sh`.
        assert_ne!(
            with.command[0], "sh",
            "a shell line would turn the plan into a command"
        );
    }

    /// **The lock refuses a second harness, and does not block on a dead one.**
    ///
    /// MEASURED on 2026-09-08: two plans launched together left
    /// `elision.rs` MUTATED in the tree, and the canary said so without being able to
    /// repair it.
    #[test]
    fn only_one_harness_writes_the_tree_at_a_time() {
        let root = std::env::temp_dir().join(format!(
            "tokenade-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("target")).unwrap();

        let held = RunLock::acquire(&root).expect("the first lock");
        // This process is alive, so a second one must be REFUSED.
        let refused = RunLock::acquire(&root);
        assert!(refused.is_err(), "two harnesses could write the same tree");
        assert!(
            refused.unwrap_err().contains("MUTATED"),
            "the refusal must say what is at stake"
        );

        drop(held);
        // Released, it can be taken again.
        let again = RunLock::acquire(&root).expect("after release");
        drop(again);

        // A lock left by a DEAD process does not block: SIGKILL
        // runs no `Drop`, and a killed harness must not freeze the
        // next one.
        std::fs::write(root.join("target/.mutate.lock"), "999999999").unwrap();
        assert!(!pid_is_alive("999999999"));
        RunLock::acquire(&root).expect("a stale lock is taken over");

        // An unreadable pid is not "alive": otherwise a corrupted file
        // would block forever.
        assert!(!pid_is_alive("pas-un-pid"));
        assert!(pid_is_alive(&std::process::id().to_string()));

        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod leftover_tests {
    use super::*;

    /// **A KILLED run leaves its mutation in the tree, and the harness must
    /// SAY so.**
    ///
    /// Without this check it answers `HALT healthy tree already red`: true, and
    /// unusable — it does not say that it is our own mutation that turns
    /// the tree red. MEASURED 2026-09-08: three runs killed in a row left
    /// `if was_above && !stays_above` replaced by `if false`, and the
    /// manual check missed it because it was looking for
    /// a list of patterns written from memory rather than the plan itself.
    #[test]
    fn a_mutation_left_by_a_killed_run_is_named() {
        let root = std::env::temp_dir().join(format!("tokenade-leftover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The file carries the REPLACEMENT and no longer the anchor: that is the exact
        // trace of a run interrupted between the write and the restore.
        std::fs::write(
            root.join("a.rs"),
            "fn f() {\n    if false {\n        g();\n    }\n}\n",
        )
        .unwrap();
        let m = Mutant {
            name: "M3 la regle ne joue plus".to_string(),
            file: "a.rs".to_string(),
            from: "    if was_above && !stays_above {".to_string(),
            to: "    if false {".to_string(),
        };
        let plan = Plan {
            filter: "test(rien)".to_string(),
            command: Vec::new(),
            mutants: vec![m],
            backdrop: Vec::new(),
        };
        match run_plan(&root, &plan) {
            Err(Halt::LeftoverMutation { name, file }) => {
                assert_eq!(file, "a.rs");
                assert!(name.starts_with("M3"), "{name}");
            }
            other => panic!("the harness did not see the leftover mutation: {other:?}"),
        }
        // And a HEALTHY tree passes this check: the anchor is there, the
        // replacement is not.
        std::fs::write(
            root.join("a.rs"),
            "fn f() {\n    if was_above && !stays_above {\n        g();\n    }\n}\n",
        )
        .unwrap();
        assert!(
            !matches!(run_plan(&root, &plan), Err(Halt::LeftoverMutation { .. })),
            "a healthy tree was taken for a mutated tree"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

/// **Which OTHER clause may have caught a surviving mutant's fixture.**
///
/// This trap bit FOUR iterations in a row despite a memory
/// entry, a rules section and a recipe: a fixture falls under two
/// refusal clauses, the mutation disarms one, the other catches it, and the
/// test stays green. "SURVIVED" alone does not say which.
///
/// The operator put it on 2026-09-09: *if the rule keeps coming back, it is
/// not mechanisable by text and the harness must carry it.*
/// This function is that mechanisation — syntactic and without
/// pretension: it returns the other REFUSAL lines of the function where the
/// mutation was placed, so that the question "which one catches my
/// fixture?" is asked in front of named lines rather than in general.
///
/// It proves nothing. It turns a mute verdict into a short list to
/// check one by one, which none of the four textual formulations
/// achieved.
pub fn competing_guards(src: &str, from: &str) -> Vec<(usize, String)> {
    let Some(at) = src.find(from.trim_start_matches('\n')) else {
        return Vec::new();
    };
    let lines: Vec<&str> = src.lines().collect();
    // The line where the mutation is placed.
    let mutated_line = src[..at].matches('\n').count();
    let mutated_span = from.lines().count().max(1);
    // Walk back up to the function signature that contains it.
    let start = (0..=mutated_line)
        .rev()
        .find(|i| {
            let t = lines[*i].trim_start();
            t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub(crate) fn ")
        })
        .unwrap_or(0);
    // And down to its closing brace, at the same indent as the signature.
    let indent = lines[start].len() - lines[start].trim_start().len();
    let end = ((start + 1)..lines.len())
        .find(|i| {
            let l = lines[*i];
            l.trim() == "}" && l.len() - l.trim_start().len() == indent
        })
        .unwrap_or(lines.len());

    let refuses = |t: &str| {
        let tr = t.trim();
        t.contains("return true")
            || t.contains("return false")
            || t.contains("return None")
            || t.contains("return (0, 0)")
            || tr == "continue;"
            || tr == "break;"
            // **The siblings of one boolean expression.** A probe
            // bit exactly there: with the clause `|| s.contains('»')` disarmed,
            // it was `|| s.contains('«')` on the next line that caught the
            // fixture — and a list that names only the `return`s misses it.
            || tr.starts_with("|| ")
            || tr.starts_with("&& ")
    };
    (start + 1..end)
        .filter(|i| *i < mutated_line || *i >= mutated_line + mutated_span)
        .filter(|i| refuses(lines[*i]))
        .map(|i| (i + 1, lines[i].trim().to_string()))
        .collect()
}

#[cfg(test)]
mod competing_guards_tests {
    use super::competing_guards;

    /// The exact earlier case: two clauses refuse, the mutation disarms
    /// one, and the harness must NAME the other.
    #[test]
    fn it_names_the_other_refusals_of_the_same_function() {
        let src = "\
fn crosses_a_structure(s: &str) -> bool {
    if s.contains(\" · \") {
        return true;
    }
    if s.contains(\"  \") {
        return true;
    }
    let alnum = s.len();
    alnum * 2 < s.len()
}
";
        let found = competing_guards(
            src,
            "    if s.contains(\"  \") {\n        return true;\n    }",
        );
        let text: Vec<&str> = found.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            text,
            vec!["return true;"],
            "the neighbouring clause is not named: {found:?}"
        );
        assert_eq!(found[0].0, 3, "the line number is wrong: {found:?}");
    }

    /// The siblings of one boolean expression count too: it is the
    /// case that served as a probe.
    #[test]
    fn it_names_the_siblings_of_a_boolean_chain() {
        let src = "\
fn crosses(s: &str) -> bool {
    s.contains(\"a\")
        || s.contains('«')
        || s.contains('»')
}
";
        let found = competing_guards(src, "        || s.contains('»')");
        let text: Vec<&str> = found.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            text,
            vec!["|| s.contains('«')"],
            "the sibling is not named: {found:?}"
        );
    }

    /// A function with no other refusal fabricates no suspect.
    #[test]
    fn a_lone_guard_names_nobody() {
        let src = "fn f(x: i32) -> bool {\n    x > 3\n}\n";
        assert!(competing_guards(src, "    x > 3").is_empty());
    }

    /// An anchor that cannot be found does not make the harness panic.
    #[test]
    fn a_missing_anchor_is_not_a_panic() {
        assert!(competing_guards("fn f() {}\n", "absent").is_empty());
    }
}
