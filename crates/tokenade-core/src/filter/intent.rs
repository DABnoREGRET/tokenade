//! **What the user ASKED for, read from their argv.**
//!
//! Folding an output someone wanted to see whole is the one fault this
//! product cannot make up for. Before any compaction, this module answers:
//! did they ask for the raw output, the help, a verbose mode, a precise
//! slice — and is the shape of the argv even readable? Every rule here is
//! bounded, because the neighbouring one already got it wrong: `-v` is NOT
//! "verbose" for eighty programs that make it "version".

use super::*;

/// Strip leading bash comment lines from a shell command string.
/// `# header\ngrep foo bar` → `grep foo bar`. Used by `cmd_wrap`
/// before splitting argv so the comment text doesn't get tokenised
/// into the argv and confuse compactor dispatch.
pub fn strip_leading_comment_lines(cmd: &str) -> &str {
    let mut rest = cmd;
    loop {
        let trimmed = rest.trim_start();
        if trimmed.starts_with('#') {
            match trimmed.find('\n') {
                Some(nl) => rest = &trimmed[nl + 1..],
                None => return "",
            }
        } else {
            return trimmed;
        }
    }
}

/// One declarer for "is this token a shell assignment": `command_rewrite`'s.
///
/// This module kept its own copy, which demanded an UPPERCASE name — the bug
/// fixed in the other copy on 2026-08-27 and never here, although this is
/// the one the ROUTER asks. Measured 2026-10-02 by the `attribution` bench:
/// `d=$(ls -dt … | head -1); grep x $d` was named `d=$(ls`, and
/// `f=/tmp/x cat $f` still routed on the value.
pub(super) use crate::command_rewrite::is_env_assignment_token;

/// True when the caller's argv shows they want the raw output untouched.
///
/// Covers two failure modes our compactors used to hit:
///
///   - **intent flags**: any token matching `verbose / debug / trace /
///     nocapture / show-output / --show-* / --print-*` (or the `-v+`
///     short forms `-v`, `-vv`, `-vvv`) signals the user explicitly
///     asked for verbose / unstructured output that a compactor would
///     re-shape. Pattern-matched rather than enumerated so new
///     tools' verbose flags work out of the box.
///   - **any pipe (`|`)**: when ANY pipe appears in the command, the
///     downstream tool reshapes the bytes, so the argv[0]-routed
///     compactor is parsing the wrong format and either inflates or
///     eats useful content. Pre-fix we tried to enumerate "reshape
///     tools" (tail/head/wc/jq/awk/sed/…); the list was always
///     incomplete. Now: any pipe → passthrough. Cost: a small fraction
///     of cases where `cmd | cat` would have compacted; benefit: no
///     more new pipe targets sneaking past us.
///
/// `2>&1` and `>` / `>>` redirects don't count — they keep the
/// producer's shape unchanged (or drop output to file entirely).
///
/// Chain operators (`&&`, `||`, `;`) are NOT handled here; the
/// PreToolUse Bash hook splits the command into segments and wraps
/// each segment with its own `tokenade wrap`, so each segment gets
/// its own correct dispatch.
pub fn user_wants_raw(argv: &[String]) -> bool {
    explicit_raw_intent(argv) || argv_shape_is_unknowable(argv)
}

/// The user ASKED for raw bytes: a verbose / debug / trace / nocapture /
/// show-* / print-* flag anywhere in the argv.
///
/// This half is byte-for-byte passthrough and always will be — the request is
/// explicit and a compactor would re-shape what was deliberately asked for.
/// Programs where a short `-v` is NOT a request for verbose output.
///
/// The pattern scan below is deliberately enum-free so it catches any tool's
/// verbose convention. The cost is that it reads a single letter without
/// knowing what that letter means to the program, and for these it means
/// something else entirely:
///
///   - `grep`/`rg` and friends: `-v` is `--invert-match`. Nothing to do with
///     verbosity, and inverting a match usually returns MORE lines, not fewer.
///   - `iptables` and the other netfilter front-ends: `-v` adds the packet and
///     byte counter COLUMNS. The result is still a table, and still foldable.
///
/// Measured on this machine's agent transcripts, 2026-09-02: of 94 943 recorded
/// Bash commands, **3 701 were `grep`/`rg` carrying a separate `-v`**, and 839
/// of those (23 %) produced output above `GREP_MIN_TOKENS` — roughly 470 900
/// tokens delivered raw because one letter was read as a verbosity dial. The
/// bundled form (`grep -rnv`) never had the problem, because the scan only
/// treats a v-ONLY short flag as verbose; so the same command compacted or not
/// depending on whether the agent wrote `-rnv` or `-rn -v`.
///
/// Long forms are unaffected everywhere: `grep --verbose` does not exist, and
/// `--verbose`/`--debug`/`--trace` keep their meaning on every program.
pub(super) const SHORT_V_IS_NOT_VERBOSE: &[&str] = &[
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ripgrep",
    "ug",
    "ack",
    "ack-grep",
    "git-grep",
    "iptables",
    "ip6tables",
    "arptables",
    "ebtables",
    "iptables-legacy",
    "iptables-nft",
    // `awk -v name=value` is a VARIABLE assignment — the letter takes an
    // argument and has never meant verbose. `sort -v` is `--version-sort`, an
    // ordering. Neither program has a verbosity dial at all, so reading the
    // letter as one only ever costs a fold: measured 2026-09-02, both come back
    // uncompacted on output that folds 300 lines to 226 without the flag.
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "sort",
    // The same test, applied to the rest of coreutils: does the program have a
    // verbosity dial at all? Verified against `--help` on 2026-09-04, and none
    // of these five does.
    //
    //   - `ls -v` is `--sort=version`, a natural sort of the numbers inside
    //     names (`f1 f2 f10` instead of `f1 f10 f2`). What `ls` calls verbose
    //     is the `-l` FORMAT word, which `-v` does not select. This one was
    //     not merely missing: a test named `ls` as the example of "a program
    //     where -v IS verbosity", so the wrong claim was pinned.
    //   - `od -v` is `--output-duplicates` and `hexdump -v` is
    //     `--no-squeezing`: both turn OFF the `*` that stands in for repeated
    //     lines. The output they produce is the most repetitive shape this
    //     tool folds, and asking for it was read as asking to keep it raw.
    //   - `nl -v N` is the starting line number and `join -v N` selects the
    //     unpairable side. Both take a value, and neither has ever meant
    //     verbose.
    //
    // Measured 2026-09-04 on `ls -R crates`, 36 416 bytes: 3 356 folded, 7 052
    // folded as `ls -Rv`, and 36 416 — nothing at all — as `ls -v -R`. Same
    // command, same bytes, and the fold depended on where the letter was
    // written.
    "ls",
    "od",
    "hexdump",
    "nl",
    "join",
    // Surveyed 2026-10-02 against `--help` on this machine, by meaning rather
    // than one incident at a time (operator: "the programs where you know it
    // does not mean real verbose").
    //
    // `-v` takes a VALUE that is not a verbosity: `docker run|create -v
    // a:b` mounts a volume (the output is the container's), `psql -v
    // NAME=VALUE` sets a variable (the output is the query's), `ffmpeg -v
    // <loglevel>` most often LOWERS the log (`-v error`). Read as "verbose",
    // a container's test log or a query result came back unfolded. Docker's
    // one real verbose `-v`, `system df -v`, has its own compactor, which
    // `raw_request_honoured` lets through past the delivery threshold anyway.
    "docker",
    "podman",
    "nerdctl",
    "psql",
    "ffmpeg",
    // `-v` prints the VERSION: one line, nothing to fold — listed so the
    // table says what the letter means, not because it costs.
    "node",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "perl",
    "make",
    "diff",
    "patch",
    "file",
    "lsof",
    // `-v` changes how CONTENT is rendered, not how much is printed: `cat -v`
    // is `--show-nonprinting` (`^M` for a carriage return), `head -v` /
    // `tail -v` only add the `==> f <==` header. The output is still the
    // file, and a read keeps its own rules.
    "cat",
    "head",
    "tail",
    // Second survey, 2026-10-02, same test (`--help` on this machine):
    // `pgrep -v` is `--inverse` (the program shares its man page with its
    // signal-sending sibling, whose `-v` is the same inversion); `findmnt -v`
    // is `--nofsroot`, `lsblk -v` `--virtio`, `free -v` `--committed`, and
    // `uname -v` `--kernel-version` — all select WHAT is listed, none how
    // chatty. `pr -v` is `--show-nonprinting`, as with `cat`. `tree -v` sorts
    // by version (tree(1); not installed here). `cmp`, `xxd` and `bc` print
    // their version.
    "pgrep",
    "pkill",
    "findmnt",
    "lsblk",
    "free",
    "uname",
    "pr",
    "tree",
    "cmp",
    "xxd",
    "bc",
];

/// Did the command ask for its own reference text?
///
/// A help text is a lookup table, and every fold of one buys its savings out
/// of the table. Measured 2026-09-04: `tar --help` came back folded from
/// **46 short options to 12**, with mentions of `--exclude` down from 16 to 5
/// — an agent reading it to find a flag was reading a document with three
/// quarters of the options removed, under a banner that said `282 line(s)`
/// and not "your options are gone".
///
/// And there is no fold that keeps them and still pays. Measured over ten
/// help texts on this machine, the option lines and their continuations are
/// **26 % to 99 % of the bytes** (median ~85 %): `curl --help all` is 99 %,
/// `node --help` 88 %, `tar --help` 87 %. A compactor that keeps the table
/// saves nothing worth having, so the honest answer is to hand the reference
/// over whole.
///
/// Long forms only, and `-h` deliberately NOT among them: it is
/// `--human-readable` on `ls`, `du`, `df`, `free` and `sort`, and reading one
/// letter without knowing what it means to the program is exactly what cost
/// `ls -v` its fold (see [`SHORT_V_IS_NOT_VERBOSE`]). A bare `help` counts
/// only as the FIRST argument, which is where `git help`, `go help`,
/// `docker help` and `openssl help` put it — never as a file called `help`
/// three arguments into a `cp`.
pub(super) fn asks_for_help(argv: &[String]) -> bool {
    let mut rest = argv.iter().skip(1);
    if rest.clone().next().map(String::as_str) == Some("help") {
        return true;
    }
    rest.any(|t| matches!(t.as_str(), "--help" | "--usage" | "-?" | "/?"))
}

pub fn explicit_raw_intent(argv: &[String]) -> bool {
    if asks_for_help(argv) {
        return true;
    }
    // Flag scan over the whole argv. Pattern-based so we catch any
    // tool's verbose convention (Rust `--nocapture`, jest `--verbose`,
    // make `--debug`, git `--trace`, etc.) without an enum.
    let prog = program_basename(argv);
    let short_v_means_something_else = SHORT_V_IS_NOT_VERBOSE.contains(&prog.as_str());
    for tok in argv.iter().skip(1) {
        let t = tok.as_str();
        if short_v_means_something_else && is_short_v_flag(t) {
            continue;
        }
        if is_verbose_flag(t) {
            return true;
        }
    }
    false
}

/// Is the request for raw bytes one this layer can honour as asked?
///
/// A help text is always handed over whole ([`asks_for_help`] says why). A
/// verbose / debug / show-* flag is honoured while the output fits under the
/// delivery threshold ([`crate::disclose::threshold_tokens`]) — the size the
/// next stage delivers inline. Above it, nothing was ever going to arrive raw:
/// the stash cut the passthrough to its first and last lines. Measured
/// 2026-10-02 on `docker system df -v` (2 341 lines, ~122 k tokens): the head
/// and tail kept one container of ten and NONE of the named volumes — the
/// 62.94 GB cache the command was run to find was in the elided middle —
/// while the program's own compactor keeps every table and summarises only
/// the anonymous volumes. So past the threshold the program's compactor
/// speaks, and the stash still holds every byte.
///
/// One rule for every program. The flag's MEANING per program still matters
/// below the threshold, which is what [`SHORT_V_IS_NOT_VERBOSE`] records.
pub fn raw_request_honoured(argv: &[String], before_tokens: usize) -> bool {
    if asks_for_help(argv) {
        return true;
    }
    explicit_raw_intent(argv) && before_tokens <= crate::disclose::threshold_tokens()
}

/// Is this `env` the environment dump rather than the launcher?
///
/// `env` is two commands sharing a name. `env FOO=1 cargo build` runs cargo
/// and its output is cargo's, which is why `env` sits in the wrapper arm at
/// all. But `env`, `env -0` and `env FOO=1` with nothing to run print the
/// ENVIRONMENT — and with nothing wrapped, the wrapper arm ends in the plain
/// generic pass. Measured 2026-09-04 on this machine's own environment, the
/// same 822 bytes came back as **786 under `env` and 354 under `printenv`**:
/// the spelling every agent types was the one missing the fold that drops
/// session noise and truncates `PATH`.
///
/// Checked while measuring, because the env compactor also redacts: this was
/// never a leak. The pipeline's redaction pass runs on both routes, so
/// `AWS_SECRET_ACCESS_KEY` and a variable merely NAMED for a secret come back
/// redacted either way.
///
/// A token that is neither a flag nor an assignment is the command being
/// launched. `env -u NAME` — where `NAME` is the flag's VALUE, not a command —
/// therefore still reads as a launcher; that is today's behaviour and this
/// rule does not change it.
pub(super) fn env_is_the_dump(argv: &[String]) -> bool {
    argv.iter()
        .skip(1)
        .all(|t| t.starts_with('-') || t.contains('='))
}

/// The `-v`, `-vv`, `-vvv`… form on its own — the half of [`is_verbose_flag`]
/// that reads a single letter. Split out so [`explicit_raw_intent`] can skip
/// exactly that half for the programs where the letter is not a verbosity dial,
/// without also disarming `--verbose` on them.
pub(super) fn is_short_v_flag(tok: &str) -> bool {
    tok.len() >= 2
        && tok.starts_with('-')
        && !tok.starts_with("--")
        && tok[1..].chars().all(|c| c == 'v')
}

/// A pipe is in the command, so **argv\[0\] does not name the shape of the
/// bytes that came out**. `git log | awk '{print $1}'` is argv\[0\] `git` and
/// a bare column of hashes.
///
/// That is the whole of the stated reason this branch existed, and it is a
/// real one — but "we cannot pick a compactor from the command" is not the
/// same claim as "these bytes must not be compacted". tokenade has a second
/// router that never consults argv\[0\] at all, and the arm right below in
/// `filter_command_inner` already gives exactly this answer to exactly this
/// problem: `head`, `tail`, `sort`, `uniq`, `wc`, `cut`, `tr`, `awk`, `sed`
/// and `xargs` route to [`auto_or_generic`], which reads the output's own
/// shape. A pipeline ENDING in one of those is the same case as running it
/// alone, and it was being answered two different ways.
///
/// The benefit the old rule bought is kept whole: nothing here enumerates
/// "reshape tools", so no new pipe target sneaks past. The content router
/// cannot be fooled by a program name it has never heard of, because it does
/// not read one.
pub fn argv_shape_is_unknowable(argv: &[String]) -> bool {
    // Any pipe → passthrough (see doc above), with one exception.
    //
    // A TRAILING bare `| cat` is not a pipeline, it is an
    // identity sink. Agents append it constantly to defeat a pager, and
    // `git log | cat` therefore lost compaction for no reason: nothing
    // downstream consumes the bytes, so folding them corrupts nothing.
    //
    // Deliberately narrow. `cat` must be the LAST stage, bare, and the only
    // pipe in the command:
    //   - with arguments (`| cat -n`, `| cat file`) it transforms or reads
    //     something else, so it is a real stage;
    //   - anywhere but last (`git log | cat | grep x`) a real consumer
    //     follows, and compacting would corrupt its input.
    let pipes = argv.iter().filter(|t| t.as_str() == "|").count();
    if pipes == 1 && argv.last().map(String::as_str) == Some("cat") {
        if let Some(pos) = argv.iter().position(|t| t == "|") {
            // Exactly `… | cat` — the pipe is immediately before the final token.
            if pos + 2 == argv.len() {
                return false;
            }
        }
    }
    pipes > 0
}

/// `find Acmeo -type f | sort`: a path lister whose later stages only REORDER
/// or SLICE (`sort`, `uniq` without `-c`, `head`, `tail`, bare `cat`) prints
/// the same shape as the lister alone, so it keeps the lister's route — the
/// part of argv before the first `|`. Routed by content instead, real paths
/// (spaces in file names, accented letters) failed the plain-listing detector
/// and fell to `generic`, which stride-samples: measured 2026-10-02 on this
/// machine's ledger and reproduced, 406 paths delivered as 34 lines, 379 of
/// them absent, "in 113 runs of 2" — while the same `find` without `| sort`
/// came back as its full grouped tree.
pub fn lister_stage_when_the_rest_only_orders(argv: &[String]) -> Option<&[String]> {
    let mut stages = argv.split(|t| t == "|");
    let first = stages.next()?;
    let prog = first.first()?.rsplit('/').next()?;
    // PowerShell's lister, too: `Get-ChildItem C:\\x -Filter *.dll |
    // Select-Object -First 400` (a real capture, 2026-10-05) went by content
    // to a dash-table fold, 10 005 bytes, where the cmdlet's reader gives 2 953.
    if !matches!(prog, "find" | "fd" | "fdfind")
        && !matches!(
            prog.to_ascii_lowercase().as_str(),
            "get-childitem" | "gci" | "dir"
        )
    {
        return None;
    }
    let mut rest = stages.peekable();
    rest.peek()?;
    rest.all(|st| match st.first().map(String::as_str) {
        Some("sort") | Some("head") | Some("tail") => true,
        Some("uniq") => !st.iter().any(|a| {
            a.starts_with('-') && !a.starts_with("--") && a.contains('c') || a == "--count"
        }),
        Some("cat") => st.len() == 1,
        Some(ps) if ps.eq_ignore_ascii_case("Sort-Object") => true,
        Some(ps) if ps.eq_ignore_ascii_case("Select-Object") => {
            st.len() == 3
                && (st[1].eq_ignore_ascii_case("-First") || st[1].eq_ignore_ascii_case("-Last"))
        }
        _ => false,
    })
    .then_some(first)
}

/// `cat F.rs | head -330`, `sed -n '1,200p' F.py | tail -50`: every stage only
/// READS or SLICES, and one of them names a source file — so the output is
/// that file's own text, and the reason a pipe routes by content ("argv[0]
/// does not name the shape") does not hold. Routed by content, an unclaimed
/// source text went to `generic`, which stride-samples: measured 2026-10-02
/// on this machine's ledger and reproduced, `cat replay.rs | head -330` lost
/// 140 of its 330 lines, one in two across the middle — the code the agent
/// asked for. Same rule as the `cat` arm: a source extension keeps the text.
pub fn pipeline_only_slices_a_source_file(argv: &[String]) -> bool {
    let mut stages = argv.split(|t| t == "|").peekable();
    stages.peek().is_some()
        && stages.all(|st| {
            let Some(first) = st.first() else {
                return false;
            };
            match first.rsplit('/').next().unwrap_or(first) {
                "cat" | "head" | "tail" | "nl" => true,
                "sed" => st.iter().any(|a| a == "-n"),
                _ => false,
            }
        })
        && argv
            .iter()
            .filter(|a| !a.starts_with('-') && a.as_str() != "|")
            .any(|a| crate::auto::is_source_code_extension(a))
}

/// Detect the blind-truncation anti-pattern `<search> | … | head/tail -N`.
///
/// Flow analysis (2026-06, n=251 transcripts / 854 Bash cmds) found that
/// **41% of all search commands** (`grep`/`rg`/`egrep`/…) were piped into
/// `head`/`tail` — slicing the result to the first/last N lines BEFORE the
/// agent has seen whether the value it's hunting lies past line N. When it
/// does, the agent burns several turns re-running blind slices. Compaction
/// cannot help with that: no compactor can hand back lines `head` never
/// emitted, so the nudge stands on its own reasoning and not on the pipe
/// branch that used to sit next to it. (That branch passed pipes through
/// untouched, and this comment said so; it routes them by content now.) What
/// the nudge does, without re-executing anything, is notice when the slice
/// returned exactly N lines (⇒ the search produced ≥N and `head` very
/// likely cut some off) and nudge the agent to re-run without the slice
/// (tokenade auto-dedupes the repeated lines, so full output stays small).
///
/// Returns `Some(n)` when the pipeline is `<search…> | … | head|tail [-n]`,
/// where `n` is the slice size (default 10 when no count flag is given).
/// Returns `None` for any other shape. Parsing is intentionally a naïve
/// `|`-split: a quoted `|` inside a regex may mis-parse, but the only
/// consequence is a missed or spurious *advisory note on stderr* — never a
/// change to stdout, exit code, or semantics.
pub fn search_head_slice_n(shell_cmd: &str) -> Option<usize> {
    const SEARCH: &[&str] = &["grep", "egrep", "fgrep", "rg", "ripgrep", "ag", "ack"];
    let stages: Vec<&str> = shell_cmd.split('|').map(str::trim).collect();
    if stages.len() < 2 {
        return None;
    }
    // First non-env-assignment token of a stage = the real program.
    let prog_of = |stage: &str| -> Option<String> {
        stage
            .split_whitespace()
            .find(|t| !is_env_assignment_token(t))
            .map(basename)
    };
    // Some earlier stage must be a search tool…
    let has_search = stages[..stages.len() - 1]
        .iter()
        .any(|s| prog_of(s).is_some_and(|p| SEARCH.contains(&p.as_str())));
    if !has_search {
        return None;
    }
    // …and the LAST stage must be head/tail.
    let last = stages[stages.len() - 1];
    let last_prog = prog_of(last)?;
    if last_prog != "head" && last_prog != "tail" {
        return None;
    }
    Some(parse_slice_count(last).unwrap_or(10))
}

/// Extract the line count from a `head`/`tail` invocation: `-N`, `-n N`,
/// `-nN`, `--lines=N`, `--lines N`. Returns `None` if none is present (the
/// caller substitutes the GNU default of 10).
pub(super) fn parse_slice_count(stage: &str) -> Option<usize> {
    let toks: Vec<&str> = stage.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if let Some(rest) = t.strip_prefix("--lines") {
            if let Some(v) = rest.strip_prefix('=') {
                return v.parse().ok();
            }
            return toks.get(i + 1).and_then(|s| s.parse().ok());
        }
        if let Some(rest) = t.strip_prefix("-n") {
            if !rest.is_empty() {
                return rest.parse().ok(); // `-n5`
            }
            return toks.get(i + 1).and_then(|s| s.parse().ok()); // `-n 5`
        }
        // Bare `-N` (head -20). Digits only after the dash.
        if let Some(num) = t.strip_prefix('-') {
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                return num.parse().ok();
            }
        }
        i += 1;
    }
    None
}

/// True when `tok` matches a verbose / debug / introspection flag.
/// Generalised over the prior enum to catch any tool's convention.
pub(super) fn is_verbose_flag(tok: &str) -> bool {
    // `-v`, `-vv`, `-vvv`, ... (short verbose).
    if tok.len() >= 2 && tok.starts_with('-') && !tok.starts_with("--") {
        let rest = &tok[1..];
        if !rest.is_empty() && rest.chars().all(|c| c == 'v') {
            return true;
        }
    }
    // Long-form flags. Match the substring AFTER stripping the `--`
    // so we catch `--no-quiet` / `--print-foo` / `--trace=on` / etc.
    let body = tok.strip_prefix("--").unwrap_or("");
    if body.is_empty() {
        return false;
    }
    // Strip `=value` suffix for matching (so `--verbose=true` works).
    let key = body.split('=').next().unwrap_or(body);
    matches!(
        key,
        "verbose"
            | "debug"
            | "trace"
            | "nocapture"
            | "no-capture"
            | "show-output"
            | "no-quiet"
            | "no-silent"
            | "message-format=short" // legacy: full key with value (kept for back-compat)
    ) || key.starts_with("show-")
        || key.starts_with("print-")
        || key.starts_with("trace-")
        || (key.starts_with("debug-") && !key.starts_with("debug-info"))
}
