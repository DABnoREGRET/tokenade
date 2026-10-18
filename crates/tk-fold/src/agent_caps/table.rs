//! **The measured matrix** — one row per (agent, action), and its
//! constructors.
//!
//! The sources were read on 2026-08-10 in the packages installed on the
//! development machine; each row records where its value was read.
//!
//! The three constructors live here because they serve only here, and because
//! they carry the distinction that gives the table its value: `pkg` reads the
//! package, `pkg_credited` separates what the agent does from what tokenade
//! counts, and `peer` labels an ASSUMPTION. A table mixing the
//! three would end up publishing the assumption as a fact.

use super::*;

pub(crate) const fn pkg(limit: Limit, file: &'static str, symbol: &'static str) -> Cap {
    Cap {
        limit,
        provenance: Provenance::Package { file, symbol },
        credited_tokens: None,
    }
}

/// A measured cap whose CREDIT was set by decision rather than by the
/// measurement. Both arguments are kept: the table still states what the agent
/// really does, and separately what tokenade counts.
pub(crate) const fn pkg_credited(
    limit: Limit,
    file: &'static str,
    symbol: &'static str,
    credited_tokens: u64,
) -> Cap {
    Cap {
        limit,
        provenance: Provenance::Package { file, symbol },
        credited_tokens: Some(credited_tokens),
    }
}

/// A cap that is NOT measured: a conservative bound borrowed from a peer of the
/// same lineage, kept for agents whose package cannot be read.
///
/// Currently UNUSED by [`TABLE`], and that is the point — every row is measured
/// today. It stays because the next uncovered agent will need it, and because
/// the alternative to a labelled guess is an unlabelled one.
#[allow(dead_code)]
pub(crate) const fn peer(limit: Limit, because: &'static str) -> Cap {
    Cap {
        limit,
        provenance: Provenance::PeerDefault { because },
        credited_tokens: None,
    }
}

/// Everything the table knows, as (agent, action, cap).
///
/// Sources were read on 2026-08-10 out of the packages installed on this
/// machine; each row records where its value was read.
pub const TABLE: &[(&str, Action, Cap)] = &[
    // ---- gemini-cli -------------------------------------------------------
    // The reason this module returns rules and not numbers.
    (
        "gemini-cli",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 2_000,
                max_line_chars: Some(2_000),
            },
            "@google/gemini-cli-core/dist/src/utils/fileUtils.js",
            "DEFAULT_MAX_LINES_TEXT_FILE / MAX_LINE_LENGTH_TEXT_FILE",
        ),
    ),
    (
        "gemini-cli",
        Action::Command,
        pkg(
            Limit::HeadTail {
                threshold_chars: 4_000_000,
                total_lines: 1_000,
                wrap_width: Some(120),
            },
            "@google/gemini-cli-core/dist/src/utils/fileUtils.js",
            "saveTruncatedContent (threshold from config.js getTruncateToolOutputThreshold)",
        ),
    ),
    // There is deliberately NO `gemini-cli / ToolResult` row. The truncation
    // above is gated on `toolName === SHELL_TOOL_NAME` in
    // scheduler/tool-executor.js, so it applies to the shell tool and to
    // nothing else. Listing it as a general tool-result cap would be the D7
    // mistake — a per-tool constant promoted to a global rule — committed in
    // the very module written to stop making it.
    // ---- qwen-code --------------------------------------------------------
    // A gemini-cli fork whose constants kept their names and changed both their
    // VALUES and their SHAPE, so inheriting gemini's rows would have been wrong
    // three times over:
    //
    //  * threshold 25 000, not 4 000 000 — a factor of 160;
    //  * the threshold is NOT context-dependent here. gemini computes
    //    `min(4 * free tokens, ceiling)`; qwen's getter returns the configured
    //    value flat (or +Infinity when <= 0). Same method name, different rule;
    //  * `truncateAndSaveToFile(..., keep = "both")` keeps HEAD AND TAIL with
    //    `head = floor(lines / 5)`, and it fires when EITHER the character
    //    threshold or the line budget is passed — gemini fires on the byte
    //    threshold alone.
    //
    // And unlike gemini, this is not shell-only: `processSingleFileContent`
    // reads the same `getTruncateToolOutputThreshold()` as `configCharLimit`,
    // so a file READ is bounded by it too. That is why no separate read-side
    // constant exists to be found — there is one rule for every tool output.
    (
        "qwen",
        Action::ReadFile,
        pkg(
            Limit::Chars(25_000),
            "@qwen-code/qwen-code/chunks/chunk-3PY4LPJF.js",
            "processSingleFileContent: configCharLimit = getTruncateToolOutputThreshold()",
        ),
    ),
    (
        "qwen",
        Action::ToolResult,
        pkg(
            Limit::Tightest(&[
                Limit::Chars(25_000),
                Limit::HeadTail {
                    threshold_chars: 25_000,
                    total_lines: 1_000,
                    wrap_width: None,
                },
            ]),
            "@qwen-code/qwen-code (chunk-GXSNGY5M.js + chunk-3PY4LPJF.js)",
            "DEFAULT_TRUNCATE_TOOL_OUTPUT_THRESHOLD=25e3 / _LINES=1e3, truncateAndSaveToFile keep=both",
        ),
    ),
    // No `qwen / ReadFile` line-count row. `READ_FILE_CHAR_CAP = 25e3` looks
    // exactly like one and belongs to the bundled code-REVIEW skill's chunk
    // planner; `MAX_LINES_PER_FILE = 400` sits beside `MAX_DIFF_SIZE_BYTES` in
    // the git-diff module. Two more well-named constants for other things.
    // ---- openclaw ---------------------------------------------------------
    // openclaw has no file or shell tool of its own: `agents/pi-tools.js`
    // imports `codingTools` from `@mariozechner/pi-coding-agent` (bundled in
    // its OWN node_modules, 0.50.7), and `pi-tools.read.js` only wraps
    // `createReadTool` for sandbox paths and image mime — no extra truncation.
    // So both channels are pi's rule, and the bundled copy was read rather than
    // assumed identical to the standalone package.
    //
    // CORRECTED 2026-08-12. This block used to carry `ToolResult = 8 000` and
    // `ToolError = 400` from `pi-embedded-subscribe.tools.js`. Those are on the
    // OBSERVER path: `sanitizeToolResult` feeds `emitAgentEvent({ stream:
    // "tool" })`, which is the event stream the UI and channels subscribe to,
    // while the model receives `evt.result` straight from the pi loop. A
    // DISPLAY cap recorded as a delivery cap — the exact mistake gemini-cli's
    // `MAXIMUM_RESULT_DISPLAY_CHARACTERS` was excluded for on the first day of
    // this table, made here anyway, and caught only because someone asked why
    // openclaw had no read row.
    (
        "openclaw",
        Action::ReadFile,
        pkg(
            Limit::Tightest(&[
                Limit::Lines {
                    max_lines: 2_000,
                    max_line_chars: None,
                },
                Limit::Chars(51_200),
            ]),
            "openclaw/node_modules/@mariozechner/pi-coding-agent 0.50.7 core/tools/truncate.js",
            "DEFAULT_MAX_LINES = 2000 / DEFAULT_MAX_BYTES = 50 * 1024, via createReadTool",
        ),
    ),
    (
        "openclaw",
        Action::Command,
        pkg(
            Limit::Tightest(&[
                Limit::Lines {
                    max_lines: 2_000,
                    max_line_chars: None,
                },
                Limit::Chars(51_200),
            ]),
            "openclaw/node_modules/@mariozechner/pi-coding-agent 0.50.7",
            "same truncate.js, reached through `codingTools`",
        ),
    ),
    (
        "openclaw",
        Action::WebFetch,
        pkg(
            Limit::Chars(50_000),
            "openclaw/dist/agents/tools/web-fetch.js",
            "DEFAULT_FETCH_MAX_CHARS, passed as runWebFetch's maxChars",
        ),
    ),
    (
        "openclaw",
        Action::InputFile,
        pkg(
            Limit::Chars(200_000),
            "openclaw/dist/media/input-files.js",
            "DEFAULT_INPUT_FILE_MAX_CHARS",
        ),
    ),
    // ---- opencode ---------------------------------------------------------
    // Measured 2026-08-10 WITHOUT an LLM turn and without credentials. The
    // agent ships as a 184 MB Bun-compiled binary, so nothing is readable at
    // rest — but `opencode serve` exposes `GET /experimental/tool`, which
    // returns the live tool schemas the agent hands its model, limits included.
    // Running the product and asking it what its own tools do beats guessing at
    // its bytes, and it costs nothing.
    (
        "opencode",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 2_000,
                max_line_chars: Some(2_000),
            },
            "opencode serve GET /experimental/tool (v0.x, 2026-08-10)",
            "read: \"returns up to 2000 lines\" / \"line longer than 2000 characters is truncated\"",
        ),
    ),
    (
        "opencode",
        Action::Command,
        pkg(
            Limit::Tightest(&[
                Limit::Chars(51_200),
                Limit::Lines {
                    max_lines: 2_000,
                    max_line_chars: None,
                },
            ]),
            "opencode serve GET /experimental/tool (v0.x, 2026-08-10)",
            "bash: \"exceeds 2000 lines or 51200 bytes, it will be truncated\"",
        ),
    ),
    // ---- grok -------------------------------------------------------------
    // A Rust binary (`crates/codegen/xai-chat-state/...`), 130 MB, whose tool
    // descriptions ARE in clear text — but the read ones are TEMPLATES:
    // "reads up to {max_lines_read} lines", "lines longer than
    // {max_chars_per_line} characters will be truncated". The identifiers are
    // in the binary, the values are substituted at runtime and are not
    // adjacent to them; `grok inspect` reports discovered config and not these.
    // Same shape as antigravity, so the same conclusion: the read caps cannot
    // be read at rest and need a live session.
    //
    // Session 2026-08-11, `grok -p`, asking it to QUOTE its own tool text
    // rather than describe itself — a retrieval, not an introspection. It
    // returned "By default, it reads up to 1000 lines starting from the
    // beginning of the file", which is the binary's template string with
    // `{max_lines_read}` filled in, character for character. That exact match
    // against a string found independently is what makes the value usable.
    (
        "grok",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 1_000,
                max_line_chars: None,
            },
            "live session 2026-08-11, verbatim quote matching the binary template",
            "`By default, it reads up to {max_lines_read} lines` -> 1000",
        ),
    ),
    // The command cap, measured WITHOUT trusting the agent's arithmetic. Asking
    // it "how many characters did you receive" produced a number it admitted
    // reconstructing from the truncation banner — worthless. Instead the
    // payload was made SELF-INDEXING (100 000 numbered lines) and the agent was
    // asked only to read four indices off it: first visible, last of the first
    // block, first after the gap, last visible. No arithmetic on its side, and
    // the answer is checkable against the generator.
    //
    // Run twice with different line shapes, which is what makes the SHAPE
    // conclusive rather than assumed:
    //   8-char lines   -> kept 0000000-0001249 and 0098750-0099999 (1 250 each)
    //   100-char lines -> kept 0000000-0000099 and 0099900-0099999 (100 each)
    // Different line counts, identical CHARACTER counts: 10 000 head + 10 000
    // tail. The budget is characters, not lines.
    //
    // The 40 000 it quoted earlier is the THRESHOLD (`{max_output_bytes}` in
    // its binary), not the delivery — two different quantities that I first
    // mistook for a contradiction and nearly recorded as one. A self-report
    // that looks inconsistent is worth a second measurement before it is worth
    // a verdict.
    (
        "grok",
        Action::Command,
        pkg(
            Limit::HeadTailChars {
                threshold_chars: 40_000,
                keep_chars: 20_000,
            },
            "live session 2026-08-11, self-indexing payload run twice",
            "threshold {max_output_bytes}=40000; kept 10 000 head + 10 000 tail",
        ),
    ),
    // The web-fetch cap is NOT a template, so it is the other row grok gets.
    (
        "grok",
        Action::WebFetch,
        pkg(
            Limit::Chars(100_000),
            "grok-linux-x86_64 (embedded tool description)",
            "\"Content longer than 100,000 characters will be truncated\"",
        ),
    ),
    // ---- pi ---------------------------------------------------------------
    // `@earendil-works/pi-coding-agent` (repo github.com/earendil-works/pi,
    // checked first). Ships readable `dist/` JS, and `core/tools/truncate.js`
    // opens by stating the rule outright:
    //
    //   "Truncation is based on two independent limits - whichever is hit
    //    first wins: Line limit (default: 2000 lines), Byte limit (default:
    //    50KB). Never returns partial lines."
    //
    // `truncateHead` serves file reads (keep the first N); bash uses the tail
    // variant, but the BOUND is the same pair, so both channels share it.
    //
    // Not recorded: `TOOL_RESULT_MAX_CHARS = 2000`, which is the cap on a tool
    // result inside a serialized SUMMARY, not on delivery — the fifth
    // perfectly-named constant in this table to mean something else. And
    // `GREP_MAX_LINE_LENGTH = 500`, which is grep-only.
    (
        "pi",
        Action::ReadFile,
        pkg(
            Limit::Tightest(&[
                Limit::Lines {
                    max_lines: 2_000,
                    max_line_chars: None,
                },
                Limit::Chars(51_200),
            ]),
            "@earendil-works/pi-coding-agent/dist/core/tools/truncate.js",
            "DEFAULT_MAX_LINES = 2000 / DEFAULT_MAX_BYTES = 50 * 1024",
        ),
    ),
    (
        "pi",
        Action::Command,
        pkg(
            Limit::Tightest(&[
                Limit::Lines {
                    max_lines: 2_000,
                    max_line_chars: None,
                },
                Limit::Chars(51_200),
            ]),
            "@earendil-works/pi-coding-agent/dist/core/tools/truncate.js",
            "same pair, tail variant for bash",
        ),
    ),
    // ---- t3-code ----------------------------------------------------------
    // T3 Code has NO agent of its own. `/usr/bin/t3code` exports
    // `CODEX_CLI_PATH` when a `codex` binary is on PATH, and its 194 MB asar
    // speaks `codex.app-server.request` / `.notification` and carries codex's
    // tool names (`unified_exec`, `exec_command`). It is a GUI over the Codex
    // engine, so its delivery caps ARE codex's — recorded by DELEGATION, with
    // the delegation named rather than the number copied, so that a change on
    // the codex row cannot leave this one behind.
    //
    // The `max_output_tokens` values in its bundle (8192 / 32000 / 64000) are
    // per-MODEL output budgets, the same class excluded for copilot.
    (
        "t3-code",
        Action::Command,
        pkg(
            Limit::Tokens(10_000),
            "/opt/t3code-bin/resources/app.asar (codex app-server client)",
            "delegates to the codex engine; see the codex/Command row",
        ),
    ),
    // ---- copilot ----------------------------------------------------------
    // The tool DESCRIPTIONS carry no numbers at all — the caps live in the
    // code. `xBe = 50 * 1024` is the default `maxOutputSizeBytes`, and
    // `largeOutputOptions` is always constructed (`aq(o, s, kbr(r))`), so the
    // 10 MB fallback `ndt` never applies in the CLI.
    //
    // Two paths meet at the same bound, and both are worth recording:
    //  * a WHOLE-file view above 51 200 bytes is REFUSED — "File too large to
    //    read at once (...). Consider using the grep tool ... or view_range";
    //  * a RANGED view is truncated at the same 51 200, with "[Output
    //    truncated. Use view_range=[N, ...] to continue reading.]".
    // Delivery is bounded by 51 200 either way, which is what this row states.
    //
    // Not recorded: `maxOutputTokens = 4096` (the MODEL's output budget, not a
    // tool result) and the 10 000-row cap on SQL results (one tool).
    (
        "copilot",
        Action::ReadFile,
        pkg(
            Limit::Chars(51_200),
            "@github/copilot/app.js",
            "xBe = 50 * 1024 (largeOutputOptions.maxOutputSizeBytes)",
        ),
    ),
    (
        "copilot",
        Action::Command,
        pkg(
            Limit::Chars(51_200),
            "@github/copilot/app.js",
            "same largeOutputOptions passed to the shell session factory",
        ),
    ),
    // ---- droid ------------------------------------------------------------
    // Factory's CLI, installed 2026-08-10 from the npm package whose
    // `repository.url` is github.com/Factory-AI/factory — checked BEFORE
    // measuring, because five of the seven candidate packages for the
    // remaining agents turned out to be third-party look-alikes, and a
    // look-alike measured carefully still produces a wrong number wearing the
    // word "measured". The agent is a 153 MB Bun binary carrying its tool
    // descriptions in clear text.
    (
        "droid",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 2_400,
                max_line_chars: None,
            },
            "@factory/cli-linux-x64-baseline/bin/droid (embedded tool schema)",
            "\"Returns up to 2400 lines from the start by default\"",
        ),
    ),
    (
        "droid",
        Action::Command,
        pkg(
            Limit::Chars(40_000),
            "@factory/cli-linux-x64-baseline/bin/droid (embedded tool schema)",
            "\"Output is truncated at 40,000 characters\", head+tail shape",
        ),
    ),
    // No per-line row for droid. `NwL(H, $ = 20, L = 2000)` looks like one and
    // truncates to 20 lines — a preview helper, an order of magnitude away
    // from the 2 400-line read tool it would have been mistaken for.
    // ---- hermes -----------------------------------------------------------
    // The easiest row in this table, and the only agent that documents its own
    // caps in prose: `tools/tool_output_limits.py` names the values, the two
    // files they used to be hardcoded in, and the upstream PR they came from.
    // Python source, installed at ~/.hermes/hermes-agent. All three are
    // overridable via `tool_output` in config.yaml; the defaults are recorded.
    (
        "hermes",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 2_000,
                max_line_chars: Some(2_000),
            },
            "hermes-agent/tools/tool_output_limits.py",
            "DEFAULT_MAX_LINES / DEFAULT_MAX_LINE_LENGTH (was file_operations.py)",
        ),
    ),
    (
        "hermes",
        Action::Command,
        pkg(
            Limit::Chars(50_000),
            "hermes-agent/tools/tool_output_limits.py",
            "DEFAULT_MAX_BYTES (was terminal_tool.MAX_OUTPUT_CHARS)",
        ),
    ),
    // ---- aider ------------------------------------------------------------
    // Measured UNBOUNDED, which is a finding and not a gap: `cmd_run` counts
    // the tokens, asks "Add Nk tokens of command output to the chat?", and on
    // yes adds the output WHOLE. Every byte tokenade folds here is a byte the
    // user would really have paid for.
    (
        "aider",
        Action::Command,
        pkg(
            Limit::Unbounded,
            "aider/commands.py",
            "cmd_run (confirm-then-add, no truncation)",
        ),
    ),
    (
        "aider",
        Action::ReadFile,
        pkg(
            Limit::Unbounded,
            "aider/commands.py",
            "cmd_add (whole file into the chat)",
        ),
    ),
    // ---- cline ------------------------------------------------------------
    // The npm package is a 149 MB Bun-compiled binary, but it carries its own
    // system prompt and tool descriptions as plain text, so the limits it TELLS
    // ITS MODEL are readable — which is the authoritative statement of what it
    // delivers, not a proxy for it.
    //
    // Shape worth recording even though it does not change the SIZE: the shell
    // cap is a MIDDLE truncation, `slice(0, ceil(n/2))` + marker + tail, so
    // 24 000 characters of head and 24 000 of tail survive. `Chars` is exact
    // for how much arrives; it is the WHICH that differs, and a caller
    // reasoning about recoverable tails needs to know there is no tail to
    // recover here — it was already delivered.
    (
        "cline",
        Action::Command,
        pkg(
            Limit::Chars(48_000),
            "cline@3.0.52 bin/.cline (embedded prompt)",
            "x0=48000, middle-truncated: \"Output beyond ~48k characters is middle-truncated\"",
        ),
    ),
    (
        "cline",
        Action::WebFetch,
        pkg(
            Limit::Chars(50_000),
            "cline@3.0.52 bin/.cline (embedded prompt)",
            "\"[Content truncated: showing first 50000 of N characters]\"",
        ),
    ),
    // No `cline / ReadFile` row, and this is a MEASURED absence rather than a
    // gap: the binary carries no read-side line or character limit, matching
    // cline#4576 (read_file returns the whole file). The scalar table's 20 000
    // for this channel is a conservative ingestion bound, not a truncation
    // point, and `channel_is_nontruncating` already says so.
    // ---- kilo-code --------------------------------------------------------
    // The READ caps are measured, from the tool description the agent hands its
    // model: "returns up to 2000 lines from the start of the file" and "any
    // line longer than 2000 characters is truncated".
    //
    // The `MAX_OUTPUT_CHARS = 45 000` that used to sit here as a PeerDefault is
    // GONE. It belongs to `codebase_search` alone, and keeping it as a stand-in
    // for a general cap was the D7 mistake preserved under a warning label. A
    // labelled guess is still a guess; now that a real cap exists for the read
    // channel, the guess has nothing left to stand in for.
    (
        "kilo-code",
        Action::ReadFile,
        pkg(
            Limit::Lines {
                max_lines: 2_000,
                max_line_chars: Some(2_000),
            },
            "@kilocode/cli (embedded read tool description)",
            "\"up to 2000 lines from the start\" / \"longer than 2000 characters is truncated\"",
        ),
    ),
    // ---- claude-code ------------------------------------------------------
    // Read out of the installed binary 2.1.225 (an ELF with the bundle inside;
    // `strings` + context, not a guess). Two things worth knowing:
    //
    //  * the read/MCP cap is NOT a constant of the build. The accessor is
    //    `NC_() ?? (gate.maxTokens ?? $C_)`: an env override
    //    (`CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`), then a SERVER-SIDE
    //    feature gate (`tengu_amber_wren`), then the local default 25 000. So
    //    this number can move without any package changing on disk — an
    //    argument, on its own, against ever hardcoding it in a second place.
    //  * it REFUSES, it does not truncate. See `Limit::RefusedAbove`.
    //
    // There is deliberately NO 2 000-line row here. The scalar table asserts
    // one; the binary contains no such string, so it does not enter this
    // module. An unverified value is exactly what `Provenance` exists to stop.
    (
        "claude-code",
        Action::ReadFile,
        pkg_credited(
            Limit::RefusedAbove { tokens: 25_000 },
            "claude 2.1.225 (bundled)",
            "$C_ / CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS, MaxFileReadTokenExceededError",
            // Measured 25 000, CREDITED 20 000 (decision, 2026-08-10). The
            // channel refuses rather than truncates, so no exact figure exists:
            // crediting the cap is a deliberate stand-in, and crediting the
            // LOWER of the two is the conservative side of a stand-in. See
            // `credit_basis`, which labels every total this feeds.
            20_000,
        ),
    ),
    (
        "claude-code",
        Action::Command,
        pkg(
            Limit::Chars(30_000),
            "claude 2.1.225 (bundled)",
            "BASH_MAX_OUTPUT_LENGTH default 30 000, user-settable to 150 000",
        ),
    ),
    (
        "claude-code",
        Action::McpToolResult,
        pkg_credited(
            Limit::RefusedAbove { tokens: 25_000 },
            "claude 2.1.225 (bundled)",
            "same accessor as the read cap; result spilled to a file, not cut",
            // Measured 25 000, CREDITED 20 000 — decision D7, which capped both
            // MCP tools at one figure so that two tools doing the same thing
            // stop being counted differently. Deliberately the conservative
            // side, and now stated where the measurement is instead of in a
            // second table that had to be kept in step by hand.
            20_000,
        ),
    ),
    // ---- codex ------------------------------------------------------------
    // The binary is a STRIPPED Rust ELF: no symbol names, and numeric constants
    // are immediates rather than named data, so the usual "find the constant"
    // sweep returns nothing. The value was recovered from the prompt text codex
    // ships to its own model, which states the default in prose. It truncates
    // rather than refusing — `Warning: truncated output (original token count:`
    // is in the same binary. Only the Command channel is claimed; codex has no
    // file tool (it reads through the shell), so there is no ReadFile row.
    (
        "codex",
        Action::Command,
        pkg(
            Limit::Tokens(10_000),
            "/usr/bin/codex (stripped ELF, 2026-07-16 build)",
            "embedded exec prompt: \"`max_output_tokens` \u{2026} Defaults to 10000 tokens\"",
        ),
    ),
    // ---- antigravity ------------------------------------------------------
    // Measured 2026-08-10 in a LIVE session: the operator drove the IDE and
    // asked the agent to read a calibrated 9 000-line file and to push its own
    // limits. A self-report is normally WEAK evidence — a model asked about its
    // own limits will happily invent them — so only the findings corroborated
    // by something the model could not have known enter this table:
    //
    //  * 800 lines fills the template `You can view at most %[1]d lines at a
    //    time`, found in `language_server` with its value stripped out;
    //  * 46 080 = 45 x 1024, and `ViewFileToolConfig` carries EXACTLY two limit
    //    fields, `max_lines_per_view` (14) and `max_bytes_per_view` (18) — the
    //    agent reported one value for each, unprompted;
    //  * `<truncated 8801 lines>` is a verbatim artefact whose arithmetic
    //    checks out: 8801 + 199 = 9 000.
    //
    // Deliberately NOT recorded, because nothing corroborates them: the agent's
    // own token estimates ("~7 000 to 12 000"), the model's context window and
    // output budget (model-level, not a tool-result cap — the same reason
    // copilot's `maxOutputTokens` is absent from this table), and a character
    // cap on command output, which its own two tests CONTRADICT: 199 lines of
    // ~40 characters is ~8 000, yet a single 5 MB line came back as ~4 000. A
    // number that disagrees with itself is not a measurement.
    //
    // Units caveat, and this row is the only one it applies to: 46 080 is
    // BYTES (Go), while every other cap here is characters (JS
    // `String.length`). They coincide on ASCII; on non-ASCII payloads this row
    // OVER-estimates what really arrives.
    (
        "antigravity",
        Action::ReadFile,
        pkg(
            Limit::Tightest(&[
                Limit::Lines {
                    max_lines: 800,
                    max_line_chars: None,
                },
                Limit::Chars(46_080),
            ]),
            "live session 2026-08-10, corroborated by ViewFileToolConfig 14/18",
            "max_lines_per_view=800 / max_bytes_per_view=46080",
        ),
    ),
    (
        "antigravity",
        Action::Command,
        pkg(
            Limit::TailLines { max_lines: 200 },
            "live session 2026-08-10",
            "observed `<truncated 8801 lines>` + the final 199 of 9 000",
        ),
    ),
    // ---- windsurf ---------------------------------------------------------
    // Windsurf is now shipped as `devin-desktop`, and the Electron bundle
    // contains a full CLI at
    // `resources/app/extensions/windsurf/devin/bin/devin` — a 139 MB Rust
    // binary whose tool descriptions are in CLEAR TEXT, unlike antigravity's
    // and grok's templated ones. So this agent needed no live session after
    // all, which is worth saying plainly: it was declared "GUI only, cannot be
    // measured" for three days on a claim nobody had tested.
    //
    // The read rule is one sentence and carries both halves of the bound:
    //   "By default, it reads up to 20000 characters starting from the
    //    beginning of the file. [...] Any lines longer than 2000 characters
    //    will be truncated."
    // A total budget AND a per-line one, which is why this cannot be a scalar:
    // 20 000 characters of 80-char lines and 20 000 characters of 5 000-char
    // lines are not the same delivery.
    (
        "windsurf",
        Action::ReadFile,
        pkg(
            Limit::Tightest(&[
                Limit::Chars(20_000),
                Limit::Lines {
                    max_lines: u64::MAX,
                    max_line_chars: Some(2_000),
                },
            ]),
            "devin-desktop resources/app/extensions/windsurf/devin/bin/devin",
            "\"reads up to 20000 characters\" + \"lines longer than 2000 characters will be truncated\"",
        ),
    ),
    // NOT recorded, and the omission is the finding: the command tool announces
    // "If the output exceeds 30000 characters, output will be truncated before
    // being returned to you." That is a THRESHOLD, and grok proved the two come
    // apart — it announced 40 000 and delivered 20 000. Recording a threshold as
    // a delivery inflates the credit by whatever the gap is, which is the one
    // direction this table must never err in.
    //
    // The gap cannot be closed at rest here. The truncation marker
    // (`... [output truncated, {} chars total]`) is referenced through a
    // relocated pointer table rather than a `lea`, so the constant beside it
    // resisted a static scan; and the ACP path takes the budget from the CLIENT
    // (`outputByteLimit`), which devin-desktop leaves unset. What IS established
    // is the ceiling: delivery is at most 30 000 characters, so the scalar's
    // 12 800 tokens (51 200 characters) over-stated this channel by at least
    // 1.7x on a number the table itself called "unpublished (closed source)".
];
