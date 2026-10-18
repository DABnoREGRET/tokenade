// The tests of `transcript_view`.
//
// Spliced in by `include!` from `transcript_view.rs`. Came from
// `claudecode_transcript_tests.rs` with their subject: a test file that
// stays behind its function becomes the test file of another module,
// which is what `compress_tests.rs` had ended up being for `skeleton`.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER documentation comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TWO_TURN: &str = r#"{"sessionId":"abc12345-deadbeef","cwd":"/tmp/p","type":"user","message":{"role":"user","content":"build me a thing"}}
{"sessionId":"abc12345-deadbeef","cwd":"/tmp/p","type":"assistant","message":{"role":"assistant","model":"claude-opus-4-7","content":[{"type":"text","text":"ok let me look"},{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/p/main.rs"}}]}}
{"sessionId":"abc12345-deadbeef","cwd":"/tmp/p","type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"fn main(){}"}]}}
"#;

    #[test]
    fn detects_real_transcript() {
        assert!(detect(SAMPLE_TWO_TURN));
        assert!(!detect("hello world"));
        assert!(!detect(r#"{"random":"json","no":"sessionId"}"#));
        assert!(!detect(r#"{"sessionId":"x","type":"weird-kind"}"#));
    }

    #[test]
    fn compact_extracts_role_text_tools_and_results() {
        let out = compact(SAMPLE_TWO_TURN);
        assert!(out.starts_with("[claude-transcript]"));
        assert!(out.contains("session=abc12345"));
        assert!(out.contains("cwd=/tmp/p"));
        assert!(out.contains("entries=3"));
        assert!(out.contains("model=claude-opus-4-7"));
        assert!(out.contains("user: build me a thing"));
        assert!(out.contains("assistant: ok let me look"));
        assert!(out.contains("tool: Read(file_path=\"/tmp/p/main.rs\")"));
        assert!(out.contains("tool_result: fn main(){}"));
    }

    #[test]
    fn compact_truncates_long_text() {
        // 1000-char message — must be truncated to ~240 + marker.
        let long = "a".repeat(1000);
        let line = format!(
            r#"{{"sessionId":"sx","cwd":"/x","type":"user","message":{{"role":"user","content":"{long}"}}}}"#
        );
        let out = compact(&line);
        assert!(out.contains("aaa")); // some of the head
        assert!(out.contains("…[+")); // truncation marker
        assert!(
            out.len() < line.len(),
            "compact ({}) should be shorter than raw JSONL line ({})",
            out.len(),
            line.len()
        );
    }

    #[test]
    fn compact_skips_malformed_lines() {
        let mixed = "not json at all\n{\"sessionId\":\"sx\",\"cwd\":\"/x\",\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n";
        let out = compact(mixed);
        assert!(out.contains("user: hi"));
        // Only 1 entry — malformed line didn't bump the counter.
        assert!(out.contains("entries=1"));
    }

    #[test]
    fn compact_produces_smaller_output_on_real_shape() {
        // A 20-turn synthetic transcript should compact to a small
        // fraction of its byte size. We don't need exact numbers,
        // just a sanity floor.
        let mut lines = Vec::new();
        for i in 0..20 {
            lines.push(format!(
                r#"{{"sessionId":"abcdef12345","cwd":"/tmp","type":"user","timestamp":"2026-05-29T00:00:00Z","requestId":"req-{i}","agentId":"a","gitBranch":"main","version":"0.1","entrypoint":"x","userType":"u","uuid":"u-{i}","parentUuid":null,"isSidechain":false,"promptId":"p","message":{{"role":"user","content":"turn {i}"}}}}"#
            ));
        }
        let raw = lines.join("\n");
        let out = compact(&raw);
        assert!(
            out.len() * 5 < raw.len(),
            "compact ({} chars) should be much smaller than raw ({})",
            out.len(),
            raw.len()
        );
    }

    #[test]
    fn a_tool_result_preview_keeps_its_verdict_at_the_end() {
        // A cargo test run: hundreds of `ok` lines, then the line the agent
        // acted on. Only the head showed `running 300 tests`.
        let mut body = String::from("running 300 tests\n");
        for i in 0..300 {
            body.push_str(&format!("test case_{i} ... ok\n"));
        }
        body.push_str("test result: FAILED. 299 passed; 1 failed");
        let block = serde_json::json!({"type": "tool_result", "content": body});
        let line = serde_json::json!({"sessionId": "s", "cwd": "/x", "type": "user",
            "message": {"role": "user", "content": [block]}}).to_string();
        let out = compact(&line);
        assert!(out.contains("running 300 tests"), "{out}");
        assert!(out.contains("test result: FAILED. 299 passed; 1 failed"), "{out}");
        assert!(out.contains("chars]…"), "{out}");
    }
}
