//! **`tokenade impact`: what breaks if I change this.**
//!
//! A dependency radius, bounded. An unbounded radius returns a list nobody
//! reads, and a radius that is too short gives false assurance — hence an
//! explicit depth and a cap on dependents that SAYS so when it bites.

use super::*;

pub(crate) fn cmd_impact(rest: &[String]) -> i32 {
    // #629 — `--json` mode for CI: emits the full symbol list (not
    // capped) and the full dependents list, so refactor-radius
    // tooling can branch on exact counts without scraping the text.
    // #680 — silent-accept sweep continued: same validator as
    // cmd_query so `impact foo --bogus` rejects + `impact foo bar`
    // rejects extra positional.
    // #1072 — batchable: collect every positional so the agent can ask
    // `tokenade impact a.rs b.rs c.rs` in ONE turn instead of N. Single
    // file keeps byte-identical output (text + legacy json shape).
    let files = match collect_positionals_with_values(
        rest,
        "impact",
        &["--json"],
        OUTPUT_MODE_FLAGS,
        "file",
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let json_mode = rest.iter().any(|a| a == "--json");
    let out_mode = match parse_output_mode(rest, "impact") {
        Ok(m) => m,
        Err(code) => return code,
    };
    let multi = files.len() > 1;
    let (idx, graph_available) = crate::fresh_index_with_calls();
    use std::collections::BTreeMap;
    // File-level call-graph centrality (computed once) orders the dependents so
    // the most heavily-used affected file — the biggest real blast radius —
    // leads the list instead of an arbitrary path order.
    let file_cen = tokenade_core::centrality::file_centrality(
        &idx.centrality(),
        idx.symbols
            .iter()
            .map(|s| (s.name.as_str(), s.file.as_str())),
    );

    let mut rc = 0;
    let mut json_payloads: Vec<serde_json::Value> = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        let file = file.as_str();
        // #937 — empty file string: `Index::impact_of`'s stem-substring
        // match used to return every indexed file as a dependent. Reject
        // here with a clear error. In batch mode keep going so one bad
        // arg doesn't sink the rest.
        if file.trim().is_empty() {
            eprintln!(
                "tokenade impact: <file> must not be empty. Pass a project-relative path \
                 (e.g. `tokenade impact crates/foo/src/lib.rs`)."
            );
            if multi {
                rc = rc.max(2);
                continue;
            }
            return 2;
        }
        // #978 — `impact <typo.rs>` used to emit "declares (0 symbols)"
        // exit 0, indistinguishable from a real empty file. Mirror
        // skeleton: a typo is loud. Resolve relative to project root.
        let resolved = {
            let p = std::path::Path::new(file);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                // Relative to where the user IS, then to the project root.
                // Since the index moved to the project root, resolving
                // only against the root made `impact index.rs` fail in the
                // directory holding `index.rs`; keeping the root as a fallback
                // preserves the documented root-relative form.
                let cwd = std::env::current_dir().unwrap_or_else(|_| root());
                let here = cwd.join(p);
                if here.exists() {
                    here
                } else {
                    root().join(p)
                }
            }
        };
        if !resolved.exists() {
            eprintln!(
                "tokenade impact: {file}: No such file or directory. \
                 Pass a path relative to the project root or an absolute path."
            );
            if multi {
                rc = rc.max(1);
                continue;
            }
            return 1;
        }
        // The #978 guard above asks whether the path is REAL. A directory is
        // real and still cannot be analysed: `impact <dir>` reported
        // "declares (0 symbols) / dependents (0)" with exit 0 — on a populated
        // module directory, a confident "nothing depends on this". `read` and
        // `skeleton` already refuse a directory; this is the same answer.
        if resolved.is_dir() {
            eprintln!(
                "tokenade impact: {file}: is a directory — name a file \
                 (e.g. `tokenade impact {file}/lib.rs`)."
            );
            if multi {
                rc = rc.max(1);
                continue;
            }
            return 1;
        }
        // The index keys symbols by ROOT-RELATIVE path (`s.file == file`, an
        // exact string compare), while the dependents half matches on file
        // STEM. So an absolute path found dependents and no declarations.
        // MEASURED 2026-08-08 on this repo, same file both ways:
        //
        //   impact crates/tokenade-core/src/detect.rs   → declares (84 symbols)
        //   impact $PWD/crates/…/detect.rs              → declares (0 symbols), dependents (6)
        //
        // Half a report, exit 0, and the refusal message two guards up
        // advertises absolute paths ("Pass a path relative to the project root
        // or an absolute path"). Normalise once, here, so both halves see the
        // key the index actually uses.
        // The index handed to this command is re-based to the cwd
        // (`fresh_index`), so the key is the path relative to the cwd.
        let cwd = std::env::current_dir().unwrap_or_else(|_| root());
        let key = resolved
            .strip_prefix(&cwd)
            .map(|p| p.to_string_lossy().into_owned())
            .or_else(|_| {
                resolved
                    .strip_prefix(root())
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|_| file.to_string());
        let mut rep = idx.impact_of(&key);
        // A file the indexer never parsed answers exactly like a leaf module:
        // "declares (0 symbols) / dependents (0)", exit 0 — a confident
        // "nothing depends on this" about a file the graph has never seen.
        //
        // The directory guard right above was written for this shape and fixed
        // only the directory half (#978). MEASURED 2026-08-08: `tokenade impact
        // README.md` on this repo returns that answer, and so would any source
        // file in a language the indexer does not support — where the claim is
        // not merely uninformative but wrong. Same lens as
        // elsewhere: an empty answer must say WHICH empty it is.
        //
        // stderr, exit unchanged: the report is still true for a file that
        // genuinely has no dependents, and callers parsing stdout keep working.
        // `symbols` alone is no longer the test for "never parsed": a test
        // file's declarations are all counted under `test_symbols`, so an
        // integration test would have been told it "contributed no symbols"
        // one line above a count of thirty-nine. Both halves, or the note
        // says the opposite of the report it introduces.
        if rep.symbols.is_empty() && rep.test_symbols.is_empty() && rep.dependents.is_empty() {
            eprintln!(
                "note: {file} contributed no symbols to the index — tokenade does not \
                 parse it as source here, so `dependents (0)` means `not in the graph`, \
                 not `nothing depends on it`."
            );
        }
        // Order dependents by centrality DESC (path asc as a stable fallback)
        // so the highest-impact affected file leads.
        rep.dependents.sort_by(|a, b| {
            let ac = file_cen.get(a).copied().unwrap_or(0.0);
            let bc = file_cen.get(b).copied().unwrap_or(0.0);
            bc.partial_cmp(&ac)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });
        // #609 — group symbols by kind (`fn`/`type`/`struct`/...) and show
        // counts; cap each kind at 12 names with "+N more" overflow.
        let mut by_kind: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for s in &rep.symbols {
            let (kind, name) = s.split_once(' ').unwrap_or(("symbol", s.as_str()));
            by_kind.entry(kind).or_default().push(name);
        }

        if json_mode {
            // CI consumers need the FULL set, not the 12-cap.
            let symbols_by_kind: Vec<serde_json::Value> = by_kind
                .iter()
                .map(|(kind, names)| {
                    let mut sorted = names.clone();
                    sorted.sort();
                    serde_json::json!({
                        "kind": kind,
                        "count": sorted.len(),
                        "names": sorted,
                    })
                })
                .collect();
            let payload = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "file": rep.file,
                "symbol_count": rep.symbols.len(),
                "test_symbol_count": rep.test_symbols.len(),
                "test_symbols": rep.test_symbols,
                "symbols_by_kind": symbols_by_kind,
                "dependents": rep.dependents,
                "dependent_count": rep.dependents.len(),
                // Machine consumers need the same caveat the text view carries:
                // `dependent_count` is a floor, not a total. See #32.
                "ambiguous_caller_count": rep.ambiguous_callers,
            });
            log_code_intel(&format!("impact {file}"), "impact_of", &payload.to_string());
            json_payloads.push(payload);
            continue;
        }

        // Reduced modes: paths → the dependent files (the "who is
        // affected" list an agent feeds to Read); count → bare
        // symbol/dependent counts; outline → the declared-symbols block
        // only, dropping the dependents list. Full is unchanged.
        let body = match out_mode {
            OutputMode::Paths => {
                let mut b = String::new();
                push_dependents(&mut b, &rep.dependents, "");
                b
            }
            OutputMode::Count => {
                let t = if rep.test_symbols.is_empty() {
                    String::new()
                } else {
                    format!(" (+{} test)", rep.test_symbols.len())
                };
                format!(
                    "symbols: {}{t}, dependents: {}\n",
                    rep.symbols.len(),
                    rep.dependents.len()
                )
            }
            OutputMode::Full | OutputMode::Outline => {
                let mut b = format!("{} declares ({} symbols):\n", rep.file, rep.symbols.len());
                for (kind, names) in by_kind {
                    // DECLARATION order, not alphabetical. Only 12 names of
                    // each kind are shown, so WHICH 12 decides whether this
                    // command answers "what does this file offer?" at all —
                    // and sorting by name answers it with whatever happens to
                    // start with "a". Measured on a test-heavy source file:
                    // 11 of the 12 function names shown were unit tests and
                    // the 151 hidden ones held the entire API. Source order
                    // puts a file's head first, which is where its API lives
                    // and where an inline test module never is.
                    b.push_str(&render_kind_line(kind, &names, 12));
                }
                // Counted, not listed. Nothing outside this file can depend on
                // a `#[cfg(test)]` declaration, so it does not belong in the
                // answer to "what depends on this file" — but saying nothing
                // would look like the file has no tests. `query <name>` still
                // finds any of them by name.
                if !rep.test_symbols.is_empty() {
                    b.push_str(&format!(
                        "  + {} test declaration(s), not shown — nothing outside this \
                         file can depend on them (find one by name with `tokenade query`)\n",
                        rep.test_symbols.len()
                    ));
                }
                // Outline stops at the declarations; Full appends the
                // dependents list.
                if out_mode == OutputMode::Full {
                    b.push_str(&format!("dependents ({}):\n", rep.dependents.len()));
                    push_dependents(&mut b, &rep.dependents, "  ");
                    // Name what the list could NOT confirm. This command is
                    // what an agent runs to scope a change, and it is
                    // measurably short: 19 listed for `redact.rs` against at
                    // least 28 real, because a name declared in several files
                    // loses its call attribution. Printing the count turns a
                    // silent gap into a known one — the fix itself needs the
                    // call qualifier.
                    if !graph_available {
                        b.push_str(
                            "  … the call graph could not be read, so dependents reached \
                             only through a qualified call are missing from this list — \
                             an absence of data, not an absence of callers.\n",
                        );
                    }
                    if rep.ambiguous_callers > 0 {
                        b.push_str(&format!(
                            "  … {} more file(s) call a name this file declares, but that \
                             name has several declarations, so the call cannot be pinned \
                             here. An UPPER bound on what is missing — some of those calls \
                             belong to the other declarations.\n",
                            rep.ambiguous_callers
                        ));
                    }
                }
                b
            }
        };
        if multi {
            if fi > 0 {
                println!();
            }
            println!("==> {file} <==");
        }
        let body = tokenade_core::redact::redact(&body);
        print!("{body}");
        log_code_intel(&format!("impact {file}"), "impact_of", &body);
    }

    if json_mode {
        // single file → exact legacy shape; multiple → nest under `files`.
        let out = if json_payloads.len() == 1 {
            serde_json::to_string_pretty(&json_payloads[0]).unwrap()
        } else {
            serde_json::to_string_pretty(&serde_json::json!({ "files": json_payloads })).unwrap()
        };
        println!("{out}");
    }
    rc
}

/// How many dependents `impact` lists before it says the rest exist.
///
/// The list is already ordered by centrality, so the head is the part that
/// scopes a change; the tail is every leaf that happens to import the module.
/// Measured on a hub file of a 7 891-file repository: 468 dependents, 21 532
/// bytes — roughly 5 400 tokens to answer "what does this affect?", most of it
/// unreadable at that length. `--json` still carries the whole list.
pub(crate) const IMPACT_MAX_DEPENDENTS: usize = 30;

/// Accept a `map` path spelled from the workspace root as well as from the
/// current directory.
///
/// The cwd spelling wins when both match: it is what the caller sees around
/// them, and re-basing is a fallback for the spelling that would otherwise be
/// refused, never a re-interpretation of one that works.
pub(crate) fn rebase_prefix_if_unmatched(
    idx: &tokenade_core::index::Index,
    p: String,
    root: &std::path::Path,
) -> String {
    let under = |q: &str| {
        idx.symbols
            .iter()
            .any(|s| tokenade_core::index::Index::path_is_under(&s.file, q))
    };
    if under(&p) {
        return p;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return p;
    };
    // `None` when the cwd IS the root — then there is no second spelling.
    let Some(rebase) = tokenade_core::platform::PathRebase::new(root, &cwd) else {
        return p;
    };
    let rebased = rebase.apply(&p);
    if under(&rebased) {
        rebased
    } else {
        p
    }
}
