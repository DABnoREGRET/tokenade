//! **A sentence an agent only needs to read once per context.**
//!
//! Repeated on every answer, an explanation is billed on every call: the
//! "exited 0, but its output reports a failure" notice was written 523 times
//! in one working session (≈ 122 KB), each time explaining again that a
//! pipeline reports only its last stage. Said once and never again, it is
//! lost at the next compaction — exactly where an agent's memory of it goes.
//! So: once per session, armed again when the context has been compacted
//! since it was last said.

/// Is the explanation named `tag` due in this context? Marks it said when it
/// is. Without a session there is no context to remember it in: always due.
pub(crate) fn due(tag: &str) -> bool {
    let Some(skey) = tokenade_core::session::session_key() else {
        return true;
    };
    let skey: String = skey
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let marker = crate::hooks::read_staging::read_pending_dir().join(format!("{tag}-{skey}"));
    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let shown = modified(&marker);
    let compacted = modified(&crate::hooks::replay::compact_ts_path(&skey));
    let due = match (shown, compacted) {
        (None, _) => true,
        (Some(s), Some(c)) => c > s,
        (Some(_), None) => false,
    };
    if due {
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, b"1");
    }
    due
}
