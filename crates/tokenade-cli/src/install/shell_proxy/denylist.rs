//! **What must NEVER be wrapped.**
//!
//! An alias put on `cd`, `exec`, `source` or a shell builtin does not change
//! a command's output: it breaks the shell. The list is long because each
//! shell has its own builtins, and `nu` has some that carry the names of
//! real programs.
//!
//! A hand-written list is the right tool here: the question is not
//! "is this a builtin?" but "does an alias there break something?",
//! and that cannot be derived from any observable property.

use super::*;

/// Commands NEVER aliased by the shell proxy, even though tokenade has a
/// compactor for them. The proxy list itself is fully programmatic —
/// binaries on PATH ∩ `tokenade_core::filter::knows_program()` — so it
/// tracks the compactor registry (per-command modules, filter.rs family
/// arms, dispatch_named aliases) with zero manual upkeep. This denylist is
/// the ONLY hand-maintained piece: exclusion is a safety judgment that
/// cannot be derived from the registry.
///
///   * text-stream utilities (`cat`, `head`, `sort`, `awk`, `sed`, `grep`,
///     …) — they exist to feed pipelines and redirections; compacting them
///     mid-stream corrupts bytes the consumer needs verbatim. The hook path
///     still compacts a solo `cat bigfile` (it sees the whole command line);
///     the alias path can't see pipeline context on every shell, so it
///     skips byte-critical programs outright.
///   * exact-bytes producers (`jq`, `yq`, `tar`, `gzip`, `openssl`,
///     checksums, `nc`) whose whole job is emitting machine-consumed
///     output.
///   * prefix wrappers (`sudo`, `env`, `timeout`, `nice`, `xargs`, …) —
///     filter.rs already unwraps these to find the real program, and
///     aliasing `sudo` would capture its interactive password prompt.
///   * `ls` / `find` / `stat` / `which` / `date` / `uname` … — output
///     routinely consumed by `$(…)`; tiny savings, real breakage risk.
///   * shells, remote shells (`ssh` forwards raw streams), and tokenade
///     itself.
pub(crate) const PROXY_DENYLIST: &[&str] = &[
    // text-stream utilities
    "cat",
    "bat",
    "batcat",
    "head",
    "tail",
    "sort",
    "uniq",
    "wc",
    "cut",
    "tr",
    "awk",
    "gawk",
    "mawk",
    "sed",
    "grep",
    "egrep",
    "fgrep",
    "zgrep",
    "ug",
    "tee",
    "xargs",
    "expand",
    // exact-bytes producers
    "jq",
    "yq",
    "fx",
    "jp",
    "base64",
    "openssl",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "cksum",
    "od",
    "xxd",
    "hexdump",
    "tar",
    "gzip",
    "gunzip",
    "zip",
    "unzip",
    "bzip2",
    "bunzip2",
    "nc",
    "ncat",
    // prefix wrappers
    "sudo",
    "doas",
    "env",
    "nice",
    "ionice",
    "nohup",
    "setsid",
    "timeout",
    "chrt",
    "eatmydata",
    "stdbuf",
    "sshpass",
    // $(…)-consumed one-liners
    "ls",
    "find",
    "locate",
    "mlocate",
    "plocate",
    "stat",
    "which",
    "whereis",
    "type",
    "pgrep",
    "pkill",
    "whoami",
    "hostname",
    "uname",
    "date",
    "id",
    "logname",
    "printenv",
    // shells, remote shells, self
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "csh",
    "tcsh",
    "fish",
    "nu",
    "pwsh",
    "powershell",
    "cmd",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "tokenade",
    // shell-builtin name collisions (fc = POSIX history builtin, sc/where
    // = Windows-only routes; shadowing builtins buys nothing)
    "fc",
    "sc",
    "where",
    "write",
    // builtins of the shells we install into — shadowing them rewires the
    // shell itself, not a program. Two observed breakages: fish's `alias`
    // helper builds its function body with `echo` (escape-interpreting
    // builtin), so once `echo` is shadowed every later alias definition
    // dies with "Missing end" at EVERY shell startup; and `test` is a fish
    // reserved keyword — `function test` is a hard parse error. `kill`
    // as a builtin understands job specs (%1) the external can't, and
    // `realpath` is a fish builtin. All of these print a handful of bytes,
    // so denying them costs nothing.
    "echo",
    "printf",
    "pwd",
    "test",
    "kill",
    "realpath",
    // Shell-STATE builtins — aliasing any of these to `__tokenade_proxy`
    // (which runs `tokenade wrap` in a CHILD shell) loses their effect on the
    // PARENT shell: `cd`/`pushd`/`popd` wouldn't change the agent's directory,
    // `export`/`unset` wouldn't set env, `source`/`.` wouldn't load the file,
    // etc. `cd` in particular is a `known_program_names()` entry AND ships a
    // `/usr/bin/cd` stub on macOS, so `proxyable_on_path` was aliasing it and
    // breaking `cd` for every no-hook (shell-proxy) user until they manually
    // un-aliased it (field report). This is the same set as
    // `command_rewrite::segment_must_not_be_wrapped`. They print ~nothing, so
    // there is zero compaction value to offset the breakage.
    "cd",
    "pushd",
    "popd",
    "export",
    "unset",
    "source",
    ".",
    "set",
    "alias",
    "unalias",
    "shopt",
    "declare",
    "typeset",
    "local",
    "readonly",
    "let",
    "eval",
    "exec",
    "trap",
    "return",
    "break",
    "continue",
    "wait",
    "exit",
    "true",
    "false",
    ":",
    // version managers & env shims that REWRITE the shell and SELF-REFERENCE.
    // pyenv/rbenv/asdf & friends regenerate their "shim" scripts on `rehash`,
    // baking in their own location resolved via `command -v <name>` / `$0`.
    // With the proxy active that lookup returns the alias we installed
    // (`pyenv='__tokenade_proxy pyenv'`) instead of the real binary, so the
    // generated shims capture the alias STRING and every python3/ruby/node…
    // shim breaks (observed: python3 dead after a reboot once the shims were
    // rebuilt under the proxy). nvm/conda/sdkman are pure shell functions
    // whose `activate`/`use` subcommands mutate the live environment —
    // shadowing the function silently disables them. None of these emit
    // compactable noise; and an agent's `pyenv install`/`pyenv versions`
    // output is still compacted by the PreToolUse Bash hook, which dispatches
    // through the registry regardless of this interactive-alias denylist.
    "pyenv",
    "rbenv",
    "nodenv",
    "goenv",
    "jenv",
    "phpenv",
    "plenv",
    "luaenv",
    "asdf",
    "mise",
    "rtx",
    "direnv",
    "nvm",
    "conda",
    "mamba",
    "micromamba",
    "sdk",
    // pyenv/conda ship dedicated libexec sub-executables (`pyenv-versions`,
    // `pyenv-install`, `conda env list`) that the compactor registry surfaces
    // as their own program names. They self-reference exactly like their
    // parent and must never be aliased either — bare `pyenv`/`conda` above
    // only covers the front binary.
    "pyenv-install",
    "pyenv-versions",
    "conda-env-list",
    // shell prompt / hook / init generators installed via `eval "$(X init …)"`.
    // The emitted init script is re-evaluated on every prompt render or `cd`
    // and frequently re-resolves the tool by name; shadowing it both wedges
    // that hot path and risks baking the alias into the generated hook. They
    // produce no compactable agent output, so aliasing them is pure downside.
    // (zinit/zsh-zinit is a sourced zsh *function*, not a PATH binary — denied
    // here so the diagnostic `--all` set can't recreate the collision either.)
    "starship",
    "starship-prompt",
    "zoxide",
    "zsh-zinit",
    "fzf",
];

/// Nushell implements many coreutils names as structured-data builtins
/// (`ps` returns a table, `cp`/`rm` have different flags…). An `alias`
/// shadows the builtin, silently swapping its semantics in user scripts —
/// so these are excluded from the nushell fragment ONLY; on POSIX shells
/// they are ordinary externals and stay proxied. Compiled from nushell's
/// command reference; names absent from the registry are harmless here.
pub(crate) const NU_BUILTIN_COLLISIONS: &[&str] = &[
    "cp", "mv", "rm", "mkdir", "du", "ps", "seq", "split", "watch", "clear", "touch", "open",
    "start", "exec", "input", "history",
];

/// The proxy alias list: every binary reachable through `path_var` (a
/// PATH-style value) that tokenade knows how to compact, minus
/// [`PROXY_DENYLIST`]. One `read_dir` pass per PATH entry — a dozen
/// syscalls total — plus pure name lookups; this runs at every shell
/// startup, so it must stay cheap. Result is sorted (stable alias output
/// → stable rc diffs).
///
/// Filtering against PATH at generation time means absent tools are never
/// shadowed — an alias pointing at a missing binary makes `command -v
/// kubectl` succeed in every script and breaks prompt engines (e.g. tide's
/// kubectl segment) that probe optional tools. And because every name must
/// pass `knows_program`, arbitrary PATH filenames (spaces, quotes, …) can
/// never leak into the generated shell fragments.
///
/// On Windows an executable is `<name>.<ext>` for ext ∈ PATHEXT and names
/// are case-insensitive; on Unix the name must match exactly.
pub(crate) fn proxyable_on_path(path_var: &std::ffi::OsStr) -> Vec<String> {
    use std::collections::HashSet;
    let win_exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    let mut present: HashSet<String> = HashSet::new();
    for dir in std::env::split_paths(path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // d_type from readdir — free on every mainstream platform.
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if cfg!(windows) {
                let lower = name.to_ascii_lowercase();
                if let Some((stem, ext)) = lower.rsplit_once('.') {
                    if win_exts.iter().any(|e| e == ext) {
                        present.insert(stem.to_string());
                        continue;
                    }
                }
                present.insert(lower);
            } else {
                present.insert(name);
            }
        }
    }
    let mut out: Vec<String> = present
        .into_iter()
        .filter(|n| !PROXY_DENYLIST.contains(&n.as_str()))
        .filter(|n| tokenade_core::filter::knows_program(n))
        .collect();
    out.sort();
    out
}

/// `--all` variant: every program tokenade knows minus the denylist,
/// ignoring PATH presence. Diagnostic only — installing this re-creates
/// the missing-binary shadowing problem on purpose.
pub(crate) fn proxyable_all() -> Vec<String> {
    tokenade_core::filter::known_program_names()
        .into_iter()
        .filter(|n| !PROXY_DENYLIST.contains(n))
        .map(str::to_string)
        .collect()
}
