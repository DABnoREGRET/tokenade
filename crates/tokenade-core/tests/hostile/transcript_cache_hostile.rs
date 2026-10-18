//! Hostile states around the transcript cache: a cache that cannot be read,
//! a cache path that is not a file, a transcript that is not there — and the
//! one that only shows up under concurrency, a temp file whose name every
//! process shares.
use std::path::PathBuf;

fn scratch(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "tokenade-hostile-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn corpus(home: &std::path::Path) -> PathBuf {
    let p = home.join(".claude").join("projects").join("-x");
    std::fs::create_dir_all(&p).unwrap();
    let f = p.join("s.jsonl");
    std::fs::write(&f, "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"/w\",\"message\":{\"id\":\"a\",\"model\":\"m\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3},\"content\":[]}}\n").unwrap();
    f
}

#[test]
fn probe_hostile_caches() {
    use tokenade_core::transcript_cache as tc;
    for (label, bytes) in [
        (
            "random bytes",
            vec![0x1fu8, 0x8b, 0x42, 0x99, 0x00, 0xff, 0x13],
        ),
        ("truncated gzip", {
            let mut v = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 3];
            v.extend_from_slice(&[0xab, 0xcd]);
            v
        }),
        ("empty", Vec::new()),
        ("text", b"ceci n'est pas un cache".to_vec()),
    ] {
        let d = scratch("bad");
        let f = corpus(&d);
        let cp = tc::cache_path_for(&d);
        std::fs::create_dir_all(cp.parent().unwrap()).unwrap();
        std::fs::write(&cp, &bytes).unwrap();
        let mut c = tc::TranscriptCache::load(&d);
        let n = c.rows_for(&f).len();
        println!("  {label:20} -> {n} row(s), no panic");
        assert_eq!(
            n, 1,
            "{label}: the corpus was not reread after an unreadable cache"
        );
        c.save();
        // The real property: the unreadable cache is REPLACED. Otherwise every
        // run reparses the whole corpus silently, which is the defect the
        // cache exists to remove.
        let mut again = tc::TranscriptCache::load(&d);
        assert_eq!(again.rows_for(&f).len(), 1, "{label}: reread broken");
        assert_eq!(
            again.files_parsed(),
            0,
            "{label}: the unreadable cache was not replaced"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn probe_cache_path_is_a_directory() {
    use tokenade_core::transcript_cache as tc;
    let d = scratch("isdir");
    let f = corpus(&d);
    let cp = tc::cache_path_for(&d);
    std::fs::create_dir_all(&cp).unwrap(); // the cache path IS a directory
    let mut c = tc::TranscriptCache::load(&d);
    assert_eq!(c.rows_for(&f).len(), 1);
    c.save(); // must not panic
    println!("  path=directory -> no panic");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn probe_unreadable_transcript() {
    use tokenade_core::transcript_cache as tc;
    let d = scratch("noent");
    let missing = d
        .join(".claude")
        .join("projects")
        .join("-x")
        .join("absent.jsonl");
    let mut c = tc::TranscriptCache::load(&d);
    assert_eq!(c.rows_for(&missing).len(), 0);
    println!("  missing file -> 0 rows, no panic");
    let _ = std::fs::remove_dir_all(&d);
}

/// Two `tokenade` runs saving the cache at once must not interleave into one
/// temp file and rename a half-written one into place.
///
/// Made deterministic rather than raced: a DIRECTORY is planted at the fixed
/// name the old code used (`<path>.tmp`). An implementation that writes there
/// cannot, and saves nothing; the shared writer in `paths` names its temp file
/// `.{stem}.{pid}.{seq}.tmp` and is unaffected. So this fails exactly on the
/// implementation that would corrupt under concurrency, every run.
#[test]
fn saving_does_not_use_a_temp_name_every_process_shares() {
    use tokenade_core::transcript_cache as tc;
    let d = scratch("concurrent");
    let f = corpus(&d);
    let cp = tc::cache_path_for(&d);
    std::fs::create_dir_all(cp.parent().unwrap()).unwrap();
    let squatted = cp.with_extension("tmp");
    std::fs::create_dir_all(&squatted).unwrap();

    let mut c = tc::TranscriptCache::load(&d);
    assert_eq!(c.rows_for(&f).len(), 1);
    c.save();

    assert!(
        cp.exists(),
        "nothing was saved: the writer is using the one temp name every process shares"
    );
    // And what landed is loadable, so a second run costs nothing.
    let mut again = tc::TranscriptCache::load(&d);
    assert_eq!(again.rows_for(&f).len(), 1);
    assert_eq!(
        again.files_parsed(),
        0,
        "the saved cache is not usable, so every run re-reads the corpus"
    );
    assert!(squatted.is_dir(), "the squatted path was disturbed");
    let _ = std::fs::remove_dir_all(&d);
}
