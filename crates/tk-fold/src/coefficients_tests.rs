// The tests of `crate::coefficients`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `coefficients`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

/// This module's credit builders, and which of them SAY where their
/// baseline comes from. Extracted so it can be checked on a known text: a guard that
/// has stopped finding what it checks passes silently.
#[cfg(test)]
mod unbounded_channel_tests {
    use super::*;

    /// An UNCAPPED channel is an unbounded claim, so the set of them is
    /// closed by name here rather than left to whatever `classify_channel`
    /// happens not to match.
    ///
    /// The self-bounded ones are legitimate: their `before` is already a
    /// per-agent credit base (`web_credit_savings` / `binary_credit_
    /// savings` / image downscale), and capping an already-bounded figure
    /// double-discounts it. Everything a tool result travels through must
    /// carry a finite cap.
    #[test]
    fn only_the_self_bounded_assiettes_go_uncapped() {
        for (source, op) in [
            ("cli", "html-to-markdown"),
            ("cli", "serp_compact"),
            ("cli", "extract-docx"),
            ("web", "batching"),
            ("search", "batching"),
        ] {
            assert_eq!(
                delivery_cap_tokens("claude-code", source, op),
                u64::MAX,
                "{source}/{op} is a self-bounded assiette and must stay uncapped"
            );
        }
        // Every channel that carries a real tool result. `hook` is the one
        // this guard was written for: the pretools record it and
        // `contains("hookwrap")` never matched it.
        for (source, op) in [
            ("hook", "session_resume_saved"),
            ("hook", "cd"),
            ("hook", "passthrough"),
            ("hook", "auto:json"),
            ("hookwrap", "generic"),
            ("hookwrap-pre-edit", "write_to_edit"),
            ("wrap", "disclose:stash"),
            ("cli", "digest_recovery"),
            ("cli", "auto-compact"),
            ("wrap-mcp", "mcp:list"),
        ] {
            let cap = delivery_cap_tokens("claude-code", source, op);
            assert!(
                cap < u64::MAX && cap > 0,
                "{source}/{op} carries no cap — an unbounded claim (got {cap})"
            );
        }
    }

    /// The hook channel is the BASH one: a shell command's stdout. Naming
    /// the value, not just "some cap", so widening `classify_channel` cannot
    /// quietly re-route it.
    #[test]
    fn the_hook_channel_is_the_bash_channel() {
        assert_eq!(
            classify_channel("hook", "cd"),
            (false, true, false),
            "source=hook is a shell command's stdout"
        );
        assert_eq!(
            delivery_cap_tokens("unknown-agent", "hook", "cd"),
            DEFAULT_CAP_BASH_TOKENS
        );
        // A hook row whose op IS read-like keeps the read channel: the
        // source widening must not out-vote the op.
        assert_eq!(
            classify_channel("hook", "read_content_dedup"),
            (true, false, false)
        );
    }
}
#[cfg(test)]
mod realistic_cap_tests {
    use super::*;

    /// The `tokenade read` channel must be classified READ, and the guard is
    /// on the value the ledger actually carries.
    ///
    /// The rule was written as `op.starts_with("auto:")` — but `auto:<kind>`
    /// is the ledger row's LABEL; the `op` on that path is the literal
    /// `auto-compact`. Counted on a real 33 817-row ledger 2026-08-08: the old
    /// test matched 0 rows, while 2 162 rows are `("cli", "auto-compact")`. So
    /// the whole channel — the one all six plugin bridges call — was
    /// discounted at the BASH cap, 1.92x under what the READ cap gives, and
    /// the server sums exactly that figure for the quota.
    ///
    /// The sibling test above says "auto:* → read" in its comment, which was
    /// true of the intent and never of any row.
    /// Every code-intel op the CLI actually writes must land on the READ cap.
    ///
    /// The list is the set of `op` literals passed to `log_code_intel` — read
    /// off the call sites, not invented. Two of them (`symbol_find`,
    /// `structure_map`) were classified BASH until 2026-08-08 because
    /// `classify_channel` named the COMMANDS (`query`, `map`) while the ledger
    /// carries the implementations. `impact_of` and `semantic_search` matched
    /// only by luck of naming.
    ///
    /// If a new code-intel op appears, add it here AND to `classify_channel`,
    /// or it silently gets the shell cap — 7 500 against 20 000.
    #[test]
    fn every_code_intel_op_is_read_classified() {
        for op in [
            "symbol_find",
            "structure_map",
            "impact_of",
            "semantic_search",
            "skeleton",
        ] {
            assert!(
                classify_channel("cli", op).0,
                "`{op}` is a Read stand-in and must be classified read"
            );
            assert_eq!(
                delivery_cap_tokens("", "cli", op),
                DEFAULT_CAP_READ_TOKENS,
                "`{op}` is discounted against the wrong cap"
            );
        }
    }

    #[test]
    fn the_cli_read_channel_is_classified_read_not_bash() {
        let (read_like, bash_like, mcp_like) = classify_channel("cli", "auto-compact");
        assert!(
            read_like,
            "`cli`/`auto-compact` is the `tokenade read` fold"
        );
        assert!(
            !bash_like && !mcp_like,
            "a cli row must claim exactly one channel"
        );
        assert_eq!(
            delivery_cap_tokens("", "cli", "auto-compact"),
            DEFAULT_CAP_READ_TOKENS,
            "the read fold must be discounted against the Read cap"
        );
        // The label-shaped spelling stays accepted: `wrap` writes `auto:json`
        // as an op, and older cli rows may too.
        assert!(classify_channel("cli", "auto:json").0);
        // A genuine shell fold on the same source stays BASH.
        let (r, b, _) = classify_channel("cli", "sysinfo");
        assert!(!r && b, "`cli`/`sysinfo` is shell output, not a read");
    }
    #[test]
    fn cli_channel_is_capped_and_self_bounded_ops_are_not() {
        // 0042-class hole (field: `auto:diff` 9.77M over 41 events, agent
        // unknown): `source="cli"` folds previously classified to NO channel
        // → u64::MAX cap → unbounded claims. Now: bash cap, auto:* → read.
        assert_eq!(delivery_cap_tokens("unknown", "cli", "diff-compact"), 7_500);
        assert_eq!(delivery_cap_tokens("unknown", "cli", "sysinfo"), 7_500);
        assert_eq!(delivery_cap_tokens("unknown", "cli", "auto:diff"), 20_000);
        // Self-bounded credit assiettes keep their honest figures.
        assert_eq!(
            delivery_cap_tokens("claude-code", "cli", "html-to-markdown"),
            u64::MAX
        );
        assert_eq!(
            delivery_cap_tokens("claude-code", "cli", "serp_compact"),
            u64::MAX
        );
        assert_eq!(
            delivery_cap_tokens("claude-code", "cli", "extract-pdf"),
            u64::MAX
        );
    }

    #[test]
    fn delivery_cap_picks_channel_by_source() {
        // channel detection: read-like sources/ops -> read cap; bash -> bash;
        // neither -> no cap (u64::MAX).
        assert_eq!(
            delivery_cap_tokens("claude-code", "hookwrap-postread", "read"),
            20_000
        );
        assert_eq!(delivery_cap_tokens("claude-code", "wrap", "grep"), 7_500);
        assert_eq!(delivery_cap_tokens("claude-code", "hookwrap", "cat"), 7_500);
        // op carries the channel when source is generic (`cli`/`hook`).
        assert_eq!(
            delivery_cap_tokens("claude-code", "cli", "skeleton"),
            20_000
        );
        assert_eq!(
            delivery_cap_tokens("claude-code", "hook", "read_content_dedup"),
            20_000
        );
        // 0042-class contract change: a cli op that is neither a read
        // stand-in nor a self-bounded assiette is a raw shell-output fold —
        // it now carries the bash cap instead of running unbounded.
        assert_eq!(
            delivery_cap_tokens("claude-code", "cli", "some-random-fold"),
            7_500
        );
        // Un-tied sources OUTSIDE the cli/wrap/mcp channels keep u64::MAX
        // (web/binary/image levers book an already-bounded credit assiette).
        assert_eq!(
            delivery_cap_tokens("claude-code", "web", "downscale"),
            u64::MAX
        );
    }

    #[test]
    fn delivery_cap_per_agent_defaults() {
        // Ex-`u64::MAX` non-truncating channels now get a FINITE conservative
        // per-agent cap (the over-count fix): a 50 M binary dump can't be
        // credited in full anymore.
        assert_eq!(delivery_cap_tokens("aider", "wrap", "grep"), 12_800); // full shell output
        assert_eq!(delivery_cap_tokens("cline", "cli", "read"), 20_000); // whole-file read
        assert_eq!(delivery_cap_tokens("gemini-cli", "wrap", "cat"), 12_800); // shell buffer
        assert_eq!(delivery_cap_tokens("grok", "cli", "read"), 12_800); // pi-lineage read

        // windsurf's shell is a CEILING read off the devin CLI's own tool text
        // ("output exceeds 30000 characters"), not the peer default it replaced.
        assert_eq!(delivery_cap_tokens("windsurf", "wrap", "cat"), 7_500);
        assert_eq!(delivery_cap_tokens("antigravity", "wrap", "cat"), 12_800);
        // NO per-agent channel is u64::MAX anymore.
        for a in [
            "aider",
            "cline",
            "gemini-cli",
            "grok",
            "windsurf",
            "antigravity",
        ] {
            assert_ne!(
                delivery_cap_tokens(a, "wrap", "cat"),
                u64::MAX,
                "{a} bash uncapped"
            );
            assert_ne!(
                delivery_cap_tokens(a, "cli", "read"),
                u64::MAX,
                "{a} read uncapped"
            );
        }
        // truncating harnesses get their source-audited caps.
        assert_eq!(delivery_cap_tokens("cursor", "cli", "read"), 2_500);
        assert_eq!(delivery_cap_tokens("opencode", "wrap", "ls"), 12_800);
        assert_eq!(delivery_cap_tokens("odysseus", "wrap", "cat"), 2_500);
    }

    /// Red before the fix. `channel_is_nontruncating` claimed gemini-cli's
    /// shell channel delivers the WHOLE output, citing gh discussion #8297
    /// ("auto-truncation is an opt-in preview"). The installed package says
    /// otherwise: `enableToolOutputTruncation ?? true` in config.js, and
    /// tool-executor.js applies `saveTruncatedContent` to every shell result.
    /// Claiming a truncating channel is non-truncating suppresses the overflow
    /// credit, so this UNDER-reported the gain on every gemini-cli shell fold.
    /// The inventory is EMPTY, and that is the point of it.
    ///
    /// It used to list three gaps between the measured module and this scalar
    /// table, each defensible on its own and the set indefensible together:
    /// two numbers with no stated relation, kept in step by a test whose whole
    /// job was to remember that somebody had meant it. Two were decisions
    /// (measured 25 000, credited 20 000) and are now written IN the module as
    /// `Cap::credited_tokens`, beside the measurement they override. One was a
    /// guess that a real measurement replaced.
    ///
    /// So the table no longer holds a second opinion: `agent_channel_default`
    /// DERIVES from `agent_caps` wherever a row yields a scalar. What is left
    /// here is a fallback for agents the module does not cover — and a
    /// disagreement is now impossible by construction rather than by vigilance,
    /// which is the difference this whole chantier was about.
    #[test]
    fn the_two_tables_cannot_disagree_because_there_is_only_one() {
        use crate::agent_caps::{cap, Action};
        let mut found: Vec<String> = Vec::new();
        for agent in crate::agent_caps::agents() {
            for (action, source, op) in [
                (Action::ReadFile, "hook", "read"),
                (Action::Command, "wrap", "cat"),
            ] {
                let Some(c) = cap(agent, action) else {
                    continue;
                };
                let Some(credited) = c.credit_tokens() else {
                    continue; // no scalar equivalent — nothing to compare
                };
                let scalar = delivery_cap_tokens(agent, source, op);
                if credited != scalar {
                    found.push(format!(
                        "{agent}/{} {credited} vs {scalar}",
                        action.as_str()
                    ));
                }
            }
        }
        assert!(
            found.is_empty(),
            "the scalar table stopped deriving from the module: {found:#?}"
        );
        // Anti-vacuity: the loop must actually have compared something, and a
        // decision must actually be in play — otherwise "no disagreement" would
        // be the trivially true statement of a comparison that never ran.
        assert!(crate::agent_caps::agents().len() >= 10);
        let read = cap("claude-code", Action::ReadFile).expect("claude-code read");
        assert!(
            read.credit_is_a_decision(),
            "the decision is still recorded"
        );
        assert_eq!(read.credit_tokens(), Some(20_000));
        assert_eq!(
            read.limit,
            crate::agent_caps::Limit::RefusedAbove { tokens: 25_000 }
        );
    }

    /// The two tables must never contradict each other about an agent they
    /// both know. This is the guard the gemini-cli drift did not have: it is
    /// not enough for each table to be internally consistent, because the
    /// product reads them BOTH and a disagreement silently picks one.
    #[test]
    fn the_measured_table_and_the_scalar_table_agree_on_truncation() {
        use crate::agent_caps::{cap, Action};
        for agent in crate::agent_caps::agents() {
            for (action, source, op) in [
                (Action::ReadFile, "hook", "read"),
                (Action::Command, "wrap", "cat"),
            ] {
                let Some(c) = cap(agent, action) else {
                    continue;
                };
                assert_eq!(
                    channel_nontruncating(agent, source, op),
                    c.limit.is_unbounded(),
                    "{agent}/{} disagrees between agent_caps and coefficients",
                    action.as_str()
                );
            }
        }
    }

    #[test]
    fn gemini_shell_truncates_by_default_and_is_credited_as_such() {
        assert!(
            !channel_nontruncating("gemini-cli", "wrap", "cat"),
            "gemini-cli truncates shell output by default (saveTruncatedContent)"
        );
        assert!(
            overflow_ratio_for_channel("gemini-cli", "wrap", "cat") > 0.0,
            "a truncated tail exists, so the recovery credit is earned"
        );
        // The module and the table must not disagree about the same channel.
        let cap = crate::agent_caps::cap("gemini-cli", crate::agent_caps::Action::Command)
            .expect("gemini-cli command cap");
        assert!(
            !cap.limit.is_unbounded(),
            "agent_caps and coefficients must tell the same story"
        );
        // hermes is a concrete product (NousResearch) -> source-audited caps.
        assert_eq!(delivery_cap_tokens("hermes", "wrap", "grep"), 12_500);
        assert_eq!(delivery_cap_tokens("hermes", "cli", "read"), 20_000);
        // genuinely unlisted agent -> global fallback default.
        assert_eq!(
            delivery_cap_tokens("unknown", "cli", "read"),
            DEFAULT_CAP_READ_TOKENS
        );
    }

    #[test]
    fn raw_output_compactors_route_to_read_cap() {
        // These CLI ops stand in for a Read of the underlying file/blob, so the
        // honest counterfactual is the Read cap — NOT u64::MAX (the old leak,
        // where `cli` source + none of the read keywords fell through uncapped).
        for op in [
            "turn-summary",
            "docs",
            "csv",
            "openapi",
            "graphql",
            "sourcemap",
            "pack",
            "code-strip",
        ] {
            assert_eq!(
                delivery_cap_tokens("claude-code", "cli", op),
                20_000,
                "op {op} should route to the Read cap"
            );
            // Regression: without the keyword these were unbounded.
            assert_ne!(delivery_cap_tokens("claude-code", "cli", op), u64::MAX);
        }
        // Per-agent Read cap still applies to these ops.
        assert_eq!(delivery_cap_tokens("cursor", "cli", "csv"), 2_500);
    }

    #[test]
    fn mcp_channel_is_capped() {
        // MCP tool results (`mcp_tool_report`: source `wrap-mcp`, ops `mcp:*` /
        // `mcp-proxy:*`) were unbounded (u64::MAX) — a 500k JSON response
        // credited 500k the harness would have truncated at ~25k.
        for (src, op) in [
            ("wrap-mcp", "mcp:get_file_contents"),
            ("wrap-mcp", "mcp-proxy:github"),
            ("cli", "mcp:list_issues"),
        ] {
            assert_eq!(
                delivery_cap_tokens("claude-code", src, op),
                DEFAULT_CAP_MCP_TOKENS,
                "mcp op {op} (src {src}) must be capped"
            );
        }
        // The per-agent read/bash table must NOT leak into the mcp channel
        // (cursor's bash cap is much lower — mcp keeps its own default).
        assert_eq!(
            delivery_cap_tokens("cursor", "wrap-mcp", "mcp:x"),
            DEFAULT_CAP_MCP_TOKENS
        );
        // An mcp-ish op that is really a file read keeps the read cap
        // (read_like wins first): honest counterfactual is the Read.
        assert_eq!(
            delivery_cap_tokens("claude-code", "wrap-mcp", "mcp:read_file"),
            20_000
        );
    }

    /// Two MCP tools that do the SAME thing must get the same counterfactual.
    ///
    /// D7: the cap rule reads the tool's NAME. `mcp:read_file` matched the
    /// read-like pattern and inherited the 20 000 read cap; `mcp:get_file_contents`
    /// missed it and fell through to the MCP default, which was 25 000. Both
    /// read a file. A 5 000-token difference in what the agent "would have
    /// received" came down to a substring, and the server sums that figure for
    /// the monthly quota.
    #[test]
    fn two_mcp_tools_that_do_the_same_thing_get_the_same_cap() {
        let read_shaped = [
            "mcp:read_file",
            "mcp:get_file_contents",
            "mcp:cat_file",
            "mcp:fetch_document",
        ];
        let caps: Vec<u64> = read_shaped
            .iter()
            .map(|op| delivery_cap_tokens("claude-code", "wrap-mcp", op))
            .collect();
        assert!(
            caps.windows(2).all(|w| w[0] == w[1]),
            "these MCP tools all read a file but are capped differently: {:?}",
            read_shaped.iter().zip(&caps).collect::<Vec<_>>()
        );
        // Anti-vacuity: a cap of 0 or u64::MAX would make the equality trivial.
        assert_eq!(caps[0], 20_000, "the agreed cap is 20 000 (D7)");
    }

    #[test]
    fn nontruncating_channels_suppress_overflow_credit() {
        // Non-truncating channels: no harness truncation => no recoverable tail
        // => overflow credit 0 (kills the 50 M binary-dump over-count).
        assert_eq!(overflow_ratio_for_channel("aider", "wrap", "strings"), 0.0);
        assert_eq!(overflow_ratio_for_channel("aider", "cli", "read"), 0.0);
        // gemini-cli is NOT in this list any more: `saveTruncatedContent` runs
        // on every shell result by default, so a recoverable tail really does
        // exist there. Asserted positively rather than deleted, because a
        // deleted line records nothing about why it left.
        assert!(overflow_ratio_for_channel("gemini-cli", "wrap", "xxd") > 0.0);
        assert_eq!(
            overflow_ratio_for_channel("windsurf", "wrap", "objdump"),
            0.0
        );
        // antigravity is NOT here any more: measured 2026-08-10, its command
        // output is tail-truncated to 200 lines.
        assert!(overflow_ratio_for_channel("antigravity", "wrap", "cat") > 0.0);
        // grok is NOT here any more: measured 2026-08-11, its command output is
        // head+tail truncated at 40 000 characters, so a tail exists to recover.
        assert!(overflow_ratio_for_channel("grok", "wrap", "base64") > 0.0);
        assert_eq!(overflow_ratio_for_channel("cline", "cli", "read"), 0.0);
        // TRUNCATING channels keep the recoverable-tail credit.
        let r = overflow_credit_ratio();
        assert_eq!(
            overflow_ratio_for_channel("claude-code", "wrap", "strings"),
            r
        );
        // `cursor` is NOT in the truncation table: nobody has measured it, so
        // it credits nothing. This asserted `r` — a test that encodes a
        // SUPPOSITION makes it indistinguishable from a measurement.
        assert_eq!(overflow_ratio_for_channel("cursor", "cli", "read"), 0.0);
        // These agents truncate the OTHER channel, so it keeps the credit:
        assert_eq!(overflow_ratio_for_channel("gemini-cli", "cli", "read"), r); // read_file 2000 lines
        assert_eq!(overflow_ratio_for_channel("cline", "wrap", "grep"), r); // terminal 500 lines
        assert_eq!(overflow_ratio_for_channel("grok", "cli", "read"), r); // pi-lineage read truncates
    }

    #[test]
    fn overflow_ratio_defaults_in_range() {
        let r = overflow_credit_ratio();
        assert!((0.0..=1.0).contains(&r));
    }

    // ---- per-agent web/binary credit model --------------------------------
    fn saved(s: &crate::Savings) -> usize {
        s.tokens_before.saturating_sub(s.tokens_after)
    }

    #[test]
    fn agent_models_mirror_registry() {
        use BinaryCreditModel::*;
        use WebCreditModel::*;
        assert_eq!(
            agent_web_binary_model("claude-code"),
            (NativeSummary, NativePdf)
        );
        assert_eq!(
            agent_web_binary_model("codex"),
            (RawHtmlShell, ExtractOrFail)
        );
        assert_eq!(
            agent_web_binary_model("gemini-cli"),
            (ReaderClean, ExtractOrFail)
        );
        assert_eq!(
            agent_web_binary_model("qwen-code"),
            (ReaderClean, ExtractOrFail)
        );
        assert_eq!(
            agent_web_binary_model("cursor"),
            (ReaderClean, ExtractOrFail)
        );
        assert_eq!(agent_web_binary_model("cline"), (NoWebTool, NoDocTool));
        assert_eq!(agent_web_binary_model("copilot"), (NoWebTool, NoDocTool));
        assert_eq!(
            agent_web_binary_model("grok"),
            (RawHtmlShell, ExtractOrFail)
        );
        // Same product surface as `copilot`, and accounted like it.
        assert_eq!(
            agent_web_binary_model("copilot-vscode"),
            (NoWebTool, NoDocTool)
        );
        // An agent we have not classified: the FLOOR on the web side, the
        // bounded turn tax on the binary one.
        assert_eq!(
            agent_web_binary_model("brand-new-agent"),
            (Unmeasured, ExtractOrFail)
        );
    }

    #[test]
    fn an_unclassified_agent_is_not_credited_the_raw_html_ceiling() {
        // The defect this pins: the catch-all used to hand an agent we cannot
        // even name the LARGEST credit in the table (the whole raw page),
        // uncapped downstream. On a 28k-token page delivered as 7k that is
        // 21_000 tokens claimed on no evidence at all.
        let (web, _) = agent_web_binary_model("an-agent-we-never-measured");
        let credited = saved(&web_credit_savings(web, 28_000, 7_000));
        let ceiling = saved(&web_credit_savings(
            WebCreditModel::RawHtmlShell,
            28_000,
            7_000,
        ));
        assert_eq!(ceiling, 21_000, "the ceiling this must no longer claim");
        // The measured web economy on what is delivered (×3), not the page.
        assert_eq!(credited, 14_000);
        assert!(
            credited < ceiling,
            "unclassified credited {credited}, ceiling {ceiling}"
        );
        // And the floor does not grow with the page.
        let bigger = saved(&web_credit_savings(web, 900_000, 7_000));
        assert_eq!(
            bigger, credited,
            "the credit follows the delivery, not the page"
        );
        // The BINARY side is deliberately NOT dropped to zero.
        let (_, bin) = agent_web_binary_model("an-agent-we-never-measured");
        assert_eq!(saved(&binary_credit_savings(bin, 4_000)), 800);
    }
    // `every_canonical_agent_slug_is_measured_or_declared_unmeasured` has
    // moved to the `guards` target: it compares this table with
    // `detect::Agent::all()`, which the fold layer can no longer name.

    /// `NativePdf` is a claim about PDFs, and it was answering for every
    /// format. Claude Code renders a PDF itself, so tokenade adds nothing
    /// there — but nothing renders a `.xlsx` or a `.docx` natively, and the
    /// extraction is exactly what makes those readable.
    #[test]
    fn native_rendering_is_a_claim_about_pdfs_only() {
        use BinaryCreditModel::*;
        // The agent-only table still answers what it always did.
        assert_eq!(agent_web_binary_model("claude-code").1, NativePdf);
        // A PDF: nothing added, nothing claimed.
        assert_eq!(binary_model_for("claude-code", true), NativePdf);
        assert_eq!(saved(&binary_credit_savings(NativePdf, 4_000)), 0);
        // Anything else: the extraction is the whole reason the file is
        // readable, and it is worth the same turn tax it is worth elsewhere.
        assert_eq!(binary_model_for("claude-code", false), ExtractOrFail);
        assert_eq!(
            saved(&binary_credit_savings(
                binary_model_for("claude-code", false),
                4_000
            )),
            800
        );
        // Agents that never had the native claim are untouched, on both sides.
        for agent in ["codex", "cline", "an-agent-we-never-measured"] {
            assert_eq!(
                binary_model_for(agent, true),
                agent_web_binary_model(agent).1,
                "{agent} changed on the PDF path"
            );
            assert_eq!(
                binary_model_for(agent, false),
                agent_web_binary_model(agent).1,
                "{agent} changed off the PDF path"
            );
        }
    }

    /// An agent nobody has measured credits no recoverable tail.
    #[test]
    fn an_unmeasured_agent_credits_no_recoverable_tail() {
        assert!(agent_truncation_is_known("claude-code"));
        assert!(!agent_truncation_is_known("an-agent-nobody-measured"));
        assert_eq!(
            overflow_ratio_for_channel("an-agent-nobody-measured", "hook", "read_disclose"),
            0.0,
            "a tail was credited for an agent nobody has looked at"
        );
        // Non-vacuity: a measured agent on a truncating channel still gets one.
        assert!(
            overflow_ratio_for_channel("codex", "hook", "read_disclose") > 0.0,
            "the ratio is zero for everyone — this test would pass vacuously"
        );
    }

    #[test]
    fn binary_credit_never_bytes_over_4() {
        // ExtractOrFail: a PDF that naive bytes/4 would price at 553k credits
        // ONLY the turn tax (800), against extracted-text tokens (after).
        let s = binary_credit_savings(BinaryCreditModel::ExtractOrFail, 4_000);
        assert_eq!(saved(&s), 800);
        assert_eq!(s.tokens_after, 4_000);
        // NoDocTool behaves like ExtractOrFail (turn tax floor).
        assert_eq!(
            saved(&binary_credit_savings(BinaryCreditModel::NoDocTool, 10)),
            800
        );
        // NativePdf (Claude): credit 0 — tokenade adds nothing.
        assert_eq!(
            saved(&binary_credit_savings(BinaryCreditModel::NativePdf, 4_000)),
            0
        );
    }

    #[test]
    fn reader_credit_is_a_flat_reliability_credit() {
        let saved = |s: &crate::Savings| s.tokens_before.saturating_sub(s.tokens_after);
        // Same flat credit whatever the delivered size — the reader's gain is
        // reaching content at all, not compressing it.
        assert_eq!(
            saved(&reader_credit_savings(841)),
            web_reliability_turn_credit() as usize
        );
        assert_eq!(
            saved(&reader_credit_savings(25_607)),
            web_reliability_turn_credit() as usize
        );
        // Regression guard on the two bugs this replaced: an API-JSON reader
        // must NOT book the ~11.5k "compression" of a body the agent would
        // never have seen, and a raw-file reader must not book 0.
        assert!(saved(&reader_credit_savings(841)) < 11_000);
        assert!(saved(&reader_credit_savings(25_607)) > 0);
        assert_eq!(reader_credit_savings(500).tokens_after, 500);
    }

    #[test]
    fn web_credit_by_model() {
        // RawHtmlShell: honest full-HTML saving (raw=28k, after=7k -> 21k).
        assert_eq!(
            saved(&web_credit_savings(
                WebCreditModel::RawHtmlShell,
                28_000,
                7_000
            )),
            21_000
        );
        assert_eq!(
            saved(&web_credit_savings(
                WebCreditModel::NoWebTool,
                28_000,
                7_000
            )),
            21_000
        );
        // ReaderClean: the measured web economy, ×3 what is delivered.
        assert_eq!(
            saved(&web_credit_savings(
                WebCreditModel::ReaderClean,
                28_000,
                7_000
            )),
            14_000
        );
        // Transparent: 0.6*28k = 16.8k, after 7k -> 9.8k.
        assert_eq!(
            saved(&web_credit_savings(
                WebCreditModel::Transparent,
                28_000,
                7_000
            )),
            9_800
        );
        // Transparent conservative guard: after > 0.6*raw -> saving 0.
        let guard = web_credit_savings(WebCreditModel::Transparent, 10_000, 8_000);
        assert_eq!(saved(&guard), 0);
    }

    #[test]
    fn web_search_credit_reflects_native_multiple() {
        // Default 3× native baseline: served 600 -> before 1800, saved 1200.
        let s = web_search_credit_savings(600);
        assert_eq!(s.tokens_before, 1_800);
        assert_eq!(s.tokens_after, 600);
        assert_eq!(saved(&s), 1_200);
        // Ratio is ~3× (66.7% saved) regardless of served size.
        assert!((s.percent() - 66.666).abs() < 0.1);
        let big = web_search_credit_savings(900);
        assert!((big.percent() - 66.666).abs() < 0.1);
    }

    #[test]
    fn web_fetch_scoped_credit_is_capped_and_floored() {
        // Large page: ~3× the excerpt (410*3=1230 < distilled 4200, > floor).
        let s = web_fetch_scoped_credit_savings(4_200, 410, 800);
        assert_eq!(s.tokens_before, 1_230);
        assert_eq!(saved(&s), 820);
        // Huge page: capped at 3× the excerpt, NOT the 31k distilled page.
        let huge = web_fetch_scoped_credit_savings(31_000, 520, 800);
        assert_eq!(huge.tokens_before, 1_560);
        assert!(huge.tokens_before < 31_000, "must not book the whole page");
        // Small page: the floor WANTS extract+turn = 1 180, but the page only
        // held 900 distilled tokens, so the cap wins. Round 38 — this used to
        // assert 1 180, i.e. it asserted booking 280 tokens the page never
        // contained. The floor is a preference; the cap is the invariant.
        let small = web_fetch_scoped_credit_savings(900, 380, 800);
        assert_eq!(small.tokens_before, 900);
        assert!(
            small.tokens_before <= 900,
            "must never book more than the page held"
        );
        // Degenerate: we delivered the whole page, so nothing was saved.
        let whole = web_fetch_scoped_credit_savings(400, 400, 800);
        assert_eq!(saved(&whole), 0);
    }
}
#[cfg(test)]
fn credit_builders(code: &str) -> Vec<(String, bool)> {
    // No truncation at the first `#[cfg(test)]`. The trunk carries its tests
    // `include!` in the MIDDLE of the file, and the family puts the trunk
    // first: cutting there threw away the five builders, which live in
    // `coefficients/web_binary.rs`. The census then returned an EMPTY
    // list — and an empty list passes every silence check.
    // The sibling `*_tests.rs` files are not in the family, so there is nothing
    // to set aside.
    let mut out = Vec::new();
    for (i, _) in code.match_indices("pub fn ") {
        let head = &code[i..];
        let name = head[7..].split('(').next().unwrap_or("");
        if !name.ends_with("_credit_savings") {
            continue;
        }
        let body = &head[..head.find("\n}\n").unwrap_or(head.len())];
        out.push((name.to_string(), body.contains("counterfactual")));
    }
    out
}
#[cfg(test)]
mod counterfactual_declaration_tests {
    use super::credit_builders;

    /// **Every credit builder says where its baseline comes from.**
    ///
    /// Five functions of this module return a `Savings`; three fabricate
    /// their "before" entirely from what was delivered, two do so
    /// per branch. A consumer cannot know them one by one: an earlier
    /// filter set aside the code-intel rows and let `web-search` come out at
    /// −67 %, exactly 1 − 1/3, amid measured rates.
    ///
    /// The guard targets the SEAM — the function name — rather than
    /// the thirteen calls: a sixth builder that does not declare itself
    /// fails here, wherever it is called.
    #[test]
    fn every_credit_builder_declares_where_its_baseline_comes_from() {
        // The FAMILY: the five builders moved down into
        // `coefficients/web_binary.rs`. Reading the trunk alone returned an
        // EMPTY census — and an empty census passes every
        // silence check.
        let root = crate::source_guard::crates_dir()
            .join("tk-fold")
            .join("src");
        let found = credit_builders(&crate::source_guard::module_family(&root, "coefficients"));
        let silent: Vec<&str> = found
            .iter()
            .filter(|(_, declares)| !*declares)
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(
            silent.is_empty(),
            "{silent:?} returns a Savings without saying whether its baseline was READ: add \
             `.counterfactual()` on the branch(es) that compute it from what was delivered"
        );
        // The NAMES, not a count. A count mutates into a tautology without
        // anything turning red — that is what happened here on the first shot — and it
        // does not say which of the five left the guard's scope either.
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "web_credit_savings",
                "web_economy_credit_savings",
                "reader_credit_savings",
                "binary_credit_savings",
                "web_search_credit_savings",
                "web_fetch_scoped_credit_savings",
            ],
            "the census no longer sees the same builders"
        );
    }

    /// And the census itself is checked on a known text: a guard that
    /// finds nothing passes, and that is the failure mode it cannot
    /// report on its own.
    #[test]
    fn the_census_finds_a_silent_builder() {
        let code = "pub fn a_credit_savings(x: usize) -> Savings { Savings::new(x, x) }\n}\n\
                    pub fn b_credit_savings(x: usize) -> Savings { Savings::new(x, x).counterfactual() }\n}\n\
                    pub fn not_a_builder(x: usize) -> usize { x }\n}\n";
        assert_eq!(
            credit_builders(code),
            vec![
                ("a_credit_savings".to_string(), false),
                ("b_credit_savings".to_string(), true),
            ]
        );
    }

    /// And the declaration is TRUE, not merely present: the branches are
    /// checked one by one.
    #[test]
    fn the_flag_follows_the_branch_and_not_the_function() {
        use super::*;
        // Fabricated end to end.
        assert!(web_search_credit_savings(1000).baseline_is_counterfactual);
        assert!(reader_credit_savings(1000).baseline_is_counterfactual);
        assert!(
            binary_credit_savings(BinaryCreditModel::ExtractOrFail, 1000)
                .baseline_is_counterfactual
        );
        // Read: the raw page is the baseline.
        assert!(
            !web_credit_savings(WebCreditModel::RawHtmlShell, 9000, 400).baseline_is_counterfactual
        );
        // Fabricated: the baseline comes from what was delivered plus a per-turn credit.
        assert!(
            web_credit_savings(WebCreditModel::ReaderClean, 9000, 400).baseline_is_counterfactual
        );
        // Ceiling bit: `before` IS the real page, hence measured.
        assert!(!web_fetch_scoped_credit_savings(500, 400, 800).baseline_is_counterfactual);
        // Wide ceiling: the multiplier decides.
        assert!(web_fetch_scoped_credit_savings(100_000, 400, 800).baseline_is_counterfactual);
    }
}

#[cfg(test)]
mod native_summary_credit_tests {
    use super::*;

    /// Claude Code: what tokenade delivers is credited at the measured web
    /// economy (−67 % ⇒ ×3), whole page or excerpt alike.
    #[test]
    fn a_page_served_to_claude_code_is_credited_at_the_measured_economy() {
        let s = web_credit_savings(WebCreditModel::NativeSummary, 56_911, 10_616);
        assert_eq!((s.tokens_before, s.tokens_after), (31_848, 10_616));
        assert!(s.baseline_is_counterfactual);
        assert_eq!(web_economy_credit_savings(1_000).tokens_before, 3_000);
    }
}
