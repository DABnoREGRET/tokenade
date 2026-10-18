//! **Cookies, which arrive as a table or as JSON.**
//!
//! A cookie table puts the secret in a COLUMN, not in a key-value
//! pair: one has to find which, and redact only that — the
//! attributes (`Path`, `Secure`, `SameSite`) are context whose removal makes
//! the output unreadable for nothing.

// ─────────────────────────────────────────────────────────────────────
// Cookies, whatever the format carrying them
// ─────────────────────────────────────────────────────────────────────
//
// These five functions lived in `csv` and `json`, the modules that know how to
// READ those formats. Yet they read nothing of them: they SCRUB, and they
// pulled `redact` — a primitive the whole tree calls, `debug_log`
// included — up into the formats layer. It was the last edge preventing
// the base layer from being extracted into a crate.
//
// The two original modules still call them, now DOWNWARDS:
// the CSV compactor needs `cookie_value_col`, the JSON compactor
// `redact_cookie_values`. That is the right dependency direction — a format may
// name a primitive, the reverse is what creates cycles.

use serde_json::Value as JsonValue;
use std::fmt::Write as _;

/// Cookie-attribute keys (case-insensitive). An object is treated as a cookie
/// export entry only when it carries a `value` field AND ≥1 of these — so a
/// plain `{"name":…,"value":…}` key/value pair is left untouched.
pub(super) const COOKIE_ATTR_KEYS: &[&str] = &[
    "domain",
    "path",
    "httponly",
    "secure",
    "samesite",
    "expirationdate",
    "hostonly",
    "session",
];

/// Unconditional cookie-table redaction for the always-run `redact()`
/// chokepoint. The in-compactor coverage only fires when the table is large
/// enough to compact; a small DevTools export copied into a prompt or read
/// from a file passes through the compactor unchanged, so it must ALSO be
/// scrubbed here (the universal boundary applied to every read/wrap/webfetch/
/// stash output).
///
/// If the first non-empty line is a cookie-export header (see
/// [`cookie_value_col`]), the `JsonValue` column of every data line is redacted in
/// place — header, all other columns, blank lines and line endings preserved.
/// Tries TSV (tab) first, then CSV (comma); DevTools copies as tab. Returns the
/// text byte-for-byte unchanged when no cookie header is present (so ordinary
/// prose / logs / non-cookie tables are never disturbed).
pub fn redact_cookie_table(text: &str) -> String {
    for delim in ['\t', ','] {
        if let Some(r) = try_redact_cookie_table(text, delim) {
            return r;
        }
    }
    text.to_string()
}

pub(super) fn try_redact_cookie_table(text: &str, delim: char) -> Option<String> {
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let header: Vec<String> = first
        .split(delim)
        .map(|s| s.trim_end_matches('\r').to_string())
        .collect();
    if header.len() < 2 {
        return None;
    }
    let vcol = cookie_value_col(&header)?;

    let mut out = String::with_capacity(text.len());
    let mut header_done = false;
    for seg in text.split_inclusive('\n') {
        let (content, nl) = match seg.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (seg, ""),
        };
        let (content, cr) = match content.strip_suffix('\r') {
            Some(c) => (c, "\r"),
            None => (content, ""),
        };
        if content.trim().is_empty() {
            out.push_str(content);
            out.push_str(cr);
            out.push_str(nl);
            continue;
        }
        if !header_done {
            // Header line: leave the column names untouched.
            header_done = true;
            out.push_str(content);
            out.push_str(cr);
            out.push_str(nl);
            continue;
        }
        let fields: Vec<&str> = content.split(delim).collect();
        if fields.len() > vcol {
            for (j, f) in fields.iter().enumerate() {
                if j > 0 {
                    out.push(delim);
                }
                if j == vcol {
                    let cnt = f.chars().count();
                    if cnt > 0 && !f.starts_with("<redacted") {
                        let _ = write!(out, "<redacted {cnt} chars>");
                    } else {
                        out.push_str(f);
                    }
                } else {
                    out.push_str(f);
                }
            }
        } else {
            out.push_str(content);
        }
        out.push_str(cr);
        out.push_str(nl);
    }
    Some(out)
}

/// Cookie-table signature detector. Returns the index of the `JsonValue` column
/// ONLY when the header looks like a browser cookie export: it has a `Name`
/// column AND a `JsonValue` column AND ≥1 cookie-attribute column. Matching is
/// case-insensitive and whitespace-trimmed. Returns `None` for anything else
/// (a two-column `Name,JsonValue` metrics/config table has no attribute column,
/// so its values are preserved).
pub fn cookie_value_col(header: &[String]) -> Option<usize> {
    const ATTRS: &[&str] = &[
        "domain",
        "path",
        "expires",
        "max-age",
        "size",
        "httponly",
        "secure",
        "samesite",
        "priority",
        "partition key",
        "partition key site",
    ];
    let mut has_name = false;
    let mut has_attr = false;
    let mut value_idx = None;
    for (i, cell) in header.iter().enumerate() {
        let c = cell.trim().to_ascii_lowercase();
        if c == "name" {
            has_name = true;
        } else if c == "value" {
            value_idx = Some(i);
        } else if ATTRS.contains(&c.as_str()) {
            has_attr = true;
        }
    }
    if has_name && has_attr {
        value_idx
    } else {
        None
    }
}

/// Unconditional cookie-JSON redaction for the always-run `redact()`
/// chokepoint. The in-compactor pre-pass only fires when the blob is routed
/// to the JSON compactor AND it actually shrinks; a small cookie export read
/// from a file or pasted into a prompt passes through unchanged, so it must
/// ALSO be scrubbed at the universal boundary.
///
/// Parses `text` as JSON, redacts the `value` field of every cookie-export
/// object (gated by the cookie signature), and re-serialises. Returns the text
/// byte-for-byte unchanged when it is not JSON, or is JSON with no cookie
/// object (so non-cookie JSON keeps its original formatting). A cheap
/// first-char guard avoids parsing every non-JSON string that flows through
/// `redact()`.
pub fn redact_cookie_json(text: &str) -> String {
    let t = text.trim_start();
    if !t.starts_with('[') && !t.starts_with('{') {
        return text.to_string();
    }
    if let Ok(mut v) = serde_json::from_str::<JsonValue>(text) {
        if redact_cookie_values(&mut v) {
            return serde_json::to_string(&v).unwrap_or_else(|_| text.to_string());
        }
    }
    text.to_string()
}

/// Recursively redact the `value` field of every cookie-export object in the
/// tree (in place). Returns `true` if anything was redacted. Idempotent: a
/// value already rendered as `<redacted …>` is skipped.
pub fn redact_cookie_values(v: &mut JsonValue) -> bool {
    let mut changed = false;
    match v {
        JsonValue::Array(items) => {
            for it in items.iter_mut() {
                changed |= redact_cookie_values(it);
            }
        }
        JsonValue::Object(map) => {
            for child in map.values_mut() {
                changed |= redact_cookie_values(child);
            }
            let has_value = map.keys().any(|k| k.eq_ignore_ascii_case("value"));
            let has_attr = map
                .keys()
                .any(|k| COOKIE_ATTR_KEYS.contains(&k.to_ascii_lowercase().as_str()));
            if has_value && has_attr {
                // Preserve the original key casing when writing back.
                if let Some(key) = map
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case("value"))
                    .cloned()
                {
                    if let Some(slot) = map.get_mut(&key) {
                        match slot {
                            JsonValue::String(s) if !s.starts_with("<redacted") => {
                                let n = s.chars().count();
                                *slot = JsonValue::String(format!("<redacted {n} chars>"));
                                changed = true;
                            }
                            JsonValue::String(_) | JsonValue::Null => {}
                            _ => {
                                *slot = JsonValue::String("<redacted>".into());
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    changed
}
