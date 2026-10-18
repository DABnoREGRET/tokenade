//! **Formatting what is displayed in a column.**
//!
//! Shortening a path, writing human-readable bytes, dating a version. Nothing
//! here decides anything; everything serves to make a line fit in a terminal
//! width without lying about what it abbreviates.
//!
//! `root()` and `log_gain` are NOT here: the first resolves the project root,
//! on which every index key depends, and the second writes to the ledger.
//! Neither formats anything.

/// `tokenade semantic-status` — read-only audit of the global
/// semantic index DB at `~/.tokenade/semantic.db`. Lists every
/// project the user has indexed with size + last-touched info, so
/// the operator can answer "what does tokenade know about?" without
/// digging into SQLite themselves.
/// Keep the tail of `p` so it fits `width` columns, marked with a leading `…`.
///
/// Counts CHARACTERS, not bytes. `&p[p.len() - 45..]` panicked outright when
/// that byte offset fell inside a multibyte character: a project stored as
/// `/home/user/Développement/…` cut at byte 13, which is the middle of the
/// `é`, and the whole listing died after its header. Measured on 1.1.9.
/// Characters are also closer to the column width the row is padded to than
/// bytes ever were, so the table lines up better as well.
///
/// Extracted rather than left inline: a decision made inside a command
/// function is one no test can reach.
pub(crate) fn shorten_path_for_column(p: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if p.chars().count() <= width {
        return p.to_string();
    }
    let keep = width - 1;
    if keep == 0 {
        // Room for the marker and nothing else; `keep - 1` below would wrap.
        return "…".to_string();
    }
    let start = p
        .char_indices()
        .rev()
        .nth(keep - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("…{}", &p[start..])
}

/// Format a byte count as a short human string ("12.3 MB" / "847 B").
pub(crate) fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// #996 — render a manifest `releasedAt` (unix seconds) as a leading
/// `", released N day(s) ago"` suffix for `tokenade upgrade --check`.
/// Returns `""` when the timestamp is `0` (unset) OR in the future (a
/// just-published manifest under clock skew shouldn't show a negative
/// age). "<1h", "Nh", and "Nd" are the three friendly buckets.
pub(crate) fn released_age_suffix(released_at: u64) -> String {
    if released_at == 0 {
        return String::new();
    }
    let now = tokenade_core::platform::now_unix_secs();
    if now <= released_at {
        return String::new();
    }
    let secs = now - released_at;
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if hours < 1 {
        ", released <1h ago".to_string()
    } else if hours < 24 {
        format!(", released {hours}h ago")
    } else if days == 1 {
        ", released 1 day ago".to_string()
    } else {
        format!(", released {days} days ago")
    }
}

/// Format a Unix-second timestamp as `YYYY-MM-DD HH:MM`. Reuses
/// the algorithm in `agent_transcripts::epoch_to_iso8601_z` and
/// trims the `:SS Z` suffix.
pub(crate) fn unix_to_iso(secs: u64) -> String {
    let iso = tokenade_core::platform::epoch_to_iso8601_z(secs);
    // Format: `YYYY-MM-DDTHH:MM:SSZ` (20 chars). We want `YYYY-MM-DD HH:MM` (16 chars).
    if iso.len() >= 16 {
        let mut s = iso[..16].to_string();
        // Replace the `T` with a space for readability.
        s.replace_range(10..11, " ");
        s
    } else {
        iso
    }
}
