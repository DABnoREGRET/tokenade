//! `--help` routing for the top-level CLI.
//!
//! Every command that answers `--help` used to do it with its own
//! `if cmd == "x" || cmd == "x_y" { … }` inside `run()` — twenty-six copies of
//! two shapes, 200 lines, and a new command was one forgotten `if` away from
//! silently falling through to the generic command list. The shapes are the
//! tables below; adding a command is one row.
//!
//! Commands that forward `--help` to an underlying tool never reach here —
//! `cmd_forwards_help` filters them out first.

/// `--help` / `-h` ANYWHERE in the arguments.
///
/// For commands whose arguments are all tokenade's own, so a flag in any
/// position is addressed to us.
pub(crate) fn asks_for_help(rest: &[String]) -> bool {
    rest.iter().any(|a| a == "--help" || a == "-h")
}

/// `--help` / `-h` only as the FIRST argument.
///
/// For commands that WRAP a command line (`wrap`, `proxy`, `raw`, `shellwrap`,
/// `filter`): a later `--help` belongs to the wrapped command, not to us.
/// `tokenade wrap 'cargo build --help'` must run cargo's help.
///
/// Both spellings existed hand-written at 28 call sites — 22 of one, 6 of the
/// other — with nothing naming the difference. Someone adding a pass-through
/// command had no signal which to copy, and someone "harmonising" them would
/// have swallowed the wrapped command's own help.
pub(crate) fn asks_for_help_leading(rest: &[String]) -> bool {
    rest.first().is_some_and(|s| s == "--help" || s == "-h")
}

/// Commands whose `--help` prints a dedicated text and exits 0.
static HELP_TEXTS: &[(&[&str], fn() -> &'static str)] = &[
    // These three were left open on purpose: their handlers own a usage that
    // is NARROWER than the global one-liner (semantic's was a single line with
    // no `-o`, no `--budget`), so routing `--help` to them would have made the
    // help worse. A real entry is the fix; every flag below was executed
    // before being written down.
    (&["semantic", "ask", "sem"], crate::semantic_help_text),
    (&["disclose"], crate::disclose_help_text),
    (&["mistake", "mistakes"], crate::mistake_help_text),
    (&["map"], crate::map_help_text),
    (&["skeleton"], crate::skeleton_help_text),
    (&["query", "find"], crate::query_help_text),
    (&["impact"], crate::impact_help_text),
    // The four aliases were absent and the two primaries present, so
    // `tokenade xref --help` — an alias `tokenade --help` documents on the
    // same run — fell all the way through to the 100-line catalogue.
    (
        &["expand-ref", "expand_ref", "xref"],
        crate::expand_ref_help_text,
    ),
    (
        &["expand-fold", "expand_fold", "xfold"],
        crate::expand_fold_help_text,
    ),
    (&["install"], crate::install_help_text),
    (&["dashboard", "dash", "stats"], crate::dashboard_help_text),
    (
        &["evict-stale", "evict_stale"],
        crate::evict_stale_help_text,
    ),
    (&["uninstall"], crate::uninstall_help_text),
    (
        &["unwrap-mcps", "unwrap_mcps"],
        crate::unwrap_mcps_help_text,
    ),
    (
        &["healthcheck", "health", "doctor"],
        crate::healthcheck_help_text,
    ),
    (
        &[
            "audit-claude-md",
            "audit-claudemd",
            "claude-md-audit",
            "claudemd",
        ],
        crate::audit_claude_md_help_text,
    ),
    (&["gain"], crate::gain_help_text),
    (&["watch"], crate::watch_help_text),
    (&["statusline"], crate::statusline_help_text),
    (&["discover"], crate::discover_help_text),
    (&["measure"], crate::measure_help_text),
    (
        &["semantic-status", "semantic_status"],
        crate::semantic_status_help_text,
    ),
    (&["detect"], crate::detect_help_text),
    (&["read"], crate::auto_help_text),
    (
        &[
            "incompatibilities",
            "incompat",
            "detect-competitors",
            "competitors",
            "scan",
        ],
        crate::competitors_help_text,
    ),
    (&["style"], crate::style_help_text),
    (
        &["mcp-wrap", "mcp_wrap", "mcp-proxy", "mcp_proxy"],
        crate::mcp_proxy::mcp_proxy_help_text,
    ),
];

/// Commands that render their own `--help` (they parse `rest` for flags the
/// generic text could not describe), so routing to the handler is the help.
static HELP_HANDLERS: &[(&[&str], fn(&[String]) -> i32)] = &[
    // `trust-filters` writes its own usage and it was unreachable: the generic
    // router answered "this command has no dedicated help yet" while the
    // handler held a paragraph explaining that a cloned repository must not get
    // to rewrite your agent's output until you trust it. Exactly the text
    // somebody typing `--help` on THIS command needs.
    (
        &["trust-filters", "trust_filters"],
        crate::cmd_trust_filters,
    ),
    (&["scrub-secret", "scrub_secret"], crate::cmd_scrub_secret),
    (&["add-compactor", "add-filter"], crate::cmd_add_compactor),
    (&["scrub-cache", "scrub_cache"], crate::cmd_scrub_cache),
    (&["redact"], crate::cmd_redact),
    (
        &["list-commands", "list_commands"],
        crate::cmd_list_commands,
    ),
    (&["ensure-hooks", "ensure_hooks"], crate::cmd_ensure_hooks),
    (&["hooks"], crate::cmd_hooks),
    (
        &["llm-proxy", "llm_proxy"],
        crate::llm_proxy::cli::cmd_llm_proxy,
    ),
    (
        &["upgrade", "self-update", "selfupdate"],
        crate::cmd_upgrade,
    ),
];

/// Answer `tokenade <cmd> --help`. Returns the process exit code.
pub(crate) fn respond(cmd: &str, rest: &[String]) -> i32 {
    if let Some((_, handler)) = HELP_HANDLERS.iter().find(|(names, _)| names.contains(&cmd)) {
        return handler(rest);
    }
    if let Some((_, text)) = HELP_TEXTS.iter().find(|(names, _)| names.contains(&cmd)) {
        println!("{}", text());
        return 0;
    }
    if !crate::KNOWN_COMMANDS.contains(&cmd) {
        if crate::warn_format_not_a_command(cmd) {
            return 2;
        }
        let suffix = match crate::nearest_command(cmd, 3) {
            Some(near) => format!(" — did you mean `{near}`?"),
            None => String::new(),
        };
        eprintln!(
            "tokenade: unknown command '{cmd}'{suffix} \
             (try `tokenade help` for the full command list)."
        );
        return 2;
    }
    // L17 follow-up — when a KNOWN command has no dedicated
    // --help text yet, prefer a focused one-line description
    // extracted from the top-level help block over dumping the
    // entire 60-line command catalogue. Users typing
    // `tokenade csv --help` want csv-specific info; the full
    // dump buries it. Falls back to crate::print_help() only when no
    // matching line exists (e.g. the cmd is registered via an
    // alias and its primary name isn't in the help block).
    if let Some(line) = crate::oneline_help_for(cmd) {
        println!("usage: tokenade {}", line.trim_start());
        println!();
        println!("(this command has no dedicated help yet; run `tokenade help` for the full command list)");
        return 0;
    }
    crate::print_help();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_answers_help_exactly_one_way() {
        // The two tables are searched in order, so a name in both would make
        // the printed text unreachable. Disjointness is the invariant.
        let mut seen = std::collections::HashSet::new();
        for (names, _) in HELP_HANDLERS {
            for n in *names {
                assert!(seen.insert(*n), "{n} is routed twice");
            }
        }
        for (names, _) in HELP_TEXTS {
            for n in *names {
                assert!(seen.insert(*n), "{n} has both a handler and a text");
            }
        }
        assert!(!seen.is_empty());
    }

    /// A row that names one spelling of a command and not its siblings sends
    /// the aliases somewhere else entirely.
    ///
    /// Measured on 1.1.9: four rows had drifted. `dash`, `stats` and `find`
    /// fell back to the one-line summary, and `xref`/`xfold` — aliases that
    /// `tokenade --help` documents on the same run — fell all the way to the
    /// 100-line catalogue. The roster is read from the dispatch, so an alias
    /// added tomorrow either lands in the row or fails here.
    #[test]
    fn a_routed_command_routes_all_its_aliases() {
        let routed: std::collections::HashSet<&str> = HELP_TEXTS
            .iter()
            .map(|(names, _)| *names)
            .chain(HELP_HANDLERS.iter().map(|(names, _)| *names))
            .flatten()
            .copied()
            .collect();
        let src = include_str!("main.rs");
        let mut groups = 0usize;
        let mut split: Vec<String> = Vec::new();
        for line in src.lines() {
            let t = line.trim();
            let Some((pats, body)) = t.split_once("=>") else {
                continue;
            };
            if !pats.trim().starts_with('"') || !body.contains("cmd_") {
                continue;
            }
            let names: Vec<&str> = pats
                .split('|')
                .filter_map(|p| p.trim().strip_prefix('"').and_then(|p| p.strip_suffix('"')))
                .collect();
            if names.len() < 2 {
                continue;
            }
            groups += 1;
            let inside: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| routed.contains(n))
                .collect();
            if !inside.is_empty() && inside.len() != names.len() {
                let missing: Vec<&str> = names
                    .iter()
                    .copied()
                    .filter(|n| !routed.contains(n))
                    .collect();
                split.push(format!("{body} routed={inside:?} but NOT {missing:?}"));
            }
        }
        assert!(
            groups >= 20,
            "the dispatch parser found only {groups} alias groups — it stopped \
             matching the source it reads"
        );
        assert!(
            split.is_empty(),
            "these commands route SOME of their aliases to a dedicated help \
             and leave the rest to the generic fallback:\n  {}",
            split.join("\n  ")
        );
    }

    /// `expand-fold` shared a row with `expand-ref`, so its `--help` printed
    /// the other command's usage (hashes, `--prompt`, `--raw`), none of
    /// which `expand-fold` accepts.
    #[test]
    fn expand_fold_has_its_own_help() {
        for name in ["expand-fold", "expand_fold", "xfold"] {
            let (_, text) = HELP_TEXTS
                .iter()
                .find(|(names, _)| names.contains(&name))
                .unwrap_or_else(|| panic!("{name} has no help row"));
            let t = text();
            assert!(
                t.starts_with("tokenade expand-fold") && t.contains("--max N"),
                "{name} --help does not describe expand-fold: {t}"
            );
        }
    }

    #[test]
    fn every_routed_name_is_a_known_command() {
        // A typo in a table row is a command whose --help silently degrades to
        // the generic list; KNOWN_COMMANDS is the spelling authority.
        let routed = HELP_TEXTS
            .iter()
            .map(|(names, _)| *names)
            .chain(HELP_HANDLERS.iter().map(|(names, _)| *names));
        for names in routed {
            for n in names {
                assert!(
                    crate::KNOWN_COMMANDS.contains(n),
                    "{n} is routed for --help but is not a known command"
                );
            }
        }
    }
}
