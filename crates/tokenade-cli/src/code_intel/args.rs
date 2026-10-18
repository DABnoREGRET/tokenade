//! Argument parsing and validation shared by the code-intelligence
//! commands.
//!
//! One spelling of "this flag is unknown", one of "this takes a single
//! target", one output-mode parser — so `query`, `impact`, `map` and
//! `skeleton` reject the same input the same way.

use super::*;

// Retained as test-covered generic validators after the per-format compactor
// commands (their former production callers) were removed 2026-07; kept for the
// next single-positional command that needs them.
#[allow(dead_code)]
pub(crate) fn validate_single_positional(
    rest: &[String],
    cmd: &str,
    allowed_flags: &[&str],
    positional_name: &str,
) -> Result<String, i32> {
    validate_single_positional_with_values(rest, cmd, allowed_flags, &[], positional_name)
}

/// Like `validate_single_positional` but ALSO recognises value-flags
/// (`--key VALUE`). Each name in `allowed_value_flags` is accepted
/// AND its next arg is consumed as the value, not mistaken for the
/// positional. Used by commands like `query --context N <symbol>`
/// where the agent might put the value-flag before OR after the
/// positional. The flag's value is NOT returned; callers re-parse it
/// out of `rest` themselves (one extra walk, but keeps the helper's
/// The `[--a | --b]` suffix of a usage line, or nothing when the command takes
/// no flags. Assembled twice, identically, at the two usage sites.
fn usage_flags(allowed_bool_flags: &[&str], allowed_value_flags: &[&str]) -> String {
    let mut flag_names: Vec<&str> = allowed_bool_flags.to_vec();
    for f in allowed_value_flags {
        flag_names.push(*f);
    }
    if flag_names.is_empty() {
        String::new()
    } else {
        format!(" [{}]", flag_names.join(" | "))
    }
}

/// What one argument is, once the shared flag rules have been applied.
enum ArgKind {
    /// A flag that consumed this many slots: 1 for a boolean or a
    /// `--flag=value`, 2 for `--flag value`.
    Consumed(usize),
    /// Not a flag — the caller decides what to do with it.
    Positional,
}

/// Value flags whose value is a COUNT, and must therefore be a positive
/// integer.
///
/// Deliberately short. `--budget 0` means "no ceiling" and is legal, `-o`
/// takes a mode name, and `--context` is checked where its own cap of 10 is
/// applied. This list is for flags where "a positive integer" is the whole
/// rule.
///
/// MEASURED 2026-09-04: `tokenade semantic "dedup" --limit abc` returned the
/// DEFAULT ten hits with exit 0 — byte-identical to passing no flag — while
/// every sibling refuses a bad value (`--budget expects a token count`,
/// `--max needs a positive integer`, `--context value 'abc' is not a
/// non-negative integer`, `hooks tail: 'abc' is not a non-negative integer`).
/// The norm is written in this verb's OWN help: "An unknown flag is refused
/// rather than folded into the query, so a typo fails loudly instead of
/// quietly changing what you asked." A value that is read and dropped changes
/// what you asked just as quietly.
const COUNT_VALUE_FLAGS: &[&str] = &["--limit", "-n"];

/// Refuse a count flag whose value is not a positive integer.
fn reject_bad_count(cmd: &str, flag: &str, value: &str) -> Result<(), i32> {
    if !COUNT_VALUE_FLAGS.contains(&flag) {
        return Ok(());
    }
    match value.parse::<usize>() {
        Ok(n) if n > 0 => Ok(()),
        Ok(_) => {
            eprintln!(
                "tokenade {cmd}: {flag} needs a positive integer, got '{value}' —                  zero hits is not an answer this command can give."
            );
            Err(2)
        }
        Err(_) => {
            eprintln!("tokenade {cmd}: {flag} needs a positive integer, got '{value}'.");
            Err(2)
        }
    }
}

/// Apply the flag rules to `rest[i]`./// Apply the flag rules to `rest[i]`.
///
/// `Err(2)` means the diagnostic is already printed. That is the whole point
/// of this function: the single-positional and variadic validators carried
/// these four messages twice, so the same mistake could one day be reported in
/// two different wordings depending on which command the user typed.
fn classify_arg(
    rest: &[String],
    i: usize,
    cmd: &str,
    allowed_bool_flags: &[&str],
    allowed_value_flags: &[&str],
) -> Result<ArgKind, i32> {
    let a = &rest[i];
    if !(a.starts_with("--") || (a.starts_with('-') && a.len() > 1)) {
        return Ok(ArgKind::Positional);
    }
    // #944 — accept `--flag=value` form (conventional GNU long-option
    // syntax). Pre-fix only `--flag value` was recognised, so `--context=2`
    // was rejected as unknown. Split on the first `=` and check the flag-name
    // part against the value-flag list; consume one slot (no value-stealing
    // risk because the value is inline).
    if let Some((flag_name, inline_val)) = a.split_once('=') {
        if allowed_value_flags.contains(&flag_name) {
            reject_bad_count(cmd, flag_name, inline_val)?;
            return Ok(ArgKind::Consumed(1));
        }
    }
    if allowed_value_flags.contains(&a.as_str()) {
        // #943 — same class as #645 / get_flag: if the next arg starts with
        // `-`, the user almost certainly forgot the value (`--context --json
        // fn` → silently ate `--json` as the value, then complained "no
        // positional"). Surface the missing value instead.
        match rest.get(i + 1) {
            None => {
                eprintln!("tokenade {cmd}: {a} requires a value but reached end of args.");
                return Err(2);
            }
            Some(v) if v.starts_with('-') && v.len() > 1 => {
                eprintln!(
                    "tokenade {cmd}: {a} requires a value but next arg is the flag '{v}'. \
                     Use `{a} <value>` (and quote the value if it contains spaces)."
                );
                return Err(2);
            }
            Some(v) => reject_bad_count(cmd, a, v)?,
        }
        return Ok(ArgKind::Consumed(2));
    }
    if !allowed_bool_flags.contains(&a.as_str()) {
        eprintln!(
            "tokenade {cmd}: unknown flag '{a}'{}. Try `tokenade {cmd} --help`.",
            nearest_flag(a, allowed_bool_flags, allowed_value_flags)
        );
        return Err(2);
    }
    Ok(ArgKind::Consumed(1))
}

/// #946 — trim-reject whitespace-only positionals. `tokenade skeleton "   "`
/// (shell expansion of `$EMPTY_VAR`) passed `"   "` through to
/// `fs::read_to_string`, which surfaces a confusing bare-OS error. Caught here
/// so all 30+ callers get ONE diagnostic instead of each reproducing the class.
fn reject_blank_positional(a: &str, cmd: &str, positional_name: &str) -> Result<(), i32> {
    if a.trim().is_empty() {
        eprintln!(
            "tokenade {cmd}: <{positional_name}> must not be empty or whitespace-only \
             (got {a:?}). Did `$EMPTY_VAR` expand to nothing?"
        );
        return Err(2);
    }
    Ok(())
}

/// return type stable across all `single_positional` callers).
#[allow(dead_code)]
pub(crate) fn validate_single_positional_with_values(
    rest: &[String],
    cmd: &str,
    allowed_bool_flags: &[&str],
    allowed_value_flags: &[&str],
    positional_name: &str,
) -> Result<String, i32> {
    let mut found: Option<&String> = None;
    let mut i = 0;
    while i < rest.len() {
        match classify_arg(rest, i, cmd, allowed_bool_flags, allowed_value_flags)? {
            ArgKind::Consumed(n) => {
                i += n;
                continue;
            }
            ArgKind::Positional => {}
        }
        let a = &rest[i];
        if found.is_some() {
            eprintln!(
                "tokenade {cmd}: unexpected positional '{a}' (only one {positional_name} accepted; \
                 quote the value if it contains spaces)."
            );
            return Err(2);
        }
        reject_blank_positional(a, cmd, positional_name)?;
        found = Some(a);
        i += 1;
    }
    match found {
        Some(p) => Ok(p.clone()),
        None => {
            let flags = usage_flags(allowed_bool_flags, allowed_value_flags);
            eprintln!("usage: tokenade {cmd} <{positional_name}>{flags}");
            Err(2)
        }
    }
}

/// Variadic sibling of `validate_single_positional_with_values`:
/// accepts N positionals (in order) instead of rejecting the 2nd, so
/// exploration commands run in ONE turn — `tokenade skeleton a.rs b.rs
/// c.rs`, `tokenade query Foo Bar` — instead of one invocation (= one
/// agent turn, one cache_read re-send) per file/symbol. Identical flag
/// handling to the single-positional helper (bool + value flags,
/// `--flag=value`, missing-value diagnostics, whitespace-reject).
/// Errors (exit 2) only when NO positional was given.
/// For a command that takes NO positionals: validate the flags and refuse any
/// bare argument, naming it.
///
/// Sibling of [`collect_positionals_with_values`], not a copy: both classify
/// through `classify_arg`, so an unknown flag, a typo suggestion and a
/// `--flag=value` are reported in ONE wording across every command. The reason
/// this exists at all is that its sibling REQUIRES a positional — reaching for
/// it on a zero-positional command turns `tokenade receipt` with no arguments
/// into a usage error, which is how this was first written and caught.
/// " — did you mean `--budget`?" for a flag that is one or two keystrokes from
/// a real one, and nothing at all otherwise.
///
/// The hint existed on ONE of the two refusal paths. `validate_simple_bool_flags`
/// computed a typo distance and named the nearest flag; `classify_arg` — the path
/// every code-intel and positional-taking command goes through — printed the bare
/// token and a pointer to `--help`. Two spellings of one refusal, and the half
/// that helps was on the side fewer commands use.
fn nearest_flag(token: &str, bools: &[&str], values: &[&str]) -> String {
    bools
        .iter()
        .chain(values.iter())
        .map(|k| (*k, tokenade_core::text_distance::typo_distance(token, k)))
        .min_by_key(|(_, d)| *d)
        .filter(|(_, d)| *d <= 3)
        .map(|(k, _)| format!(" — did you mean `{k}`?"))
        .unwrap_or_default()
}

pub(crate) fn refuse_all_positionals(
    rest: &[String],
    cmd: &str,
    allowed_bool_flags: &[&str],
    allowed_value_flags: &[&str],
) -> Result<(), i32> {
    let mut i = 0;
    while i < rest.len() {
        match classify_arg(rest, i, cmd, allowed_bool_flags, allowed_value_flags)? {
            ArgKind::Consumed(n) => i += n,
            ArgKind::Positional => {
                eprintln!(
                    "tokenade {cmd}: unexpected positional argument '{}' (this command \
                     takes no positional arguments; try `tokenade {cmd} --help`).",
                    rest[i]
                );
                return Err(2);
            }
        }
    }
    Ok(())
}

pub(crate) fn collect_positionals_with_values(
    rest: &[String],
    cmd: &str,
    allowed_bool_flags: &[&str],
    allowed_value_flags: &[&str],
    positional_name: &str,
) -> Result<Vec<String>, i32> {
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match classify_arg(rest, i, cmd, allowed_bool_flags, allowed_value_flags)? {
            ArgKind::Consumed(n) => {
                i += n;
                continue;
            }
            ArgKind::Positional => {}
        }
        let a = &rest[i];
        reject_blank_positional(a, cmd, positional_name)?;
        found.push(a.clone());
        i += 1;
    }
    if found.is_empty() {
        let flags = usage_flags(allowed_bool_flags, allowed_value_flags);
        eprintln!("usage: tokenade {cmd} <{positional_name}...>{flags}");
        return Err(2);
    }
    Ok(found)
}

/// Shared `--output-mode <mode>` / `-o <mode>` / `--format <mode>`
/// selector for the exploration commands (query, impact, map,
/// skeleton, semantic). Lets an agent ask for ONLY the slice it needs
/// — a bare file list to feed `Read`, a match count, or signatures —
/// instead of the full rich output, cutting tokens/turns.
///
/// `Full` (the default when the flag is absent) keeps every command's
/// output byte-identical to before, so the flag is purely additive.
/// Not every mode is meaningful on every command; a command that has
/// no distinct rendering for a mode falls back to `Full` (documented
/// per-command).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// Current behaviour, unchanged. Default.
    Full,
    /// Only the file paths concerned, one per line, deduped + sorted.
    Paths,
    /// Only counts (matches / symbols / dependents), no detail.
    Count,
    /// Signatures / locations only, no bodies or extra explainability.
    Outline,
}

/// Parse `--output-mode <v>` / `--format <v>` / `-o <v>` (and their
/// `=`-joined forms) out of `rest`. Absent → `Full`. Unknown value →
/// `Err(2)` with a clear message. The recognised flag *names* must
/// also be registered as value-flags in each command's positional
/// validator so their value isn't mistaken for a positional.
/// Commands where `--json` is a real flag, and therefore where it can collide
/// with `-o <mode>`.
///
/// `skeleton` and `semantic` refuse `--json` as an unknown flag, and that
/// refusal says more than this one would — so they are deliberately absent,
/// and a guard drives the binary to keep this list from rotting.
const JSON_CAPABLE_COMMANDS: &[&str] = &["query", "impact", "map"];

/// Both `--json` and `-o <mode>` choose the SHAPE of the answer, and only one
/// can win.
///
/// MEASURED 2026-09-04: `tokenade map <path> --json -o paths` returned 5 728
/// bytes, byte-identical to `--json` alone, where `-o paths` alone returns
/// 1 159 — five times what the caller asked for, silently. Same on `query`.
/// The rule this repo keeps applying: an argument that would otherwise be read
/// and dropped is refused instead, because a value silently discarded changes
/// what you asked just as surely as a value misread.
fn reject_json_with_output_mode(rest: &[String], cmd: &str, flag: &str) -> Result<(), i32> {
    if !JSON_CAPABLE_COMMANDS.contains(&cmd) || !rest.iter().any(|a| a == "--json") {
        return Ok(());
    }
    eprintln!(
        "tokenade {cmd}: {flag} and --json both choose the shape of the answer — pick one. \
         The JSON document already carries the fields `{flag}` would have selected."
    );
    Err(2)
}

pub(crate) fn parse_output_mode(rest: &[String], cmd: &str) -> Result<OutputMode, i32> {
    let mut mode = OutputMode::Full;
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        let val: Option<String> = if let Some(v) = a.strip_prefix("--output-mode=") {
            Some(v.to_string())
        } else if let Some(v) = a.strip_prefix("--format=") {
            Some(v.to_string())
        } else if let Some(v) = a.strip_prefix("-o=") {
            Some(v.to_string())
        } else if a == "--output-mode" || a == "--format" || a == "-o" {
            match rest.get(i + 1) {
                Some(v) => {
                    i += 1;
                    Some(v.clone())
                }
                None => {
                    eprintln!(
                        "tokenade {cmd}: {a} requires a value (full | paths | count | outline)."
                    );
                    return Err(2);
                }
            }
        } else {
            None
        };
        if let Some(v) = val {
            reject_json_with_output_mode(rest, cmd, a)?;
            mode = match v.as_str() {
                "full" => OutputMode::Full,
                "paths" | "paths_only" | "paths-only" => OutputMode::Paths,
                "count" => OutputMode::Count,
                "outline" | "signatures" | "outline_only" => OutputMode::Outline,
                other => {
                    eprintln!(
                        "tokenade {cmd}: unknown --output-mode '{other}' \
                         (expected: full | paths | count | outline)."
                    );
                    return Err(2);
                }
            };
        }
        i += 1;
    }
    Ok(mode)
}

/// Parse `--budget N` / `--budget=N`. `Ok(None)` means unbounded (the
/// historical behaviour); a malformed value is an error, not a silent
/// fallback — a budget the caller believes is in force but isn't would be
/// worse than no budget at all.
pub(crate) fn parse_budget(rest: &[String], cmd: &str) -> Result<Option<usize>, i32> {
    let mut i = 0;
    let mut found = None;
    while i < rest.len() {
        let a = rest[i].as_str();
        let raw = if let Some(v) = a.strip_prefix("--budget=") {
            i += 1;
            Some(v.to_string())
        } else if a == "--budget" {
            let v = rest.get(i + 1).cloned();
            i += 2;
            match v {
                Some(v) => Some(v),
                None => {
                    eprintln!("tokenade {cmd}: --budget needs a token count");
                    return Err(2);
                }
            }
        } else {
            i += 1;
            None
        };
        if let Some(raw) = raw {
            match raw.parse::<usize>() {
                // `0` is the explicit "no ceiling" escape. It exists because
                // the ceiling became a DEFAULT: a caller who wants every byte
                // must be able to say so, and must not have to guess a number
                // large enough to mean "all".
                Ok(n) => found = Some(n),
                _ => {
                    eprintln!(
                        "tokenade {cmd}: --budget expects a token count (0 = no ceiling), got '{raw}'"
                    );
                    return Err(2);
                }
            }
        }
    }
    Ok(found)
}

/// What the budget could afford for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The full rendering fits.
    Full,
    /// The full rendering does not fit but a short stand-in does. The item
    /// still appears, in less detail — the caller learns it EXISTS, which a
    /// silent drop denies them.
    Degraded,
    /// Nothing fits.
    Refused,
}

impl TokenBudget {
    pub(crate) fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            used: 0,
            shown: 0,
            degraded: 0,
            dropped: 0,
        }
    }

    /// Admit `full` if it fits, else `degraded` if THAT fits, else refuse.
    ///
    /// Quality only ever decreases down the list: once an item has been
    /// degraded, no later item is rendered in full, even if a short one would
    /// have fitted. Without that rule a ranked list would read "rank 1 full,
    /// rank 2 one line, rank 3 full" — which invites the caller to read rank 3
    /// as more important than rank 2.
    pub(crate) fn admit_or_degrade(&mut self, full: &str, degraded: &str) -> Admission {
        let Some(limit) = self.limit else {
            self.shown += 1;
            return Admission::Full;
        };
        if self.dropped > 0 {
            self.dropped += 1;
            return Admission::Refused;
        }
        let est = tokenade_core::tokenizer::estimate;
        // First item is always admitted in full: a budget smaller than the top
        // result should still answer the question.
        if self.degraded == 0 && (self.shown == 0 || self.used + est(full) <= limit) {
            self.used += est(full);
            self.shown += 1;
            return Admission::Full;
        }
        let cost = est(degraded);
        if self.used + cost <= limit {
            self.used += cost;
            self.shown += 1;
            self.degraded += 1;
            return Admission::Degraded;
        }
        self.dropped += 1;
        Admission::Refused
    }

    /// One line stating what the budget cost, or nothing when it never bit.
    pub(crate) fn footer(&self) -> String {
        let Some(limit) = self.limit else {
            return String::new();
        };
        if self.dropped == 0 && self.degraded == 0 {
            return String::new();
        }
        if self.dropped == 0 {
            return format!(
                "\n[tokenade:budget] {} shown, {} reduced to a one-line summary — {} tokens \
                 spent against a {limit}-token ceiling. Raise with --budget {} for the full \
                 skeletons.",
                self.shown,
                self.degraded,
                self.used,
                self.used.max(limit).saturating_mul(2)
            );
        }
        // "{used} of {limit}" would print "222 of 120" whenever the first item
        // alone overran the budget — a nonsense reading of a real situation.
        // State the spend and the ceiling as two separate facts instead.
        format!(
            "\n[tokenade:budget] {} shown, {} withheld — {} tokens spent against a \
             {limit}-token ceiling. Raise with --budget {} to see more.",
            self.shown,
            self.dropped,
            self.used,
            self.used.max(limit).saturating_mul(2)
        )
    }
}

/// Parse a `--context N` / `-C N` value-flag out of `rest`, where N
/// is a non-negative integer capped at 10. Returns 0 when the flag
/// is absent. Shared by `cmd_query` (Phase 2 #931) and the MCP
/// `symbol_find` handler so both surfaces share the same cap +
/// error message.
pub(crate) fn parse_context_flag(rest: &[String], cmd: &str) -> Result<usize, i32> {
    let mut i = 0;
    while i < rest.len() {
        // #944 — accept `--context=N` / `-C=N` (conventional
        // equals-form) in addition to the space-separated
        // `--context N`. The validator (#944) already lets the
        // equals-form through; this parser needs to recognise
        // it too or the value gets dropped silently.
        let inline_val: Option<&str> = if let Some(v) = rest[i].strip_prefix("--context=") {
            Some(v)
        } else {
            rest[i].strip_prefix("-C=")
        };
        let raw_val: Option<String> = if let Some(v) = inline_val {
            Some(v.to_string())
        } else if rest[i] == "--context" || rest[i] == "-C" {
            match rest.get(i + 1) {
                Some(v) => Some(v.clone()),
                None => {
                    eprintln!("tokenade {cmd}: --context requires a value (0..=10).");
                    return Err(2);
                }
            }
        } else {
            None
        };
        if let Some(v) = raw_val {
            let n: usize = match v.parse() {
                Ok(n) => n,
                Err(_) => {
                    eprintln!(
                        "tokenade {cmd}: --context value '{v}' is not a non-negative integer."
                    );
                    return Err(2);
                }
            };
            if n > 10 {
                eprintln!(
                    "tokenade {cmd}: --context value {n} exceeds cap of 10 (re-reading whole files would defeat the point of symbol_find)."
                );
                return Err(2);
            }
            return Ok(n);
        }
        i += 1;
    }
    Ok(0)
}

#[cfg(test)]
mod count_flag_tests {
    use super::{collect_positionals_with_values, COUNT_VALUE_FLAGS};

    const VALUE_FLAGS: &[&str] = &[
        "--output-mode",
        "--format",
        "-o",
        "--budget",
        "--limit",
        "-n",
    ];

    fn parse(args: &[&str]) -> Result<Vec<String>, i32> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        collect_positionals_with_values(&owned, "semantic", &[], VALUE_FLAGS, "query")
    }

    /// A count that is not a count is refused, not swallowed.
    ///
    /// MEASURED 2026-09-04: `semantic "dedup" --limit abc` returned the
    /// default ten hits with exit 0, byte-identical to passing no flag at all.
    #[test]
    fn a_count_flag_refuses_a_value_that_is_not_a_count() {
        for bad in [
            vec!["dedup", "--limit", "abc"],
            vec!["dedup", "--limit", "0"],
            vec!["dedup", "--limit=abc"],
            vec!["dedup", "-n", "3.5"],
            vec!["dedup", "-n", "0"],
        ] {
            assert_eq!(parse(&bad), Err(2), "accepted: {bad:?}");
        }
        for good in [
            vec!["dedup", "--limit", "3"],
            vec!["dedup", "-n", "1"],
            vec!["dedup", "--limit=25"],
            // Not a count flag: zero is this one's way of saying "no ceiling".
            vec!["dedup", "--budget", "0"],
            // Nor is a mode name.
            vec!["dedup", "-o", "paths"],
            vec!["dedup"],
        ] {
            assert_eq!(
                parse(&good),
                Ok(vec!["dedup".to_string()]),
                "refused: {good:?}"
            );
        }
    }

    /// The list is short on purpose, and its shortness is the rule: a flag
    /// whose zero is meaningful (`--budget 0` = no ceiling) must never join it.
    #[test]
    fn the_count_flag_list_holds_only_flags_whose_zero_is_meaningless() {
        assert_eq!(COUNT_VALUE_FLAGS, &["--limit", "-n"]);
    }
}

#[cfg(test)]
mod output_shape_tests {
    use super::{parse_output_mode, OutputMode, JSON_CAPABLE_COMMANDS};

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Two ways of choosing the shape of an answer cannot both win.
    ///
    /// MEASURED 2026-09-04: `map <path> --json -o paths` returned 5 728 bytes,
    /// byte-identical to `--json` alone, where `-o paths` alone returns 1 159.
    #[test]
    fn json_and_an_output_mode_cannot_both_choose_the_shape() {
        for cmd in ["query", "impact", "map"] {
            assert_eq!(
                parse_output_mode(&a(&["--json", "-o", "paths"]), cmd),
                Err(2),
                "[{cmd}] the mode was swallowed"
            );
            // Either alone is untouched.
            assert_eq!(
                parse_output_mode(&a(&["-o", "paths"]), cmd),
                Ok(OutputMode::Paths)
            );
            assert_eq!(
                parse_output_mode(&a(&["--json"]), cmd),
                Ok(OutputMode::Full)
            );
        }
        // A command with no `--json` of its own keeps its own refusal, which
        // says more: `unknown flag '--json'`.
        for cmd in ["semantic", "skeleton"] {
            assert_eq!(
                parse_output_mode(&a(&["--json", "-o", "paths"]), cmd),
                Ok(OutputMode::Paths),
                "[{cmd}] took over a refusal that belongs to the flag validator"
            );
        }
    }

    /// The list is the load-bearing part, so it is asserted as a list.
    #[test]
    fn the_json_capable_list_is_the_three_commands_that_have_the_flag() {
        assert_eq!(JSON_CAPABLE_COMMANDS, &["query", "impact", "map"]);
    }
}
