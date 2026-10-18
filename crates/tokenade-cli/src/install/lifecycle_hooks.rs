#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};
mod claude_code;
mod peers;
pub(crate) use claude_code::*;
pub(crate) use peers::*;

/// What an install pass amounts to: nothing to do, an entry refreshed in
/// place, or a new one written.
///
/// The two booleans never travel apart and exist only to produce this verdict,
/// and both installers spelled the translation out. One name for the rule.
fn install_outcome(path: PathBuf, any_work: bool, any_refresh: bool) -> HookOutcome {
    if !any_work {
        HookOutcome::AlreadyPresent
    } else if any_refresh {
        HookOutcome::Refreshed(path)
    } else {
        HookOutcome::Installed(path)
    }
}

/// The prologue both lifecycle-hook installers share: the opt-out gate, the
/// parent directory, and the settings document.
///
/// Absent → an empty document. Unparsable → FAILURE, never a silent `{}`: this
/// file is the user's whole hooks config, and writing back over a parse error
/// would erase everything but our own entries. Thirty-seven lines, written
/// twice, with that rule stated once in each copy.
fn open_hooks_document(path: &Path, dry_run: bool) -> Result<serde_json::Value, HookOutcome> {
    if is_hook_opted_out() {
        return Err(HookOutcome::OptedOut);
    }
    if !dry_run {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Err(HookOutcome::Failed(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )));
            }
        }
    }
    let doc: serde_json::Value = match fs::read_to_string(path) {
        Err(_) => serde_json::json!({}),
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return Err(HookOutcome::Failed(format!(
                    "unparsable {} — left untouched: {e}",
                    path.display()
                )))
            }
        },
    };
    if !doc.is_object() {
        return Err(HookOutcome::Failed(format!(
            "{} is valid JSON but not an object — left untouched",
            path.display()
        )));
    }
    Ok(doc)
}

/// The `hooks` map of a settings document, created — or reset when it is there
/// but not an object — so the caller can write event entries into it.
fn hooks_object(doc: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    let hooks = doc
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    hooks.as_object_mut().unwrap()
}

/// Shared installer for Claude-style JSON lifecycle-hook files (Codex's
/// `hooks.json`, Grok's `user-settings.json`): a document whose `hooks` key maps
/// each event name to an array of `{matcher, hooks:[{type,command,_tag,…}]}`
/// groups. Idempotent and drift-correcting via the `_tag` marker.
fn install_lifecycle_hooks_json(
    exe: &str,
    debug_log_path: Option<&str>,
    path: PathBuf,
    specs: &[(&str, &str, &str, &str)],
    dry_run: bool,
) -> HookOutcome {
    let mut doc = match open_hooks_document(&path, dry_run) {
        Ok(d) => d,
        Err(o) => return o,
    };
    let hooks_obj = hooks_object(&mut doc);

    let mut any_work = false;
    let mut any_refresh = false;
    for (event, matcher, subcmd, tag) in specs.iter().copied() {
        let command = build_hook_command(exe, subcmd, debug_log_path);
        let arr = ensure_array_entry(hooks_obj, event);
        let mut stale: Option<usize> = None;
        let mut already = false;
        for (i, g) in arr.iter().enumerate() {
            let (matched, has_tag) = group_matches_tokenade_hook(g, tag, subcmd);
            if !matched {
                continue;
            }
            let cmd_ok = g
                .get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|a| a.first())
                .and_then(|hk| hk.get("command"))
                .and_then(|c| c.as_str())
                .map(|s| s == command)
                .unwrap_or(false);
            let matcher_ok = g
                .get("matcher")
                .and_then(|m| m.as_str())
                .map(|m| m == matcher)
                .unwrap_or(false);
            if cmd_ok && has_tag && matcher_ok {
                already = true;
            } else {
                stale = Some(i);
            }
            break;
        }
        if already {
            continue;
        }
        any_work = true;
        if let Some(i) = stale {
            arr.remove(i);
            any_refresh = true;
        }
        arr.push(serde_json::json!({
            "matcher": matcher,
            "hooks": [{
                "type": "command",
                "command": command,
                "_tag": tag,
                "statusMessage": "tokenade compaction",
            }],
        }));
    }
    let outcome = |p: PathBuf| install_outcome(p, any_work, any_refresh);
    if dry_run {
        return outcome(path);
    }
    if let Err(e) = write_json_atomic(&path, &serde_json::to_string_pretty(&doc).unwrap()) {
        return HookOutcome::Failed(format!("write {}: {e}", path.display()));
    }
    outcome(path)
}

/// Owner directive 2026-07-04: ONE weighted-tokens formula per language.
/// Rust = gain::weighted_tokens/weighted_from_parts; JS = webdash's
/// weightedTok. Any new inline `*5`/`/10` weighting drifts silently —
/// this scan pins the blessed sites.
#[test]
fn weighted_formula_is_centralized() {
    // The WHOLE family, derived from the directory. The comment that followed
    // already said why — "a split module the scan does not name is a module
    // the scan does not see" — and the hand-written list proved it: `gain.rs`
    // went from two children to seven, the weighting moved down into
    // `gain/weigh.rs`, and this guard counted zero.
    //
    // The CRATE is derived too, since 2026-09-13. It used to be written by
    // hand (`crates_dir().join("tokenade-core")`), and `webdash` moved into
    // `tk-agents`: the guard PANICKED on a missing file instead of saying what
    // it was looking for. `workspace_module_root` finds the crate that holds
    // the module, and checks that only one holds it.
    use tokenade_core::source_guard::workspace_module_family as family;
    let gain = family("gain");
    assert_eq!(
        gain.matches("saturating_mul(5)").count(),
        1,
        "gain.rs must hold exactly ONE x5 weighting (weighted_tokens)"
    );
    // The dashboard's JS moved out of `webdash.rs` into `webdash/dashboard.js`;
    // both are scanned so the count stays 1 wherever the weighting is written.
    let webdash = family("webdash");
    assert_eq!(
        webdash.matches("out * 5").count(),
        1,
        "the web dashboard must hold exactly ONE JS x5 weighting (weightedTok)"
    );
    // The big consumers must NOT re-derive the formula inline.
    // Every file of the dashboard CLI, not just its entry module: the
    // statusline and web-payload builders moved into submodules, and a scan
    // that still named only the parent would pass by covering nothing.
    let dash = family("dashboard_cli");
    assert_eq!(
        dash.matches("* 5").count() + dash.matches("*5").count(),
        0,
        "dashboard_cli.rs must call gain::weighted_from_parts, never inline weights. \
         A bold `**5` in a doc comment trips this too — spell the number out in \
         the prose; do NOT loosen the scan, its bluntness is the point."
    );
}

#[cfg(test)]
include!("lifecycle_hooks_tests.rs");
