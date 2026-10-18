//! Read a command line: flags, attached values, subcommand.
//!
//! Five predicates of a few lines, called from 31 files — almost
//! all of them `cmd/` compactors choosing their variant from the
//! flags. They lived in `filter.rs`, the dispatcher, because
//! the dispatcher was their first client: a command compactor therefore
//! had to climb up a layer to know whether `--json` is present.
//!
//! Nothing here knows what a fold, a format or a command is. It is
//! reading `argv`.

/// True when any element of `argv` is exactly `flag`.
pub fn has_flag(argv: &[String], flag: &str) -> bool {
    argv.iter().any(|a| a == flag)
}

/// True when any element of `argv` is exactly any of `flags`.
/// Cheap O(N × M) loop — both are tiny in practice.
pub fn has_any_flag(argv: &[String], flags: &[&str]) -> bool {
    argv.iter().any(|a| flags.contains(&a.as_str()))
}

/// True when any element of `argv` starts with `prefix`. Useful for
/// matching whole flag families (`--pretty=`, `--format=`,
/// `--message-format=`) without enumerating every suffix.
/// Used by per-command variant detectors (git_log `--pretty=`,
/// ls `--color=`, etc.).
pub fn has_flag_prefix(argv: &[String], prefix: &str) -> bool {
    argv.iter().any(|a| a.starts_with(prefix))
}

/// Return the value bound to `flag`, supporting both `--flag=value`
/// and `--flag value`. Returns `None` if the flag is absent OR if
/// `--flag` is the last token (no value to bind).
pub fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let eq_prefix = format!("{flag}=");
    for (i, a) in argv.iter().enumerate() {
        if let Some(v) = a.strip_prefix(&eq_prefix) {
            return Some(v);
        }
        if a == flag {
            return argv.get(i + 1).map(String::as_str);
        }
    }
    None
}

/// Walk past pre-subcommand flags to find the actual subcommand.
/// Each entry here is `(program, &[flags-that-take-a-value])` — flags
/// that swallow the next argv token (so we skip 2 not 1). Other flags
/// (starting with `-`) are assumed standalone.
pub fn find_subcommand<'a>(prog: &str, argv: &'a [String]) -> &'a str {
    let value_flags: &[&str] = match prog {
        "git" => &[
            "-C",
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--exec-path",
        ],
        // cargo's top-level value-flags. Without these,
        // `cargo --jobs 4 test` returns "4" as the subcommand and
        // routes through the wrong compactor (same bug class as
        // as for kubectl). Booleans like --frozen/--locked/--offline
        // intentionally OMITTED — they take no value, so the next
        // token IS the subcommand.
        "cargo" => &[
            "+Z",
            "--config",
            "--target-dir",
            "--manifest-path",
            // Additions:
            "--color",
            "-j",
            "--jobs",
            "--target",
            "-Z",
        ],
        // docker/podman top-level value-flags. The common
        // `docker --log-level debug ps` invocation used to return
        // "debug" as the subcommand. Same bug class as kubectl.
        "docker" | "podman" => &[
            "-H",
            "--host",
            "--context",
            "--config",
            "--tls-cert",
            "--tls-key",
            // Additions:
            "--log-level",
            "-l",
            "--tlscacert",
            "--tlscert",
            "--tlskey",
        ],
        // Keep this list in sync with kubectl's global "options"
        // section that take a value (`kubectl options`). Missing entries
        // here cause `find_subcommand` to misidentify the flag VALUE as
        // the subcommand — e.g. `kubectl --token X get pods` returned
        // "X" instead of "get" and routed the output through the wrong
        // compactor. Also covers helm/nomad/consul/vault which share
        // this dispatcher (their flag names overlap heavily).
        "kubectl" => &[
            "--context",
            "--namespace",
            "-n",
            "--kubeconfig",
            "--cluster",
            "--user",
            // Auth & TLS flags.
            "--token",
            "--as",
            "--as-group",
            "--as-uid",
            "--certificate-authority",
            "--client-certificate",
            "--client-key",
            "--server",
            "-s",
            "--tls-server-name",
            "--request-timeout",
            "--cache-dir",
            "--password",
            "--username",
            "--profile",
            "--profile-output",
        ],
        // npm/pnpm/yarn/bun top-level value-flags. Without
        // these, `npm --prefix /custom install pkg` returns "--prefix"
        // as the subcommand, which falls through to npm_run instead
        // of npm_install (same bug class as kubectl / cargo). The
        // boolean flags `--silent`/`--quiet`/`--json` are
        // intentionally NOT here — they're processed by the
        // dispatcher itself before the subcommand lookup.
        "npm" | "pnpm" | "yarn" | "bun" => &[
            "--prefix",
            "--cache",
            "--registry",
            "--userconfig",
            "--globalconfig",
            "--workspace",
            "-w",
            // pnpm-specific
            "--filter",
            "-F",
            "--dir",
            "-C",
            // yarn-specific
            "--cwd",
        ],
        _ => return argv.get(1).map(String::as_str).unwrap_or(""),
    };
    let mut i = 1usize;
    while let Some(tok) = argv.get(i) {
        // rustup toolchain override (`cargo +nightly build`).
        // The `+toolchain` prefix is a single token that selects which
        // toolchain runs cargo. Without this skip, `+nightly` was
        // returned as the subcommand (it doesn't start with `-`).
        // Only applies to cargo — `+` isn't a special prefix in
        // git/docker/kubectl/etc.
        if prog == "cargo" && tok.starts_with('+') && tok.len() > 1 {
            i += 1;
            continue;
        }
        if !tok.starts_with('-') {
            return tok.as_str();
        }
        // `--flag=value` is one token; `--flag value` is two.
        if tok.contains('=') {
            i += 1;
            continue;
        }
        if value_flags.iter().any(|f| tok == f) {
            i += 2;
        } else {
            i += 1;
        }
    }
    ""
}

pub fn basename(s: &str) -> String {
    // Strip a leading subshell paren and any surrounding quotes BEFORE taking
    // the path basename, so a command launched as `(echo …)`, `'python3' …` or
    // `"git" …` still routes to its real compactor (and reports under the real
    // program name) instead of the literal `(echo` / `'python3'` token. These
    // showed up as their own bogus "uncovered" rows on the dashboard.
    let s = s.trim_start_matches('(');
    let s = s.trim_matches(|c| c == '\'' || c == '"');
    let s = s.rsplit(['/', '\\']).next().unwrap_or(s);
    // Windows: `curl.exe` IS `curl`. Without this the `.exe` forms miss the
    // routing entirely — `curl.exe` never reached the curl compactor, and
    // `python.exe` / `php.exe` never reached the content-transparent arm —
    // and each reported as its own bogus "uncovered" row on the dashboard.
    // `.exe` is case-insensitive on Windows (`PYTHON.EXE`). `str::get` keeps
    // the split on a char boundary, so a non-ASCII name can't panic here.
    // `.cmd`, `.bat` and `.ps1` are the same story one layer up: npm installs
    // `npm.cmd` / `npx.cmd` / `npm.ps1` shims on Windows, and those ARE npm.
    // Measured 2026-10-02 in fleet data: `npm.cmd` 54 events / 120.7k tokens
    // and `npx.cmd` 10 events reported "uncovered", `php.ps1` 5.
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = s.get(..s.len().saturating_sub(ext.len())) {
            if !stem.is_empty() && s[stem.len()..].eq_ignore_ascii_case(ext) {
                return stem.to_string();
            }
        }
    }
    // A package manager run through its own Node entry script (`…/pnpm.js
    // install`, corepack's shims). Named, not stripped by extension: `server.js`
    // is somebody's script, `pnpm.js` is pnpm. Fleet: `pnpm.js` 142 events.
    match s {
        "pnpm.js" | "pnpm.cjs" => return "pnpm".to_string(),
        "npm-cli.js" => return "npm".to_string(),
        "npx-cli.js" => return "npx".to_string(),
        "yarn.js" | "yarn.cjs" => return "yarn".to_string(),
        "corepack.js" | "corepack.cjs" => return "corepack".to_string(),
        _ => {}
    }
    s.to_string()
}

/// Is the short option `letter` set, alone (`-c`) or in a cluster (`-rci`)?
///
/// Written privately twice before it moved here: `ls -lR` never reached the
/// recursive compactor because `-R` was matched exactly (§ ls_dispatcher), and
/// `grep -rci …` / `grep -rlIiE …` never reached the count and file-list folds
/// for the same reason — measured 2026-10-02, `grep -rci stash crates` came
/// back as "2011 match(es) in 2011 file(s)", every file at 1, when 1 802 of
/// them counted 0. Case-sensitive (`-l` is not `-L`), never from a `--long`
/// flag, and a cluster stops at the first letter in `takes_value`: in
/// `grep -ec` the `c` is the PATTERN that `-e` takes, not `--count`.
pub fn has_short_flag_letter(argv: &[String], letter: char, takes_value: &str) -> bool {
    argv.iter().skip(1).any(|a| {
        let Some(cluster) = a.strip_prefix('-') else {
            return false;
        };
        if cluster.starts_with('-') {
            return false;
        }
        for c in cluster.chars() {
            if c == letter {
                return true;
            }
            if takes_value.contains(c) {
                return false;
            }
        }
        false
    })
}

/// `awk 'NR>=200 && NR<=300' f.py` is `sed -n 200,300p f.py` written another
/// way: a program made only of `NR` conditions (and an optional `{print}`)
/// selects lines and prints them unchanged. Routed as a stream filter, its
/// output went to content detection — measured 2026-10-02 on a real
/// `connectors_toolkit.py`, Python whose `#` comments read as headings was
/// folded as MARKDOWN, 73 of 101 lines missing. Any other program transforms
/// what it prints and keeps the content route.
pub fn awk_only_selects_lines(argv: &[String]) -> bool {
    let joined = argv
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let Some(open) = joined.find(['\'', '"']) else {
        return false;
    };
    let quote = joined[open..].chars().next().unwrap_or('\'');
    let Some(len) = joined[open + 1..].find(quote) else {
        return false;
    };
    let program = &joined[open + 1..open + 1 + len];
    let mut rest = program.replace(' ', "");
    for print in ["{print$0}", "{print}"] {
        if let Some(stripped) = rest.strip_suffix(print) {
            rest = stripped.to_string();
        }
    }
    let rest = rest.replace("NR", "");
    !program.trim().is_empty()
        && program.contains("NR")
        && rest
            .chars()
            .all(|c| c.is_ascii_digit() || "<>=!&|()".contains(c))
}
