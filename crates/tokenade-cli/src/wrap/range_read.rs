//! A `sed -n 'A,Bp' FILE` already covered by earlier reads this session.
//!
//! The `Read` route answers a slice it already delivered with a pointer
//! (`[tokenade:read-range-dedup unchanged]`); the same slice asked through
//! `sed` came back whole. Measured on October's transcripts: 26 % of
//! the bytes Bash delivered were `sed`, and ~1 360 `sed -n` reads were
//! entirely inside lines the session had already been given, the file
//! unchanged — about 44 600 lines served twice.
//!
//! Same registry as the `Read` route (`dedup_read_ranges`, blobs in
//! `dedup_content` so `expand-ref` answers), its own key space, and the same
//! proofs as the bash `§ref` fold: a session, its subagent generation, no
//! compaction since, and the file's version (mtime and size) unchanged.

use std::path::{Path, PathBuf};

/// `sed -n 'A,Bp' FILE` — exactly one file, nothing else on the line.
/// Returns the file and the 0-based `[start, end)` lines it prints.
pub(super) fn sed_range_read(shell_cmd: &str, cwd: &Path) -> Option<(PathBuf, u64, u64)> {
    let words: Vec<&str> = shell_cmd.split_whitespace().collect();
    let [sed, n, range, file] = words.as_slice() else {
        return None;
    };
    if *sed != "sed" || *n != "-n" || file.contains(['|', ';', '&', '>', '<', '$', '*', '`']) {
        return None;
    }
    let range = range.trim_matches(['\'', '"']).strip_suffix('p')?;
    let (a, b) = range.split_once(',')?;
    let (a, b): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
    if a == 0 || b < a {
        return None;
    }
    let file = file.trim_matches(['\'', '"']);
    let path = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        cwd.join(file)
    };
    path.is_file().then_some((path, a - 1, b))
}

fn version_of(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{mtime}|{}", m.len()))
}

/// The registry's session: the wrap session AND its subagent generation — a
/// subagent shares the parent's session id and never saw the parent's reads.
fn registry_session(skey: &str) -> String {
    let gen = crate::hooks::read_pretool::sidechain_generation(skey);
    format!("bash:{skey}:g{gen}")
}

fn file_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    tokenade_core::dedup_ref::range_file_key(&format!("bash-sed:{}", path.display()))
}

/// A `Read` that delivered `[start, end)` verbatim answers a later `sed` of
/// those lines too: 598 October `sed -n` reads lay inside an earlier `Read`
/// of the same unchanged file. Its slice is already stored under
/// `hash`; this files the range where the `sed` route looks.
pub(crate) fn register_read_slice(
    parsed: &serde_json::Value,
    path: &Path,
    start: u64,
    end: u64,
    hash: &str,
) {
    let session = crate::hooks::session_id_of(parsed);
    let (Some(skey), Some(version)) = (super::dedup::wrap_session_key(session), version_of(path))
    else {
        return;
    };
    tokenade_core::dedup_ref::register_read_range(
        &registry_session(&skey),
        &file_key(path),
        &version,
        start,
        end,
        hash,
    );
}

/// The pointer for a covered read, or `None` (serve the lines).
pub(super) fn covered(
    skey: &Option<String>,
    shell_cmd: &str,
    cwd: &Path,
    output: &str,
) -> Option<String> {
    let skey = skey.as_ref()?;
    let (path, start, end) = sed_range_read(shell_cmd, cwd)?;
    if crate::hooks::read_pretool::compaction_recent(skey) {
        return None;
    }
    let cover = tokenade_core::dedup_ref::range_is_covered(
        &registry_session(skey),
        &file_key(&path),
        &version_of(&path)?,
        start,
        end,
        crate::hooks::read_staging::read_context_recency_secs(),
    )?;
    let marker = format!(
        "[tokenade:read-range-dedup unchanged]\n(lines {}–{end} of `{}` were already printed earlier this session \
         and the file has not changed — scroll up, or `tokenade expand-ref {cover}` to see them again.)",
        start + 1,
        path.display()
    );
    (marker.len() < output.len()).then_some(marker)
}

/// Record a read that delivered its lines VERBATIM, so a later read inside
/// them can point here. Anything else (folded, cut, failed) records nothing.
pub(super) fn register(skey: &Option<String>, shell_cmd: &str, cwd: &Path, delivered: &str) {
    let Some(skey) = skey else { return };
    let Some((path, start, end)) = sed_range_read(shell_cmd, cwd) else {
        return;
    };
    let (Ok(full), Some(version)) = (std::fs::read_to_string(&path), version_of(&path)) else {
        return;
    };
    let slice = crate::hooks::read_pretool::slice_lines(&full, start, end);
    if slice.is_empty() || slice.trim_end() != delivered.trim_end() {
        return;
    }
    let hash = tokenade_core::dedup_ref::hash_content(slice.as_bytes());
    tokenade_core::dedup_ref::register(cwd, &hash, &slice);
    let end = start + slice.lines().count() as u64;
    tokenade_core::dedup_ref::register_read_range(
        &registry_session(skey),
        &file_key(&path),
        &version,
        start,
        end,
        &hash,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_single_file_range_is_a_read() {
        let d = std::env::temp_dir();
        let f = d.join("tokenade-range-read-probe.txt");
        std::fs::write(&f, "a\nb\nc\n").unwrap();
        let p = f.display().to_string();
        assert_eq!(
            sed_range_read(&format!("sed -n '2,3p' {p}"), &d),
            Some((f.clone(), 1, 3))
        );
        assert_eq!(
            sed_range_read(&format!("sed -n 2,3p {p}"), &d),
            Some((f.clone(), 1, 3))
        );
        for c in [
            format!("sed -n '2,3p' {p} | head"),
            format!("sed -n '2p' {p}"),
            format!("sed -n '3,2p' {p}"),
            format!("sed -i '2,3p' {p}"),
            format!("sed -n '2,3p' {p} {p}"),
        ] {
            assert_eq!(sed_range_read(&c, &d), None, "{c}");
        }
        let _ = std::fs::remove_file(&f);
    }
}
