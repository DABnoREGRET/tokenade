// The tests of `crate::claudecode_transcript`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `claudecode_transcript`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod ts_tests {
    use super::{days_from_civil, iso8601_to_epoch_ms};

    #[test]
    fn epoch_zero_and_seconds() {
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00.123Z"), Some(123));
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00.1Z"), Some(100));
    }

    #[test]
    fn space_separator_and_naive() {
        assert_eq!(iso8601_to_epoch_ms("1970-01-01 00:00:00Z"), Some(0));
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00"), Some(0)); // naive = UTC
    }

    #[test]
    fn timezone_offset_shifts_to_utc() {
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(
            iso8601_to_epoch_ms("1970-01-01T00:00:00-01:00"),
            Some(3_600_000)
        );
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T02:30:00+0230"), Some(0)); // no colon
    }

    #[test]
    fn roundtrips_known_dates() {
        for s in ["1970-01-01", "2000-02-29", "2026-07-04", "1999-12-31"] {
            let ms = iso8601_to_epoch_ms(&format!("{s}T00:00:00Z")).unwrap();
            assert_eq!(&crate::platform::epoch_to_iso8601_z(ms / 1000)[..10], s);
        }
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(iso8601_to_epoch_ms(""), None);
        assert_eq!(iso8601_to_epoch_ms("garbage"), None);
        assert_eq!(iso8601_to_epoch_ms("2026-13-01T00:00:00Z"), None); // month
        assert_eq!(iso8601_to_epoch_ms("2026-07-04T25:00:00Z"), None); // hour
        assert_eq!(iso8601_to_epoch_ms("2026-07-04X00:00:00Z"), None); // separator
        assert_eq!(iso8601_to_epoch_ms("20x6-07-04T00:00:00Z"), None); // non-digit
    }

    #[test]
    fn days_from_civil_is_civil_from_days_inverse() {
        for z in [-1000i64, 0, 1, 20_601, 30_000] {
            let iso = crate::platform::epoch_to_iso8601_z((z.max(0) as u64) * 86_400);
            let (y, m, d) = (
                iso[0..4].parse().unwrap(),
                iso[5..7].parse().unwrap(),
                iso[8..10].parse().unwrap(),
            );
            if z >= 0 {
                assert_eq!(days_from_civil(y, m, d), z);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch home, and the env lock for the test's whole life: the
    /// readers resolve `CLAUDE_CONFIG_DIR` before `home`, and a test elsewhere
    /// in the crate (`detect_tests`, under `ScratchHome`) sets it — taken
    /// unlocked, the transcripts were looked for in that test's directory.
    fn tmp_home(label: &str) -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>) {
        let g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        (tmp_home_unlocked(label), g)
    }

    fn tmp_home_unlocked(label: &str) -> std::path::PathBuf {
        // Sweeps every stale `tokenade-*` sibling, not just this prefix — see
        // `testtmp`. This helper was the largest single leak on the machine
        // where it was measured (5 182 MB over 259 abandoned copies), which is
        // why it is the first to adopt.
        crate::testtmp::sweep_stale_scratch_dirs();
        let d = std::env::temp_dir().join(format!(
            "tokenade-claudecode-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// The cheap pre-parse filter is a SUPERSET test, and this holds it to
    /// that: every record shape that counts must survive it, including the
    /// ones written with the whitespace and key order a hand-rolled needle
    /// would miss. A filter that is merely *usually* right silently
    /// UNDERCOUNTS, which is the one direction a savings report must never
    /// fail in.
    #[test]
    fn the_cheap_prefilter_moves_no_number() {
        let (home, _env) = tmp_home("prefilter");
        // Same records twice: once compact, once with spaces after the colons
        // and the `type` key last. Both must be counted.
        let compact_asst = r#"{"type":"assistant","timestamp":"2026-08-31T10:00:00Z","cwd":"/p","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let spaced_asst = r#"{ "timestamp" : "2026-08-31T10:00:01Z" , "cwd" : "/p" , "message" : { "content" : [ { "type" : "tool_use" , "id" : "t2" , "name" : "Bash" } ] , "usage" : { "input_tokens" : 10 , "output_tokens" : 5 } } , "type" : "assistant" }"#;
        let compact_res = r#"{"type":"user","timestamp":"2026-08-31T10:00:02Z","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"aaaa bbbb cccc dddd"}]}}"#;
        let spaced_res = r#"{ "timestamp" : "2026-08-31T10:00:03Z" , "cwd" : "/p" , "message" : { "content" : [ { "tool_use_id" : "t2" , "content" : "aaaa bbbb cccc dddd" , "type" : "tool_result" } ] } , "type" : "user" }"#;
        // Shapes that must NOT count, and are exactly what the filter drops.
        let noise = r#"{"type":"summary","summary":"a chat about nothing","leafUuid":"x"}"#;
        let plain_user = r#"{"type":"user","timestamp":"2026-08-31T10:00:04Z","cwd":"/p","message":{"content":[{"type":"text","text":"hello"}]}}"#;
        write_session(
            &home,
            "p",
            "s.jsonl",
            &[
                compact_asst,
                spaced_asst,
                compact_res,
                spaced_res,
                noise,
                plain_user,
                "not json at all",
            ],
        );
        let rows = aggregate(&home);
        let row = rows
            .iter()
            .find(|r| r.cwd == "/p")
            .unwrap_or_else(|| panic!("no /p row in {rows:?}"));
        // Both tool_use records were seen: the spaced one carries `"assistant"`
        // with the key last, which is precisely the case a naive
        // `"type":"assistant"` needle would drop.
        assert_eq!(
            row.tools.total(),
            2,
            "a record written with spaces after its colons was dropped: {row:?}"
        );
        // Both tool_results were summed, and the plain user message was not.
        assert!(
            row.tool_result_tokens > 0,
            "no tool_result tokens counted: {row:?}"
        );
        let one = row.tool_result_tokens / 2;
        assert!(
            one > 0 && row.tool_result_tokens == one * 2,
            "the two identical tool_results did not contribute equally: {} \
             — one of the two shapes was skipped",
            row.tool_result_tokens
        );
    }

    fn write_session(home: &Path, proj_dir: &str, file: &str, lines: &[&str]) {
        let dir = home.join(".claude").join("projects").join(proj_dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), lines.join("\n")).unwrap();
    }

    /// The fused `digest` must be the five readers, not merely close to them.
    ///
    /// It replaced five separate walks of the corpus with one loop, and every
    /// number the dashboard shows comes out of it. The five original readers
    /// are still here and still correct, so the cheapest possible guard is to
    /// run both and demand equality — on a corpus carrying the shapes that
    /// make the readers disagree if the fusion drifts: a duplicate message id
    /// (each reader dedups on its own scope), a record on the cutoff boundary,
    /// a tool_use/tool_result pair split across records, two session files,
    /// and a `<synthetic>` model the model panel must drop.
    #[test]
    fn fused_digest_equals_the_five_readers() {
        let (home, _env) = tmp_home("digestfuse");
        let a = |id: &str, ts: &str, model: &str, inp: u64, out: u64, cc: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","cwd":"/w/one","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{inp},"output_tokens":{out},"cache_creation_input_tokens":{cc},"cache_read_input_tokens":0}},"content":[{{"type":"tool_use","id":"tu_{id}","name":"Bash"}}]}}}}"#
            )
        };
        let r = |ts: &str, use_id: &str, body: &str| {
            format!(
                r#"{{"type":"user","timestamp":"{ts}","cwd":"/w/one","message":{{"content":[{{"type":"tool_result","tool_use_id":"{use_id}","content":"{body}"}}]}}}}"#
            )
        };
        write_session(
            &home,
            "-w-one",
            "sess-a.jsonl",
            &[
                &a("m1", "2026-07-01T10:00:00Z", "claude-opus-5", 100, 20, 40),
                &r("2026-07-01T10:00:01Z", "tu_m1", "aaaa bbbb cccc dddd"),
                // Same id again: every reader's dedup must drop it identically.
                &a("m1", "2026-07-01T11:00:00Z", "claude-opus-5", 100, 20, 40),
                &a("m2", "2026-07-02T09:00:00Z", "<synthetic>", 5, 5, 0),
                &a("m3", "2026-07-02T09:30:00Z", "claude-sonnet-5", 70, 10, 0),
                &r("2026-07-02T09:31:00Z", "tu_m3", "eeee ffff"),
                // Result whose tool_use lives in the OTHER file: "unknown".
                &r("2026-07-02T09:32:00Z", "tu_zzz", "gggg hhhh"),
            ],
        );
        write_session(
            &home,
            "-w-two",
            "sess-b.jsonl",
            &[
                &a("m4", "2026-06-30T23:59:59Z", "claude-opus-5", 11, 3, 7),
                &a("m5", "2026-07-03T08:00:00Z", "claude-opus-5", 12, 4, 0),
                &r("2026-07-03T08:00:05Z", "tu_m5", "iiii jjjj kkkk"),
            ],
        );

        for cutoff in [
            None,
            Some("2026-07-01T10:00:00Z"),
            Some("2026-07-02T09:31:00Z"),
        ] {
            let d = digest(&home, cutoff);
            assert_eq!(
                d.models_by_day,
                model_usage_by_day(&home, cutoff),
                "models {cutoff:?}"
            );
            assert_eq!(
                d.tools_by_day,
                tool_tokens_by_day(&home, cutoff),
                "tools {cutoff:?}"
            );
            assert_eq!(
                d.cache_creation_by_day,
                cache_creation_by_day(&home, cutoff),
                "cache_creation {cutoff:?}"
            );
            assert_eq!(
                d.projects.len(),
                aggregate_since(&home, cutoff).len(),
                "projects {cutoff:?}"
            );
            for (got, want) in d.projects.iter().zip(aggregate_since(&home, cutoff)) {
                assert_eq!(got.cwd, want.cwd);
                assert_eq!(
                    got.tool_result_tokens_by_tool,
                    want.tool_result_tokens_by_tool
                );
            }
            // The two lifetime tables ignore the cutoff by design.
            assert_eq!(
                d.lifetime_models_by_day,
                model_usage_by_day(&home, None),
                "lifetime models {cutoff:?}"
            );
            let mut got = d
                .session_models
                .iter()
                .map(|s| (s.sess.clone(), s.model.clone(), s.start_ms, s.end_ms))
                .collect::<Vec<_>>();
            let mut want = session_models(&home, None)
                .iter()
                .map(|s| (s.sess.clone(), s.model.clone(), s.start_ms, s.end_ms))
                .collect::<Vec<_>>();
            got.sort();
            want.sort();
            assert_eq!(got, want, "session_models {cutoff:?}");
        }
        // Not a vacuous pass: the corpus really does carry all of it.
        let d = digest(&home, None);
        assert!(
            d.models_by_day.len() >= 3,
            "days: {:?}",
            d.models_by_day.keys()
        );
        assert!(d.tools_by_day.values().any(|t| t.contains_key("Bash")));
        assert!(d.tools_by_day.values().any(|t| t.contains_key("unknown")));
        assert!(!d.session_models.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn model_usage_by_day_aggregates_and_skips_synthetic() {
        let (home, _env) = tmp_home("modelday");
        write_session(
            &home,
            "p1",
            "s1.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-06-09T10:00:00Z","cwd":"/p","message":{"model":"claude-opus-4-8","usage":{"input_tokens":100,"cache_creation_input_tokens":50,"output_tokens":20}}}"#,
                r#"{"type":"assistant","timestamp":"2026-06-09T11:00:00Z","cwd":"/p","message":{"model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":5}}}"#,
                r#"{"type":"assistant","timestamp":"2026-06-10T09:00:00Z","cwd":"/p","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":7,"output_tokens":3}}}"#,
                r#"{"type":"assistant","timestamp":"2026-06-10T09:05:00Z","cwd":"/p","message":{"model":"<synthetic>","usage":{"input_tokens":9,"output_tokens":9}}}"#,
                r#"{"type":"user","timestamp":"2026-06-10T09:06:00Z","cwd":"/p","message":{}}"#,
            ],
        );
        let m = model_usage_by_day(&home, None);
        assert_eq!(m.len(), 2);
        assert_eq!(m["2026-06-09"]["claude-opus-4-8"], (160, 25, 2));
        assert_eq!(m["2026-06-10"]["claude-haiku-4-5"], (7, 3, 1));
        assert!(!m["2026-06-10"].contains_key("<synthetic>"));
        // install-timestamp cutoff applies
        let m2 = model_usage_by_day(&home, Some("2026-06-10T00:00:00Z"));
        assert!(!m2.contains_key("2026-06-09"));
        assert!(m2.contains_key("2026-06-10"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn tool_tokens_by_day_buckets_per_tool_per_day_with_cutoff() {
        let (home, _env) = tmp_home("tooltokday");
        // tool_use (assistant) seeds id→name; tool_result (user) on the
        // SAME day charges body tokens to that tool. A second day gets a
        // separate bucket. Body token counts are tokenizer estimates, so
        // assert they're non-zero and partitioned by day, not exact.
        write_session(
            &home,
            "p1",
            "s1.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-06-09T10:00:00Z","cwd":"/p","message":{"content":[{"type":"tool_use","id":"a1","name":"Read"}]}}"#,
                r#"{"type":"user","timestamp":"2026-06-09T10:00:01Z","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"a1","content":"some file body text here"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-06-10T09:00:00Z","cwd":"/p","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash"}]}}"#,
                r#"{"type":"user","timestamp":"2026-06-10T09:00:01Z","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"command output line one\nline two"}]}}"#,
            ],
        );
        let m = tool_tokens_by_day(&home, None);
        assert_eq!(m.len(), 2, "two distinct days");
        assert!(m["2026-06-09"].get("Read").copied().unwrap_or(0) > 0);
        assert!(m["2026-06-10"].get("Bash").copied().unwrap_or(0) > 0);
        assert!(!m["2026-06-09"].contains_key("Bash"));
        // cutoff drops the earlier day
        let m2 = tool_tokens_by_day(&home, Some("2026-06-10T00:00:00Z"));
        assert!(!m2.contains_key("2026-06-09"));
        assert!(m2.contains_key("2026-06-10"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn session_output_tokens_sums_assistant_output_only() {
        let (home, _env) = tmp_home("sessout");
        let p = home.join("t.jsonl");
        fs::write(
            &p,
            [
                r#"{"type":"assistant","message":{"usage":{"output_tokens":20}}}"#,
                r#"{"type":"user","message":{"content":[]}}"#,
                r#"{"type":"assistant","message":{"usage":{"output_tokens":5}}}"#,
                r#"not json at all"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(session_output_tokens(&p), 25);
        // missing file → 0, never panics.
        assert_eq!(session_output_tokens(&home.join("nope.jsonl")), 0);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn batching_saved_credits_avoided_resends() {
        let (home, _env) = tmp_home("batchsv");
        let p = home.join("t.jsonl");
        let ra = "A".repeat(400);
        let rb = "B".repeat(120);
        let rc = "C".repeat(120);
        // K=3 batched tool_use → result i spared in (K-1-i) later turns:
        // saved = sa*2 + sb*1 + sc*0.
        fs::write(
            &p,
            [
                r#"{"type":"assistant","message":{"id":"msg_n","content":[{"type":"tool_use","id":"n"}]}}"#.to_string(),
                r#"{"type":"attachment","attachment":{"type":"hook_success","toolUseID":"n","stdout":"{\"additionalContext\":\"[tokenade] You've run several commands one-per-turn. When…\"}"}}"#.to_string(),
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a"},{"type":"tool_use","id":"b"},{"type":"tool_use","id":"c"}]}}"#.to_string(),
                format!(
                    r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"a","content":"{ra}"}},{{"type":"tool_result","tool_use_id":"b","content":"{rb}"}},{{"type":"tool_result","tool_use_id":"c","content":"{rc}"}}]}}}}"#
                ),
            ]
            .join("\n"),
        )
        .unwrap();
        let sa = crate::tokenizer::estimate(&ra) as u64;
        let sb = crate::tokenizer::estimate(&rb) as u64;
        // Right after a tokenade reminder: the share it caused at distance 1 (0.81).
        assert_eq!(
            batching_saved_tokens(&p),
            ((sa * 2 + sb) as f64 * 0.81).round() as u64
        );
        // A lone tool_use (K=1) is not batching → no credit.
        let p2 = home.join("single.jsonl");
        fs::write(
            &p2,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x"}]}}"#,
        )
        .unwrap();
        assert_eq!(batching_saved_tokens(&p2), 0);
        let _ = fs::remove_dir_all(&home);
    }

    /// The agent batches on its own; tokenade gets the share its LAST reminder
    /// caused, decreasing with distance, and nothing before any reminder.
    #[test]
    fn a_parallel_group_away_from_a_reminder_is_the_agent_s_own() {
        let (home, _env) = tmp_home("batchown");
        let p = home.join("t.jsonl");
        let r = "A".repeat(400);
        let call = |mid: &str, id: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"id":"{mid}","content":[{{"type":"tool_use","id":"{id}"}}]}}}}"#
            )
        };
        let result = |id: &str| {
            format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{r}"}}]}}}}"#
            )
        };
        let nudge = r#"{"type":"attachment","attachment":{"toolUseID":"n","stdout":"[tokenade] You've run several commands one-per-turn."}}"#.to_string();
        // No reminder at all.
        fs::write(
            &p,
            [call("m1", "a"), call("m1", "b"), result("a"), result("b")].join("\n"),
        )
        .unwrap();
        assert_eq!(batching_saved_tokens(&p), 0);
        // The share decreases with distance from the reminder, never to zero.
        let at = |gap: usize| {
            let mut lines = vec![call("m0", "n"), nudge.clone()];
            lines.extend((1..gap).map(|i| call(&format!("s{i}"), &format!("x{i}"))));
            lines.extend([call("mb", "a"), call("mb", "b"), result("a"), result("b")]);
            fs::write(&p, lines.join("\n")).unwrap();
            batching_saved_tokens(&p)
        };
        let near = at(1);
        assert_eq!(
            near,
            (crate::tokenizer::estimate(&r) as f64 * 0.81).round() as u64
        );
        assert_eq!(
            at(60),
            (crate::tokenizer::estimate(&r) as f64 * 0.22).round() as u64
        );
        assert!(at(300) > 0, "no turn limit: the plateau holds");
        let _ = fs::remove_dir_all(&home);
    }

    /// Claude Code's real shape: one record per `tool_use`, all carrying the
    /// same `message.id` and `usage`, each result in its own record. Two
    /// separate messages of one call each are NOT a batch.
    #[test]
    fn a_batch_written_one_record_per_call_is_still_a_batch() {
        let (home, _env) = tmp_home("batchsplit");
        let p = home.join("t.jsonl");
        let ra = "A".repeat(400);
        let rb = "B".repeat(120);
        let call = |mid: &str, id: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"id":"{mid}","usage":{{"output_tokens":9}},"content":[{{"type":"tool_use","id":"{id}"}}]}}}}"#
            )
        };
        let result = |id: &str, body: &str| {
            format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{body}"}}]}}}}"#
            )
        };
        let nudge = r#"{"type":"attachment","attachment":{"toolUseID":"n","stdout":"[tokenade] You've Read several files one-per-turn."}}"#.to_string();
        let lines = [
            call("msg_0", "n"),
            nudge,
            call("msg_1", "a"),
            call("msg_1", "b"),
            result("a", &ra),
            result("b", &rb),
            call("msg_2", "x"),
            result("x", &ra),
            call("msg_3", "y"),
            result("y", &ra),
        ];
        fs::write(&p, lines.join("\n")).unwrap();
        assert_eq!(
            batching_saved_tokens(&p),
            (crate::tokenizer::estimate(&ra) as f64 * 0.80).round() as u64
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn tool_breakdown_buckets_known_names() {
        let mut tb = ToolBreakdown::default();
        tb.bump("Read");
        tb.bump("Read");
        tb.bump("Grep");
        tb.bump("Bash");
        tb.bump("WebFetch");
        tb.bump("Edit");
        tb.bump("MultiEdit");
        tb.bump("mcp__tokenade__exec_compact");
        tb.bump("mcp__plugin_tokenade_tokenade__symbol_find");
        tb.bump("SomeRandomCustomTool");
        assert_eq!(tb.read, 2);
        assert_eq!(tb.grep, 1);
        assert_eq!(tb.bash, 1);
        assert_eq!(tb.web, 1);
        assert_eq!(tb.edit, 2);
        assert_eq!(tb.tokenade, 2);
        assert_eq!(tb.other, 1);
        assert_eq!(tb.total(), 10);
        assert_eq!(tb.bypass_eligible(), 5); // read*2 + grep + bash + web
    }

    #[test]
    fn tokenade_bucket_rejects_lookalike_names() {
        let mut tb = ToolBreakdown::default();
        // Real tokenade tools — should bucket as tokenade.
        tb.bump("mcp__tokenade__exec_compact");
        tb.bump("mcp__plugin_tokenade_tokenade__symbol_find");
        // Adversarial lookalike — would have matched the old
        // `name.contains("tokenade__")` check.
        tb.bump("mcp__rogue__tokenade__shim");
        // Adversarial: tokenade as the SECOND segment of an unrelated
        // server — must not bucket as tokenade.
        tb.bump("mcp__not_us_tokenade__something");
        assert_eq!(tb.tokenade, 2);
        // The two adversarial calls land in `other`.
        assert_eq!(tb.other, 2);
    }

    #[test]
    fn cutoff_filter_applies_to_tool_use_too() {
        let (home, _env) = tmp_home("cutoff-tools");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                // Pre-cutoff: tool_use should NOT be counted.
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","name":"Read","id":"a"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
                // Post-cutoff: tool_use IS counted.
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-05-29T00:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash","id":"b"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        let stats = aggregate_since(&home, Some("2026-03-01T00:00:00Z"));
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[0].tools.read, 0,
            "pre-cutoff tool_use must be filtered"
        );
        assert_eq!(stats[0].tools.bash, 1);
        assert_eq!(stats[0].tools.total(), 1);
    }

    #[test]
    fn install_timestamp_clamps_extreme_values() {
        let (home, _env) = tmp_home("install-clamp-low");
        let tdir = home.join(".tokenade");
        fs::create_dir_all(&tdir).unwrap();
        // 1970-01-01 — below the 2024 floor.
        fs::write(tdir.join("install.ts"), "100").unwrap();
        let ts = install_timestamp_epoch(&home).unwrap();
        assert!(
            ts >= 1_704_067_200,
            "should clamp to 2024-01-01 floor, got {ts}"
        );

        // Year 9999 — above the 2099 ceiling.
        let home2 = tmp_home_unlocked("install-clamp-high");
        let tdir2 = home2.join(".tokenade");
        fs::create_dir_all(&tdir2).unwrap();
        fs::write(tdir2.join("install.ts"), "253402300800").unwrap();
        let ts2 = install_timestamp_epoch(&home2).unwrap();
        assert!(
            ts2 <= 4_102_444_800,
            "should clamp to 2099 ceiling, got {ts2}"
        );
    }

    #[test]
    fn handles_multi_cwd_in_same_session_file() {
        let (home, _env) = tmp_home("multi-cwd");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/a","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","name":"Read","id":"a1"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
                r#"{"type":"assistant","cwd":"/b","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","name":"Bash","id":"b1"}],"usage":{"input_tokens":5,"output_tokens":15}}}"#,
                r#"{"type":"assistant","cwd":"/a","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","name":"Read","id":"a2"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        let stats = aggregate(&home);
        assert_eq!(stats.len(), 2, "two cwds → two rows");

        let a = stats.iter().find(|s| s.cwd == "/a").expect("/a bucket");
        assert_eq!(a.input_tokens, 11, "/a sums msg 1+3");
        assert_eq!(a.output_tokens, 21);
        assert_eq!(a.assistant_messages, 2);
        assert_eq!(a.tools.read, 2, "both /a messages had Read tool_use");
        assert_eq!(a.tools.bash, 0, "/a never used Bash");
        assert_eq!(a.sessions, 1, "same session file counts once per cwd");

        let b = stats.iter().find(|s| s.cwd == "/b").expect("/b bucket");
        assert_eq!(b.input_tokens, 5);
        assert_eq!(b.output_tokens, 15);
        assert_eq!(b.assistant_messages, 1);
        assert_eq!(b.tools.bash, 1);
        assert_eq!(b.tools.read, 0, "/b never used Read");
        assert_eq!(b.sessions, 1);
    }

    #[test]
    fn bump_recognises_legacy_supertokensaver_namespace() {
        // Regression: users who migrated from the
        // super-token-saver project name still have `mcp__supertokensaver__*`
        // tool calls in their transcripts. These DO route through
        // tokenade (via wrap-mcps) so they must count toward the
        // tokenade column, not the `other` bucket.
        let mut b = ToolBreakdown::default();
        b.bump("mcp__supertokensaver__exec_compact");
        b.bump("mcp__sts__skeleton");
        b.bump("mcp__tokenade__semantic_search");
        assert_eq!(b.tokenade, 3, "all three legacy/current names must count");
        assert_eq!(b.other, 0, "none should land in `other`");
    }

    /// Regression test: `tool_result_tokens` (the AGGREGATE used
    /// by the Projects bypass calc) must EXCLUDE responses from
    /// tokenade-namespace tools, because those are tokenade output, not
    /// compaction opportunities. The per-tool bucket still gets them
    /// (Functions section render filters tokenade at display time).
    /// Previously the parser summed every tool_result body
    /// unconditionally, inflating the Projects section's bypass figure
    /// by the amount of tokenade MCP response tokens.
    #[test]
    fn tool_result_tokens_excludes_tokenade_namespace_aggregate() {
        let (home, _env) = tmp_home("trt-738");
        // The strings in tool_result.content are sized so the tokenizer
        // gives a deterministic, easy-to-compare token count. We just
        // need: Read response > tokenade response, both non-trivial.
        let read_payload = "x".repeat(800); // ~200 tokens via 4-bytes/tok heuristic
        let tk_payload = "y".repeat(80); //  ~20 tokens
        let sessions = [
            // Assistant emits two tool_uses (no interpolation — to_string()
            // keeps clippy::useless_format happy).
            r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","name":"Read","id":"r1"},{"type":"tool_use","name":"mcp__tokenade__symbol_find","id":"t1"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string(),
            // User responds with both tool_results
            format!(
                r#"{{"type":"user","cwd":"/p","timestamp":"2026-01-01T00:00:02Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"r1","content":"{read_payload}"}},{{"type":"tool_result","tool_use_id":"t1","content":"{tk_payload}"}}]}}}}"#
            ),
        ];
        let session_refs: Vec<&str> = sessions.iter().map(|s| s.as_str()).collect();
        write_session(&home, "-p", "s.jsonl", &session_refs);

        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1);
        let row = &stats[0];
        // The Read bucket gets the Read response tokens.
        let read_tokens = *row
            .tool_result_tokens_by_tool
            .get("Read")
            .expect("Read bucket present");
        // The tokenade bucket gets the tokenade response tokens.
        let tk_tokens = *row
            .tool_result_tokens_by_tool
            .get("mcp__tokenade__symbol_find")
            .expect("tokenade MCP bucket present");
        assert!(read_tokens > 0 && tk_tokens > 0, "both buckets populated");

        // CRITICAL ASSERTION: aggregate equals Read only, not Read+tokenade.
        assert_eq!(
            row.tool_result_tokens, read_tokens,
            "aggregate must EXCLUDE tokenade-namespace responses (got {}, expected {})",
            row.tool_result_tokens, read_tokens
        );
        assert!(
            row.tool_result_tokens < read_tokens + tk_tokens,
            "aggregate must be strictly less than sum-with-tokenade",
        );
    }

    #[test]
    fn aggregates_tool_use_per_cwd() {
        let (home, _env) = tmp_home("tool-use");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"Read","id":"a"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","name":"Bash","id":"b"},{"type":"tool_use","name":"mcp__tokenade__exec_compact","id":"c"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].tools.read, 1);
        assert_eq!(stats[0].tools.bash, 1);
        assert_eq!(stats[0].tools.tokenade, 1);
        assert_eq!(stats[0].tools.total(), 3);
    }

    /// Claude Code writes ONE assistant response as several transcript entries
    /// carrying the same `message.id` and a byte-identical `usage`. Measured on
    /// 129 real transcripts: 31% of assistant entries were such repeats, and
    /// summing them inflated totals by +203.7%.
    #[test]
    fn one_response_split_across_entries_is_counted_once() {
        let (home, _env) = tmp_home("dedup-one-response");
        let u = r#""usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":5,"cache_read_input_tokens":900}"#;
        let mk = |id: &str, ts: &str, extra: &str| {
            format!(
                r#"{{"type":"assistant","cwd":"/p","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-4-8",{u},{extra}}}}}"#
            )
        };
        // One response, three entries: prose, more prose, then the tool call.
        let l1 = mk(
            "msg_same",
            "2026-07-31T10:00:00Z",
            r#""content":[{"type":"text","text":"thinking"}]"#,
        );
        let l2 = mk(
            "msg_same",
            "2026-07-31T10:00:00Z",
            r#""content":[{"type":"text","text":"more"}]"#,
        );
        let l3 = mk("msg_same", "2026-07-31T10:00:00Z", r#""content":[]"#);
        // A genuinely different response must still be counted.
        let l4 = mk("msg_other", "2026-07-31T10:01:00Z", r#""content":[]"#);
        write_session(
            &home,
            "p1",
            "s1.jsonl",
            &[l1.as_str(), l2.as_str(), l3.as_str(), l4.as_str()],
        );

        let stats = aggregate(&home);
        let p = stats.iter().find(|p| p.cwd == "/p").expect("project row");
        assert_eq!(p.input_tokens, 200, "two responses, not four entries");
        assert_eq!(p.output_tokens, 40);
        assert_eq!(p.cache_read_tokens, 1800);
        assert_eq!(p.assistant_messages, 2);

        let m = model_usage_by_day(&home, None);
        // fresh(100) + cache write(5) = 105 per response, twice.
        assert_eq!(m["2026-07-31"]["claude-opus-4-8"], (210, 40, 2));

        let cc = cache_creation_by_day(&home, None);
        assert_eq!(cc["2026-07-31"], 10, "cache writes must not triple either");
    }

    /// The server-reporting accumulator is a separate code path in this file and
    /// had the same bug: its totals become `output_lean`/`batching` events in
    /// `gain_estimated.jsonl`, which the reporter POSTs upstream.
    #[test]
    fn the_reported_output_estimate_is_deduped_too() {
        let same = r#"{"type":"assistant","message":{"id":"msg_x","usage":{"output_tokens":300},"content":[{"type":"text","text":"hello"}]}}"#;
        let other = r#"{"type":"assistant","message":{"id":"msg_y","usage":{"output_tokens":300},"content":[{"type":"text","text":"hello"}]}}"#;
        let (out_tripled, _) = estimate_from_lines(vec![same, same, same].into_iter());
        let (out_single, _) = estimate_from_lines(vec![same].into_iter());
        assert_eq!(
            out_tripled, out_single,
            "three entries of one response must report what one entry reports"
        );
        let (out_two, _) = estimate_from_lines(vec![same, other].into_iter());
        assert!(
            out_two > out_single,
            "two DISTINCT responses must still add up"
        );
    }

    #[test]
    fn aggregates_assistant_usage_by_cwd() {
        let (home, _env) = tmp_home("aggregate");
        write_session(
            &home,
            "-home-me-proj",
            "session-a.jsonl",
            &[
                r#"{"type":"summary","sessionId":"a"}"#,
                r#"{"type":"user","cwd":"/home/me/proj","timestamp":"2026-01-01T00:00:00Z"}"#,
                r#"{"type":"assistant","cwd":"/home/me/proj","timestamp":"2026-01-01T00:00:01Z","message":{"usage":{"input_tokens":10,"output_tokens":50,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}}}"#,
                r#"{"type":"assistant","cwd":"/home/me/proj","timestamp":"2026-01-01T00:00:02Z","message":{"usage":{"input_tokens":5,"output_tokens":25}}}"#,
            ],
        );

        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1);
        let p = &stats[0];
        assert_eq!(p.cwd, "/home/me/proj");
        assert_eq!(p.sessions, 1);
        assert_eq!(p.input_tokens, 15);
        assert_eq!(p.output_tokens, 75);
        assert_eq!(p.cache_creation_tokens, 100);
        assert_eq!(p.cache_read_tokens, 200);
        assert_eq!(p.assistant_messages, 2);
        assert_eq!(p.last_ts, "2026-01-01T00:00:02Z");
        assert_eq!(p.total_tokens(), 15 + 75 + 100);
    }

    #[test]
    fn merges_sessions_with_same_cwd_across_dirs() {
        let (home, _env) = tmp_home("merge");
        // Two encoded dirs both pointing at the same cwd happens when
        // Claude Code is upgraded and changes its encoding scheme.
        write_session(
            &home,
            "-old-encoding",
            "s1.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            ],
        );
        write_session(
            &home,
            "-new-encoding",
            "s2.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-02-01T00:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            ],
        );
        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1, "both dirs should fold into one cwd");
        assert_eq!(stats[0].sessions, 2);
        assert_eq!(stats[0].input_tokens, 11);
        assert_eq!(stats[0].output_tokens, 22);
        assert_eq!(stats[0].last_ts, "2026-02-01T00:00:00Z");
    }

    #[test]
    fn missing_home_returns_empty() {
        let (home, _env) = tmp_home("missing");
        // No .claude dir at all in the temp home.
        let stats = aggregate(&home);
        assert!(stats.is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped_without_panicking() {
        let (home, _env) = tmp_home("malformed");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                "not json at all",
                r#"{"type":"assistant","cwd":"/p","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#,
                r#"{"truncated mid-line"#,
            ],
        );
        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].input_tokens, 1);
        assert_eq!(stats[0].output_tokens, 2);
    }

    #[test]
    fn non_assistant_records_produce_no_bucket() {
        let (home, _env) = tmp_home("non-assistant");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"user","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}"#,
                r#"{"type":"attachment","cwd":"/p","timestamp":"2026-01-01T00:00:01Z"}"#,
            ],
        );
        let stats = aggregate(&home);
        // After the multi-cwd refactor we route per assistant message,
        // so a session with NO assistant output produces no bucket at
        // all — the dashboard would have had to filter zero-rows
        // anyway. Cleaner: never create them.
        assert!(stats.is_empty(), "no assistant messages → no row");
    }

    #[test]
    fn assistant_record_without_usage_still_counts_tools() {
        // A real corner case from production transcripts: the model
        // emits a tool_use but the platform truncates the usage block
        // on a tier-limit error. We still want to count the tool call
        // (the model "tried"), even if there's no token accounting.
        let (home, _env) = tmp_home("usage-missing");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","name":"Read","id":"a"}]}}"#,
            ],
        );
        let stats = aggregate(&home);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].input_tokens, 0);
        assert_eq!(stats[0].tools.read, 1);
        assert_eq!(stats[0].sessions, 1);
        // assistant_messages counts entries we *successfully* tallied
        // tokens for — 0 here because the usage block was missing.
        assert_eq!(stats[0].assistant_messages, 0);
    }

    #[test]
    fn since_filter_excludes_earlier_messages() {
        let (home, _env) = tmp_home("since-filter");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":100}}}"#,
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-05-29T00:00:00Z","message":{"usage":{"input_tokens":5,"output_tokens":5}}}"#,
            ],
        );
        let stats = aggregate_since(&home, Some("2026-03-01T00:00:00Z"));
        assert_eq!(stats.len(), 1);
        // Only the May message survives the cutoff.
        assert_eq!(stats[0].input_tokens, 5);
        assert_eq!(stats[0].output_tokens, 5);
        assert_eq!(stats[0].assistant_messages, 1);
    }

    #[test]
    fn since_none_is_equivalent_to_aggregate() {
        let (home, _env) = tmp_home("since-none");
        write_session(
            &home,
            "-x",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/p","timestamp":"2026-01-01T00:00:00Z","message":{"usage":{"input_tokens":7,"output_tokens":3}}}"#,
            ],
        );
        let baseline = aggregate(&home);
        let explicit = aggregate_since(&home, None);
        assert_eq!(baseline.len(), explicit.len());
        assert_eq!(baseline[0].total_tokens(), explicit[0].total_tokens());
    }

    #[test]
    fn epoch_to_iso8601_known_dates() {
        // 2026-05-29T04:13:00Z — example install timestamp.
        // Computed: (2026-1970)*365.25 + ... easier to just verify round-trip
        // against a known unix anchor:
        // 2026-01-01T00:00:00Z = 1767225600
        assert_eq!(
            crate::platform::epoch_to_iso8601_z(1_767_225_600),
            "2026-01-01T00:00:00Z"
        );
        // Unix epoch itself.
        assert_eq!(
            crate::platform::epoch_to_iso8601_z(0),
            "1970-01-01T00:00:00Z"
        );
        // 2000-01-01 leap year transition.
        assert_eq!(
            crate::platform::epoch_to_iso8601_z(946_684_800),
            "2000-01-01T00:00:00Z"
        );
    }

    #[test]
    fn install_timestamp_reads_existing_marker() {
        let (home, _env) = tmp_home("install-existing");
        let tdir = home.join(".tokenade");
        fs::create_dir_all(&tdir).unwrap();
        // 2024-06-01 — well within the [2024, 2099] clamp window.
        fs::write(tdir.join("install.ts"), "1717200000").unwrap();
        assert_eq!(install_timestamp_epoch(&home), Some(1_717_200_000));
    }

    #[test]
    fn install_timestamp_backfills_from_gainlog() {
        let (home, _env) = tmp_home("install-backfill-gain");
        let tdir = home.join(".tokenade");
        fs::create_dir_all(&tdir).unwrap();
        fs::write(
            tdir.join("gain.jsonl"),
            r#"{"ts":1717200000,"op":"x","before":1,"after":0}
{"ts":1717200050,"op":"y","before":2,"after":1}"#,
        )
        .unwrap();
        // First call backfills the marker AND returns the value.
        assert_eq!(install_timestamp_epoch(&home), Some(1_717_200_000));
        // install.ts now exists on disk. Note we persist the CLAMPED
        // value, so a future read returns the same number.
        let written = fs::read_to_string(tdir.join("install.ts")).unwrap();
        assert_eq!(written.trim(), "1717200000");
    }

    #[test]
    fn install_timestamp_falls_back_to_dir_mtime() {
        let (home, _env) = tmp_home("install-mtime");
        let tdir = home.join(".tokenade");
        fs::create_dir_all(&tdir).unwrap();
        // No install.ts, no gain.jsonl — only the dir exists.
        let ts = install_timestamp_epoch(&home).expect("dir mtime should yield a ts");
        // Sanity: should be in [year 2000, year 2200).
        assert!(ts > 946_684_800);
        assert!(ts < 7_258_118_400);
    }

    #[test]
    fn output_base_excludes_tool_use_share() {
        let text = "a".repeat(100);
        let input = "b".repeat(100);
        // 1000 output tokens, content ~half prose / half tool_use by bytes
        // (tool_use bytes = name "" + serialized input ""b*100"" ≈ 102).
        let mixed = format!(
            r#"{{"type":"assistant","message":{{"usage":{{"output_tokens":1000}},"content":[{{"type":"text","text":"{text}"}},{{"type":"tool_use","id":"t1","name":"","input":"{input}"}}]}}}}"#
        );
        let (out, _) = estimate_from_lines(std::iter::once(mixed.as_str()));
        assert!(out > 400 && out < 600, "prorated ~half, got {out}");

        // Pure tool_use turn: no prose/thinking to trim → base 0.
        let tool_only = format!(
            r#"{{"type":"assistant","message":{{"usage":{{"output_tokens":1000}},"content":[{{"type":"tool_use","id":"t2","name":"Bash","input":"{input}"}}]}}}}"#
        );
        let (out2, _) = estimate_from_lines(std::iter::once(tool_only.as_str()));
        assert_eq!(out2, 0, "pure tool_use credits no output-lean base");

        // Pure prose turn: full output_tokens counts.
        let text_only = r#"{"type":"assistant","message":{"usage":{"output_tokens":1000},"content":[{"type":"text","text":"hello"}]}}"#;
        let (out3, _) = estimate_from_lines(std::iter::once(text_only));
        assert_eq!(out3, 1000, "pure prose credits full base");
    }

    #[test]
    fn session_estimate_cached_caps_bytes_and_resumes_across_calls() {
        let (home, _env) = tmp_home("estimate-cap");
        let transcript = home.join("t.jsonl");
        let state = home.join("t.state");
        // One prose assistant line per turn, each ~1000 output_tokens. Pad the
        // text so a handful of lines exceed MAX_SCAN_BYTES_PER_CALL — the scan
        // must stop at the cap and finish over multiple calls, never in one.
        let pad = "x".repeat(2 * 1024 * 1024); // 2 MiB per line
        let line = format!(
            r#"{{"type":"assistant","message":{{"usage":{{"output_tokens":1000}},"content":[{{"type":"text","text":"{pad}"}}]}}}}"#
        );
        let lines: Vec<String> = (0..10).map(|_| line.clone()).collect(); // ~20 MiB > 16 MiB cap
        fs::write(&transcript, format!("{}\n", lines.join("\n"))).unwrap();
        let total_out: u64 = 10 * 1000;

        // First call cannot consume the whole file (cap < total size).
        let (out1, _) = session_estimate_cached(&transcript, &state);
        assert!(out1 < total_out, "first call is byte-capped, got {out1}");
        // Cursor advanced.
        let off1: u64 =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&state).unwrap())
                .unwrap()["off"]
                .as_u64()
                .unwrap();
        assert!(off1 > 0 && off1 < fs::metadata(&transcript).unwrap().len());

        // Subsequent calls resume from the cursor and converge to the total.
        let mut out = out1;
        for _ in 0..10 {
            out = session_estimate_cached(&transcript, &state).0;
        }
        assert_eq!(out, total_out, "converges to full total across turns");
    }

    #[test]
    fn session_estimate_cached_resets_when_transcript_shrinks() {
        let (home, _env) = tmp_home("estimate-reset");
        let transcript = home.join("t.jsonl");
        let state = home.join("t.state");
        let big = r#"{"type":"assistant","message":{"usage":{"output_tokens":500},"content":[{"type":"text","text":"hi"}]}}"#;
        fs::write(&transcript, format!("{big}\n{big}\n")).unwrap();
        let (out1, _) = session_estimate_cached(&transcript, &state);
        assert_eq!(out1, 1000);
        // Transcript shrinks (a `/resume` trim). off > len → reset + rescan the
        // smaller file. The bounded scan still returns the correct new total.
        fs::write(&transcript, format!("{big}\n")).unwrap();
        let (out2, _) = session_estimate_cached(&transcript, &state);
        assert_eq!(out2, 500, "reset rescan reflects the shrunken transcript");
    }

    #[test]
    fn install_timestamp_iso_matches_epoch_form() {
        let (home, _env) = tmp_home("install-iso");
        let tdir = home.join(".tokenade");
        fs::create_dir_all(&tdir).unwrap();
        fs::write(tdir.join("install.ts"), "1767225600").unwrap();
        assert_eq!(
            install_timestamp_iso(&home),
            Some("2026-01-01T00:00:00Z".into())
        );
    }

    #[test]
    fn sort_is_most_recent_first() {
        let (home, _env) = tmp_home("sort");
        write_session(
            &home,
            "-a",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/a","timestamp":"2026-01-01T00:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        write_session(
            &home,
            "-b",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/b","timestamp":"2026-03-01T00:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        write_session(
            &home,
            "-c",
            "s.jsonl",
            &[
                r#"{"type":"assistant","cwd":"/c","timestamp":"2026-02-01T00:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ],
        );
        let stats = aggregate(&home);
        let cwds: Vec<&str> = stats.iter().map(|p| p.cwd.as_str()).collect();
        assert_eq!(cwds, vec!["/b", "/c", "/a"]);
    }
}
#[cfg(test)]
mod usage_tests {
    use super::*;

    /// A transcript is a file another program wrote; tokenade only reads it.
    /// The day bucket is `ts[..10]`. The guard used to be `ts.len() < 10` — a
    /// BYTE length.
    /// A timestamp of four CJK characters is 12 bytes, clears the guard, and
    /// byte 10 lands inside the fourth character: slicing a `&str` there is a
    /// panic, and it takes down the whole usage report over one bad line in one
    /// file. Same byte-as-char class as the markdown compactors fixed on
    /// 2026-08-03.
    #[test]
    fn a_non_ascii_timestamp_does_not_take_down_the_usage_report() {
        let home = std::env::temp_dir().join(format!("tokenade-ts-utf8-{}", std::process::id()));
        let dir = home.join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&dir).unwrap();
        let bad = "{\"type\":\"assistant\",\"timestamp\":\"\u{65e5}\u{65e5}\u{65e5}\u{65e5}\",\"message\":{\"id\":\"m1\",\"model\":\"claude-x\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}";
        let good = "{\"type\":\"assistant\",\"timestamp\":\"2026-08-04T10:00:00Z\",\"message\":{\"id\":\"m2\",\"model\":\"claude-x\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}";
        std::fs::write(dir.join("s.jsonl"), format!("{bad}\n{good}\n")).unwrap();
        let got = model_usage_by_day(&home, None);
        assert_eq!(
            got.len(),
            1,
            "the unparseable day must be skipped, not bucketed: {got:?}"
        );
        // The malformed line is skipped; the well-formed one still counts.
        assert!(
            got.contains_key("2026-08-04"),
            "a neighbouring good record was lost: {got:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[cfg(test)]
mod native_turns_context {
    use super::*;

    /// A native group's spared calls would each have re-read the context the
    /// group was sent with: (k − 1) × context × the reminder's share.
    #[test]
    fn a_native_group_credits_the_context_its_spared_calls_would_have_read() {
        let call = |mid: &str, id: &str, ctx: u64| {
            format!(
                r#"{{"type":"assistant","message":{{"id":"{mid}","usage":{{"input_tokens":10,"cache_read_input_tokens":{},"output_tokens":5}},"content":[{{"type":"tool_use","id":"{id}"}}]}}}}"#,
                ctx - 10
            )
        };
        let nudge = r#"{"type":"attachment","attachment":{"toolUseID":"n","stdout":"[tokenade] You've run several commands one-per-turn."}}"#.to_string();
        let lines = [
            call("m0", "n", 50_000),
            nudge,
            call("m1", "a", 100_000),
            call("m1", "b", 100_000),
        ];
        let p = estimate_parts(lines.iter().map(|s| s.as_str()));
        assert_eq!(p.turns_context, 81_000);
        // No reminder before it: the agent's own habit, nothing credited.
        let p = estimate_parts(
            [call("m1", "a", 100_000), call("m1", "b", 100_000)]
                .iter()
                .map(|s| s.as_str()),
        );
        assert_eq!(p.turns_context, 0);
    }
}

#[cfg(test)]
mod session_timeline {
    use super::*;

    #[test]
    fn a_timeline_counts_each_call_once_and_dates_each_compaction() {
        let rec = |ts: &str, mid: &str, ctx: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{mid}","usage":{{"input_tokens":4,"cache_read_input_tokens":{}}}}}}}"#,
                ctx - 4
            )
        };
        let lines = [
            rec("2026-10-05T10:00:00Z", "m1", 1_000),
            rec("2026-10-05T10:00:00Z", "m1", 1_000),
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-10-05T10:00:05Z","compactMetadata":{"trigger":"auto","preTokens":668190}}"#.to_string(),
            rec("2026-10-05T10:00:09Z", "m2", 12_000),
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-05T10:00:10Z","message":{"id":"s1","usage":{"input_tokens":9}}}"#.to_string(),
        ];
        let mut tl = SessionTimeline::default();
        let mut last = String::new();
        extend_timeline(&mut tl, &mut last, lines.iter().map(|s| s.as_str()));
        let t0 = 1_791_194_400; // 2026-10-05T10:00:00Z
        assert_eq!(tl.calls, vec![(t0, 1_000), (t0 + 9, 12_000)]);
        assert_eq!(tl.compactions, vec![(t0 + 5, 668_190)]);
    }

    /// A Codex rollout: one call per new `total_tokens` (the event repeats),
    /// the context is `last_token_usage.input_tokens`, a compaction recorded
    /// both ways is one, and the window states the threshold.
    #[test]
    fn a_codex_rollout_gives_its_calls_compactions_and_threshold() {
        let tc = |ts: &str, total: u64, input: u64| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":1408}},"model_context_window":258400}}}}}}"#
            )
        };
        let lines = [
            tc("2026-10-05T10:00:00.003Z", 15_310, 15_246),
            tc("2026-10-05T10:00:01Z", 15_310, 15_246),
            r#"{"timestamp":"2026-10-05T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
            r#"{"timestamp":"2026-10-05T10:00:05Z","type":"compacted","payload":{"message":"summary"}}"#.to_string(),
            r#"{"timestamp":"2026-10-05T10:00:06Z","type":"event_msg","payload":{"type":"context_compacted"}}"#.to_string(),
            tc("2026-10-05T10:00:09Z", 30_000, 9_000),
        ];
        let mut tl = SessionTimeline::default();
        let mut last = String::new();
        extend_timeline(&mut tl, &mut last, lines.iter().map(|s| s.as_str()));
        let t0 = 1_791_194_400;
        assert_eq!(tl.calls, vec![(t0, 15_246), (t0 + 9, 9_000)]);
        assert_eq!(tl.compactions, vec![(t0 + 5, 0)]);
        assert_eq!(tl.compact_at, 232_560);
    }
}
