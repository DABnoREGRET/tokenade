//! **Commands launched through ANOTHER one.**
//!
//! `python -m pytest`, `npm run test`, `uv run`: the program the user typed is
//! not the one whose output we read. Recognising the real executor is what
//! lets the right compactor answer — otherwise the interpreter would be
//! dispatched, and no compactor knows "python".

use super::*;

/// **The program's compactor COMPETES with the generic one.**
///
/// Three families wrote it identically — `ping`/`traceroute`,
/// `mount`/`lsblk`, `ipconfig`/`tasklist` — and all three returned the named
/// result as soon as it existed, never letting the format detector run. Yet a
/// module can return its input UNCHANGED: that is what `ps.rs` does on a
/// `ps aux` output, and the sysinfo family carried the same fault (measured
/// 4 387 bytes returned as is against 2 016).
///
/// The SHORTER of the two wins.
pub(super) fn named_or_generic(
    prog: &str,
    output: &str,
    before: usize,
    label: &str,
) -> FilterResult {
    let named = crate::cmd::dispatch_named(prog, output);
    let fallback = auto_or_generic_labelled(output, before, label);
    let chosen = named_if_it_folded(fallback.output.clone(), named, output);
    if chosen.len() == fallback.output.len() {
        return fallback;
    }
    let after = tokenizer::estimate(&chosen);
    FilterResult {
        filter: label.to_string(),
        output: chosen,
        savings: Savings::new(before, after),
    }
}

pub(super) fn subcommand_spec(
    prog: &str,
    sub: &str,
    output: &str,
    before: usize,
) -> Option<FilterResult> {
    if sub.is_empty() || sub == prog {
        return None;
    }
    // `-` → `_` here and not downstream: `spec::dispatch` retries a hyphenated
    // key on its own, `dispatch_auto` does not — and this asks `dispatch_auto`
    // first. A mutation that drops the replacement survives every fixture whose
    // subcommand resolves through the spec table, which is why it is written
    // down rather than left to look untested.
    let key = format!("{}_{}", prog, sub.replace('-', "_"));
    let c = crate::cmd::dispatch_auto(&key, output)
        .or_else(|| crate::cmd::spec::dispatch(&key, output))?;
    let after = tokenizer::estimate(&c);
    if after >= before {
        return None;
    }
    Some(FilterResult {
        filter: prog.to_string(),
        output: c,
        savings: Savings::new(before, after),
    })
}

pub(super) fn auto_or_generic_labelled(output: &str, before: usize, label: &str) -> FilterResult {
    let (_, compacted, _) = crate::auto::compact(output);
    let after = tokenizer::estimate(&compacted);
    FilterResult {
        filter: label.to_string(),
        output: compacted,
        savings: Savings::new(before, after),
    }
}

pub(super) fn filter_wrapped(argv: &[String], output: &str, _before: usize) -> FilterResult {
    // sudo/timeout/nice/etc. wrap a real command. Route by the wrapped
    // command — which is NOT simply `argv[1..]`: the wrapper's own flags and
    // its numeric argument sit in between.
    //
    // `timeout 300 cargo test` used to route on `300`, so it matched no
    // compactor and fell through to `generic` — on precisely the long-running
    // commands an agent puts a timeout on. `program_basename` grew the same
    // fix; routing and label MUST agree or the telemetry names a gap that is
    // not the real one.
    // The WRAPPER's own name, not `program_basename` — that one resolves the
    // wrapped program, which is the thing this is trying to find.
    let wrapper = argv
        .first()
        .map(|a| basename(a.trim_matches(|c| c == '\'' || c == '"')).to_string())
        .unwrap_or_default();
    let i = skip_wrapper_prefix(&wrapper, argv, 1);
    let inner: Vec<String> = argv.iter().skip(i).cloned().collect();
    if inner.is_empty() {
        return generic_filter_result(output);
    }
    filter_command(&inner, output)
}

pub(super) fn generic_filter_result(output: &str) -> FilterResult {
    let before = tokenizer::estimate(output);
    let compacted = generic(output);
    let after = tokenizer::estimate(&compacted);
    FilterResult {
        filter: "generic".to_string(),
        output: compacted,
        savings: Savings::new(before, after),
    }
}

// ─── Helper shims expected by extracted dispatchers — EMPTY ───
//
// This section held one-line redirections: a dispatcher in `cmd/linux/`
// called `filter::<name>`, which called back the `cmd/linux/` compactor doing
// the work. The round trip decided nothing; it only made `cmd` name `filter`,
// and kept the `cmd ↔ filter` knot (81 sites) inextricable. The last 36 were
// inlined, the final batch being `git_diff`, `git_diff_for_log`,
// `cargo_test_json`, `find_long`, `npm_json` and `npm_silent`.
//
// The two graves that matter, because they name a defect the inlined form can
// no longer produce:
//
// - `cargo_test_default`, removed on 2026-05-31: it called `dispatch(&[], …)`,
//   which fell back to the default arm → infinite recursion (#407).
// - `linter`, removed the same day, same class (#385): the default arm of
//   `linter_dispatcher` called back `filter::linter`, which called the
//   dispatcher back with an empty argv.
//
// A shim that calls back its own caller is invisible when reading the two
// files separately. Do not write one again: a command dispatcher calls the
// compactor, never `filter`.

// Most pytest_* variants share line format (dots / FAILED markers /
// summary) and use the same compactor. The exception is `--tb=no`, where
// the user asked for *no* tracebacks, so its compactor also elides any
// FAILURES/ERRORS block (see `pytest_tb_no`).

/// Python test runners are launched *through the interpreter* (`python -m
/// unittest`, `python3 -m pytest`, …), so `prog` is `python`/`python3` and the
/// big routing `match` would send them to `auto_or_generic` (content-sniff),
/// which never reaches a test-specific compactor — unittest in particular gets
/// mis-routed to the stacktrace folder and comes out disordered. This detects
/// `python[-version] -m <runner>` and returns the runner module so the caller
/// can route to the right compactor. Generalises beyond unittest: pytest, nose,
/// nose2, green and xmlrunner are all commonly invoked this way.
pub(super) fn python_m_test_runner(argv: &[String]) -> Option<&'static str> {
    const RUNNERS: [&str; 6] = ["unittest", "pytest", "nose2", "nose", "green", "xmlrunner"];
    let prog = basename(argv.first()?);
    if !matches!(
        prog.as_str(),
        "python" | "python3" | "python2" | "py" | "pypy" | "pypy3"
    ) {
        return None;
    }
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        // `-m <runner>` (module in the next arg)
        if a == "-m" {
            let m = argv.get(i + 1)?.as_str();
            return RUNNERS
                .iter()
                .copied()
                .find(|r| m == *r || m.starts_with(&format!("{r}.")));
        }
        // `-m<runner>` (glued, e.g. `-munittest`)
        if let Some(rest) = a.strip_prefix("-m") {
            if !rest.is_empty() {
                return RUNNERS
                    .iter()
                    .copied()
                    .find(|r| rest == *r || rest.starts_with(&format!("{r}.")));
            }
        }
        i += 1;
    }
    None
}

/// Every test runner we recognise once a wrapper is stripped — the union of
/// the JS, Ruby, PHP and Python ecosystems. `python -m` is handled by
/// `python_m_test_runner` (it also accepts `unittest`/`nose…` which are stdlib
/// modules, not standalone executables); this set is the standalone runners a
/// package-runner (`npx`, `bunx`, `pnpm dlx`, `bundle exec`, `poetry run`, …)
/// would launch by name.
pub(super) const WRAPPED_RUNNERS: [&str; 13] = [
    // JS
    "jest",
    "vitest",
    "mocha",
    "cypress",
    "playwright",
    "jasmine",
    "ava",
    "tap",
    // Ruby
    "rspec",
    // PHP
    "phpunit",
    "pest",
    // Python (standalone entrypoints, distinct from `python -m`)
    "pytest",
    "nose2",
];

/// Resolve `WRAPPED_RUNNERS` from a package-runner invocation. Given the index
/// of the wrapper's *first argument*, skip the wrapper's own flags and any
/// value-taking flag, then match the first bare token against the runner set.
/// Tolerates a `@scope/`-prefixed or versioned spec (`jest@29`, `@scope/jest`).
/// Shared by every package-runner branch so they all skip flags consistently.
pub(super) fn first_pkg_runner(argv: &[String], start: usize) -> Option<&'static str> {
    let mut i = start;
    while i < argv.len() {
        let a = argv[i].as_str();
        // Flags that consume the following arg (npx `-p pkg`, `--package pkg`).
        if a == "-p" || a == "--package" {
            i += 2;
            continue;
        }
        // Any other flag (`-y`, `--yes`, `--no-install`, `--package=…`, `-c`).
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        let pkg = a.rsplit('/').next().unwrap_or(a);
        let pkg = pkg.split('@').next().unwrap_or(pkg);
        return WRAPPED_RUNNERS.iter().copied().find(|r| *r == pkg);
    }
    None
}

/// A test runner launched through a WRAPPER that masks it from program-name
/// routing. Generalises `python_m_test_runner` to every language's wrapper:
///   - interpreter:      `python -m <runner>`           (delegated)
///   - JS package-runner: `npx <r>`, `bunx <r>`,
///                        `pnpm dlx|exec <r>`, `yarn dlx|exec <r>`
///   - Ruby:             `bundle exec <r>`
///   - Python:           `poetry|uv|pdm|hatch|pipenv|rye run <r>`
/// In each case `prog` is the wrapper (no output shape of its own) and the big
/// routing `match` would send it to `auto_or_generic` / `pkgmgr_dispatch`,
/// never reaching a test compactor — the same gap `python -m unittest` had.
/// Returns the resolved runner so the caller can route to its compactor.
/// (Direct `jest`/`rspec`/`phpunit`/… and `npm/pnpm/yarn/bun test` already
/// route via their own arms; this only covers the masking wrappers.)
pub(super) fn wrapped_test_runner(argv: &[String]) -> Option<&'static str> {
    if let Some(r) = python_m_test_runner(argv) {
        return Some(r);
    }
    let prog = basename(argv.first()?);
    match prog.as_str() {
        // Bare package-runners: the runner is the first non-flag token.
        "npx" | "bunx" => first_pkg_runner(argv, 1),
        // Two-token launchers: `<tool> <verb> <runner>`. Only the launch verbs
        // run an arbitrary command; other subcommands (`pnpm install`) are not
        // test runs and must not match.
        "pnpm" | "yarn" => match argv.get(1).map(|s| s.as_str()) {
            Some("dlx") | Some("exec") => first_pkg_runner(argv, 2),
            _ => None,
        },
        "bundle" => match argv.get(1).map(|s| s.as_str()) {
            Some("exec") => first_pkg_runner(argv, 2),
            _ => None,
        },
        "poetry" | "uv" | "pdm" | "hatch" | "pipenv" | "rye" => {
            match argv.get(1).map(|s| s.as_str()) {
                Some("run") => first_pkg_runner(argv, 2),
                _ => None,
            }
        }
        // PHP: the interpreter masks the runner in both idiomatic forms —
        // `php artisan test` (Laravel's wrapper around PHPUnit/Pest) and
        // `php vendor/bin/phpunit` (the vendored entrypoint, which
        // `first_pkg_runner` resolves through its path basename). Any other
        // `php <script>` resolves to None and falls to the interpreter arm,
        // which is correct: a script has no test-runner shape.
        "php" => match argv.get(1).map(|s| basename(s)).as_deref() {
            Some("artisan") => match argv.get(2).map(|s| s.as_str()) {
                Some("test") => Some("phpunit"),
                _ => None,
            },
            _ => first_pkg_runner(argv, 1),
        },
        _ => None,
    }
}

/// Route a wrapper-resolved runner to its dedicated compactor. pytest variants
/// reuse the pytest dispatcher (reporter-aware); the unittest family is
/// block-structured (own compactor); everything else goes through the
/// order-preserving line-based test engine (`test_runner_dispatch` → pytest),
/// which is strictly better than the generic deduper / stacktrace folder.
pub(super) fn route_wrapped_runner(
    runner: &str,
    argv: &[String],
    output: &str,
) -> (&'static str, String) {
    match runner {
        "pytest" => (
            "pytest",
            crate::cmd::linux::pytest_dispatcher::dispatch(argv, output),
        ),
        "unittest" | "nose" | "nose2" | "green" | "xmlrunner" => {
            ("unittest", crate::cmd::linux::unittest::compact(output))
        }
        _ => (
            "test-runner",
            crate::cmd::linux::test_runner_dispatcher::dispatch(runner, argv, output),
        ),
    }
}
