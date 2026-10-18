# Per-command variant dispatch

How tokenade routes each Bash command to a per-flag optimised
compactor instead of a one-size-fits-all parser. This is the
"V1–V18" architecture established 2026-05-29.

## Why per-variant matters

A single command has many parameter shapes that look NOTHING like
each other:

```
git log                  → "commit <hash>\nAuthor: …\nDate: …\n\n    subject"
git log --oneline        → "<hash> <subject>"
git log --graph          → "* commit <hash>\n|\\\n| * commit …"
git log --stat           → "commit + Author + Date + subject + 'file | N ++--'"
git log --pretty=format: → user-defined per-line
git log -p               → commits interleaved with full diff hunks
```

A parser tuned for the default verbose form returns an EMPTY string
for `--oneline` because it looks for `commit <hash>` headers that
don't exist in that shape. That was the production bug observed on
2026-05-29.

A safety net (in `filter_command`) catches empty output and falls
back to `generic` — but that loses ALL the per-format optimisations
the per-variant parsers exist to provide. The right answer is
per-variant dispatch.

## The pattern

Each command with multiple shapes has a `<command>_dispatch(argv,
output)` function:

```rust
fn cargo_test_dispatch(argv: &[String], output: &str) -> String {
    // CHECK ORDER MATTERS — flags that rewrite the WHOLE output
    // shape come first, then shape-preserving flags.
    let json_fmt = flag_value(argv, "--message-format")
        .map(|v| v.starts_with("json"))
        .unwrap_or(false);
    if json_fmt { return cargo_test_json(output); }

    if has_flag(argv, "--list") { return cargo_test_list(output); }
    if has_flag(argv, "--no-run") { return cargo_test_no_run(output); }
    if has_any_flag(argv, &["--quiet", "-q"]) { return cargo_test_quiet(output); }
    if has_any_flag(argv, &["--nocapture", "--show-output"]) { return cargo_test_nocapture(output); }
    cargo_test_default(output)
}
```

Three helpers live in `filter.rs` for argv inspection:

| Helper                            | Asks                                        |
|-----------------------------------|---------------------------------------------|
| `has_flag(argv, "--json")`        | exact-token presence                        |
| `has_any_flag(argv, &["-q", "--quiet"])` | any-of presence                      |
| `flag_value(argv, "--format")`    | value, supporting `--flag=v` AND `--flag v` |
| `has_flag_prefix(argv, "--pretty=")` | flag family ("any --pretty=*")           |

## Dispatch ordering rules

1. **Format flags first**: `--json`, `--output-format=…`,
   `--message-format=…` REWRITE the entire output. Always check
   them before sub-mode flags.

2. **Sub-mode flags next**: `--list`, `--no-run`, `--collect-only`
   change what the command DOES (and thus the output shape) but
   keep the same encoding.

3. **Verbosity flags last**: `--quiet`, `--verbose`, `--debug`
   modulate the same shape — these are tightening / loosening
   variations of the default.

4. **Default fallback**: the original parser, unchanged. New
   variants can be added without touching the default path.

## Per-command coverage matrix (as of 2026-05-29)

| Command                    | Variants handled                                              |
|----------------------------|---------------------------------------------------------------|
| `cargo test`               | `--list`, `--no-run`, `--message-format=json[…]`, `-q`, `--nocapture`, default + failures |
| `cargo build/check/clippy` | `--message-format=json[…]`, `-q`, default                     |
| `git log`                  | `--oneline`, `--pretty=format:`/`oneline`, `--graph`, `--stat`, `--shortstat`, `--name-only`, `-p`/`--patch`, default |
| `git status`               | `--porcelain`, `--porcelain=v2`, `-s`, `-sb`/`-bs`, `-z`, default |
| `git diff`                 | `--stat`, `--shortstat`, `--name-only`, `--name-status`, `--word-diff`, `--check`, `-U0`, default |
| `pytest`                   | `--collect-only`/`--co`, `--co -q`, `-v`/`-vv`, `--tb=line`/`no`, default |
| `npm`/`pnpm`/`yarn`        | `install`/`ci`/`audit`, `test`, `run`/`build`, `ls`, `outdated`, `--json`, `--silent`, default |
| `ls`                       | `-l`/`-la`/`-lh`, `-R`, default                                |
| `grep`/`rg`                | `-c`/`--count`, `-l`/`-L`, `-C N`/`-A`/`-B`, `--json`, default |
| `find`/`fd`                | `-print0`, `-ls`, default                                      |
| `docker`/`podman`          | `ps`/`images`/`network`, `logs`, `inspect`, `--format json`, `exec`, default |
| `kubectl`/`helm`/…         | `get` (default/-o json/yaml/wide/name), `describe`, `logs`, `diff`, `apply`/`delete`, `version` |
| `make`/`gmake`             | `-s`/`--silent`, `-d`/`--debug`, `-n`/`--just-print`, default |
| `gradle`/`./gradlew`       | `-q`, `-d`/`--debug`/`-i`/`--info`, default                    |
| `mvn`/`./mvnw`             | `-q`, default (drops `[INFO]` noise)                           |
| `dotnet`                   | `--verbosity`, default                                         |
| Linters (ruff/eslint/mypy/black/…) | `--json`/`--format=json`, `--format=junit`/`github`, `--diff`, `-q`, default |
| `jq`/`yq`                  | `-r`/`--raw-output`, default (routes JSON to auto)             |
| `curl`/`wget`              | `-v`/`--verbose`, `-I`/`--head`, `-s`, default                 |
| `dig`/`nslookup`           | `+short`, `+trace`, default                                    |
| Package managers (apt/brew/pacman/gem/pip/…) | `--json`, `-q`/`--quiet`, default            |
| `systemctl status`         | dedicated parser                                               |

## Adding a new variant

1. **Capture a real fixture**. Pipe the command's output to a file
   so the hook doesn't compact it before you see it:
   ```bash
   <cmd-with-flag> > /tmp/raw.txt 2>&1
   wc -l /tmp/raw.txt && head /tmp/raw.txt
   ```
2. **Add the variant detector** to the relevant `*_dispatch`. If
   the format flag REWRITES the whole shape, check it FIRST.
3. **Implement the per-variant parser** as `<command>_<variant>`.
4. **Add a test** asserting:
   - the SIGNAL survives (failures, errors, identifiers, etc.)
   - the NOISE is stripped (framing, progress, ANSI, banners)
   - the wrong dispatch isn't picked (cross-variant test)
5. **Update this matrix** so the next reader sees the new variant.

## What the safety net catches

Even with per-variant dispatch, a parser can return empty for an
unfamiliar shape. `filter_command` has two safety nets:

- `safety_net_empty` — non-empty input → non-empty compactor result.
  If a compactor returns empty for non-empty input, fall back to
  `generic` and LOG the trigger to `~/.tokenade/debug.log` with
  `event:"safety_net_empty"` so the missing variant surfaces in
  triage.
- `safety_net_inflated` — compactor made things bigger. Fall back
  to cleaned-generic, log to `event:"safety_net_inflated"`.

Both events carry the compactor name + first command word + input
shape so a maintainer can grep:
```bash
grep '"event":"safety_net_empty"' ~/.tokenade/debug.log | jq -c '.details | {compactor, argv_first, argv_sub, input_head}'
```

That list IS the "what variant did I miss?" backlog.

## Performance

Per-variant dispatch is ~free at runtime: each detector is a few
string comparisons over `argv`, which is always <20 elements. The
heavy work is the parser itself, which runs once regardless of
dispatch.

The TEST cost is real (~5-10 unit tests per command) but pays for
itself the first time someone runs an unfamiliar variant in prod.
