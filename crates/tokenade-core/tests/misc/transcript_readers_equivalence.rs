//! The five Claude Code transcript readers must produce, from the shared
//! per-file cache, exactly what they produced when each opened every file for
//! itself.
//!
//! The corpus here is synthetic and fixed, because the real one is being
//! appended to while any experiment runs: comparing two dashboards a minute
//! apart drifts by a dozen lines on ledger churn alone, which cannot
//! distinguish "unchanged" from "slightly wrong". Every branch the old readers
//! had is represented — duplicated usage, a `user` record with and without a
//! `tool_result`, a record with no `cwd`, a record whose `usage` is not an
//! object, a `<synthetic>` model, a tokenade-namespaced tool, and timestamps
//! either side of a cutoff.
//!
//! Lives in its own integration binary: it redirects `HOME`, and doing that in
//! the lib test binary races every other test that reads a store.

use std::path::{Path, PathBuf};

fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "tokenade-treq-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_corpus(home: &Path) -> PathBuf {
    let proj = home.join(".claude").join("projects").join("-w-demo");
    std::fs::create_dir_all(&proj).unwrap();
    let f = proj.join("sess-one.jsonl");
    let lines = [
        // Assistant with usage + a tool_use that a later tool_result names.
        r#"{"type":"assistant","timestamp":"2026-01-01T10:00:00Z","cwd":"/w/a","message":{"id":"m1","model":"claude-x","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":7,"cache_read_input_tokens":3},"content":[{"type":"tool_use","id":"t1","name":"Bash"}]}}"#,
        // The SAME response repeated, as Claude Code writes it: must be counted once.
        r#"{"type":"assistant","timestamp":"2026-01-01T10:00:01Z","cwd":"/w/a","message":{"id":"m1","model":"claude-x","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":7,"cache_read_input_tokens":3},"content":[]}}"#,
        // A user record carrying the tool_result for t1.
        r#"{"type":"user","timestamp":"2026-01-01T10:00:02Z","cwd":"/w/a","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"aaaa bbbb cccc dddd eeee"}]}}"#,
        // A user record with no tool_result: must not create a session bucket.
        r#"{"type":"user","timestamp":"2026-01-01T10:00:03Z","cwd":"/w/zzz","message":{"content":[{"type":"text","text":"hello"}]}}"#,
        // Same DAY as the cutoff but before it to the second, with a cwd and
        // real usage: the window is compared at full ISO precision, so this
        // must vanish under the windowed rendering and stay under the full
        // one. A cache keyed by day rather than by timestamp would keep it.
        r#"{"type":"assistant","timestamp":"2026-01-02T10:59:59Z","cwd":"/w/a","message":{"id":"m9","model":"claude-y","usage":{"input_tokens":4242,"output_tokens":11},"content":[]}}"#,
        // No cwd: no honest bucket, but still model usage.
        r#"{"type":"assistant","timestamp":"2026-01-02T11:00:00Z","message":{"id":"m2","model":"claude-y","usage":{"input_tokens":100,"output_tokens":50},"content":[]}}"#,
        // usage is not an object: token accounting must skip it.
        r#"{"type":"assistant","timestamp":"2026-01-02T11:00:01Z","cwd":"/w/a","message":{"id":"m3","model":"claude-y","usage":null,"content":[]}}"#,
        // `<synthetic>` carries no real API usage.
        r#"{"type":"assistant","timestamp":"2026-01-02T11:00:02Z","cwd":"/w/a","message":{"id":"m4","model":"<synthetic>","usage":{"input_tokens":999,"output_tokens":999},"content":[]}}"#,
        // A tokenade-namespaced tool: bucketed, but out of the comparable total.
        r#"{"type":"assistant","timestamp":"2026-01-03T12:00:00Z","cwd":"/w/b","message":{"id":"m5","model":"claude-x","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"t2","name":"mcp__tokenade__map"}]}}"#,
        r#"{"type":"user","timestamp":"2026-01-03T12:00:01Z","cwd":"/w/b","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"ffff gggg hhhh"}]}]}}"#,
        // A tool_result whose tool_use was never seen: "unknown".
        r#"{"type":"user","timestamp":"2026-01-03T12:00:02Z","cwd":"/w/b","message":{"content":[{"type":"tool_result","tool_use_id":"nope","content":"iiii jjjj"}]}}"#,
        // An EMPTY tool_result: zero tokens. `tool_tokens_by_day` drops it
        // rather than opening a day bucket for nothing, and `aggregate_since`
        // still counts the (zero) contribution against its tool.
        r#"{"type":"user","timestamp":"2026-01-04T09:00:00Z","cwd":"/w/b","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":""}]}}"#,
    ];
    std::fs::write(&f, format!("{}\n", lines.join("\n"))).unwrap();
    home.to_path_buf()
}

/// One printable rendering of all five readers, so a single string comparison
/// covers every one of them.
fn render(home: &Path, since: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let mut agg = tokenade_core::claudecode_transcript::aggregate_since(home, since);
    agg.sort_by(|a, b| a.cwd.cmp(&b.cwd));
    for p in &agg {
        let mut tools: Vec<_> = p.tool_result_tokens_by_tool.iter().collect();
        tools.sort();
        let _ = writeln!(
            s,
            "agg cwd={} sessions={} msgs={} in={} out={} cc={} cr={} trt={} tools={:?} last={}",
            p.cwd,
            p.sessions,
            p.assistant_messages,
            p.input_tokens,
            p.output_tokens,
            p.cache_creation_tokens,
            p.cache_read_tokens,
            p.tool_result_tokens,
            tools,
            p.last_ts
        );
    }
    let _ = writeln!(
        s,
        "models={:?}",
        tokenade_core::claudecode_transcript::model_usage_by_day(home, since)
    );
    let _ = writeln!(
        s,
        "cachecreate={:?}",
        tokenade_core::claudecode_transcript::cache_creation_by_day(home, since)
    );
    let mut tt: Vec<_> = tokenade_core::claudecode_transcript::tool_tokens_by_day(home, since)
        .into_iter()
        .collect();
    tt.sort();
    let _ = writeln!(s, "tooltokens={tt:?}");
    let mut sm = tokenade_core::claudecode_transcript::session_models(home, since);
    sm.sort_by_key(|a| (a.sess.clone(), a.model.clone()));
    for m in &sm {
        let _ = writeln!(
            s,
            "sessmodel sess={} model={} start={} end={}",
            m.sess, m.model, m.start_ms, m.end_ms
        );
    }
    s
}

/// The expected rendering, captured by running the SAME fixture through the
/// readers as they were before the shared cache existed. Any divergence here
/// is a number the dashboard would now report differently.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The expected renderings were captured by running this exact fixture through
/// the readers as they were BEFORE the shared cache existed, with
/// `TOKENADE_CAPTURE_TRANSCRIPT_FIXTURE=1`. Any divergence is a number the
/// dashboard would now report differently.
fn expected(name: &str, actual: &str) -> String {
    let p = fixture(name);
    if std::env::var("TOKENADE_CAPTURE_TRANSCRIPT_FIXTURE").is_ok() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, actual).unwrap();
    }
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("fixture {name} unreadable: {e}"))
}

#[test]
fn the_five_readers_agree_with_the_pre_cache_numbers() {
    let home = scratch();
    write_corpus(&home);
    // No env override needed: the cache lives under the home it describes,
    // which is the scratch one. That property is worth asserting here, since
    // getting it wrong meant the suite read and wrote the developer's cache.
    assert!(
        tokenade_core::transcript_cache::cache_path_for(&home).starts_with(&home),
        "the cache is not stored beside the corpus it describes"
    );

    let full = render(&home, None);
    let want_full = expected("transcript_readers_expected.txt", &full);
    assert_eq!(full, want_full, "the unwindowed numbers moved");

    // A cutoff mid-corpus: the window is compared at FULL ISO precision, which
    // is why the cache stores timestamps rather than days.
    let windowed = render(&home, Some("2026-01-02T11:00:01Z"));
    assert_eq!(
        windowed,
        expected("transcript_readers_expected_since.txt", &windowed),
        "the windowed numbers moved"
    );

    // Reading again must not change anything: a warm cache is the path every
    // run after the first takes, and it is the one that could silently differ.
    assert_eq!(
        render(&home, None),
        want_full,
        "the warm cache disagrees with the cold one"
    );

    let _ = std::fs::remove_dir_all(&home);
}
