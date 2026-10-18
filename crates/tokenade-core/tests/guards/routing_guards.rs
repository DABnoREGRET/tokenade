//! Guards on WHICH compactor answers a command, as opposed to what it
//! answers. Three failure modes, each of which shipped at least once:
//!
//! 1. A module file stops being routed at all and nothing notices, because
//!    its unit tests keep passing on a function no caller reaches.
//! 2. Two routes own the same name, both answer, and the generic one wins —
//!    the hand-tuned compactor becomes dead code with its tests still green.
//! 3. A parity harness proves a row against one per-OS copy while dispatch
//!    routes to another.
//!
//! (1) is checked without any input at all, which is what makes it cheap and
//! quiet: it only fails when reachability actually changes.

use std::collections::BTreeSet;
use std::path::Path;
use tokenade_core::cmd;

/// The name a module file dispatches under: its stem, minus the artificial
/// suffixes `build.rs` strips. `DISPATCH_KEY_SUFFIXES` is generated from the
/// same list `dispatch_key_of` uses, so the two cannot drift apart.
fn dispatch_key_of(stem: &str) -> &str {
    for sfx in cmd::DISPATCH_KEY_SUFFIXES {
        if let Some(s) = stem.strip_suffix(sfx) {
            return s;
        }
    }
    stem
}

fn command_modules() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cmd");
    let mut out = Vec::new();
    for os in ["linux", "macos", "windows"] {
        let Ok(rd) = std::fs::read_dir(root.join(os)) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem == "mod" {
                continue;
            }
            out.push((os.to_string(), stem.to_string()));
        }
    }
    out
}

/// Every per-command module is reachable from some route.
///
/// This is the test that was missing. `build.rs` skipped directory modules
/// with `path.ends_with("mod.rs")`, which is equally true of `chmod.rs`,
/// `lsmod.rs`, `usermod.rs`, `insmod.rs`, `rmmod.rs`, … — eleven compiled
/// compactors that never got a dispatch arm. Nothing failed: their unit
/// tests called `try_compact` directly and passed, and no output test can
/// see a compactor that is never asked.
///
/// It costs nothing to keep green (adding a module keeps it green; only
/// losing a route breaks it), which is why it is an assertion and not a
/// pinned snapshot of the whole routing table.
#[test]
fn every_command_module_is_reachable() {
    let mods = command_modules();
    let unreachable: Vec<String> = mods
        .iter()
        .filter(|(_, stem)| {
            let key = dispatch_key_of(stem);
            !cmd::dispatch_auto_has_prog(key) && !cmd::dispatch_named_has_prog(key)
        })
        .map(|(os, stem)| format!("{os}/{stem}.rs"))
        .collect();
    assert!(
        mods.len() > 500,
        "module scan found only {} files — the walk is broken, not the tree",
        mods.len()
    );
    assert_eq!(
        unreachable,
        Vec::<String>::new(),
        "these compactors are compiled but no program name routes to them"
    );
}

/// Content shapes every command's output is drawn from. Deliberately generic:
/// this test is about WHICH route answers, and a shape only has to make two
/// routes commit to an answer for a disagreement to surface.
fn battery() -> Vec<String> {
    let mut v = vec![String::new(), "   \n".into(), "single line".into()];
    let mut table = String::from("NAME   STATUS   AGE\n");
    for i in 0..120 {
        table.push_str(&format!("svc{i}   Running  {i}h\n"));
    }
    v.push(table);
    v.push(
        (0..120)
            .map(|i| format!("key{i}: value {i}\nname: thing {i}\n\n"))
            .collect(),
    );
    v.push(
        (0..200)
            .map(|i| format!("plain row {i} of output"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    v.push(
        (0..80)
            .map(|i| format!("(see the transcript file for row {i})"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut dashed = String::from("KEY        VALUE\n---------  ---------\n");
    for i in 0..90 {
        dashed.push_str(&format!("net.k{i}    {i}\n"));
    }
    v.push(dashed);
    v.push(
        (0..300)
            .map(|i| format!("net.ipv4.conf.k{i} = {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    v.push(
        (0..40)
            .map(|i| format!("/p/f{i}.rs:{i}:  matched text"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    v
}

/// Names owned by BOTH a spec row and a module, where both answer and the
/// answers differ — so the one dispatch consults first silently replaces the
/// other.
fn shadowing_names(inputs: &[String]) -> Vec<String> {
    let mut names: Vec<&str> = cmd::AUTO_DISPATCH_PROGS.to_vec();
    names.sort_unstable();
    names.dedup();
    let mut bad = BTreeSet::new();
    for n in names {
        if !cmd::spec::has_key(n) || !cmd::dispatch_auto_has_prog(n) {
            continue;
        }
        for t in inputs {
            let (row, module) = (cmd::spec::dispatch(n, t), cmd::dispatch_auto(n, t));
            // One side declining is not shadowing: `dispatch_named` chains
            // them with `or_else`, so whichever answers is delivered. Only
            // TWO answers that differ mean one route is being replaced.
            if row.is_some() && module.is_some() && row != module {
                bad.insert(n.to_string());
                break;
            }
        }
    }
    bad.into_iter().collect()
}

/// Pinned, and each one is understood — see
/// `cmd::spec::tests::the_pinned_divergences_on_realistic_output`, which
/// feeds these same names output they can actually produce and finds only
/// `nmap` divergent. They differ here because the battery hands a
/// Kubernetes-shaped table to `id` and `which`.
///
/// A name appearing that is NOT on this list is a hand-written compactor
/// that just became dead code.
const KNOWN_SHADOWED: &[&str] = &[
    "id", "ls", "shutdown", "type", "uptime", "wall", "which", "write",
];

#[test]
fn no_row_shadows_a_module_on_synthetic_shapes() {
    let found = shadowing_names(&battery());
    assert_eq!(
        found,
        KNOWN_SHADOWED
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "the set of names where a spec row replaces a live module changed"
    );
}

fn captured_outputs() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "txt") {
                if let (Some(s), Ok(body)) = (
                    p.file_stem().and_then(|s| s.to_str()),
                    std::fs::read_to_string(&p),
                ) {
                    out.push((s.to_string(), body));
                }
            }
        }
    }
    walk(&root, &mut out);
    out
}

/// A declarative ROW must not deliver MORE than the module it shadows.
///
/// This is the half the synthetic battery cannot do: a module that decides
/// on content — `chmod` wants octal modes — refuses any fabricated input and
/// therefore never contradicts anyone. On a real `chmod -v` body it folded to
/// 13 lines where its row folded to 67, and only a real capture shows it.
///
/// **The question changed shape.** The test required EQUALITY — "no row
/// replaces a live module" — and the corpus's first linux capture refuted it:
/// on a real 417,031-byte `ls -la /usr/bin`, the row returns **6,571 bytes**
/// (94 lines, with its `[tokenade:ls-cmd]` banner) and the module **315,060**
/// (4,491 lines). The row does not shadow the module, it does the work.
/// Requiring equality claimed a disagreement is always a regression; what is
/// true, and what `chmod` showed, is that a row must not deliver MORE.
///
/// Coverage remains the limit: the corpus resolves 61 of the 1466 routed
/// names. Dropping a `<command>_<flags>.txt` into `tests/fixtures/` extends
/// this test without touching a line of code — that is how it was extended.
#[test]
fn no_row_delivers_more_than_the_module_it_shadows() {
    let mut checked = 0usize;
    let mut shadowed: Vec<String> = Vec::new();
    for (stem, body) in captured_outputs() {
        // Fixture stems carry the flags (`sysctl_hw`, `arp_a`, `ls_la`);
        // walk back to the program name the same way the bench does.
        let mut prog = stem.clone();
        let resolved = loop {
            if cmd::dispatch_auto_has_prog(&prog) || cmd::dispatch_named_has_prog(&prog) {
                break true;
            }
            match prog.rfind('_') {
                Some(i) => prog.truncate(i),
                None => break false,
            }
        };
        if !resolved || !cmd::spec::has_key(&prog) || !cmd::dispatch_auto_has_prog(&prog) {
            continue;
        }
        checked += 1;
        let (row, module) = (
            cmd::spec::dispatch(&prog, &body),
            cmd::dispatch_auto(&prog, &body),
        );
        if let (Some(row), Some(module)) = (row, module) {
            if row.len() > module.len() {
                shadowed.push(format!(
                    "{stem} -> {prog}: the row returns {} B, the module {} B",
                    row.len(),
                    module.len()
                ));
            }
        }
    }
    shadowed.sort();
    assert!(
        checked > 50,
        "only {checked} captured bodies reached a two-owner name — the corpus \
         walk or the stem resolution broke, and this test is now inert"
    );
    assert_eq!(
        shadowed,
        Vec::<String>::new(),
        "a declarative row delivers MORE than the module it shadows, on a \
         captured output of a real run"
    );
}

/// The `detect` gate of a `try_compact` is written once, not seventy-seven
/// times.
///
/// `cmd_entry_points!` derives `compact` FROM `try_compact` and serves 785
/// modules; the modules holding the INVERSE relation each spelled out their
/// `try_compact`. MEASURED by `tokenade-bench --bin duplication`: **56 copies**
/// of one body and **21** of another, identical.
///
/// This is not cosmetic. This gate IS the contract that makes a compactor
/// defensive: "if it is not my output, I do not answer", which makes a wrong
/// recognition fold NOTHING instead of destroying. Spelled out seventy-seven
/// times it can be forgotten once; that is the class this guard closes.
#[test]
fn no_module_spells_the_detect_gate_by_hand() {
    // Written with `concat!` rather than a line continuation: the first
    // version carried THIS file's indentation in the middle of the searched
    // string, so the guard looked for a body that exists nowhere and went
    // green on a tree where the defect was present. A guard that cannot fail
    // is tested by handing it the defect to find.
    const GATE: &str = "pub fn try_compact(text: &str) -> Option<String> {\n    if !detect(text) {\n        return None;\n    }\n";
    const A: &str = concat!(
        "pub fn try_compact(text: &str) -> Option<String> {\n    if !detect(text) {\n",
        "        return None;\n    }\n    Some(compact(text))\n}"
    );
    const B: &str = concat!(
        "pub fn try_compact(text: &str) -> Option<String> {\n    if !detect(text) {\n",
        "        return None;\n    }\n    super::aws_json::compact_if_json(text)\n}"
    );
    // The anchor: both forms MUST start with the gate, otherwise the
    // constants have drifted and the test no longer looks for anything.
    assert!(A.starts_with(GATE) && B.starts_with(GATE));

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cmd");
    let mut spelled = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            // The `archetypes` FAMILY, not the trunk alone: it is the one that
            // DECLARES the gate, and the day the declaration moves down into
            // `archetypes/<x>.rs` this guard would accuse its own declarer.
            let is_the_declarer = p.file_name().and_then(|f| f.to_str()) == Some("archetypes.rs")
                || p.parent()
                    .and_then(|d| d.file_name())
                    .and_then(|f| f.to_str())
                    == Some("archetypes");
            if is_the_declarer {
                continue;
            }
            seen += 1;
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            if body.contains(A) || body.contains(B) {
                spelled.push(
                    p.strip_prefix(&root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    // ANCHOR assertion: it is not about the modules but about the path that
    // finds them, so it cannot be killed on a healthy tree — and without it a
    // moved `src/cmd` would report "no offending module" on zero files read.
    assert!(seen > 500, "only {seen} modules seen — has the path moved?");
    spelled.sort();
    assert!(
        spelled.is_empty(),
        "these modules spell out a `detect` gate that `detect_gated_try_compact!` \
         writes: {spelled:?}. A copied gate is a forgettable gate."
    );
}

/// The `detect` gate holds in BOTH forms of the macro, including the one that
/// delegates the fold to a family compactor.
///
/// It only held in one: mutated, the delegated form folded without having
/// recognised and **none of the 21 modules concerned noticed** — their tests
/// check that `compact` does not panic, never that `try_compact` REFUSES.
///
/// What the gate refuses depends on the module's `detect`, and on those 21 it
/// is `any_output`: the shape defence lives in the family compactor, the gate
/// only holds emptiness. That is little, and that is precisely why it must be
/// written once rather than copied — measured while writing this test, whose
/// first version assumed a shape `detect` that does not exist here.
#[test]
fn the_delegated_form_refuses_output_it_did_not_recognise() {
    use tokenade_core::cmd::linux::aws_autoscaling_set_desired_capacity as m;
    // The module's gate: an empty output is not its own.
    assert!(
        !m::detect(""),
        "the module's `detect` must refuse emptiness"
    );
    assert_eq!(m::try_compact(""), None, "the `detect` gate was skipped");
    assert_eq!(m::try_compact("   \n\n"), None);
    // And the SHAPE defence, delegated: what is not JSON does not pass.
    assert_eq!(m::try_compact("total 4\ndrwxr-xr-x 2 u g 40 .\n"), None);

    // The NON-delegated form holds the full contract: its `detect` judges the
    // shape, and a foreign output is refused.
    use tokenade_core::cmd::linux::apropos;
    let not_apropos = "just some prose\nwith nothing that looks like a man page\n";
    assert!(!apropos::detect(not_apropos));
    assert_eq!(apropos::try_compact(not_apropos), None);
    assert_eq!(apropos::try_compact(""), None);
}
