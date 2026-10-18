//! Per-file, incremental extraction of the transcript records the dashboard
//! reads — so a corpus that only ever GROWS is never re-read whole.
//!
//! ## Why
//!
//! Five readers in [`crate::claudecode_transcript`] walk `~/.claude/projects`
//! and each opens every session file, materialises every line as a
//! `serde_json::Value`, and throws almost all of it away. Measured 2026-09-03
//! on a 1.8 GB corpus: `tokenade dashboard` took **27 s** while `gain`,
//! `receipt` and `statusline` — reading the same ledgers but no transcripts —
//! took 7-11 ms. `scan_session`'s own comment had already bisected it in
//! August ("the whole 25 s is THIS scan … the remaining 24 s is Value
//! materialisation") and proposed a typed parse. A typed parse makes the pass
//! cheaper; it is still a pass over everything, and the corpus grows every day.
//!
//! ## What is cached
//!
//! Only the fields the five readers actually consume, per record: the
//! timestamp, the cwd, the message id and model, the canonical usage string
//! (so dedup reproduces its fingerprint exactly), the four token counts, and
//! the tool_use / tool_result plumbing with the tool_result body already
//! tokenised — that estimate is one of the more expensive things the old scan
//! recomputed on every invocation.
//!
//! Deliberately NOT cached: anything a reader decides for itself. No window
//! filtering (`since_iso` varies per call and is compared at full ISO
//! precision, so caching per-day would move the boundary day's numbers), and
//! no dedup (each reader scopes its own `UsageDedup` and the order of
//! `admit` calls is part of the result).
//!
//! ## Incrementality
//!
//! A transcript is append-only. An entry records the byte offset just past the
//! last COMPLETE line it consumed; a later run seeks there and parses only
//! what arrived since. A file that shrank, or whose modification time moved
//! backwards, is re-read whole — the cheap assumption is the one that silently
//! corrupts totals, so it is the one not made.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Bump when the extracted shape changes: an old cache is then ignored rather
/// than mixed with a new one. 3: `dedup_key` of a `msg_…` id no longer hashes
/// the file, so keys cached under 2 would never match across sessions.
const CACHE_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ToolResult {
    pub use_id: String,
    pub tokens: u64,
}

/// One transcript record, reduced to what the readers need.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Row {
    /// `true` for an `assistant` record, `false` for a `user` record carrying
    /// at least one `tool_result` block. Nothing else is kept: the readers
    /// skip every other record type before touching any state.
    pub assistant: bool,
    pub ts: String,
    /// Empty when the record carried no `cwd`. Kept rather than dropped
    /// because only some readers require one.
    pub cwd: String,
    pub model: String,
    /// Dedup fingerprint, computed at extraction where the id and the usage
    /// object are still in hand. It replaces both — storing the id and the
    /// canonical usage STRING made those two fields the bulk of the cache
    /// (232 MB on a 1.8 GB corpus, which a reader then had to load).
    /// `None` = the record had no id, which dedup has always admitted.
    pub dedup_key: Option<u64>,
    /// Whether `message.usage` was an object at all. `scan_session` skips the
    /// token accounting when it is not, and a cached row that forgot this
    /// would start counting records the live reader had always dropped.
    pub usage_is_object: bool,
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
    pub tool_uses: Vec<ToolUse>,
    pub tool_results: Vec<ToolResult>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Entry {
    /// Byte offset just past the last complete line consumed.
    consumed: u64,
    /// File length when this entry was last refreshed. Distinct from
    /// `consumed`, which stops at the last NEWLINE: the bytes between them are
    /// a trailing line with no terminator.
    len: u64,
    mtime_ns: u128,
    /// How many of the trailing entries in `rows` came from that unterminated
    /// line. They are re-derived on every refresh and never resumed past,
    /// because the next write may extend that very line.
    tail_rows: usize,
    rows: Vec<Row>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    files: HashMap<String, Entry>,
}

/// Where the cache lives: beside the corpus it describes, under the SAME home
/// the readers were pointed at.
///
/// Not the process's `$HOME`: every reader here takes an explicit `home_dir`,
/// and a cache that ignored it described one corpus while being stored against
/// another — which in the test suite meant reading and writing the developer's
/// real cache while aggregating a synthetic one. `TOKENADE_TRANSCRIPT_CACHE`
/// still overrides, for a caller that wants to place it exactly.
pub fn cache_path_for(home: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_TRANSCRIPT_CACHE") {
        return PathBuf::from(p);
    }
    home.join(".tokenade").join("transcripts.cache.bin.gz")
}

/// Loaded once per process and written back only if something changed.
pub struct TranscriptCache {
    file: CacheFile,
    dirty: bool,
    path: PathBuf,
    /// How many files this instance actually opened and parsed. The whole
    /// point of the cache is that this stays at zero on a second run, and
    /// nothing else observes it — a validity rule that quietly stops matching
    /// costs no wrong numbers, only all of the speed.
    parsed: usize,
}

impl TranscriptCache {
    pub fn load(home: &Path) -> Self {
        let path = cache_path_for(home);
        // postcard + gzip, not JSON: the first cut of this was JSON with named
        // fields and weighed 232 MB, which a reader then had to parse on every
        // run — a cache that costs what it saves.
        let file = std::fs::read(&path)
            .ok()
            .and_then(|raw| {
                use std::io::Read as _;
                let mut d = flate2::read::GzDecoder::new(&raw[..]);
                let mut out = Vec::new();
                d.read_to_end(&mut out).ok()?;
                postcard::from_bytes::<CacheFile>(&out).ok()
            })
            .filter(|c| c.version == CACHE_VERSION)
            .unwrap_or(CacheFile {
                version: CACHE_VERSION,
                files: HashMap::new(),
            });
        Self {
            file,
            dirty: false,
            path,
            parsed: 0,
        }
    }

    /// Rows for `path`, parsing only what the cache does not already hold.
    pub fn rows_for(&mut self, path: &Path) -> &[Row] {
        let key = path.to_string_lossy().to_string();
        let (len, mtime_ns) = match std::fs::metadata(path) {
            Ok(m) => (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ),
            Err(_) => {
                // Unreadable: hand back whatever is cached rather than
                // pretending the file is empty.
                return self
                    .file
                    .files
                    .get(&key)
                    .map(|e| &e.rows[..])
                    .unwrap_or(&[]);
            }
        };

        let start = match self.file.files.get(&key) {
            // Untouched since the last refresh: nothing to do.
            Some(e) if len == e.len && mtime_ns == e.mtime_ns => {
                return &self.file.files[&key].rows;
            }
            // Longer and not older: an append. Resume at the last NEWLINE, not
            // at the last byte — what followed it was an unterminated line and
            // the append may well have been its remainder.
            Some(e) if len > e.len && mtime_ns >= e.mtime_ns => e.consumed,
            // Everything else — shrank, rotated, or REWRITTEN IN PLACE at the
            // same length — is re-read whole. That last case is not
            // hypothetical: a test fixture reuses its path, and resuming into
            // it handed back the previous run's rows for a file that no longer
            // held them.
            _ => 0,
        };

        let (new_rows, consumed, tail_rows) = extract_from(path, start);
        let entry = self.file.files.entry(key.clone()).or_default();
        if start == 0 {
            entry.rows.clear();
        } else {
            // Drop what the previous refresh derived from the unterminated
            // line: it is about to be re-derived, possibly as a longer record.
            let keep = entry.rows.len().saturating_sub(entry.tail_rows);
            entry.rows.truncate(keep);
        }
        entry.rows.extend(new_rows);
        entry.consumed = consumed;
        entry.len = len;
        entry.tail_rows = tail_rows;
        entry.mtime_ns = mtime_ns;
        self.dirty = true;
        self.parsed += 1;
        &self.file.files[&key].rows
    }

    /// How many files this instance has opened and parsed.
    pub fn files_parsed(&self) -> usize {
        self.parsed
    }

    /// Persist if anything was parsed. Failures are silent: this is a cache,
    /// and a dashboard must not fail because a cache could not be written.
    pub fn save(&self) {
        if !self.dirty {
            return;
        }
        let p = &self.path;
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(bytes) = postcard::to_allocvec(&self.file) else {
            return;
        };
        use std::io::Write as _;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        if enc.write_all(&bytes).is_err() {
            return;
        }
        let Ok(gz) = enc.finish() else { return };
        // Through the shared writer, not a local copy of it: its temp name
        // carries the pid and a counter, so two tokenade runs saving at once
        // cannot interleave into one `<path>.tmp` and rename a half-written
        // file into place.
        let _ = crate::paths::write_atomic_bytes(p, &gz);
    }

    /// Drop every file no longer on disk. Called by the maintenance pass so a
    /// cache does not outlive the corpus it describes.
    pub fn forget_missing(&mut self) -> usize {
        let before = self.file.files.len();
        self.file.files.retain(|k, _| Path::new(k).exists());
        let removed = before - self.file.files.len();
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }
}

/// The ONE cache a process uses.
///
/// The five readers each used to open every file for themselves; sharing this
/// is what makes "read once, apply every parser to that read" true inside a
/// single run, exactly as the persisted entries make it true across runs.
/// Whoever touches a file first pays for it; the other four get it from here.
pub fn with_shared<R>(home: &Path, f: impl FnOnce(&mut TranscriptCache) -> R) -> R {
    static SHARED: std::sync::OnceLock<std::sync::Mutex<Option<(PathBuf, TranscriptCache)>>> =
        std::sync::OnceLock::new();
    let m = SHARED.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = m.lock().unwrap_or_else(|p| p.into_inner());
    let want = cache_path_for(home);
    let reload = match guard.as_ref() {
        Some((p, _)) => *p != want,
        None => true,
    };
    if reload {
        if let Some((_, old)) = guard.as_ref() {
            old.save();
        }
        *guard = Some((want, TranscriptCache::load(home)));
    }
    let (_, cache) = guard.as_mut().expect("just set");
    let out = f(cache);
    cache.save();
    out
}

/// Parse `path` from byte offset `start`.
///
/// Returns the rows, the offset just past the last COMPLETE line, and how many
/// of the returned rows came from a trailing line with no newline.
///
/// Both halves matter. The unterminated line IS read — `BufReader::lines`
/// yields it, so every reader here always counted it, and a corpus whose last
/// record has no trailing newline is the normal case, not an edge one. But the
/// offset does not advance past it: a session being appended to right now ends
/// mid-record, and resuming past it would lose what it becomes. So its rows
/// are returned and marked, and re-derived on the next refresh.
fn extract_from(path: &Path, start: u64) -> (Vec<Row>, u64, usize) {
    let scope = path.to_string_lossy().to_string();
    let scope = scope.as_str();
    let Ok(mut file) = std::fs::File::open(path) else {
        return (Vec::new(), start, 0);
    };
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return (Vec::new(), start, 0);
    }
    let mut rows = Vec::new();
    let mut consumed = start;
    let mut tail_rows = 0usize;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let terminated = buf.last() == Some(&b'\n');
                let line = String::from_utf8_lossy(&buf);
                if let Some(r) = extract_line(scope, &line) {
                    rows.push(r);
                    if !terminated {
                        tail_rows += 1;
                    }
                }
                if !terminated {
                    // Read, returned, but NOT consumed: the next write may
                    // extend this very line.
                    break;
                }
                consumed += n as u64;
            }
            Err(_) => break,
        }
    }
    (rows, consumed, tail_rows)
}

/// The same admission rule the readers apply, applied once.
///
/// The cheap substring pre-filter is kept from `scan_session`: it is a
/// superset test (an `assistant` record cannot be written without the quoted
/// value appearing literally), so it can only skip lines a full parse would
/// have dropped anyway.
fn extract_line(scope: &str, line: &str) -> Option<Row> {
    if !line.contains("\"assistant\"") && !line.contains("\"tool_result\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = v["type"].as_str().unwrap_or("");
    let assistant = match kind {
        "assistant" => true,
        "user" => {
            let has = v["message"]["content"].as_array().is_some_and(|arr| {
                arr.iter()
                    .any(|b| b["type"].as_str() == Some("tool_result"))
            });
            if !has {
                return None;
            }
            false
        }
        _ => return None,
    };

    let usage = &v["message"]["usage"];
    let mut row = Row {
        assistant,
        ts: v["timestamp"].as_str().unwrap_or_default().to_string(),
        cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
        model: v["message"]["model"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        dedup_key: crate::usage_dedup::fingerprint_for(scope, v["message"]["id"].as_str(), usage),
        usage_is_object: usage.is_object(),
        input: usage["input_tokens"].as_u64().unwrap_or(0),
        cache_creation: usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        cache_read: usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
        output: usage["output_tokens"].as_u64().unwrap_or(0),
        tool_uses: Vec::new(),
        tool_results: Vec::new(),
    };

    if let Some(blocks) = v["message"]["content"].as_array() {
        for b in blocks {
            match b["type"].as_str() {
                Some("tool_use") => {
                    if let Some(name) = b["name"].as_str() {
                        row.tool_uses.push(ToolUse {
                            id: b["id"].as_str().unwrap_or_default().to_string(),
                            name: name.to_string(),
                        });
                    }
                }
                Some("tool_result") => {
                    // Tokenised here, once. This estimate over `tool_result`
                    // bodies — the largest strings in the corpus — is a large
                    // part of what the old scan repeated on every run.
                    let tokens = if let Some(s) = b["content"].as_str() {
                        crate::tokenizer::estimate(s) as u64
                    } else if let Some(arr) = b["content"].as_array() {
                        arr.iter()
                            .filter_map(|c| c["text"].as_str())
                            .map(|s| crate::tokenizer::estimate(s) as u64)
                            .sum()
                    } else {
                        0
                    };
                    row.tool_results.push(ToolResult {
                        use_id: b["tool_use_id"].as_str().unwrap_or_default().to_string(),
                        tokens,
                    });
                }
                _ => {}
            }
        }
    }
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-tcache-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn line(ts: &str, id: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","cwd":"/w","message":{{"id":"{id}","model":"m","usage":{{"input_tokens":1,"output_tokens":{out}}},"content":[]}}}}"#
        )
    }

    /// The point of the cache: a file that GREW must cost only its new bytes,
    /// and must produce exactly the rows a full re-read would.
    #[test]
    fn an_appended_file_is_parsed_only_from_where_it_stopped() {
        let d = scratch("append");
        let f = d.join("s.jsonl");
        std::fs::write(
            &f,
            format!(
                "{}\n{}\n",
                line("2026-01-01T00:00:00Z", "a", 5),
                line("2026-01-02T00:00:00Z", "b", 7)
            ),
        )
        .unwrap();

        let (rows1, consumed1, _tail) = extract_from(&f, 0);
        assert_eq!(rows1.len(), 2);
        assert_eq!(consumed1, std::fs::metadata(&f).unwrap().len());

        // Append one record; resuming must yield ONLY it.
        let mut s = std::fs::read_to_string(&f).unwrap();
        s.push_str(&format!("{}\n", line("2026-01-03T00:00:00Z", "c", 9)));
        std::fs::write(&f, &s).unwrap();
        let (rows2, consumed2, _tail) = extract_from(&f, consumed1);
        assert_eq!(rows2.len(), 1, "resume re-read more than the tail");
        assert_eq!(rows2[0].output, 9, "the tail row is not the one appended");
        assert_eq!(consumed2, std::fs::metadata(&f).unwrap().len());

        // And the incremental result equals the whole-file result.
        let (whole, _, _tail) = extract_from(&f, 0);
        let joined: Vec<(String, u64)> = rows1
            .iter()
            .chain(rows2.iter())
            .map(|r| (r.ts.clone(), r.output))
            .collect();
        let direct: Vec<(String, u64)> = whole.iter().map(|r| (r.ts.clone(), r.output)).collect();
        assert_eq!(joined, direct, "incremental and whole-file disagree");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A last line with no trailing newline is the NORMAL case, not an edge
    /// one, and it must still be counted.
    ///
    /// The first cut of this stopped before any unterminated line, on the
    /// reasoning that a session being appended to ends mid-record. Half right:
    /// `BufReader::lines` yields that line, so every reader here had always
    /// counted it, and forty-four existing tests — whose fixtures join their
    /// records with `\n` and end without one — went red at once. The record is
    /// now returned AND left unconsumed, so an append that extends it is seen
    /// whole rather than twice.
    #[test]
    fn a_complete_last_line_without_a_newline_is_still_counted() {
        let d = scratch("no-eol");
        let f = d.join("s.jsonl");
        let a = line("2026-01-01T00:00:00Z", "a", 5);
        let b = line("2026-01-02T00:00:00Z", "b", 7);
        std::fs::write(&f, format!("{a}\n{b}")).unwrap(); // no trailing newline
        let (rows, consumed, tail) = extract_from(&f, 0);
        assert_eq!(rows.len(), 2, "the unterminated last record was dropped");
        assert_eq!(tail, 1, "the unterminated record was not marked as tail");
        assert_eq!(
            consumed,
            (a.len() + 1) as u64,
            "consumed past the unterminated record, so an append would lose it"
        );

        // Now the writer finishes the line and adds another. The record must
        // appear ONCE, not twice, through the cache that resumes.
        let mut c = TranscriptCache::load(&d);
        assert_eq!(c.rows_for(&f).len(), 2);
        let c_line = line("2026-01-03T00:00:00Z", "c", 9);
        std::fs::write(&f, format!("{a}\n{b}\n{c_line}")).unwrap();
        let rows = c.rows_for(&f);
        assert_eq!(
            rows.len(),
            3,
            "the record that was unterminated is duplicated or lost: {:?}",
            rows.iter().map(|r| r.output).collect::<Vec<_>>()
        );
        assert_eq!(
            rows.iter().map(|r| r.output).collect::<Vec<_>>(),
            vec![5, 7, 9]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The second read must parse NOTHING. That is the entire feature: a
    /// corpus that only grows is otherwise re-read on every invocation, which
    /// is what made `tokenade dashboard` take 27 seconds while every other
    /// surface over the same data took milliseconds.
    ///
    /// Asserted on a file with no trailing newline, because that is the shape
    /// nearly every session file has and it is the one a validity rule keyed
    /// on the wrong offset silently fails on — correct numbers, no cache.
    #[test]
    fn a_second_read_of_an_unchanged_file_parses_nothing() {
        let d = scratch("warm");
        let f = d.join("s.jsonl");
        std::fs::write(
            &f,
            format!(
                "{}\n{}",
                line("2026-01-01T00:00:00Z", "a", 5),
                line("2026-01-02T00:00:00Z", "b", 7)
            ),
        )
        .unwrap();

        let mut c = TranscriptCache::load(&d);
        assert_eq!(c.rows_for(&f).len(), 2);
        assert_eq!(c.files_parsed(), 1, "the first read must parse the file");
        assert_eq!(c.rows_for(&f).len(), 2);
        assert_eq!(
            c.files_parsed(),
            1,
            "the second read parsed again — the cache is not being used"
        );
        c.save();

        // And a NEW instance, loading from disk, must not parse either.
        let mut c2 = TranscriptCache::load(&d);
        assert_eq!(c2.rows_for(&f).len(), 2);
        assert_eq!(
            c2.files_parsed(),
            0,
            "the persisted cache was ignored on the next run"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A session being written to right now ends mid-line. Consuming it would
    /// drop the record it is about to become.
    #[test]
    fn a_partial_trailing_line_is_left_for_next_time() {
        let d = scratch("partial");
        let f = d.join("s.jsonl");
        let complete = format!("{}\n", line("2026-01-01T00:00:00Z", "a", 5));
        let partial = r#"{"type":"assistant","timestamp":"2026-01-0"#;
        std::fs::write(&f, format!("{complete}{partial}")).unwrap();

        let (rows, consumed, _tail) = extract_from(&f, 0);
        assert_eq!(rows.len(), 1, "the partial line was counted");
        assert_eq!(
            consumed,
            complete.len() as u64,
            "consumed past the last complete line, so the partial record would be lost"
        );

        // Finish the line: the record must now appear, exactly once.
        let rest =
            r#"2T00:00:00Z","cwd":"/w","message":{"id":"b","model":"m","usage":{},"content":[]}}"#
                .to_string();
        let mut s = std::fs::read_to_string(&f).unwrap();
        s.push_str(&rest);
        s.push('\n');
        std::fs::write(&f, &s).unwrap();
        let (rows2, _, _tail) = extract_from(&f, consumed);
        assert_eq!(rows2.len(), 1);
        assert_eq!(
            rows2[0].ts, "2026-01-02T00:00:00Z",
            "the completed line is not the one read back"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A shrunk or rewritten file must be re-read whole rather than resumed
    /// into the middle of a different document.
    #[test]
    fn a_shrunk_file_is_re_read_whole() {
        let d = scratch("shrunk");
        let _cache = d.join("c.json");
        let f = d.join("s.jsonl");
        std::fs::write(
            &f,
            format!(
                "{}\n{}\n",
                line("2026-01-01T00:00:00Z", "a", 5),
                line("2026-01-02T00:00:00Z", "b", 7)
            ),
        )
        .unwrap();

        let mut c = TranscriptCache::load(&d);
        assert_eq!(c.rows_for(&f).len(), 2);

        // Rewrite it shorter, with different content.
        std::fs::write(&f, format!("{}\n", line("2026-02-01T00:00:00Z", "z", 1))).unwrap();
        let rows = c.rows_for(&f);
        assert_eq!(rows.len(), 1, "stale rows survived a rewrite");
        assert_eq!(
            rows[0].ts, "2026-02-01T00:00:00Z",
            "stale content survived the rewrite"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The dedup decision must survive being reduced to the one `u64` the
    /// cache stores: it replaced the message id and the canonical usage
    /// string, which together were the bulk of the cached bytes.
    #[test]
    fn the_cached_key_reproduces_the_dedup_decision() {
        let raw = r#"{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":3}"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();

        let mut by_value = crate::usage_dedup::UsageDedup::new();
        assert!(by_value.admit("f", Some("id1"), &v));
        assert!(
            !by_value.admit("f", Some("id1"), &v),
            "the Value path did not dedup"
        );

        let key = crate::usage_dedup::fingerprint_for("f", Some("id1"), &v);
        let mut by_key = crate::usage_dedup::UsageDedup::new();
        assert!(by_key.admit_key(key));
        assert!(!by_key.admit_key(key), "the cached-key path did not dedup");

        // A different scope is a different response, and a record with no id
        // is always admitted — both are what `admit` has always done.
        assert!(by_key.admit_key(crate::usage_dedup::fingerprint_for("g", Some("id1"), &v)));
        assert_eq!(crate::usage_dedup::fingerprint_for("f", None, &v), None);
        assert!(by_key.admit_key(None));
        assert!(by_key.admit_key(None), "a record with no id was deduped");
    }
}
