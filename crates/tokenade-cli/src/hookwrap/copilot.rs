//! **The Copilot hook, which does not speak the same language.**
//!
//! Same job as the Bash hook — fold what is returned to the agent — but the
//! host has neither the same input contract nor the same way of declining.
//! Keeping it apart prevents a fix written for one from being misapplied to
//! the other.

use super::*;

pub(crate) fn cmd_hookwrap_copilot() -> i32 {
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        return 0;
    }
    {
        // Attribute the saving to Copilot in the ledger/dashboard.
        std::env::set_var("TOKENADE_AGENT", "copilot");
        if let Some((out, before, after)) = copilot_fold(&input) {
            if after < before {
                tokenade_core::gain::record(
                    "copilot",
                    "copilot-posttool",
                    "copilot-posttool",
                    tokenade_core::Savings::new(before, after),
                );
            }
            println!("{out}");
            return 0;
        }
    }
    // Redact-only replacement: fires when the fold declined (small output,
    // no net saving) but the result carries secrets.
    if let Some(out) = copilot_redact_only(&input) {
        println!("{out}");
    }
    0
}

/// Extract the model-visible text from a Copilot postToolUse payload —
/// shared by the fold and the redact-only fallback.
pub(crate) fn copilot_result_text(v: &serde_json::Value) -> Option<&str> {
    v["toolResult"]["textResultForLlm"]
        .as_str()
        .or_else(|| v["tool_result"]["text_result_for_llm"].as_str())
        .or_else(|| v["tool_response"].as_str())
        .or_else(|| v["tool_response"]["stdout"].as_str())
        .or_else(|| v["tool_response"]["output"].as_str())
}

/// SECURITY fallback of [`cmd_hookwrap_copilot`]: when the fold does not
/// replace the result, secrets must still not reach the model. Returns the
/// `modifiedResult` JSON with ONLY redaction applied, or `None` when the
/// text is already clean (the common case — pure passthrough).
pub(crate) fn copilot_redact_only(input: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    let text = copilot_result_text(&v)?;
    let safe = redact::redact_for_model(text);
    if safe == text {
        return None;
    }
    Some(
        serde_json::json!({
            "modifiedResult": { "resultType": "success", "textResultForLlm": safe }
        })
        .to_string(),
    )
}

/// Pure core of [`cmd_hookwrap_copilot`], testable without stdin. Given the
/// raw postToolUse input JSON, returns `Some((stdout_json, before_tok,
/// after_tok))` with the `modifiedResult` replacement when folding saved
/// tokens, or `None` (passthrough) for any shape we don't recognize or that
/// didn't shrink.
pub(crate) fn copilot_fold(input: &str) -> Option<(String, usize, usize)> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    // Native camelCase, then the VS Code-compatible snake_case, then the
    // Claude-compatible shapes in case Copilot routes those to the hook.
    let text = copilot_result_text(&v)?;
    if text.len() < 800 {
        return None; // too small to beat folding overhead
    }
    // Already folded upstream (PATH shim, a wrapped MCP, an earlier pass) →
    // passthrough. "Did it get shorter" is NOT the same question: measured
    // 2026-08-07, a second fold of a git log still shaved 54 bytes off a
    // summary and a third grew it back — the shrink test accepts a second
    // authority on the same payload, which is what the "one authority per
    // payload" rule exists to prevent.
    if tokenade_core::fold_marker::already_folded(text) {
        return None;
    }
    let before = tokenade_core::tokenizer::estimate(text);
    let folded = tokenade_core::auto::compact(text).1;
    if folded.is_empty() || folded.len() >= text.len() {
        return None; // no gain → passthrough
    }
    // SECURITY: the generic fold does not scrub — same print-site redact
    // as cmd_auto (#902) / cmd_filter (#904). Idempotent no-op when clean.
    let folded = redact::redact_for_model(&folded);
    // Recovery pointer, like every other channel that drops bytes. This hook
    // was the exception: the MCP proxy and the web batch both call
    // `attach_drop_recovery`, `wrap` calls it, and Copilot did not — so a
    // Copilot user saw "N lines elided" with no way back, which is visible
    // loss with no remedy rather than a saving. It also gives the fold a
    // marker, so the one-authority guard above can recognise our own output
    // downstream. Appends, never replaces: the fold itself is untouched.
    let dropped = before.saturating_sub(tokenade_core::tokenizer::estimate(&folded));
    let folded = crate::wrap::attach_drop_recovery(
        folded,
        text,
        dropped,
        before,
        &tokenade_core::disclose::SourceHint::Shell,
        "copilot-hook",
    );
    let after = tokenade_core::tokenizer::estimate(&folded);
    let out = serde_json::json!({
        "modifiedResult": { "resultType": "success", "textResultForLlm": folded }
    })
    .to_string();
    Some((out, before, after))
}

#[cfg(test)]
mod copilot_tests {
    use super::copilot_fold;

    fn big_log() -> String {
        let mut s = String::new();
        for i in 0..500 {
            s.push_str(&format!(
                "2026-07-04T10:00:{:02} INFO worker {i} status=ok latency={}ms\n",
                i % 60,
                i % 13
            ));
        }
        s.push_str("2026-07-04T10:08:00 ERROR checkout failed: payment declined");
        s
    }

    #[test]
    fn folds_native_camelcase_and_emits_modified_result() {
        let log = big_log();
        let input = serde_json::json!({
            "toolName": "Bash",
            "toolResult": { "resultType": "success", "textResultForLlm": log }
        })
        .to_string();
        let (out, before, after) = copilot_fold(&input).expect("should fold a big log");
        assert!(after < before, "tokens shrank");
        let ov: serde_json::Value = serde_json::from_str(&out).unwrap();
        let folded = ov["modifiedResult"]["textResultForLlm"].as_str().unwrap();
        assert!(folded.len() < log.len(), "folded shorter than raw");
        assert!(folded.contains("payment declined"), "needle preserved");
        assert_eq!(ov["modifiedResult"]["resultType"], "success");
    }

    #[test]
    fn snake_case_and_claude_shapes_also_parse() {
        let log = big_log();
        for input in [
            serde_json::json!({ "tool_result": { "text_result_for_llm": log } }).to_string(),
            serde_json::json!({ "tool_response": log }).to_string(),
        ] {
            assert!(
                copilot_fold(&input).is_some(),
                "shape should fold: {input:.40}"
            );
        }
    }

    #[test]
    fn passthrough_on_small_or_unknown() {
        assert!(copilot_fold("not json").is_none());
        assert!(copilot_fold(
            &serde_json::json!({"toolResult":{"textResultForLlm":"tiny"}}).to_string()
        )
        .is_none());
        assert!(copilot_fold(&serde_json::json!({"unrelated":1}).to_string()).is_none());
    }

    // SECURITY (#4-followup): the fold's output must be scrubbed, and a
    // small output that declines the fold must still get a redact-only
    // replacement — secrets are never a passthrough case.
    #[test]
    fn fold_and_redact_only_scrub_secrets() {
        use super::copilot_redact_only;
        let mut log = big_log();
        log.push_str("\nexport API_KEY=sk-ant-SECRET123\n");
        let input = serde_json::json!({
            "toolResult": { "resultType": "success", "textResultForLlm": log }
        })
        .to_string();
        let (out, _, _) = copilot_fold(&input).expect("big log folds");
        assert!(
            !out.contains("sk-ant-SECRET123"),
            "fold leaked the key: {out:.200}"
        );

        // Small output → fold declines → redact-only fallback fires.
        let small = serde_json::json!({
            "toolResult": { "resultType": "success",
                            "textResultForLlm": "Authorization: Bearer TOPSECRET456\nok" }
        })
        .to_string();
        assert!(copilot_fold(&small).is_none(), "below fold threshold");
        let red = copilot_redact_only(&small).expect("redact-only fires");
        assert!(!red.contains("TOPSECRET456"), "redact-only leaked: {red}");
        assert!(red.contains("<redacted>"), "marker present: {red}");
        // Clean output → no replacement at all (pure passthrough).
        let clean = serde_json::json!({
            "toolResult": { "resultType": "success", "textResultForLlm": "all good" }
        })
        .to_string();
        assert!(copilot_redact_only(&clean).is_none());
    }

    // The 2026-08-08 widening of `already_folded` gave `copilot_fold` a new
    // reason to decline: the result carries a tokenade banner. Declining must
    // not mean "unscrubbed" — the redact-only fallback runs right after, and
    // this pins the two together, because in `cmd_auto` the equivalent early
    // return printed the bytes raw and leaked a live key.
    #[test]
    fn a_banner_carrying_result_still_reaches_the_redact_only_fallback() {
        use super::{copilot_fold, copilot_redact_only};
        let secret = format!("sk-ant-api03-{}", "A".repeat(80));
        let text = format!(
            "[tokenade:git] 82 line(s)\nkey: {secret}\n{}",
            "filler line to clear the 800-byte floor\n".repeat(40)
        );
        let input = serde_json::json!({
            "toolResult": { "textResultForLlm": text }
        })
        .to_string();
        assert!(
            copilot_fold(&input).is_none(),
            "one authority per payload: a folded result must not be folded again"
        );
        let out = copilot_redact_only(&input).expect("redact-only must still fire");
        assert!(
            !out.contains(&secret),
            "declining to fold leaked the key: {out}"
        );
    }

    /// A command PreToolUse DECLINED for safety (`ssh`, `vim`, a TTY
    /// invocation) is never rewritten, so nothing folds its output. This path
    /// was computing the fold and writing it to a log. Measured on a realistic
    /// `sshpass -p … ssh -T host 'ps -eo pid,tty,stat,time,cmd'`: 31 779 bytes
    /// in, 155 out, `log_only`.
    #[test]
    fn a_declined_command_gets_its_fold_served() {
        let mut env = tokenade_core::platform::ScopedEnv::new();
        env.set("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        // The recovery pointer this test asserts comes from
        // `stash_for_reference`, which resolves the stash from process env.
        // Same omission once found in `tests/wrap.rs`, and the same order —
        // `ENV_LOCK` outermost, `ENV_RW_LOCK` inside it.
        let _rw = crate::tests::env_guard();
        let out = (0..400)
            .map(|i| {
                format!(
                    "  1234{i}  ?        Ss     0:0{} /usr/bin/python3 /srv/w.py --shard {i}",
                    i % 9
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let served = super::serve_declined_fold_for_test(
            "sshpass -p SECRET ssh -T -o StrictHostKeyChecking=no root@h 'ps -eo pid,tty,stat,time,cmd'",
            &out,
        )
        .expect("a declined command's fold is served");
        assert!(
            served.len() * 20 < out.len(),
            "served {} of {}",
            served.len(),
            out.len()
        );
        // The bytes that went away must still be reachable, by the same
        // mechanism `wrap` uses.
        assert!(
            served.contains("expand-ref"),
            "the fold is unrecoverable:\n{served}"
        );
    }

    /// Output that a first authority has ALREADY folded is not folded again
    /// here — the same rule, on the channel that carries the most traffic. The
    /// case is real on this path: an excluded command whose payload arrives
    /// through a bridge that already folded it.
    #[test]
    fn an_already_folded_payload_is_not_folded_twice() {
        let mut env = tokenade_core::platform::ScopedEnv::new();
        env.set("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        let out = (0..400)
            .map(|i| {
                format!("  1234{i}  ?        Ss     0:00 /usr/bin/python3 /srv/w.py --shard {i}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let once = super::serve_declined_fold_for_test("ssh -T root@h 'ps -eo pid,cmd'", &out)
            .expect("the first fold is served");
        assert!(
            super::serve_declined_fold_for_test("ssh -T root@h 'ps -eo pid,cmd'", &once).is_none(),
            "a second authority folded the same payload again:\n{once}"
        );
    }

    /// A command PreToolUse DOES rewrite is not this path's business: folding
    /// it here would be a second authority over the same payload.
    #[test]
    fn a_command_pretool_rewrites_is_left_alone() {
        let mut env = tokenade_core::platform::ScopedEnv::new();
        env.set("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        let out = (0..1500)
            .map(|i| format!("file_{i:04}.txt"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            super::serve_declined_fold_for_test("ls -1 /srv/data", &out).is_none(),
            "PreToolUse owns this command"
        );
    }

    /// And a host that ignores `updatedToolOutput` gets nothing, which is
    /// exactly what it got before — never a banner it would pay for twice.
    #[test]
    fn a_host_without_the_field_gets_no_replacement() {
        let mut env = tokenade_core::platform::ScopedEnv::new();
        env.set("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "0");
        let out = (0..400)
            .map(|i| {
                format!("  1234{i}  ?        Ss     0:00 /usr/bin/python3 /srv/w.py --shard {i}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            super::serve_declined_fold_for_test("ssh -T root@h 'ps -eo pid,cmd'", &out).is_none(),
            "a host that ignores the field must see no replacement"
        );
    }

    // SECURITY: the Bash PostToolUse redact-only replacement — secrets in
    // a NON-compacted tool_response (bypass / excluded / gated paths) are
    // replaced; clean outputs emit nothing; sibling fields survive.
    #[test]
    fn bash_posttool_redact_only_replacement() {
        let parsed = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "env"},
            "tool_response": {
                "stdout": "HOME=/root\nAWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n",
                "stderr": "",
                "interrupted": false
            }
        });
        let rc = super::emit_redacted_bash_output_if_needed(
            &parsed,
            parsed
                .pointer("/tool_response/stdout")
                .unwrap()
                .as_str()
                .unwrap(),
            "",
            std::time::Instant::now(),
        );
        assert_eq!(rc, Some(0), "secret stdout must trigger the replacement");

        let clean = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "a.txt\nb.txt\n", "stderr": ""}
        });
        let rc = super::emit_redacted_bash_output_if_needed(
            &clean,
            "a.txt\nb.txt\n",
            "",
            std::time::Instant::now(),
        );
        assert_eq!(rc, None, "clean output must stay untouched");
    }

    // The Bash channel now declines to fold an already-folded payload (one
    // authority per payload). Every previous widening of `already_folded` opened a hole
    // somewhere, so lock the ordering that keeps this one shut: the
    // redact-only replacement fires at the TOP of the dispatcher, long
    // before the fold gate, so a secret cannot ride in behind a fold
    // banner. If someone ever moves the gate above the scrub, this fails.
    #[test]
    fn a_fold_banner_cannot_smuggle_a_secret_past_the_bash_hook() {
        let secret = format!("AKIA{}", "H4KLNPQRSTUV2WXY");
        let folded = format!(
            "[tokenade] folded 4000 -> 900 bytes (recover: tokenade expand-ref deadbeefcafe)\n\
             deploy key: {secret}\n"
        );
        assert!(
            tokenade_core::fold_marker::already_folded(&folded),
            "fixture must actually look folded, else this test proves nothing"
        );
        let parsed = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "cat deploy.log"},
            "tool_response": {"stdout": folded, "stderr": ""}
        });
        let rc = super::emit_redacted_bash_output_if_needed(
            &parsed,
            parsed
                .pointer("/tool_response/stdout")
                .unwrap()
                .as_str()
                .unwrap(),
            "",
            std::time::Instant::now(),
        );
        assert_eq!(
            rc,
            Some(0),
            "an already-folded payload still has to go through the scrub"
        );
    }
}
