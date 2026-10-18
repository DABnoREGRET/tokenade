//! **What `tokenade install` accepts, and what it REFUSES.**
//!
//! A misspelled option that passes silently installs something other than
//! what the person asked for, and they will only find out at the first
//! surprising behaviour. The tables here are therefore exhaustive, and the
//! refusal names the offending option — there are also forms that contradict
//! each other, and accepting both would let the last argument win without
//! saying anything.

use super::*;

/// #644 — Known install flags. Two sets: BOOL flags that act as
/// toggles, and VALUE flags that consume the next positional. The
/// validator walks the argv left-to-right, skipping value slots
/// after VALUE flags, and flags any other `--*` token as unknown.
pub(crate) const INSTALL_BOOL_FLAGS: &[&str] = &[
    "--help",
    "-h",
    "--yes",
    "-y",
    "--dry-run",
    "--no-hook",
    "--no-wrap-mcps",
    "--with-hook",
    "--with-claude-md",
    "--with-lsp-enforcement-hook",
    "--with-brevity-skill",
    "--no-cleanup",
    "--per-project",
    "--no-debug-log",
    "--no-autocompact",
    "--defaults",
];
// `--ref` stays accepted (and ignored): install commands copied from older
// referral pages still carry it.
pub(crate) const INSTALL_VALUE_FLAGS: &[&str] = &[
    "--profile",
    "--debug-log",
    "--only",
    "--ref",
    "--autocompact-pct",
];

pub(crate) fn validate_install_flags(rest: &[String]) -> Result<(), i32> {
    let mut skip_next = false;
    for (i, a) in rest.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if !a.starts_with("--") && !a.starts_with('-') {
            // #647 — `tokenade install` takes NO positional args (only
            // flags). A leftover positional like `tokenade install
            // claude-code` (user forgot `--only`) used to silently
            // proceed with default behaviour. Reject so the user
            // notices their forgotten flag. Hint at the most likely
            // intent based on the value: agent-shaped → suggest
            // --only, profile-shaped → suggest --profile.
            let hint = if [
                "claude-code",
                "cursor",
                "codex",
                "devin-desktop",
                "opencode",
                "copilot",
                "kilo-code",
                "openclaw",
                "droid",
                "hermes",
                "cline",
                "aider",
                "pi",
            ]
            .contains(&a.as_str())
            {
                format!(" — did you mean `--only {a}`?")
            } else if ["coding", "agents", "analysis", "benchmark"].contains(&a.as_str()) {
                format!(" — did you mean `--profile {a}`?")
            } else {
                String::new()
            };
            eprintln!(
                "tokenade install: unexpected positional argument '{a}'{hint} \
                 (try `tokenade install --help` for the flag list)."
            );
            return Err(2);
        }
        let token = a.as_str();
        if INSTALL_BOOL_FLAGS.contains(&token) {
            continue;
        }
        if INSTALL_VALUE_FLAGS.contains(&token) {
            // #645 — the next positional must be a real value, not
            // another --flag. `--debug-log --no-cleanup` used to
            // silently consume `--no-cleanup` as the path; now it
            // surfaces as a missing-value error so the user sees
            // their typo / forgotten value.
            match rest.get(i + 1) {
                Some(next) if !next.starts_with('-') => {
                    skip_next = true;
                    continue;
                }
                _ => {
                    eprintln!(
                        "tokenade install: flag '{token}' expects a value (e.g. \
                         `{token} <value>`). Try `tokenade install --help`."
                    );
                    return Err(2);
                }
            }
        }
        // Unknown — surface with a typo_distance hint over both flag sets.
        // typo_distance (not plain Levenshtein) handles long install flags
        // like `--with-lsp-enforcement-hook`: a user typing `--with` gets
        // suggested `--with-claude-md` via the prefix window instead of no
        // hint at all (full distance >3 from any long flag).
        let mut all: Vec<&'static str> = Vec::new();
        all.extend_from_slice(INSTALL_BOOL_FLAGS);
        all.extend_from_slice(INSTALL_VALUE_FLAGS);
        let near = all
            .iter()
            .map(|k| (*k, tokenade_core::text_distance::typo_distance(token, k)))
            .min_by_key(|(_, d)| *d)
            .filter(|(_, d)| *d <= 3)
            .map(|(k, _)| k);
        let suffix = match near {
            Some(n) => format!(" — did you mean `{n}`?"),
            None => String::new(),
        };
        eprintln!(
            "tokenade install: unknown flag '{token}'{suffix} \
             (try `tokenade install --help` for the full list)."
        );
        return Err(2);
    }
    Ok(())
}

/// One flag verdict, in ONE place: accept a known flag, or refuse it by name
/// with the nearest real flag when there is one.
///
/// Extracted so the commands that take a POSITIONAL as well as flags can reuse
/// the same verdict — `validate_simple_bool_flags` refuses every positional by
/// design, which is right for `uninstall`/`unwrap-mcps` and wrong for
/// `mcp-wrap install <server>`. Those had no validator at all as a result.
pub(crate) fn reject_unknown_flag(token: &str, cmd: &str, allowed: &[&str]) -> Result<(), i32> {
    if allowed.contains(&token) {
        return Ok(());
    }
    let near = allowed
        .iter()
        .map(|k| (*k, tokenade_core::text_distance::typo_distance(token, k)))
        .min_by_key(|(_, d)| *d)
        .filter(|(_, d)| *d <= 3)
        .map(|(k, _)| k);
    let suffix = match near {
        Some(n) => format!(" — did you mean `{n}`?"),
        None => String::new(),
    };
    eprintln!(
        "tokenade {cmd}: unknown flag '{token}'{suffix} \
         (try `tokenade {cmd} --help` for the flag list)."
    );
    Err(2)
}

/// Like [`validate_simple_bool_flags`], but for a subcommand that legitimately
/// takes up to `max_positionals` positional arguments. Same flag verdict, same
/// wording; only the positional rule differs.
pub(crate) fn validate_flags_and_positionals(
    rest: &[String],
    cmd: &str,
    allowed: &[&str],
    max_positionals: usize,
    positional_name: &str,
) -> Result<(), i32> {
    let mut seen = 0usize;
    for a in rest {
        if a.starts_with('-') && a.len() > 1 {
            reject_unknown_flag(a, cmd, allowed)?;
            continue;
        }
        seen += 1;
        if seen > max_positionals {
            if max_positionals == 0 {
                eprintln!(
                    "tokenade {cmd}: unexpected positional argument '{a}' \
                     (this command takes no positional arguments; try `tokenade {cmd} --help`)."
                );
            } else {
                eprintln!(
                    "tokenade {cmd}: unexpected extra argument '{a}' \
                     (this command takes at most {max_positionals} {positional_name}; \
                     try `tokenade {cmd} --help`)."
                );
            }
            return Err(2);
        }
    }
    Ok(())
}

/// #655 — validate-or-error for simple bool-only subcommands (uninstall,
/// unwrap-mcps): reject unknown flags + reject positional args, with a
/// Levenshtein typo hint. Without this, `tokenade uninstall --dryrun`
/// (typo) silently performed a real uninstall — the strip-pass took a
/// backup so users had a recovery path, but the UX is hostile and a
/// future destructive step without a backup would lose data.
/// Two flags choose the SHAPE of the answer: only one can win.
///
/// The rule this repository applies — "an argument that would otherwise be
/// read then thrown away is refused, because a silently discarded value
/// changes what you asked for as surely as a misread one" — was written in
/// `code_intel/args.rs` and applied there only. MEASURED on 1.1.9:
/// `tokenade gain --history --json` returned the aggregate, `--history`
/// dropped without a word, where `tokenade map --json -o paths` refuses. One
/// rule, two opposite behaviours in the same product. It now lives here, next
/// to `validate_simple_bool_flags` which both already call.
///
/// `note` says what each form yields, because the refusal must teach which
/// one to pick.
pub(crate) fn reject_conflicting_shape_flags(
    rest: &[String],
    cmd: &str,
    a: &str,
    b: &str,
    note: &str,
) -> Result<(), i32> {
    let has = |f: &str| rest.iter().any(|x| x == f);
    if !(has(a) && has(b)) {
        return Ok(());
    }
    eprintln!("tokenade {cmd}: {a} and {b} both choose the shape of the answer — pick one. {note}");
    Err(2)
}

pub(crate) fn validate_simple_bool_flags(
    rest: &[String],
    cmd: &str,
    allowed: &[&str],
) -> Result<(), i32> {
    for a in rest {
        if a.starts_with("--") || a.starts_with('-') {
            reject_unknown_flag(a, cmd, allowed)?;
            continue;
        }
        // Not flag-shaped — these commands take no positionals.
        eprintln!(
            "tokenade {cmd}: unexpected positional argument '{a}' \
             (this command takes no positional arguments; try `tokenade {cmd} --help`)."
        );
        return Err(2);
    }
    Ok(())
}

pub(crate) const UNINSTALL_FLAGS: &[&str] =
    &["--help", "-h", "--dry-run", "--headless", "--keep-data"];
pub(crate) const UNWRAP_MCPS_FLAGS: &[&str] = &["--help", "-h", "--dry-run"];
pub(crate) const EVICT_STALE_FLAGS: &[&str] = &["--help", "-h", "--models"];
pub(crate) const SCRUB_SECRET_FLAGS: &[&str] = &["--help", "-h", "--dry-run", "--yes"];
pub(crate) const SCRUB_CACHE_FLAGS: &[&str] = &["--help", "-h", "--dry-run"];
/// #779 — `cmd_upgrade` falls through `--check` / `--on` / `--off`
/// detection to the default branch which swaps the binary. A typo
/// like `--offline` silently triggered a real upgrade. Apply the same
/// validator the other destructive-flag commands use.
pub(crate) const UPGRADE_FLAGS: &[&str] = &["--help", "-h", "--check", "--on", "--off"];

/// #657 — scrub-secret accepts EXACTLY ONE positional (the substring
/// to redact). Before this validator: `scrub-secret foo --dryrun` ran
/// the real scrub (typo silently ignored); `scrub-secret foo bar`
/// scrubbed only "foo" and dropped "bar" (user who tried to pass a
/// space-bearing secret without quotes lost half of it). Reject both.
pub(crate) fn validate_scrub_secret_flags(rest: &[String]) -> Result<(), i32> {
    let mut seen_needle = false;
    for a in rest {
        if a.starts_with("--") || a.starts_with('-') {
            let token = a.as_str();
            if SCRUB_SECRET_FLAGS.contains(&token) {
                continue;
            }
            let near = SCRUB_SECRET_FLAGS
                .iter()
                .map(|k| (*k, tokenade_core::text_distance::typo_distance(token, k)))
                .min_by_key(|(_, d)| *d)
                .filter(|(_, d)| *d <= 3)
                .map(|(k, _)| k);
            let suffix = match near {
                Some(n) => format!(" — did you mean `{n}`?"),
                None => String::new(),
            };
            eprintln!(
                "tokenade scrub-secret: unknown flag '{token}'{suffix} \
                 (try `tokenade scrub-secret --help` for the flag list)."
            );
            return Err(2);
        }
        // Positional: only one allowed.
        if seen_needle {
            eprintln!(
                "tokenade scrub-secret: unexpected positional argument '{a}' \
                 (only one substring is supported; quote the value if it contains spaces, \
                  e.g. `tokenade scrub-secret \"foo bar\"`)."
            );
            return Err(2);
        }
        seen_needle = true;
    }
    Ok(())
}
