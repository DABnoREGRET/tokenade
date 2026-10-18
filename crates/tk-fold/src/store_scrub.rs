//! One scan for every "drop the rows whose payload matches" scrub.
//!
//! Three stores keep derived copies of user content — `semantic.db` holds
//! postcard blobs of each project's symbols and comments, `file_search.db`
//! holds raw chunks of file text — and a scrub of either asks the same
//! question of every row: does this payload carry the thing we are removing?
//! The scan was written once per store per question (needle / redactable),
//! which is where the copies come from; the answer differs, the walk does not.
//!
//! Payload is read as bytes on purpose. A blob is not UTF-8 as a whole, but a
//! credential that went in as text is still a byte run inside it, and a TEXT
//! column reads back as bytes just as well.

use rusqlite::Connection;

/// Keys of the rows whose payload satisfies `matches`.
///
/// `select_sql` must yield exactly two columns: a TEXT key and the payload.
/// Duplicate keys are collapsed, because a store that splits one file across
/// many rows (`file_chunks`) answers with the same key more than once and the
/// caller deletes by key.
///
/// The payload column may be TEXT or BLOB and is read as bytes either way. A
/// plain `row.get::<Vec<u8>>` refuses TEXT with an `InvalidType`, which the
/// lenient callers below turn into "nothing matched" — a scrub that reports a
/// clean store while holding the secret.
pub fn victim_keys<F>(
    conn: &Connection,
    select_sql: &str,
    matches: F,
) -> Result<Vec<String>, rusqlite::Error>
where
    F: Fn(&[u8]) -> bool,
{
    let mut out: Vec<String> = Vec::new();
    let mut stmt = conn.prepare(select_sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let payload: Vec<u8> = match row.get_ref(1)? {
            rusqlite::types::ValueRef::Text(t) => t.to_vec(),
            rusqlite::types::ValueRef::Blob(b) => b.to_vec(),
            _ => Vec::new(),
        };
        if matches(&payload) && !out.contains(&key) {
            out.push(key);
        }
    }
    Ok(out)
}

/// True when `needle` appears in `payload` as a byte run. An empty needle
/// matches nothing: it would otherwise select every row in the store.
pub fn contains_bytes(payload: &[u8], needle: &str) -> bool {
    if needle.is_empty() || needle.len() > payload.len() {
        return false;
    }
    payload
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

/// True when the redact patterns recognise something in `payload`.
///
/// Lossy conversion is right here: the payload is not necessarily UTF-8 as a
/// whole, and a credential that went in as text survives the conversion as
/// text.
pub fn is_redactable(payload: &[u8]) -> bool {
    let text = String::from_utf8_lossy(payload);
    crate::redact::redact(&text) != text
}

/// Size of `path` before and after `vacuum` runs.
///
/// The measurement, not the VACUUM: each store opens its own connection
/// (`file_search.db` carries sqlite-vec virtual tables and cannot be rebuilt
/// without the extension registered), but the metadata dance around it — down
/// to falling back to `before` when the file has gone — was written out four
/// times.
/// **Compact a SQLite store, and report what it gave back.**
///
/// `(before, after)` in bytes, like the [`vacuum_sizes`] it wraps.
///
/// Written once because it was written FIVE times: `dedup_ref`, `file_search`,
/// `fold_store`, `index_db` and `semantic_db` carried the same body word for
/// word. The duplication instrument only saw four — the fifth
/// calls its opener `open_conn` instead of `open`, and a search for identical
/// bodies does not see through an identifier. The day a store's VACUUM
/// has to change (a `PRAGMA`, a lock, a trace), it would
/// today have to change five times or be forgotten four.
pub fn vacuum_store<E>(
    path: &std::path::Path,
    open: impl FnOnce() -> Result<Connection, E>,
) -> (u64, u64) {
    vacuum_sizes(path, || {
        if let Ok(conn) = open() {
            let _ = conn.execute("VACUUM", []);
        }
    })
}

pub fn vacuum_sizes(path: &std::path::Path, vacuum: impl FnOnce()) -> (u64, u64) {
    let before = std::fs::metadata(path).map_or(0, |m| m.len());
    vacuum();
    let after = std::fs::metadata(path).map_or(before, |m| m.len());
    (before, after)
}

// ─────────────────────────────────────────────────────────────────────
// The inventory of a project store
// ─────────────────────────────────────────────────────────────────────
//
// `index_db` and `semantic_db` asked the same question of two tables: the
// list of projects, from most recent to oldest, and forgetting a project.
// Bodies identical up to the table name and the timestamp column
// — and the return type itself was written twice. A defect in
// this read was therefore fixed twice, or once without anyone
// knowing.

/// (project path, timestamp, bytes, files, symbols).
pub type ProjectSummary = (std::path::PathBuf, u64, u64, u64, u64);

/// The projects of a store table, from most recently touched to
/// oldest.
///
/// `table` and `ts_col` are interpolated into the SQL because they are
/// literals from THIS repo — a table name cannot be a bound parameter,
/// and no caller passes a string from outside here. The project path,
/// on the other hand, stays a parameter everywhere it is compared.
pub fn list_projects(
    conn: &Connection,
    table: &str,
    ts_col: &str,
) -> Result<Vec<ProjectSummary>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT project_path, {ts_col}, size_bytes, file_count, symbol_count
         FROM {table} ORDER BY {ts_col} DESC"
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            std::path::PathBuf::from(r.get::<_, String>(0)?),
            r.get::<_, i64>(1)? as u64,
            r.get::<_, i64>(2)? as u64,
            r.get::<_, i64>(3)? as u64,
            r.get::<_, i64>(4)? as u64,
        ))
    })?;
    rows.collect()
}

/// Removes a project's entry (its directory has disappeared, for example).
/// Returns the number of rows touched, 0 or 1.
pub fn forget_project(
    conn: &Connection,
    table: &str,
    project_path: &std::path::Path,
) -> Result<usize, rusqlite::Error> {
    let key = project_path.display().to_string();
    conn.execute(
        &format!("DELETE FROM {table} WHERE project_path=?1"),
        rusqlite::params![key],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE t(k TEXT, v BLOB)").unwrap();
        c
    }

    #[test]
    fn a_key_is_named_once_however_many_of_its_rows_match() {
        let c = db();
        for i in 0..3 {
            c.execute("INSERT INTO t VALUES('f', ?1)", [format!("secret {i}")])
                .unwrap();
        }
        let v = victim_keys(&c, "SELECT k, v FROM t", |p| contains_bytes(p, "secret")).unwrap();
        assert_eq!(v, vec!["f".to_string()]);
    }

    #[test]
    fn a_text_column_is_searched_like_a_blob() {
        let c = db();
        c.execute("INSERT INTO t VALUES('f', 'token = hunter2')", [])
            .unwrap();
        let v = victim_keys(&c, "SELECT k, v FROM t", |p| contains_bytes(p, "hunter2")).unwrap();
        assert_eq!(v, vec!["f".to_string()]);
    }

    #[test]
    fn an_empty_needle_selects_nothing() {
        let c = db();
        c.execute("INSERT INTO t VALUES('f', 'anything')", [])
            .unwrap();
        let v = victim_keys(&c, "SELECT k, v FROM t", |p| contains_bytes(p, "")).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn a_needle_longer_than_the_payload_does_not_panic() {
        assert!(!contains_bytes(b"ab", "abc"));
    }

    #[test]
    fn a_vanished_file_reports_no_growth() {
        let p = std::path::Path::new("/nonexistent/tokenade/none.db");
        assert_eq!(vacuum_sizes(p, || {}), (0, 0));
    }

    #[test]
    fn redactable_sees_a_credential_inside_a_non_utf8_blob() {
        let mut blob = vec![0xffu8, 0xfe];
        blob.extend_from_slice(b"aws_secret_access_key = AKIAIOSFODNN7EXAMPLE");
        assert!(is_redactable(&blob));
        assert!(!is_redactable(b"pub fn deploy() {}"));
    }

    /// **A store does not rewrite its own VACUUM.**
    ///
    /// Five modules carried its body word for word, and the duplication
    /// instrument only saw four: the fifth names its opener
    /// `open_conn`, and a search for identical bodies does not see through
    /// an identifier. A SOURCE guard, then, rather than a behaviour
    /// test: it is the SPELLING that must stay unique.
    #[test]
    fn every_store_delegates_its_vacuum() {
        // The library roots, not just this crate's: the stores
        // are spread across the layers since the split, and a guard that
        // reads only one finds three stores out of five without saying so.
        let dirs = crate::source_guard::crate_srcs();
        let mut seen = 0usize;
        for ent in dirs
            .iter()
            .flat_map(|d| std::fs::read_dir(d).expect("a source root").flatten())
        {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).expect("source lisible");
            let Some(i) = src.find("pub fn vacuum_db()") else {
                continue;
            };
            seen += 1;
            let body = &src[i..src[i..].find("\n}\n").map(|e| i + e).unwrap_or(src.len())];
            assert!(
                body.contains("store_scrub::vacuum_store("),
                "`{}` rewrites its VACUUM instead of delegating",
                p.display()
            );
        }
        assert!(
            seen >= 5,
            "only {seen} store(s) found: the guard no longer looks at anything"
        );
    }

    /// **And it REALLY compacts.** The source guard above says the
    /// five stores delegate; this one says the delegate does the work.
    /// Without it, emptying the body of `vacuum_store` left everything green: the
    /// five faithfully called a function that did nothing.
    #[test]
    fn the_shared_vacuum_actually_reclaims() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-vacuum-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, blob TEXT)", [])
                .unwrap();
            let payload = "x".repeat(4096);
            for i in 0..400 {
                conn.execute("INSERT INTO t (id, blob) VALUES (?1, ?2)", (i, &payload))
                    .unwrap();
            }
            conn.execute("DELETE FROM t", []).unwrap();
        }
        let bloated = std::fs::metadata(&path).unwrap().len();
        assert!(
            bloated > 512 * 1024,
            "the database is not big enough to prove anything ({bloated} B)"
        );

        let (before, after) = vacuum_store(&path, || Connection::open(&path));
        assert_eq!(before, bloated, "the BEFORE size must be the file's");
        assert!(
            after * 4 < before,
            "nothing was given back: {after} vs {before} — the store was not opened"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
