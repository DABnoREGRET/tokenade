//! The session key a ledger row is attributed to, and the eviction that
//! keeps abandoned sessions from accumulating.

use crate::session::sessions_dir;

/// Remove per-session state files older than `ttl_secs` from
/// [`sessions_dir`]. These files (webfetch cache, websearch seen-set,
/// overhead counters) previously had NO automatic cleanup — one-to-few were
/// left behind per session forever. Best-effort; returns files removed.
pub fn evict_stale_sessions(ttl_secs: u64) -> usize {
    crate::platform::evict_files_older_than(&sessions_dir(), ttl_secs)
}

/// **The session's EXACT model**, as the host declared it to the hook.
///
/// Written by `session_lifecycle::note_session_model` to
/// `~/.tokenade/sessions/<key>-model` on every turn where the host names its
/// model. It is the finest granularity available on the client side: the
/// model of THE session that produced this event, not the dominant model of
/// its day (what `stamp_event_models` infers from transcripts, for lack of
/// anything better, at send time).
///
/// A single spelling: the hook reads the same file through
/// `session_state::session_model`, which delegates here.
pub fn session_model_for(skey: &str) -> Option<String> {
    if skey.is_empty() {
        return None;
    }
    std::fs::read_to_string(sessions_dir().join(format!("{skey}-model")))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod the_exact_model_of_the_session {
    /// **The finest granularity available.** The reporter used to infer the
    /// model from transcripts at send time: exact session, otherwise the
    /// session covering the timestamp, otherwise **the DAY's dominant model**.
    /// An agent switching models during the day thus valued its savings at
    /// another model's price. The session, however, is known at write time,
    /// and the host declared its model to the hook.
    ///
    /// The home directory is moved for the duration of the test: this module
    /// reads a real file. Through `ScratchHome`, which sets HOME *and*
    /// USERPROFILE under the environment lock and restores both — setting
    /// `HOME` alone redirects nothing on Windows, as the 1.1.12 Windows build
    /// showed.
    #[test]
    fn a_session_that_declared_its_model_hands_it_over() {
        let home = crate::platform::ScratchHome::new("session-model");
        let dir = home.path().join(".tokenade").join("sessions");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("abc123-model"), "claude-opus-5\n").expect("write");
        // A session without a file, and one with an EMPTY file: both mean
        // "unknown", not an empty string passing itself off as a model id.
        std::fs::write(dir.join("empty-model"), "  \n").expect("write");

        let got = super::session_model_for("abc123");
        let empty = super::session_model_for("empty");
        let missing = super::session_model_for("never-seen");
        let no_key = super::session_model_for("");

        assert_eq!(got.as_deref(), Some("claude-opus-5"));
        assert_eq!(empty, None, "an empty file is not a model");
        assert_eq!(missing, None);
        assert_eq!(no_key, None);
    }
}
