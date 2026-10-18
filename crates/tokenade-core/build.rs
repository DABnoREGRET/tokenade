//! Generates `pub mod X;` lists from the on-disk layout of
//! `src/cmd/{linux,macos,windows}/`.
//!
//! Why a build script?
//!
//! The command-compactor architecture is one file per command. The
//! L96/L97/L98 batches alone contributed >400 files; future batches
//! will push that into the thousands. Hand-curating a `pub mod foo;`
//! line per file in `cmd/<os>/mod.rs` is a chore that produces
//! mechanical merge conflicts and silently-dropped modules.
//!
//! This script walks each `cmd/<os>/` directory at compile time and
//! emits a `cmd_<os>_mods.rs` artifact in `OUT_DIR` containing one
//! `pub mod X;` line per `.rs` file found (and per subdirectory that
//! is itself a module). Each `cmd/<os>/mod.rs` then pulls that
//! artifact in with a single `include!` macro.
//!
//! Adding a new compactor is now: drop `cmd/<os>/<command>.rs` in
//! place, recompile. No `mod.rs` edit required.
//!
//! The script also re-runs whenever any `.rs` under those directories
//! changes (Cargo's `rerun-if-changed` directive).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Forward Cargo's TARGET triple so updater::current_target_triple() can read it via env!().
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=TOKENADE_TARGET_TRIPLE={target}");
    println!("cargo:rerun-if-env-changed=TARGET");

    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let mut all_entries: Vec<(String, String, String)> = Vec::new(); // (os, module, dispatch_key)
    for sub in ["linux", "macos", "windows"] {
        let src = Path::new(&manifest).join("src").join("cmd").join(sub);
        let entries = emit_module_list(&src, sub, &out_dir);
        for (m, path) in &entries {
            // Skip directory modules — they're subcommand dispatchers
            // (`cargo/`, `docker/`, `git/`) whose top-level mod.rs
            // doesn't expose `try_compact`. Their parent dispatcher
            // handles routing.
            //
            // Match the file NAME, not a path suffix: `chmod.rs`,
            // `crmod.rs` and `dsmod.rs` all end in `mod.rs`, so the
            // suffix test skipped three real commands. They got no
            // `dispatch_auto` arm and no explicit one either, which
            // left `chmod`'s dedicated compactor unreachable.
            if Path::new(path).file_name().is_some_and(|f| f == "mod.rs") {
                continue;
            }
            let key = dispatch_key_of(m);
            all_entries.push((sub.to_string(), m.clone(), key));
        }
        println!("cargo:rerun-if-changed={}", src.display());
    }
    emit_auto_dispatch(&all_entries, &out_dir);

    // The file that holds the `match` is FOUND, not named. It was called
    // `src/filter.rs`; the day the dispatch moved down into
    // `src/filter/dispatch.rs`, the build stopped on « `match
    // (prog.as_str(), sub)` block not found » — a build panic for a
    // mere file move. We require exactly ONE: two
    // `match` blocks of this shape, and the generated table would only speak
    // of one of them, silently.
    let filter_src = the_one_source_carrying(
        &Path::new(&manifest).join("src"),
        "match (prog.as_str(), sub)",
    );
    println!("cargo:rerun-if-changed={}", filter_src.display());
    emit_filter_arm_progs(&filter_src, &out_dir);
}

/// The only file under `root` that holds `needle`.
///
/// Panics naming what it found: a `build.rs` that searches badly must
/// SAY so, otherwise it generates an empty table and everything depending on it becomes
/// silently wrong.
fn the_one_source_carrying(root: &Path, needle: &str) -> PathBuf {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut all = Vec::new();
    walk(root, &mut all);
    all.sort();
    let hits: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| {
            // A test file often SPELLS OUT what it checks; it does not hold
            // the shipped code.
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
                && fs::read_to_string(p).is_ok_and(|b| b.contains(needle))
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "`{needle}` must live in a single file under {} — found: {hits:?}",
        root.display()
    );
    hits.into_iter().next().unwrap()
}

/// Extract the program names matched by the explicit arms of
/// `filter_command`'s `match (prog.as_str(), sub)` block and emit them as
/// `pub static FILTER_ARM_PROGS: &[&str]`. These names (git, cargo, kubectl,
/// docker, …) have family compactors wired directly in filter.rs rather than
/// per-command modules, so they are invisible to the auto-dispatch table.
/// Scanning the source here keeps `knows_program` in sync with the match
/// automatically — adding an arm to filter.rs updates the generated list on
/// the next build, with no hand-maintained mirror.
///
/// A tuple pattern is recognised as `("name", X)` where X is `_` or a string
/// literal — return-value tuples like `("git-status", git_status_dispatch(…))`
/// have an expression in second position and are skipped. Scanning stops at
/// the default `_ =>` arm.
fn emit_filter_arm_progs(filter_src: &Path, out_dir: &str) {
    let src = fs::read_to_string(filter_src)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", filter_src.display()));
    let start = src
        .find("match (prog.as_str(), sub)")
        .expect("filter.rs: `match (prog.as_str(), sub)` block not found");
    let region = &src[start..];
    let end = region
        .find("\n        _ => {")
        .expect("filter.rs: default `_ =>` arm not found after the match");
    let region = &region[..end];

    let mut names: Vec<String> = Vec::new();
    // Same scan, one level finer: the (program, subcommand) pairs. Needed to
    // answer "is THIS subcommand covered" — `git` being in `names` says
    // nothing about `git ls-files`, and advice built on the program name
    // alone tells people to write a compactor that already exists.
    let mut pairs: Vec<(String, String)> = Vec::new();
    let bytes = region.as_bytes();
    let mut i = 0;
    while let Some(off) = region[i..].find("(\"") {
        let name_start = i + off + 2;
        let Some(name_len) = region[name_start..].find('"') else {
            break;
        };
        let name = &region[name_start..name_start + name_len];
        let mut j = name_start + name_len + 1; // past closing quote
        if bytes.get(j) == Some(&b',') {
            j += 1;
            while bytes.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
                j += 1;
            }
            // Second tuple element must be a pattern (`_` or a literal),
            // not an expression — that's what distinguishes a match arm
            // from a returned (name, compacted) pair.
            if matches!(bytes.get(j), Some(&b'_') | Some(&b'"'))
                // Names with a path separator (`./gradlew`) can never be a
                // PATH basename — not useful for aliasing.
                && !name.contains('/')
            {
                names.push(name.to_string());
                if bytes.get(j) == Some(&b'"') {
                    let sub_start = j + 1;
                    if let Some(sub_len) = region[sub_start..].find('"') {
                        pairs.push((
                            name.to_string(),
                            region[sub_start..sub_start + sub_len].to_string(),
                        ));
                    }
                }
            }
        }
        i = name_start + name_len + 1;
    }
    names.sort();
    names.dedup();
    pairs.sort();
    pairs.dedup();
    assert!(
        pairs.iter().any(|(p, s)| p == "git" && s == "status"),
        "filter.rs arm scan looks broken: expected (git, status) among the pairs"
    );
    assert!(
        names.iter().any(|n| n == "git") && names.iter().any(|n| n == "cargo"),
        "filter.rs arm scan looks broken: expected at least git+cargo, got {names:?}"
    );

    let mut buf = String::with_capacity(names.len() * 16 + 256);
    buf.push_str(
        "// AUTO-GENERATED by build.rs from src/filter.rs. Do not edit.\n\
         /// Program names with an explicit family arm in `filter_command`'s\n\
         /// match (git, cargo, kubectl, …) — extracted from the source so the\n\
         /// list can never drift from the match itself.\n\
         pub static FILTER_ARM_PROGS: &[&str] = &[\n",
    );
    for n in &names {
        buf.push_str("    \"");
        buf.push_str(&n.replace('\\', "\\\\").replace('"', "\\\""));
        buf.push_str("\",\n");
    }
    buf.push_str("];\n");
    buf.push_str(
        "/// The (program, subcommand) pairs those arms actually name. A program\n\
         /// in `FILTER_ARM_PROGS` can still have uncovered subcommands.\n\
         pub static FILTER_ARM_PAIRS: &[(&str, &str)] = &[\n",
    );
    for (p, s) in &pairs {
        buf.push_str("    (\"");
        buf.push_str(&p.replace('\\', "\\\\").replace('"', "\\\""));
        buf.push_str("\", \"");
        buf.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
        buf.push_str("\"),\n");
    }
    buf.push_str("];\n");
    let out_file = Path::new(out_dir).join("filter_arm_progs.rs");
    fs::write(&out_file, buf).unwrap_or_else(|e| {
        panic!("cannot write {}: {e}", out_file.display());
    });
}

/// Strip the suffixes we used in module names to avoid Rust-keyword
/// collisions or per-OS clashes. The on-disk filename is the source
/// of truth; the dispatch key is the user-typed command name.
/// Suffixes a module filename may carry that are NOT part of the program
/// name. Emitted as `cmd::DISPATCH_KEY_SUFFIXES` so a test can undo the
/// mapping instead of keeping a second copy of this list that drifts.
const KEY_SUFFIXES: &[&str] = &[
    "_cmd",
    "_msg",
    "_macos",
    "_win",
    "_shell",
    "_builtin",
    "_runtime",
    "_progress",
    "_short",
    "_dump",
    "_live",
    "_ext",
    "_cli",
    "_linux",
    "_summary",
    "_info",
    "_csv",
    "_fs",
];

fn dispatch_key_of(module: &str) -> String {
    // Strip the artificial suffixes used in module names to dodge
    // Rust-keyword or per-OS clashes. After stripping, return the
    // filename stem AS-IS — most Unix commands use underscores in
    // their real name (`sw_vers`, `vm_stat`, `system_profiler`,
    // `dot_clean`, `fs_usage`, …). Commands like `apt-get` whose
    // module file is `apt_get.rs` are routed via an explicit arm in
    // [`crate::cmd::dispatch_named`]; we do NOT silently rewrite
    // every underscore to a dash here.
    for sfx in KEY_SUFFIXES {
        if let Some(stem) = module.strip_suffix(sfx) {
            return stem.to_string();
        }
    }
    module.to_string()
}

fn emit_module_list(dir: &Path, label: &str, out_dir: &str) -> Vec<(String, String)> {
    // Each entry: (module_name, absolute_path_to_module_root).
    // For a file module `foo.rs` the path is `<dir>/foo.rs`.
    // For a directory module `bar/` the path is `<dir>/bar/mod.rs`.
    let mut entries: Vec<(String, String)> = Vec::new();
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => panic!("cannot read {}: {e}", dir.display()),
    };
    for ent in read.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == "mod.rs" {
            continue;
        }
        let path = ent.path();
        if path.is_dir() {
            let mod_rs = path.join("mod.rs");
            if mod_rs.is_file() {
                entries.push((name.to_string(), mod_rs.to_string_lossy().to_string()));
            }
        } else if let Some(stem) = name.strip_suffix(".rs") {
            entries.push((stem.to_string(), path.to_string_lossy().to_string()));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = String::with_capacity(entries.len() * 80);
    buf.push_str(
        "// AUTO-GENERATED by build.rs from on-disk layout. Do not edit.\n\
         // Source dir: src/cmd/",
    );
    buf.push_str(label);
    buf.push_str("/\n\n");
    for (m, p) in &entries {
        // `include!` injects this list at the OUT_DIR include site, so
        // Rust would look for `<OUT_DIR>/<m>.rs` by default. Pin the
        // path explicitly to the real source location.
        buf.push_str("#[path = \"");
        // Escape backslashes for Windows hosts.
        for c in p.chars() {
            if c == '\\' {
                buf.push_str("\\\\");
            } else {
                buf.push(c);
            }
        }
        buf.push_str("\"]\n#[allow(non_snake_case)] pub mod ");
        buf.push_str(m);
        buf.push_str(";\n");
    }
    let out_file = Path::new(out_dir).join(format!("cmd_{label}_mods.rs"));
    fs::write(&out_file, buf).unwrap_or_else(|e| {
        panic!("cannot write {}: {e}", out_file.display());
    });
    entries
}

/// Emit `cmd_auto_dispatch.rs` — a `dispatch_auto(prog, text)` function
/// containing a giant match over every per-command module's dispatch
/// key. Used as the trailing fallback inside `dispatch_named` so we
/// don't have to hand-maintain ~600+ arms whose only logic is "route
/// program name X to module Y".
///
/// OS priority: Linux > macOS > Windows. If the same dispatch key
/// exists in multiple OSes, the earlier (Linux first) wins by
/// returning `Some(out)` first; the others trail with `.or_else`.
/// Modules that own the prog name (via `dispatch_named` explicit arm)
/// never reach here, so this is always a safe fallback.
fn emit_auto_dispatch(all: &[(String, String, String)], out_dir: &str) {
    use std::collections::BTreeMap;
    // dispatch_key -> Vec<(os, module)>, preserving OS priority order.
    let priority = ["linux", "macos", "windows"];
    let mut by_key: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (os, m, key) in all {
        if key.is_empty() {
            continue;
        }
        by_key
            .entry(key.clone())
            .or_default()
            .push((os.clone(), m.clone()));
    }
    // Sort each bucket by OS priority.
    for v in by_key.values_mut() {
        v.sort_by_key(|(os, _)| priority.iter().position(|p| p == os).unwrap_or(99));
    }

    // Hyphen aliases: for every underscore key (e.g. `apt_get`) that has no
    // corresponding hyphen key (`apt-get`) already in the table, emit a
    // second match arm that routes `apt-get` to the same module. This lets
    // developers drop `my_tool.rs` and have `my-tool` routed automatically
    // without a hand-maintained explicit arm in `dispatch_named`.
    //
    // Safe for real-underscore commands (sw_vers, vm_stat, …): those program
    // names never appear with a hyphen at runtime, so the alias arm is dead
    // code. No false matches.
    let mut hyphen_aliases: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for key in by_key.keys() {
        if key.contains('_') {
            let h = key.replace('_', "-");
            if !by_key.contains_key(&h) {
                hyphen_aliases.insert(h, key.clone());
            }
        }
    }

    let mut buf = String::with_capacity(by_key.len() * 80);
    buf.push_str(
        "// AUTO-GENERATED by build.rs. Do not edit.\n\
         //\n\
         // `dispatch_auto(prog, text)` provides the trailing fallback used\n\
         // by `dispatch_named` — one arm per per-command module. Routes\n\
         // <command> to the matching module, with Linux > macOS > Windows\n\
         // fallthrough for shared names.\n\
         //\n\
         // Adding a new compactor is therefore: drop a file into\n\
         // `src/cmd/<os>/<command>.rs`, recompile. Routing is automatic.\n\n",
    );
    // The keys that STILL have a stub module on disk.
    //
    // Emitted so a parity test can ask "is there something to compare this row
    // against?" instead of guessing. Without it, `spec.rs` had no way to select
    // the comparable rows, and the equivalence its module doc claimed to prove
    // was proven by nothing — `builtin_spec_parity_with_stubs` never existed
    // and `candidate_specs_match_live_stubs` had an empty key list.
    buf.push_str(
        "/// Filename suffixes stripped to obtain a dispatch key. Lets a test\n\
         /// map a module file back to the name that routes to it.\n\
         pub static DISPATCH_KEY_SUFFIXES: &[&str] = &[\n",
    );
    for sfx in KEY_SUFFIXES {
        buf.push_str("    \"");
        buf.push_str(sfx);
        buf.push_str("\",\n");
    }
    buf.push_str("];\n\n");
    buf.push_str("pub static AUTO_DISPATCH_KEYS: &[&str] = &[\n");
    for key in by_key.keys() {
        buf.push_str("    \"");
        buf.push_str(&key.replace('\\', "\\\\").replace('"', "\\\""));
        buf.push_str("\",\n");
    }
    buf.push_str("];\n\n");
    buf.push_str(
        "pub fn dispatch_auto(prog: &str, text: &str) -> Option<String> {\n\
         \x20   match prog {\n",
    );
    for (key, mods) in &by_key {
        // Escape any inner quotes/backslashes in the dispatch key.
        let key_esc = key.replace('\\', "\\\\").replace('"', "\\\"");
        buf.push_str("        \"");
        buf.push_str(&key_esc);
        buf.push_str("\" => ");
        // Chain: first os tries try_compact, the rest with or_else.
        for (i, (os, m)) in mods.iter().enumerate() {
            if i == 0 {
                buf.push_str("crate::cmd::");
                buf.push_str(os);
                buf.push_str("::");
                buf.push_str(m);
                buf.push_str("::try_compact(text)");
            } else {
                buf.push_str("\n            .or_else(|| crate::cmd::");
                buf.push_str(os);
                buf.push_str("::");
                buf.push_str(m);
                buf.push_str("::try_compact(text))");
            }
        }
        buf.push_str(",\n");
    }
    // Emit auto-generated hyphen alias arms.
    for (alias, orig_key) in &hyphen_aliases {
        if let Some(mods) = by_key.get(orig_key) {
            let alias_esc = alias.replace('\\', "\\\\").replace('"', "\\\"");
            buf.push_str("        \"");
            buf.push_str(&alias_esc);
            buf.push_str("\" => ");
            for (i, (os, m)) in mods.iter().enumerate() {
                if i == 0 {
                    buf.push_str("crate::cmd::");
                    buf.push_str(os);
                    buf.push_str("::");
                    buf.push_str(m);
                    buf.push_str("::try_compact(text)");
                } else {
                    buf.push_str("\n            .or_else(|| crate::cmd::");
                    buf.push_str(os);
                    buf.push_str("::");
                    buf.push_str(m);
                    buf.push_str("::try_compact(text))");
                }
            }
            buf.push_str(",\n");
        }
    }
    buf.push_str("        _ => None,\n    }\n}\n\n");

    // Emit dispatch_auto_has_prog: returns true iff the program name appears
    // in the auto-dispatch table (i.e., there exists at least one compactor
    // module for it). Used to distinguish "unknown program" (None) from
    // "known but content too short to fold" (Some passthrough) in routing tests.
    buf.push_str(
        "/// Returns `true` if `prog` has at least one auto-dispatch compactor module.\n\
         /// Does NOT run `try_compact` — pure name lookup.\n\
         pub fn dispatch_auto_has_prog(prog: &str) -> bool {\n",
    );
    if by_key.is_empty() {
        buf.push_str("    let _ = prog;\n    false\n}\n");
    } else {
        buf.push_str("    matches!(prog,\n");
        let keys: Vec<&String> = by_key.keys().collect();
        let alias_keys: Vec<&String> = hyphen_aliases.keys().collect();
        let total = keys.len() + alias_keys.len();
        let mut idx = 0usize;
        for key in &keys {
            let key_esc = key.replace('\\', "\\\\").replace('"', "\\\"");
            buf.push_str("        \"");
            buf.push_str(&key_esc);
            buf.push('"');
            idx += 1;
            if idx < total {
                buf.push_str(" |\n");
            } else {
                buf.push('\n');
            }
        }
        for alias in &alias_keys {
            let alias_esc = alias.replace('\\', "\\\\").replace('"', "\\\"");
            buf.push_str("        \"");
            buf.push_str(&alias_esc);
            buf.push('"');
            idx += 1;
            if idx < total {
                buf.push_str(" |\n");
            } else {
                buf.push('\n');
            }
        }
        buf.push_str("    )\n}\n");
    }

    // Emit the same name set as an enumerable slice. `dispatch_auto_has_prog`
    // answers "is this name known?"; AUTO_DISPATCH_PROGS answers "what names
    // are known?" — needed by the shell proxy to build its alias list
    // programmatically instead of hand-maintaining one.
    buf.push_str(
        "\n/// Every program name routed by `dispatch_auto` (keys + hyphen aliases).\n\
         /// Same set `dispatch_auto_has_prog` accepts, as an enumerable slice.\n\
         pub static AUTO_DISPATCH_PROGS: &[&str] = &[\n",
    );
    for key in by_key.keys().chain(hyphen_aliases.keys()) {
        let key_esc = key.replace('\\', "\\\\").replace('"', "\\\"");
        buf.push_str("    \"");
        buf.push_str(&key_esc);
        buf.push_str("\",\n");
    }
    buf.push_str("];\n");

    let out_file = Path::new(out_dir).join("cmd_auto_dispatch.rs");
    fs::write(&out_file, buf).unwrap_or_else(|e| {
        panic!("cannot write {}: {e}", out_file.display());
    });
}
