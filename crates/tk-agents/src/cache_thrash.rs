//! Cache-thrash detection over Claude Code transcripts.
//!
//! A *thrash* event is an API call that re-WRITES most of the previous
//! call's context as `cache_creation` while the previous prompt cache was
//! provably still alive (gap ≤ its write TTL). Field audit 2026-07-12
//! (38.7k deduped API calls): these rewrites cost ~24% of total spend, hit
//! ~18% of NEW user turns (vs 0.4% of tool-loop steps) at a flat rate
//! across context sizes, and survive exactly to the tools+system prefix —
//! the documented signature of a request-shape change (thinking parameter
//! OFF→ON, effort level, tool/skill list mutation, model fallback), NOT of
//! TTL expiry or near-window trimming. Live repro 2026-07-12: toggling
//! `MAX_THINKING_TOKENS` 0→N between turns rewrites the whole message
//! layer; N→M and N→0 do not.
//!
//! This module only DETECTS and reports (telemetry class `thrash`,
//! accounting-neutral): the countermeasure — pinning the request shape —
//! is evaluated server-side from these rows before any default flips.
//! Scanning is incremental (byte offset persisted by the caller) and
//! tail-capped so the per-turn hook stays cheap.

use serde::{Deserialize, Serialize};

/// Max new bytes examined per scan; older excess is skipped and continuity
/// reset (no event spans the skip, so no false positives — only misses).
const SCAN_CAP_BYTES: u64 = 4 * 1024 * 1024;
/// A rewrite smaller than this is delta-noise, never thrash.
const MIN_REWRITE_TOKENS: u64 = 20_000;
/// Fraction of the previous context that must be re-written to call it a
/// full rewrite (matches the field-audit detector).
const REWRITE_FRACTION: f64 = 0.5;
/// Per-scan event cap: a thrashing marathon must not flood the ledger.
const MAX_EVENTS_PER_SCAN: usize = 20;

/// Incremental scan cursor + inter-call continuity, persisted between turns
/// by the caller (one small JSON per session).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanState {
    /// Byte offset of the first unprocessed line.
    pub off: u64,
    /// message.id of the last assistant API call already evaluated (its
    /// later content-block lines repeat the same usage and must be skipped).
    pub last_mid: String,
    /// Previous call's context size (cr+cc+in), 0 = none yet.
    pub prev_ctx: u64,
    /// Previous call's timestamp (epoch seconds).
    pub prev_ts: f64,
    /// Previous call's cache-write TTL in seconds (300 or 3600).
    pub prev_ttl: u64,
    /// Turn-boundary markers seen since the previous call.
    pub saw_user_text: bool,
    pub saw_meta: bool,
    pub saw_scheduled: bool,
}

/// One detected full-context rewrite inside the live cache window.
#[derive(Debug, Clone, PartialEq)]
pub struct ThrashEvent {
    /// Tokens re-written (`cache_creation_input_tokens`).
    pub rewritten: u64,
    /// Previous call's context size.
    pub prev_ctx: u64,
    /// Seconds since the previous call.
    pub gap_secs: u64,
    /// What preceded the call: `userturn` / `meta` / `scheduled` / `toolloop`.
    pub trigger: &'static str,
}

/// Parse an ISO-8601 `YYYY-MM-DDTHH:MM:SS[.fff]Z` timestamp to epoch seconds.
/// Returns `None` on any other shape (offsets other than Z are not emitted
/// by Claude Code transcripts).
pub fn iso_to_epoch(ts: &str) -> Option<f64> {
    let b = ts.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, mo, d) = (num(&ts[0..4])?, num(&ts[5..7])?, num(&ts[8..10])?);
    let (h, mi, s) = (num(&ts[11..13])?, num(&ts[14..16])?, num(&ts[17..19])?);
    // days-from-civil (Howard Hinnant), valid for the Gregorian calendar
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let frac: f64 = ts[19..]
        .strip_suffix('Z')
        .filter(|r| r.is_empty() || r.starts_with('.'))
        .and_then(|r| {
            if r.is_empty() {
                Some(0.0)
            } else {
                format!("0{r}").parse().ok()
            }
        })?;
    Some((days * 86_400 + h * 3_600 + mi * 60 + s) as f64 + frac)
}

fn u64_at(v: &serde_json::Value, path: &[&str]) -> u64 {
    let mut cur = v;
    for k in path {
        cur = &cur[*k];
    }
    cur.as_u64().unwrap_or(0)
}

/// Scan the transcript's new bytes for thrash events. Returns the events
/// (capped) and the advanced state. Never errors: unreadable/shrunk files
/// reset the cursor and report nothing.
pub fn scan_new(path: &std::path::Path, state: &ScanState) -> (Vec<ThrashEvent>, ScanState) {
    use std::io::{BufRead, Seek};
    let mut st = state.clone();
    let mut out = Vec::new();
    let Ok(f) = std::fs::File::open(path) else {
        return (out, st);
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < st.off {
        st = ScanState::default(); // trimmed/rotated: restart clean
    }
    if len == st.off {
        return (out, st);
    }
    let mut rd = std::io::BufReader::new(f);
    if len.saturating_sub(st.off) > SCAN_CAP_BYTES {
        // Way behind (first run on a huge transcript): skip to the tail and
        // drop continuity so the skipped span can't fabricate an event.
        st = ScanState {
            off: len - SCAN_CAP_BYTES,
            ..ScanState::default()
        };
        let _ = rd.seek(std::io::SeekFrom::Start(st.off));
        let mut skip = String::new();
        st.off += rd.read_line(&mut skip).unwrap_or(0) as u64; // half line
    } else {
        let _ = rd.seek(std::io::SeekFrom::Start(st.off));
    }
    let mut line = String::new();
    loop {
        line.clear();
        let n = rd.read_line(&mut line).unwrap_or(0);
        if n == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break; // partial trailing line: re-read next scan
        }
        st.off += n as u64;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("assistant") => {
                let mid = v["message"]["id"].as_str().unwrap_or("");
                if mid.is_empty() || mid == st.last_mid {
                    continue; // same API call, later content block
                }
                let u = &v["message"]["usage"];
                let cc = u64_at(u, &["cache_creation_input_tokens"]);
                let cr = u64_at(u, &["cache_read_input_tokens"]);
                let inp = u64_at(u, &["input_tokens"]);
                if u.is_null() || cc + cr + inp == 0 {
                    continue;
                }
                let ts = v["timestamp"]
                    .as_str()
                    .and_then(iso_to_epoch)
                    .unwrap_or(0.0);
                let c1 = u64_at(u, &["cache_creation", "ephemeral_1h_input_tokens"]);
                if st.prev_ctx > 0 && st.prev_ts > 0.0 && ts > 0.0 {
                    let gap = (ts - st.prev_ts).max(0.0) as u64;
                    if cc >= MIN_REWRITE_TOKENS
                        && (cc as f64) > REWRITE_FRACTION * st.prev_ctx as f64
                        && gap <= st.prev_ttl
                        && out.len() < MAX_EVENTS_PER_SCAN
                    {
                        let trigger = if st.saw_scheduled {
                            "scheduled"
                        } else if st.saw_meta {
                            "meta"
                        } else if st.saw_user_text {
                            "userturn"
                        } else {
                            "toolloop"
                        };
                        out.push(ThrashEvent {
                            rewritten: cc,
                            prev_ctx: st.prev_ctx,
                            gap_secs: gap,
                            trigger,
                        });
                    }
                }
                st.last_mid = mid.to_string();
                st.prev_ctx = cr + cc + inp;
                st.prev_ts = ts;
                st.prev_ttl = if c1 > 0 { 3600 } else { 300 };
                st.saw_user_text = false;
                st.saw_meta = false;
                st.saw_scheduled = false;
            }
            Some("user") => {
                if v["isMeta"].as_bool().unwrap_or(false) {
                    st.saw_meta = true;
                    continue;
                }
                let c = &v["message"]["content"];
                let has_text = c.is_string()
                    || c.as_array()
                        .is_some_and(|a| a.iter().any(|b| b["type"].as_str() == Some("text")));
                if has_text {
                    st.saw_user_text = true;
                }
            }
            Some("system") if v["subtype"].as_str() == Some("scheduled_task_fire") => {
                st.saw_scheduled = true;
            }
            _ => {}
        }
    }
    (out, st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn call(mid: &str, ts: &str, cr: u64, cc: u64, ttl1h: bool) -> String {
        let (c5, c1) = if ttl1h { (0, cc) } else { (cc, 0) };
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{mid}","usage":{{"input_tokens":2,"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cc},"cache_creation":{{"ephemeral_5m_input_tokens":{c5},"ephemeral_1h_input_tokens":{c1}}}}}}}}}"#
        )
    }

    fn write_tmp(name: &str, lines: &[String]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tk-thrash-test-{name}-{}.jsonl",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn iso_to_epoch_known_value() {
        // `date -u -d '2026-07-12T00:00:00Z' +%s` → 1783814400
        assert_eq!(iso_to_epoch("2026-07-12T00:00:00Z"), Some(1783814400.0));
        assert_eq!(iso_to_epoch("2026-07-12T00:00:01.500Z"), Some(1783814401.5));
        assert_eq!(iso_to_epoch("garbage"), None);
    }

    #[test]
    fn detects_userturn_rewrite_inside_ttl() {
        let p = write_tmp(
            "userturn",
            &[
                call("m1", "2026-07-12T10:00:00Z", 100_000, 5_000, true),
                r#"{"type":"user","message":{"content":"go on"}}"#.into(),
                call("m2", "2026-07-12T10:01:00Z", 15_000, 90_000, true),
            ],
        );
        let (ev, st) = scan_new(&p, &ScanState::default());
        std::fs::remove_file(&p).ok();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].trigger, "userturn");
        assert_eq!(ev[0].rewritten, 90_000);
        assert_eq!(st.prev_ctx, 15_000 + 90_000 + 2);
    }

    #[test]
    fn no_event_for_delta_writes_or_expired_gap() {
        let p = write_tmp(
            "delta",
            &[
                call("m1", "2026-07-12T10:00:00Z", 100_000, 5_000, true),
                call("m2", "2026-07-12T10:00:30Z", 105_000, 3_000, true), // delta
                call("m3", "2026-07-12T12:00:00Z", 0, 110_000, true),     // gap > 1h TTL
            ],
        );
        let (ev, _) = scan_new(&p, &ScanState::default());
        std::fs::remove_file(&p).ok();
        assert!(ev.is_empty());
    }

    #[test]
    fn five_minute_ttl_bounds_the_window() {
        let p = write_tmp(
            "fivemin",
            &[
                call("m1", "2026-07-12T10:00:00Z", 100_000, 5_000, false), // 5m write
                call("m2", "2026-07-12T10:10:00Z", 0, 104_000, false), // 10 min later: expired, not thrash
            ],
        );
        let (ev, _) = scan_new(&p, &ScanState::default());
        std::fs::remove_file(&p).ok();
        assert!(ev.is_empty());
    }

    #[test]
    fn repeated_blocks_of_same_call_counted_once_and_offset_resumes() {
        let l1 = call("m1", "2026-07-12T10:00:00Z", 100_000, 5_000, true);
        let l2 = call("m1", "2026-07-12T10:00:01Z", 100_000, 5_000, true);
        let p = write_tmp("resume", &[l1, l2]);
        let (ev, st) = scan_new(&p, &ScanState::default());
        assert!(ev.is_empty());
        assert_eq!(st.last_mid, "m1");
        // append a rewrite call; rescan from the saved offset
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"content":"next"}}}}"#).unwrap();
        writeln!(
            f,
            "{}",
            call("m2", "2026-07-12T10:02:00Z", 15_000, 95_000, true)
        )
        .unwrap();
        drop(f);
        let (ev2, st2) = scan_new(&p, &st);
        std::fs::remove_file(&p).ok();
        assert_eq!(ev2.len(), 1);
        assert_eq!(ev2[0].trigger, "userturn");
        assert!(st2.off > st.off);
    }

    #[test]
    fn shrunk_file_resets_cleanly() {
        let p = write_tmp(
            "shrunk",
            &[call("m1", "2026-07-12T10:00:00Z", 100_000, 5_000, true)],
        );
        let st = ScanState {
            off: 1 << 30,
            ..Default::default()
        };
        let (ev, st2) = scan_new(&p, &st);
        std::fs::remove_file(&p).ok();
        assert!(ev.is_empty());
        assert_eq!(st2.last_mid, "m1");
    }
}
