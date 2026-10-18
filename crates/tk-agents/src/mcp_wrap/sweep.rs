//! **Walk all known configurations, in one pass.**
//!
//! The path the user takes: `wrap`/`unwrap` on everything that was
//! discovered, with a dry run that says what WOULD CHANGE without touching
//! anything, and a backup before every real write.

use super::*;

/// Outcome of a single `wrap_all` / `unwrap_all` pass on one config
/// file. Used to render a tidy summary line during `tokenade install`.
#[derive(Debug, Default, Clone)]
pub struct WrapReport {
    pub config_path: String,
    pub wrapped: Vec<String>,
    pub skipped_already_wrapped: Vec<String>,
    pub skipped_self: Vec<String>,
    pub skipped_unrecognised_shape: Vec<String>,
    /// Servers the host app installed and manages (`EntryCommand::HostApp`).
    pub skipped_host_app: Vec<String>,
    pub unwrapped: Vec<String>,
    /// Entries whose wrapper was rewritten in place from the
    /// stale `tokenade` bare name (older wraps) to the current absolute
    /// path. Auto-healing — user no longer needs `unwrap-mcps &&
    /// install` for this specific case.
    pub refreshed_wrappers: Vec<String>,
    /// Path of the timestamped `.bak` copy written before this config was
    /// modified by a targeted `mcp-proxy install`/`uninstall` pass. `None`
    /// for the bulk install-time wrappers (which rely on the atomic
    /// tmp+rename write alone) and for read-only / no-change passes.
    pub backup: Option<String>,
    pub error: Option<String>,
}

impl WrapReport {
    pub fn total_touched(&self) -> usize {
        self.wrapped.len() + self.unwrapped.len() + self.refreshed_wrappers.len()
    }
}

/// Wrap every MCP entry in every known agent config file. Idempotent
/// and safe to call on every `tokenade install` AND on every
/// `tokenade-mcp` boot (Strat B re-scan).
///
/// `excluded_names` lets the caller exclude specific entries
/// (typically `tokenade` itself; the caller-provided wrapper name).
pub fn wrap_all_known_configs(excluded_names: &[&str]) -> Vec<WrapReport> {
    wrap_all_known_configs_except(excluded_names, &[])
}

/// [`wrap_all_known_configs`] minus the paths the caller already covered
/// another way — an agent's MCP hook, for example. A skipped path is not
/// "uncovered": it is covered by the other route, and wrapping it as well
/// would fold twice.
pub fn wrap_all_known_configs_except(
    excluded_names: &[&str],
    skip_paths: &[std::path::PathBuf],
) -> Vec<WrapReport> {
    config_paths()
        .into_iter()
        .filter(|p| !skip_paths.contains(p))
        .map(|p| wrap_one_config(&p, excluded_names))
        .collect()
}

/// Undo the wrap on NAMED paths. Called when another route
/// takes over: leaving the wrap in place would fold twice a
/// result the hook already folds.
pub fn unwrap_paths(paths: &[std::path::PathBuf]) -> Vec<WrapReport> {
    paths.iter().map(|p| unwrap_one_config(p)).collect()
}

/// Dry-run variant of [`wrap_all_known_configs`] — reports what WOULD
/// be wrapped without modifying any file. Powers `install --dry-run`'s
/// wrap-mcps preview (which previously used a magic-name exclusion
/// hack that still wrote to disk).
pub fn wrap_all_known_configs_dry(excluded_names: &[&str]) -> Vec<WrapReport> {
    wrap_all_known_configs_dry_except(excluded_names, &[])
}

/// Dry-run variant of [`wrap_all_known_configs_except`].
pub fn wrap_all_known_configs_dry_except(
    excluded_names: &[&str],
    skip_paths: &[std::path::PathBuf],
) -> Vec<WrapReport> {
    config_paths()
        .into_iter()
        .filter(|p| !skip_paths.contains(p))
        .map(|p| wrap_one_config_dry(&p, excluded_names))
        .collect()
}

/// Unwrap every MCP entry in every known agent config file. Called
/// by `tokenade uninstall` BEFORE the tokenade entry itself is
/// removed.
pub fn unwrap_all_known_configs() -> Vec<WrapReport> {
    cleanup_config_paths()
        .into_iter()
        .map(|p| unwrap_one_config(&p))
        .collect()
}

/// Dry-run variant of [`unwrap_all_known_configs`] — collects what
/// WOULD be unwrapped across all detected configs without modifying
/// any file. Powers `tokenade uninstall --dry-run`.
pub fn unwrap_all_known_configs_dry() -> Vec<WrapReport> {
    cleanup_config_paths()
        .into_iter()
        .map(|p| unwrap_one_config_dry(&p))
        .collect()
}

/// Timestamped-`.bak` copy of `path`, written BEFORE any targeted rewrite so
/// a corrupt/interrupted edit is always recoverable. The suffix is appended
/// to the FULL filename (`config.json.1720000000.bak`) so the original
/// extension survives. Returns the backup path.
pub(super) fn backup_config(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let ts = crate::platform::now_unix_secs();
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{ts}.bak"));
    let bak = std::path::PathBuf::from(name);
    std::fs::copy(path, &bak)?;
    Ok(bak)
}

/// Wrap MCP entries across every discovered agent config. `only = Some(name)`
/// restricts to that single server; `None` wraps every third-party entry.
/// Idempotent (already-wrapped entries are skipped). Writes a timestamped
/// backup before modifying each file, then the atomic locked write — a
/// user's config is never left corrupt or half-written. Powers
/// `tokenade mcp-proxy install [<server>]`.
pub fn wrap_mcp_servers(only: Option<&str>) -> Vec<WrapReport> {
    config_paths()
        .iter()
        .map(|p| process_named_config(p, only, true))
        .collect()
}

/// Unwrap MCP entries — the inverse of [`wrap_mcp_servers`], restoring each
/// entry's original command/args from the [`ORIGINAL_KEY`] snapshot. `only`
/// restricts to one server; `None` restores all. Idempotent. Powers
/// `tokenade mcp-proxy uninstall [<server>|--all]`.
pub fn unwrap_mcp_servers(only: Option<&str>) -> Vec<WrapReport> {
    cleanup_config_paths()
        .iter()
        .map(|p| process_named_config(p, only, false))
        .collect()
}

/// Read → (un)wrap the requested entries in-memory → back up → atomic write.
/// Never writes when parsing fails (a hand-edited/corrupt config is left
/// untouched and surfaced as `report.error`) or when nothing changed.
/// The TOML half of every wrap/unwrap entry point.
///
/// `Some(report)` when `path` is a TOML agent config (Codex, Grok) and this
/// pass owns it; `None` when it is JSON and the caller should continue with
/// its own parser. Factored out the moment a second caller appeared: the
/// install-time `wrap_one_config` had NO TOML branch, so `tokenade install`
/// answered `parse: expected value at line 1 column 1` on a Codex config
/// while `tokenade mcp-proxy install` handled the same file. Two paths for
/// one operation, one of them updated — the failure this module keeps
/// producing.
pub(super) fn toml_config_pass(
    path: &Path,
    only: Option<&str>,
    wrap: bool,
    dry_run: bool,
    mut report: WrapReport,
) -> Option<WrapReport> {
    if !path.extension().is_some_and(|e| e == "toml") {
        return None;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            report.error = Some(format!("read: {e}"));
            return Some(report);
        }
    };
    // Same extension, two shapes. `transform_toml_mcp_servers` returns
    // `Ok(None)` when the file carries no `mcp_servers` table, which
    // is exactly the case of a Reasonix config.toml: we then try the
    // array shape. A file that has neither stays intact.
    let transformed = match transform_toml_mcp_servers(&content, only, wrap, &mut report) {
        Ok(None) => transform_toml_plugins_array(&content, only, wrap, &mut report),
        other => other,
    };
    match transformed {
        Err(e) => report.error = Some(format!("{e} — left untouched")),
        Ok(None) => {}
        Ok(Some(new_content)) => {
            if dry_run {
                return Some(report);
            }
            // Record the backup path like the JSON pass does. It was taken
            // either way, but only the JSON branch reported it — so a Codex
            // user who wanted to undo a wrap by hand was told nothing, while
            // a Gemini user on the same run got the path. The backup is
            // exactly what you reach for when something went wrong; not
            // naming it is the difference between having one and knowing it.
            match backup_config(path) {
                Ok(b) => report.backup = Some(b.display().to_string()),
                Err(e) => {
                    report.error = Some(format!("backup: {e}"));
                    return Some(report);
                }
            }
            if let Err(e) = write_locked(path, &new_content) {
                report.error = Some(format!("write: {e}"));
            }
        }
    }
    Some(report)
}

/// `process_named_config` has no dry-run mode; naming the constant beats a
/// bare `false` at the call site.
pub(super) fn dry_run_never() -> bool {
    false
}

pub(super) fn process_named_config(path: &Path, only: Option<&str>, wrap: bool) -> WrapReport {
    let mut report = WrapReport {
        config_path: path.display().to_string(),
        ..Default::default()
    };
    // TOML agents (Codex, Grok) take the line-surgical path: same
    // report, same backup, different parser. Until this existed the two
    // were simply absent from `mcp-proxy install` — discovery listed no
    // site for them, so their third-party MCP servers were never wrapped.
    if let Some(r) = toml_config_pass(path, only, wrap, dry_run_never(), report.clone()) {
        return r;
    }
    let mut root: Value = match read_config_root(path, " — left untouched") {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    if wrap {
        wrap_in_root_selecting(
            &mut root,
            &|name| match only {
                Some(want) if name != want => Pick::Ignore,
                _ if name == "tokenade" => Pick::NotOurs,
                _ => Pick::Take,
            },
            &mut report,
        );
    } else {
        unwrap_in_root_selecting(
            &mut root,
            &|name| only.is_none_or(|want| name == want),
            &mut report,
        );
    }
    // A refreshed wrapper (new tokenade path, the app's name added) is a
    // change too; counting only fresh wraps left both in memory, unwritten.
    let changed = if wrap {
        !report.wrapped.is_empty() || !report.refreshed_wrappers.is_empty()
    } else {
        !report.unwrapped.is_empty()
    };
    if !changed {
        return report;
    }
    // Serialise FIRST — if this somehow fails we never touched the file.
    let pretty = match serde_json::to_string_pretty(&root) {
        Ok(p) => p,
        Err(e) => {
            report.error = Some(format!("serialise: {e}"));
            return report;
        }
    };
    match backup_config(path) {
        Ok(b) => report.backup = Some(b.display().to_string()),
        Err(e) => {
            // No backup → refuse to write. Safety over completeness.
            report.error = Some(format!("backup failed, config left untouched: {e}"));
            return report;
        }
    }
    if let Err(e) = write_locked(path, &pretty) {
        report.error = Some(format!("write: {e}"));
    }
    report
}

/// Every server map a JSON agent config can carry:
///
/// - top-level `mcpServers` — most agents, and Claude Code's USER scope;
/// - top-level `mcp` — OpenCode / KiloCode;
/// - `projects[*].mcpServers` — `~/.claude.json`, which is where
///   `claude mcp add` puts a server by DEFAULT (the `local` scope).
///
/// That last one is the point. `purge_json_config` has always walked
/// `projects[*]`; the wrap/unwrap paths only ever looked at the top level,
/// so `tokenade mcp-proxy install` silently skipped the most common MCP
/// location of the flagship agent. Routing all four call sites through one
/// walker is what keeps them from drifting apart again.
pub(super) fn for_each_server_map(
    root: &mut Value,
    mut f: impl FnMut(&mut serde_json::Map<String, Value>),
) {
    for key in ["mcpServers", "mcp"] {
        if let Some(m) = root.get_mut(key).and_then(|o| o.as_object_mut()) {
            f(m);
        }
    }
    if let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) {
        for proj in projects.values_mut() {
            if let Some(m) = proj.get_mut("mcpServers").and_then(|o| o.as_object_mut()) {
                f(m);
            }
        }
    }
}

/// **Which entry this pass looks at.**
///
/// Two wrappers existed — one targeting ONE named server, one
/// excluding a list — with the same forty-line body except for the selection,
/// hence the same fix to make twice. And it is not the same
/// "no": an entry outside the filter is not looked at at all, an excluded
/// entry IS one of ours and the report must say so. Both meanings are
/// variants here, and the body is written once.
pub(super) enum Pick {
    /// To process.
    Take,
    /// Outside the scope of this pass: nothing to report.
    Ignore,
    /// Ours, or explicitly excluded: counted in `skipped_self`.
    NotOurs,
}

pub(super) fn wrap_in_root_selecting(
    root: &mut Value,
    pick: &dyn Fn(&str) -> Pick,
    report: &mut WrapReport,
) {
    let agent = super::discover::agent_for_config_path(Path::new(&report.config_path));
    for_each_server_map(root, |servers| {
        for (name, entry) in servers.iter_mut() {
            match pick(name.as_str()) {
                Pick::Ignore => continue,
                Pick::NotOurs => {
                    report.skipped_self.push(name.clone());
                    continue;
                }
                Pick::Take => {}
            }
            let Some(obj) = entry.as_object_mut() else {
                report.skipped_unrecognised_shape.push(name.clone());
                continue;
            };
            if obj.contains_key(ORIGINAL_KEY) {
                // Auto-heal stale wraps. Older wraps used
                // bare "tokenade" instead of the absolute current_exe
                // path — fine for shell-launched agents but breaks
                // GUI clients that don't inherit PATH. Detect and
                // rewrite in place to the current absolute path,
                // without touching `original` (so unwrap-mcps still
                // restores the user's original command faithfully).
                // A wrap from before the app was named gets its name too.
                let unnamed =
                    agent.is_some() && obj.get("env").and_then(|e| e.get(AGENT_ENV)).is_none();
                if unnamed {
                    put_agent_env(obj, agent.as_deref());
                }
                if refresh_stale_wrapper(obj) || unnamed {
                    report.refreshed_wrappers.push(name.clone());
                } else {
                    report.skipped_already_wrapped.push(name.clone());
                }
                continue;
            }
            match classify_command(
                obj.get("command").and_then(|c| c.as_str()),
                obj.get("args")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str()),
            ) {
                EntryCommand::RoutedByHand => {
                    report.skipped_already_wrapped.push(name.clone());
                    continue;
                }
                EntryCommand::OurOwn => {
                    report.skipped_self.push(name.clone());
                    continue;
                }
                EntryCommand::HostApp => {
                    report.skipped_host_app.push(name.clone());
                    continue;
                }
                EntryCommand::ThirdParty => {}
            }
            match wrap_entry(obj, agent.as_deref()) {
                Ok(()) => report.wrapped.push(name.clone()),
                Err(()) => report.skipped_unrecognised_shape.push(name.clone()),
            }
        }
    });
}

/// Unwrapping, written once. Here a "no" has only one meaning — we do not
/// look at this entry — so a boolean suffices where wrapping
/// needed three cases.
pub(super) fn unwrap_in_root_selecting(
    root: &mut Value,
    in_scope: &dyn Fn(&str) -> bool,
    report: &mut WrapReport,
) {
    for_each_server_map(root, |servers| {
        for (name, entry) in servers.iter_mut() {
            if !in_scope(name.as_str()) {
                continue;
            }
            let Some(obj) = entry.as_object_mut() else {
                continue;
            };
            if obj.contains_key(ORIGINAL_KEY) && unwrap_entry(obj).is_ok() {
                report.unwrapped.push(name.clone());
            }
        }
    });
}

/// Read, wrap (in-memory), serialise, write back. File-locked via
/// fs2 to survive concurrent `tokenade install` + `tokenade-mcp`
/// rescans hitting the same config.
pub fn wrap_one_config(path: &Path, excluded_names: &[&str]) -> WrapReport {
    wrap_one_config_inner(path, excluded_names, false)
}

/// Dry-run companion: same detection / report as [`wrap_one_config`]
/// but never serialises or writes. Used by `install --dry-run`.
pub fn wrap_one_config_dry(path: &Path, excluded_names: &[&str]) -> WrapReport {
    wrap_one_config_inner(path, excluded_names, true)
}

pub(super) fn wrap_one_config_inner(
    path: &Path,
    excluded_names: &[&str],
    dry_run: bool,
) -> WrapReport {
    let mut report = WrapReport {
        config_path: path.display().to_string(),
        ..Default::default()
    };
    if let Some(r) = toml_config_pass(path, None, true, dry_run, report.clone()) {
        return r;
    }
    let mut root: Value = match read_config_root(path, "") {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    wrap_in_root_selecting(
        &mut root,
        &|name| {
            if excluded_names.contains(&name) || name == "tokenade" {
                Pick::NotOurs
            } else {
                Pick::Take
            }
        },
        &mut report,
    );
    // Also flush on refreshes (stale-wrapper auto-heal), not just
    // fresh wraps. dry_run still short-circuits — we only PREVIEW.
    let needs_write = !report.wrapped.is_empty() || !report.refreshed_wrappers.is_empty();
    if !needs_write || dry_run {
        return report;
    }
    let pretty = match serde_json::to_string_pretty(&root) {
        Ok(p) => p,
        Err(e) => {
            report.error = Some(format!("serialise: {e}"));
            return report;
        }
    };
    if let Err(e) = write_locked(path, &pretty) {
        report.error = Some(format!("write: {e}"));
    }
    report
}

pub fn unwrap_one_config(path: &Path) -> WrapReport {
    unwrap_one_config_inner(path, false)
}

/// Dry-run variant: inspect the config, populate the report with
/// what WOULD be unwrapped, but skip the disk write. Used by
/// `tokenade uninstall --dry-run` to preview without changing state.
pub fn unwrap_one_config_dry(path: &Path) -> WrapReport {
    unwrap_one_config_inner(path, true)
}

pub(super) fn unwrap_one_config_inner(path: &Path, dry_run: bool) -> WrapReport {
    let mut report = WrapReport {
        config_path: path.display().to_string(),
        ..Default::default()
    };
    if let Some(r) = toml_config_pass(path, None, false, dry_run, report.clone()) {
        return r;
    }
    let mut root: Value = match read_config_root(path, "") {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    unwrap_in_root_selecting(&mut root, &|_| true, &mut report);
    if report.unwrapped.is_empty() || dry_run {
        // Dry-run: short-circuit BEFORE serialise+write. The report
        // already lists what would be unwrapped — caller prints it.
        return report;
    }
    let pretty = match serde_json::to_string_pretty(&root) {
        Ok(p) => p,
        Err(e) => {
            report.error = Some(format!("serialise: {e}"));
            return report;
        }
    };
    if let Err(e) = write_locked(path, &pretty) {
        report.error = Some(format!("write: {e}"));
    }
    report
}
