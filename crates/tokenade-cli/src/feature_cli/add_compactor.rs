//! `tokenade add-compactor` — scaffolding d'un compacteur par commande.
#![allow(unused_imports)]
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;

/// `tokenade turn-summary` — extractive summary of a conversation turn read on
/// stdin (or from a file), folded to a sentence budget.
/// Full TOML schema reference for `tokenade add-compactor --help`. Every
/// operation a user compactor can declare, what it does, and a worked example.
/// The uncovered-command nudge points agents here.
const ADD_COMPACTOR_HELP: &str = "\
tokenade add-compactor — install a custom command compactor

USAGE:
  tokenade add-compactor --file <filter.toml>   install + validate a filter file
  tokenade add-compactor < filter.toml          read the TOML from stdin instead
  tokenade add-compactor --from <sample.txt> --command \"<cmd line>\"
                                                scaffold a starter filter from a
                                                REAL captured output (prints TOML)
  tokenade add-compactor --list                 list installed user compactors
  tokenade add-compactor --help                 show this message

A compactor is a declarative TOML filter (no code) that tightens the output of a
command tokenade only compacts generically. It is validated (every regex is
compiled and every [[tests]] case is run) before install, then written to
~/.tokenade/filters/<name>.toml and applied to matching commands from the next
run on. Its savings appear in `tokenade gain` / the dashboard under its name.

FIELDS:
  name              (required) report label; also the filename.
  match_command     (required) regex tested against the whole command line
                    (e.g. \"^mytool\\\\s+status\"). When SEVERAL filters match a
                    command, the one covering the LONGEST span wins (a specific
                    pattern beats a broad one). Reusable templates expand here:
                    {{runner}} → (npx|bunx|pnpm dlx|yarn dlx),
                    {{py-runner}} → (poetry run|uv run|python -m). An unknown
                    {{token}} is a hard error.
  priority          (optional, default 0) tie-break ONLY between equally specific
                    (equal-span) matches; higher wins. Never overrides a longer
                    match.
  draft             (optional) set by `--from`; an install is REFUSED until you
                    review the draft and remove this line.

OPERATIONS (applied in this order; set at least one):
  match_output      list of { pattern, summary }: if `pattern` (regex) matches
                    anywhere in the output, the ENTIRE output is replaced by the
                    one-line `summary` (e.g. a clean build → \"build: ok\").
  strip_ansi        true|false: remove ANSI colour escape sequences.
  replace           list of { from, to }: regex substitution (`to` may use \\1…).
  strip_lines_matching   list of regexes: drop every line matching any of them.
  on_empty          string: if everything above stripped the output empty, emit
                    this single confirmation line instead.
  head_lines        keep the first N lines …
  tail_lines        … and the last M lines (so the final outcome line survives);
                    omitted middle becomes a `… [K lines omitted]` marker.
  max_lines         hard cap on total lines kept (head), with a `… [K more]` tag.

SELF-TESTS (optional, recommended — they gate the install):
  [[tests]]
  input = \"…raw output…\"
  expect_contains = \"…\"     # OR  expect_equals = \"…\"

EXAMPLE (save it, then `tokenade add-compactor --file mytool.toml`):
  name = \"mytool-status\"
  match_command = \"^mytool\\\\s+status\"
  strip_ansi = true
  strip_lines_matching = [\"^DEBUG\", \"^\\\\s*$\"]
  match_output = [{ pattern = \"All systems go\", summary = \"mytool: ok\" }]
  head_lines = 40
  tail_lines = 5
  on_empty = \"mytool: ok\"
  [[tests]]
  input = \"DEBUG boot\\nWARN disk 90%\\nAll systems go\"
  expect_contains = \"mytool: ok\"
";

pub(crate) fn cmd_add_compactor(rest: &[String]) -> i32 {
    use std::io::Read;

    if crate::help_routing::asks_for_help(rest) {
        print!("{ADD_COMPACTOR_HELP}");
        return 0;
    }

    // A bare argument was SILENTLY DROPPED here: `add-filter foo.toml` fell
    // through to the stdin branch, read an empty stdin, and reported a TOML
    // parse error about a document the user never wrote. Name the mistake and
    // the flag that was meant.
    if let Err(c) = crate::code_intel::refuse_all_positionals(
        rest,
        "add-compactor",
        &["--list", "--help", "-h"],
        &["--file", "--from", "--from-transcript", "--command"],
    ) {
        return c;
    }

    if rest.iter().any(|a| a == "--list") {
        let filters = tokenade_core::user_filters::load_all();
        if filters.is_empty() {
            println!(
                "No user compactors installed. Add one with \
                 `tokenade add-compactor --file <f>.toml` (see --help)."
            );
            return 0;
        }
        println!("Installed user compactors ({}):", filters.len());
        for f in filters {
            println!("  {:<24} match: {}", f.name, f.match_re.as_str());
        }
        return 0;
    }

    // Scaffold mode: GENERATE a starter filter from a real captured sample of a
    // command's output. Prints the draft TOML to stdout (redirect it to a file,
    // review, remove `draft = true`, then install with --file). The match
    // pattern is derived from --command, never from the sample bytes.
    if let Some(path) = get_flag(rest, "--from").or_else(|| get_flag(rest, "--from-transcript")) {
        let Some(command) = get_flag(rest, "--command") else {
            eprintln!(
                "tokenade add-compactor --from <sample>: also pass --command \"<the command \
                 line>\" so the match pattern can be derived (e.g. --command \"cargo test\")"
            );
            return 2;
        };
        let sample = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade add-compactor: cannot read sample {path}: {e}");
                return 2;
            }
        };
        match tokenade_core::user_filters::draft_filter_toml(command, &sample) {
            Ok(toml_src) => {
                print!("{toml_src}");
                eprintln!(
                    "\n# ^ Draft written to stdout. Save it, review it, delete `draft = true`, \
                     then:\n#   tokenade add-compactor --file <saved>.toml"
                );
                return 0;
            }
            Err(e) => {
                eprintln!("tokenade add-compactor --from: {e}");
                return 2;
            }
        }
    }

    // Source: --file <path>, else stdin.
    let src = if let Some(path) = get_flag(rest, "--file") {
        match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade add-compactor: cannot read {path}: {e}");
                return 2;
            }
        }
    } else {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprintln!(
                "usage: tokenade add-compactor --file <f.toml>  (or pipe TOML on \
                 stdin; `--help` for the format)"
            );
            return 2;
        }
        let mut s = String::new();
        if crate::bounded_input::read_stdin_lossy(&mut s).is_err() {
            eprintln!("tokenade add-compactor: failed to read stdin");
            return 2;
        }
        // An EMPTY stdin is not an invalid filter, and saying "missing field
        // `name`" about a document nobody wrote sends the reader looking for a
        // typo in a file that does not exist. This is the non-tty case: a hook,
        // a script, or `< /dev/null`, where the usage line above never fires.
        if s.trim().is_empty() {
            eprintln!(
                "tokenade add-compactor: nothing on stdin. Pass `--file <f.toml>`, or pipe \
                 the filter in (`cat f.toml | tokenade add-compactor`); `--help` for the format."
            );
            return 2;
        }
        s
    };

    // Validate: compiles every regex + runs the embedded [[tests]].
    let compiled = match tokenade_core::user_filters::parse_and_validate(&src) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tokenade add-compactor: invalid filter:\n{e}");
            return 2;
        }
    };

    // A machine-generated draft is not ready to run: refuse until the author has
    // reviewed it and removed the `draft = true` marker.
    if compiled.draft {
        eprintln!(
            "tokenade add-compactor: this file is a DRAFT (draft = true). Review the pattern \
             and the test's `expect_equals`, delete the `draft = true` line, then re-run."
        );
        return 2;
    }

    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("tokenade add-compactor: cannot resolve home directory");
        return 2;
    };
    let dir = home.join(".tokenade").join("filters");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "tokenade add-compactor: cannot create {}: {e}",
            dir.display()
        );
        return 2;
    }
    let safe: String = compiled
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let safe = if safe.is_empty() {
        "filter".to_string()
    } else {
        safe
    };
    let dest = dir.join(format!("{safe}.toml"));
    if let Err(e) = std::fs::write(&dest, &src) {
        eprintln!(
            "tokenade add-compactor: cannot write {}: {e}",
            dest.display()
        );
        return 2;
    }
    println!(
        "Installed compactor '{}' → {}",
        compiled.name,
        dest.display()
    );
    println!(
        "Applies to commands matching: {}. List all with \
         `tokenade add-compactor --list`.",
        compiled.match_re.as_str()
    );
    0
}
