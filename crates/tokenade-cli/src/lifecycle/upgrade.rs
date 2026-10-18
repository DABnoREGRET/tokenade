//! **Update, and hand control back to the right binary.**
//!
//! An `upgrade` replaces the executable that is currently running. What
//! follows must therefore go through the NEW one, not the one just replaced —
//! and what belongs to the user (the auto-compaction percentage they set by
//! hand) must survive the upgrade, otherwise it reverts to our value every
//! time.

use super::*;

/// `tokenade upgrade` — explicit auto-update check. Same logic as the
/// background spawn in `main()` (D4), but synchronous and verbose. Use
/// when the user wants a controlled upgrade right now.
///
/// Subcommands:
///   tokenade upgrade           — check + swap if new version available
///   tokenade upgrade --check   — check only, don't swap (dry-run)
///   tokenade upgrade --off     — disable auto-updates persistently
///   tokenade upgrade --on      — re-enable auto-updates
/// Lay down the local LLM proxy for every detected agent it would add a
/// feature to. Best-effort and quiet about the boring outcomes: an upgrade
/// that starts narrating installs it did not need is an upgrade nobody reads.
/// **Post-upgrade work belongs to the version just installed, not to the one
/// being replaced.**
///
/// This process IS the old binary: it downloaded the new one and put it in
/// place, but its own code is the previous one. Everything it does next —
/// writing services, probing ports, deciding an agent is not installed — it
/// does with the logic of the stale version.
///
/// Measured, and it is the defect that made a fix look like it did not take:
/// 1.1.15 fixed the `(deleted)` path in the unit, and the 1.1.14 → 1.1.15
/// upgrade rewrote it BROKEN, because 1.1.14 is what wrote it. The fix would
/// only have taken on the NEXT upgrade.
///
/// So we delegate to the freshly installed binary. If it cannot be launched,
/// we fall back to doing the work in-process — a partial post-upgrade beats no
/// post-upgrade at all.
pub(crate) fn proxy_after_upgrade() {
    let installed = tokenade_core::platform::stable_exe();
    // A single declarer for "are we the installed binary?": written by hand
    // here and in the supervisor, the comparison answered wrong in one of the
    // two directions (the kernel's ` (deleted)` suffix).
    if !tokenade_core::platform::running_the_installed_binary()
        && std::path::Path::new(&installed).exists()
    {
        // `internal_command` marks the child as OUR plumbing (and hides the
        // Windows console). Stdout is not captured: what the new binary
        // announces lands in the user's terminal, as if we were writing it —
        // which we are.
        let mut cmd = tokenade_core::process_util::internal_command(&installed);
        cmd.args(["llm-proxy", "auto-install"]);
        let ran = cmd.status().map(|s| s.success()).unwrap_or(false);
        if ran {
            return;
        }
    }
    proxy_after_upgrade_in_process()
}

fn proxy_after_upgrade_in_process() {
    use crate::llm_proxy::cli::{ensure_installed_for, AutoOutcome};
    let present =
        tokenade_core::detect::detect_all_present(tokenade_core::platform::home_dir().as_deref());
    for o in ensure_installed_for(&present) {
        match o {
            AutoOutcome::Installed { agent, listen, .. } => println!(
                "  ✓ local LLM proxy installed for {} on {listen}",
                agent.as_str()
            ),
            AutoOutcome::RolledBack { agent, why } => println!(
                "  ◐ local LLM proxy not installed for {}: {why}. Config put back.",
                agent.as_str()
            ),
            AutoOutcome::Already(_) | AutoOutcome::Declined(_) => {}
        }
    }
}

pub(crate) fn cmd_upgrade(rest: &[String]) -> i32 {
    if rest.iter().any(|a| a == "--auto-background") {
        // Hidden: the detached child spawned by spawn_background_check.
        tokenade_core::updater::run_auto_background();
        return 0;
    }
    if crate::help_routing::asks_for_help(rest) {
        println!(
            "usage: tokenade upgrade [--check | --on | --off]\n\nFetch the signed release manifest from the latest GitHub Release, verify\nthe Ed25519 signature against the embedded public key, and (unless\n--check) swap the running binary atomically with the new version.\n\nBy default a background check runs at most every 24 hours from every\ntokenade invocation; the swap happens transparently. Use this command\nto force an immediate check or to toggle the background check.\n\nOptions:\n  --check    just verify what's available; print to stdout, no swap\n  --off      disable auto-update (writes ~/.tokenade/auto-update-off)\n  --on       re-enable auto-update\n  --help     show this message\n\nEnv:\n  TOKENADE_NO_AUTO_UPDATE=1     skip the background check (one shot)\n  TOKENADE_UPDATE_ENDPOINT=...  override the manifest server (CI/staging)"
        );
        return 0;
    }
    // #779 — reject unknown flags BEFORE dispatching, so a typo like
    // `--offline` (for `--off`) doesn't silently fall through to the
    // default branch which would perform a real binary swap.
    if let Err(code) = validate_simple_bool_flags(rest, "upgrade", UPGRADE_FLAGS) {
        return code;
    }
    use tokenade_core::updater;

    if rest.iter().any(|a| a == "--off") {
        // #991 — use the canonical sentinel-path helper. Pre-fix, both
        // arms below inline-constructed the same path that
        // `updater::is_opted_out()` reads via `opt_out_sentinel()`; a
        // rename of the filename in updater.rs would silently desync
        // the CLI from the reader.
        let path = updater::opt_out_sentinel();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        match std::fs::write(&path, b"1") {
            Ok(_) => {
                println!("✓ Auto-update disabled. (sentinel at {})", path.display());
                println!("  Re-enable with: tokenade upgrade --on");
                0
            }
            Err(e) => {
                eprintln!("✗ Could not write sentinel: {e}");
                1
            }
        }
    } else if rest.iter().any(|a| a == "--on") {
        let path = updater::opt_out_sentinel();
        // #783 — pre-fix this printed "re-enabled" whether or not the
        // sentinel existed, so the user couldn't tell if their command
        // actually flipped anything. Distinguish the two cases.
        let existed = path.is_file();
        if existed {
            match std::fs::remove_file(&path) {
                Ok(_) => {
                    println!("✓ Auto-update re-enabled. Next CLI invocation will check (subject to 24h TTL).");
                    0
                }
                Err(e) => {
                    eprintln!("✗ Could not remove sentinel at {}: {e}", path.display());
                    1
                }
            }
        } else {
            println!(
                "⊙ Auto-update was already enabled (no opt-out sentinel at {}). Nothing to do.",
                path.display()
            );
            0
        }
    } else if rest.iter().any(|a| a == "--check") {
        match updater::check_only() {
            Ok(Some(info)) => {
                // #996 — human-friendly size + age. Pre-fix the size
                // printed as raw bytes (`12345678 bytes`) and the
                // manifest's released_at was deserialised but never
                // shown. Surfacing the age lets the user spot
                // very-fresh-vs-aged releases at a glance before
                // committing to the swap.
                let released_suffix = released_age_suffix(info.released_at);
                println!(
                    "Update available: {} → {} ({} for {}){released_suffix}",
                    env!("CARGO_PKG_VERSION"),
                    info.version,
                    human_bytes(info.size),
                    info.target,
                );
                println!("  URL: {}", info.url);
                println!("  Run `tokenade upgrade` (no flag) to swap.");
                0
            }
            Ok(None) => {
                println!("✓ Up to date (v{}).", env!("CARGO_PKG_VERSION"));
                0
            }
            Err(e) => {
                eprintln!("✗ Update check failed: {e}");
                1
            }
        }
    } else {
        // Default: full check + swap, synchronously and verbosely.
        match updater::check_and_swap_now() {
            Ok(Some(new_version)) => {
                println!(
                    "✓ Upgraded {} → {}.",
                    env!("CARGO_PKG_VERSION"),
                    new_version
                );

                // Refresh the auto-compaction threshold on upgrade too: a newer
                // build may map more models or carry a better default. Only when
                // a Claude settings.json already exists (don't fabricate one on a
                // blind upgrade); never clobbers a hand-set value (marker-guarded).
                if let Some(home) = tokenade_core::platform::home_dir() {
                    refresh_autocompact_after_upgrade(&home);
                }
                // The local LLM proxy, for the agents it adds a feature to.
                //
                // `upgrade` swapped the binary and nothing else, so a user who
                // lets the updater do its work never got the proxy — it is
                // offered by `install`, which an upgrader has no reason to run
                // again. The daily auto-check covers the same ground within
                // 24h; doing it here too means the upgrade a user WATCHED is
                // also the moment it happens, instead of some later invocation.
                //
                // Same function as `install` and the daily check, so the three
                // paths cannot drift: it skips agents the proxy adds nothing
                // to, honours a previous `llm-proxy uninstall`, and rolls the
                // config back if nothing answers.
                proxy_after_upgrade();
                // AFTER the repairs, not before: `proxy_after_upgrade`
                // restarts daemons left on the previous inode, so this
                // sentence can only be true once it has run. Announced up
                // front, it claimed "nothing is stale" at the exact moment
                // three services were still serving the old code (measured
                // 2026-09-15).
                print_stale_mcp_nudge(Some(
                    "No stale tokenade processes detected — the upgrade is live.",
                ));
                0
            }
            Ok(None) => {
                println!("✓ Up to date (v{}).", env!("CARGO_PKG_VERSION"));
                0
            }
            Err(e) => {
                eprintln!("✗ Upgrade failed: {e}");
                // Point at the manifest, not the bare domain: the bucket
                // root serves 404 (no index document on R2 custom domains),
                // so users who open the bare URL wrongly conclude the
                // server is down.
                eprintln!(
                    "  - Check network reachability to {}/manifest.json",
                    tokenade_core::env_compat::update_endpoint()
                );
                eprintln!(
                    "  - Check {} is writable",
                    std::env::current_exe()
                        .map(|p| p
                            .parent()
                            .unwrap_or(std::path::Path::new("?"))
                            .display()
                            .to_string())
                        .unwrap_or_else(|_| "?".into())
                );
                eprintln!("  - Set TOKENADE_UPDATE_ENDPOINT=... to override the server");
                // Self-update can be wedged by a bug in THIS (old) binary —
                // e.g. the pre-0.4.1 global 10 s deadline could never finish
                // the ~130 MB download. The package manager is the recovery
                // path that doesn't depend on the broken updater.
                eprintln!("  - Or reinstall out-of-band: npm install -g @tokenade/cli@latest");
                1
            }
        }
    }
}

/// Step 1: give every third-party MCP entry tokenade wrapped back its original
/// launch command.
pub(crate) fn unwrap_third_party_mcps(dry_run: bool) {
    let unwrap_reports = if dry_run {
        tokenade_core::mcp_wrap::unwrap_all_known_configs_dry()
    } else {
        tokenade_core::mcp_wrap::unwrap_all_known_configs()
    };
    let mut total_unwrapped = 0usize;
    for r in &unwrap_reports {
        if let Some(err) = &r.error {
            eprintln!("  {} — unwrap error: {err}", r.config_path);
            continue;
        }
        if !r.unwrapped.is_empty() {
            println!(
                "  {} {} entry(ies) in {}: {}",
                if dry_run { "would unwrap" } else { "unwrapped" },
                r.unwrapped.len(),
                r.config_path,
                r.unwrapped.join(", ")
            );
            total_unwrapped += r.unwrapped.len();
        }
    }
    if total_unwrapped > 0 {
        if dry_run {
            println!(
                "◐ {total_unwrapped} third-party MCP entry(ies) WOULD be restored \
                  (re-run without --dry-run to apply)."
            );
        } else {
            println!(
                "✓ restored {total_unwrapped} third-party MCP entry(ies) to their original form"
            );
        }
    } else {
        println!("  no wrapped MCP entries found.");
    }
}

/// Step 3: drop tokenade's OWN MCP entry from every known agent config.
pub(crate) fn remove_tokenade_self_entries(dry_run: bool) {
    let removal_reports = if dry_run {
        tokenade_core::mcp_wrap::remove_self_entry_all_known_configs_dry()
    } else {
        tokenade_core::mcp_wrap::remove_self_entry_all_known_configs()
    };
    let mut total_self_removed = 0usize;
    for r in &removal_reports {
        if let Some(err) = &r.error {
            eprintln!("  {} — self-entry removal error: {err}", r.config_path);
            continue;
        }
        if r.removed {
            let glyph = if dry_run { "◐" } else { "✓" };
            let verb = if dry_run { "would remove" } else { "removed" };
            println!(
                "  {glyph} {verb} tokenade self-entry from {}",
                r.config_path
            );
            total_self_removed += 1;
        }
    }
    if total_self_removed == 0 {
        println!("  no tokenade self-entry found in known agent configs.");
    }
}

/// `tokenade uninstall` — undo everything `tokenade install` did
/// without removing the tokenade binary itself (that's the package
/// manager's job). Restores every wrapped MCP to its original
/// shape AND drops the tokenade MCP entry from each agent config.
/// `tokenade uninstall` — undo the integration, then the data, then the binary.
///
/// ## What it removes
///
/// MCP wrappers (restored to their originals FIRST, so a crashed uninstall
/// still leaves working MCPs), hook entries in every agent's settings, the PATH
/// shim, `~/.tokenade` and `~/.cache/tokenade` **whole** — which takes the
/// stash, the ledgers, the indexes and any local-model weights with them — and
/// finally the binary.
///
/// ## What it deliberately does NOT remove
///
/// **The scaffold block tokenade wrote into each project's `CLAUDE.md`.**
/// Nothing removes it, on purpose: those files live inside the user's own
/// repositories and are almost always tracked by git. Deleting or rewriting
/// them during an uninstall would produce uncommitted changes across every
/// project the user had ever opened — an uninstall that dirties working trees
/// is worse than one that leaves a comment block behind.
///
/// The consequence is worth stating rather than discovering: after an
/// uninstall those blocks remain, and the registry that knew WHERE they are
/// (`~/.tokenade/projects-registry.txt`) has just been deleted with everything
/// else, so there is no longer any list of them. A user who wants them gone
/// removes them the same way they would any other block in a file they own:
/// the marker is `<!-- tokenade-scaffold -->`, and grepping for it finds every
/// one.
///
/// `--keep-data` preserves `~/.tokenade` and `~/.cache/tokenade` (the savings
/// ledger) while still undoing the integration.
/// `◐` for a preview, `✓` for a step that happened. One glyph rule for the
/// whole command — it was spelled out at four call sites.
pub(crate) fn glyph(dry_run: bool) -> &'static str {
    if dry_run {
        "◐"
    } else {
        "✓"
    }
}

/// Re-set the auto-compaction threshold after a swap: a newer build may map
/// more models or carry a better default. Only when a Claude settings.json
/// already exists (no config fabricated on a blind upgrade), never over a
/// hand-set value (marker-guarded), and never after `--no-autocompact`:
/// that is a choice, not a missing value, and re-setting the default here
/// undid it on every upgrade.
fn refresh_autocompact_after_upgrade(home: &std::path::Path) {
    let sp = tokenade_core::platform::claude_config_dir_for(home).join("settings.json");
    if !sp.exists() || crate::install_cli::saved_install_choices(home).no_autocompact {
        return;
    }
    // `None`: no percentage is imposed here, so the one recorded at install
    // time is carried over. A value edited by hand is recognised as such and
    // left in place.
    match crate::install::set_autocompact_window(&sp, false, None) {
        Ok(crate::install::AutoCompact::Set(t, _)) => {
            println!("  ✓ auto-compact threshold refreshed → {t} tokens")
        }
        Ok(crate::install::AutoCompact::HandSet(t)) => {
            println!("  ✓ auto-compact threshold left at your own {t} tokens")
        }
        _ => {}
    }
}

#[cfg(test)]
mod autocompact_after_upgrade_tests {
    use super::*;

    #[test]
    fn a_declined_threshold_is_not_put_back_by_an_upgrade() {
        let home = std::env::temp_dir().join(format!("tok-upg-ac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let sp = tokenade_core::platform::claude_config_dir_for(&home).join("settings.json");
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, "{}").unwrap();
        crate::install_cli::save_install_args(&home, &["--no-autocompact".to_string()]);
        refresh_autocompact_after_upgrade(&home);
        assert_eq!(std::fs::read_to_string(&sp).unwrap(), "{}");
        // Control: without the choice, the same call does set it.
        crate::install_cli::save_install_args(&home, &[]);
        refresh_autocompact_after_upgrade(&home);
        assert_ne!(std::fs::read_to_string(&sp).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
