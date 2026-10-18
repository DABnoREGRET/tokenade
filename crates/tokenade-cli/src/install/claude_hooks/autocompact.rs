//! **The auto-compaction threshold, which BELONGS to the user.**
//!
//! We propose a value; if they change it by hand, it is theirs. An ownership
//! mark that only records a boolean does not see the next edit, and their
//! number reverted to ours on every update — so it records the VALUE we
//! wrote, which makes « they changed it » decidable.

use super::*;

/// Opt-out for the auto-compaction-threshold feature: `TOKENADE_AUTOCOMPACT=0`.
pub(crate) fn autocompact_enabled() -> bool {
    !matches!(
        std::env::var("TOKENADE_AUTOCOMPACT").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

/// Percent of the context window at which to compact (default 70).
///
/// Three sources, in this order: the `--autocompact-pct` flag of
/// `tokenade install`, the `TOKENADE_AUTOCOMPACT_PCT` variable, then the
/// default. The flag goes first because it is WRITTEN in the command: a value
/// set through an environment variable leaves no trace of what was chosen,
/// and the user who reruns the install six months later restores the default
/// without knowing it.
///
/// A value outside the 10–95 bounds is IGNORED rather than clamped:
/// `--autocompact-pct 200` is a typo, and compacting at 95 % while believing
/// you asked for 200 is worse than falling back to the default.
pub(crate) fn explicit_pct(from_flag: Option<u32>) -> Option<u32> {
    from_flag.filter(|p| (10..=95).contains(p)).or_else(|| {
        std::env::var("TOKENADE_AUTOCOMPACT_PCT")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|p| (10..=95).contains(p))
    })
}

/// The percentage actually applied: what the user SAID, otherwise what they
/// said last time (`stored`, read back from the file), otherwise the default.
/// This is the link that makes a `tokenade upgrade` carry over the chosen
/// setting instead of silently restoring 70 %.
pub(crate) fn autocompact_pct(from_flag: Option<u32>, stored: Option<u32>) -> u32 {
    explicit_pct(from_flag)
        .or(stored.filter(|p| (10..=95).contains(p)))
        .unwrap_or(tokenade_core::autocompact::DEFAULT_PCT)
}

/// What we wrote last time, to tell OUR value from a hand-set one — and to
/// carry over the chosen setting:
///
/// ```json
/// "_tokenadeAutoCompact": { "pct": 55, "window": 550000 }
/// ```
///
/// Historically a plain `true`, still read as « ours, value unknown ». A bare
/// `true` does NOT allow spotting a hand edit: that is exactly the defect
/// this shape fixes.
pub(crate) const AUTOCOMPACT_MARKER: &str = "_tokenadeAutoCompact";

/// What the file says about our last write.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Stamp {
    /// The marker is present (so the value comes from us).
    ours: bool,
    /// The percentage requested then, if it was recorded.
    pct: Option<u32>,
    /// The token count written then, if it was recorded. `None` on a legacy
    /// marker (`true`): nothing to compare, so nothing to detect.
    window: Option<u64>,
}

pub(crate) fn read_stamp(settings: &serde_json::Value) -> Stamp {
    match settings.get(AUTOCOMPACT_MARKER) {
        Some(serde_json::Value::Bool(true)) => Stamp {
            ours: true,
            ..Stamp::default()
        },
        Some(serde_json::Value::Object(m)) => Stamp {
            ours: true,
            pct: m.get("pct").and_then(|v| v.as_u64()).map(|v| v as u32),
            window: m.get("window").and_then(|v| v.as_u64()),
        },
        _ => Stamp::default(),
    }
}

/// What `set_autocompact_window` decided, so the caller can SAY it.
#[derive(Debug, PartialEq)]
pub(crate) enum AutoCompact {
    /// Written (or, under `--dry-run`, what would be): the threshold in
    /// tokens and the applied percentage. The percentage travels with the
    /// threshold because the caller DISPLAYS it, and recomputing it elsewhere
    /// would be two spellings of one decision.
    Set(u64, u32),
    /// Left as-is: the file's value is not the one we wrote, so someone
    /// changed it by hand. Carries that value.
    HandSet(u64),
    /// Nothing to do (`TOKENADE_AUTOCOMPACT=0`, non-object file, unreadable).
    Skipped,
}

/// The decision, as a PURE function of the file's state. Kept separate
/// because it is the only thing that matters here and it can be tested
/// without writing a file: the rest of `set_autocompact_window` is
/// serialisation.
///
/// `current`: the two spellings of the threshold in the file — the
/// `autoCompactWindow` key and the environment entry. BOTH are compared: a
/// user told « change this threshold here » may edit either one, and only
/// looking at the key would overwrite their edit of the environment entry.
pub(crate) fn autocompact_decision(
    stamp: Stamp,
    key: Option<u64>,
    env: Option<u64>,
    explicit: Option<u32>,
) -> Option<u64> {
    let present = key.or(env);
    match present {
        // Nothing in the file: ours to write.
        None => None,
        Some(v) => {
            // A percentage requested on the command line is an ORDER: it is
            // the only way to take back control of a hand-set value, and the
            // user just typed it.
            if explicit.is_some() {
                return None;
            }
            if !stamp.ours {
                return Some(v);
            }
            match stamp.window {
                // Legacy marker (`true`): no trace of what we wrote, so no
                // detectable edit. Previous behaviour.
                None => None,
                Some(w) => {
                    // The key wins when the two diverge: it is the one the
                    // documentation names.
                    let edited = key.is_some_and(|k| k != w) || env.is_some_and(|e| e != w);
                    if edited {
                        Some(key.filter(|k| *k != w).or(env).unwrap_or(v))
                    } else {
                        None
                    }
                }
            }
        }
    }
}

/// Ask a Claude-Code-schema agent to auto-compact EARLIER — at `pct`% of its
/// context window rather than the native ~95% — by writing `autoCompactWindow`
/// (a token count) into its `settings.json`, plus the matching
/// `CLAUDE_CODE_AUTO_COMPACT_WINDOW` env entry (needed for proactive compaction
/// on 1M-context local models). A smaller prefix is cheaper to re-read every
/// turn (cache_read) and cheaper to re-create on cache expiry. Multi-OS: the
/// path is home-relative; multi-agent: works for any Claude-Code-schema
/// settings.json (Claude Code + forks), a no-op elsewhere.
///
/// NEVER replaces a hand-set value — neither one that was there before us,
/// nor one edited after us (see [`autocompact_decision`]). The only way to
/// take back control is to SAY so: `--autocompact-pct N`.
pub(crate) fn set_autocompact_window(
    settings_path: &Path,
    dry_run: bool,
    pct: Option<u32>,
) -> Result<AutoCompact, String> {
    if !autocompact_enabled() {
        return Ok(AutoCompact::Skipped);
    }
    // Absent file → start empty. UNPARSABLE file (hand-edit, merge conflict)
    // → bail: `.ok()` used to swallow the parse error into `json!({})` and
    // the write below then REPLACED the user's whole settings.json with a
    // tokenade-only document. Never clobber what we can't parse.
    let mut settings: serde_json::Value = match fs::read_to_string(settings_path) {
        Err(_) => serde_json::json!({}),
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return Err(format!(
                    "unparsable {}: {e} — left untouched",
                    settings_path.display()
                ))
            }
        },
    };
    if !settings.is_object() {
        return Ok(AutoCompact::Skipped); // valid JSON but not an object — never clobber
    }
    let stamp = read_stamp(&settings);
    let key = settings.get("autoCompactWindow").and_then(|v| v.as_u64());
    let env_val = settings
        .get("env")
        .and_then(|e| e.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()));
    let explicit = explicit_pct(pct);
    if let Some(theirs) = autocompact_decision(stamp, key, env_val, explicit) {
        // A hand-set value: we don't touch it. We only align the two
        // spellings with each other, otherwise the environment entry keeps OUR
        // old number and the user's edit has no effect — they believe they set
        // the threshold and nothing moves.
        if !dry_run && (key != Some(theirs) || env_val != Some(theirs)) {
            let obj = settings.as_object_mut().unwrap();
            obj.insert("autoCompactWindow".into(), serde_json::json!(theirs));
            obj.remove(AUTOCOMPACT_MARKER); // the value is no longer ours
            let env = obj.entry("env").or_insert_with(|| serde_json::json!({}));
            if let Some(e) = env.as_object_mut() {
                e.insert(
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(),
                    serde_json::json!(theirs.to_string()),
                );
            }
            write_settings(settings_path, &settings)?;
        }
        return Ok(AutoCompact::HandSet(theirs));
    }
    let model = settings.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let pct = autocompact_pct(pct, stamp.pct);
    let threshold = tokenade_core::autocompact::threshold_for_model(model, pct);
    if dry_run {
        return Ok(AutoCompact::Set(threshold, pct));
    }
    let obj = settings.as_object_mut().unwrap();
    obj.insert("autoCompactWindow".into(), serde_json::json!(threshold));
    obj.insert(
        AUTOCOMPACT_MARKER.into(),
        serde_json::json!({ "pct": pct, "window": threshold }),
    );
    let env = obj.entry("env").or_insert_with(|| serde_json::json!({}));
    if let Some(e) = env.as_object_mut() {
        e.insert(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(),
            serde_json::json!(threshold.to_string()),
        );
    }
    write_settings(settings_path, &settings)?;
    Ok(AutoCompact::Set(threshold, pct))
}

/// Backup then atomic write, like everything that touches this file. A
/// `fs::write` truncates first: an interruption (Ctrl-C, ENOSPC, OOM-kill)
/// between the truncation and the write leaves a half-written
/// `settings.json`, and Claude Code then refuses to start — from a mere
/// `tokenade upgrade`.
pub(crate) fn write_settings(
    settings_path: &Path,
    settings: &serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = settings_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    crate::backup_file(settings_path);
    crate::write_json_atomic(settings_path, &(body + "\n"))
        .map_err(|e| format!("write {}: {e}", settings_path.display()))
}
