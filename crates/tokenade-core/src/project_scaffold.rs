//! Rules-file scaffold for AI coding agents.
//!
//! The CLI lecture (`tokenade map`, wrap, exec, « Keep output lean ») lives
//! in each agent's **machine-global** rules file. Project files carry a
//! pointer so a clone does not re-tax every session, and so two files in
//! the same directory cannot stack the same ~1 200 tokens.
//!
//! Agents with no global address (Aider, Odysseus, …) keep the
//! lecture in the project file they actually read.
//!
//! Hook-first since the 2026-06 pivot: the MCP server is no longer
//! installed by default, so the scaffold nudges CLI commands, not
//! `mcp__tokenade__*` tools.
//!
//! Invoked from TWO entry points:
//!   1. **SessionStart / MCP `initialize` handler** — auto-runs on the
//!      first session in any project where tokenade is wired in.
//!   2. **CLI** `tokenade install` — seeds the files up front.
//!
//! Idempotent. Non-destructive. Detects which agents the user has
//! installed (via `~/.<agent>/` presence) and only scaffolds files
//! for those agents.

use std::path::{Path, PathBuf};

/// HTML comment marker that bounds our scaffold block in any rules
/// file. Lets us detect "already appended" on subsequent runs.
pub const SCAFFOLD_MARKER: &str = "<!-- tokenade-scaffold -->";

/// Rules files written for EVERY project regardless of detected
/// agents. `CLAUDE.md` for Claude Code, `AGENTS.md` for the
/// cross-agent standard (Cursor ≥2025-12, Codex, Zed, Continue).
const PRIMARY_AGENT_FILES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// Project files a specific agent reads, which keep the whole lesson as long
/// as one of their PRESENT readers has no global. Empty list: we do not know
/// who reads them (Roo, legacy files) — the lesson stays.
///
/// `.github/copilot-instructions.md` stayed complete in every case, for
/// Copilot VS Code; Copilot CLI ALSO loads it, next to its global
/// `~/.copilot/copilot-instructions.md` — captured ×2 on 2026-09-28 on a
/// machine without VS Code.
///
/// Hermes loads `.hermes.md` / `HERMES.md` (first match, parents up to the
/// git root) — not `.hermes_rules.md`.
fn dedicated_project_file_readers(target: &Path) -> Option<&'static [crate::detect::Agent]> {
    use crate::detect::Agent::*;
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let readers: &'static [crate::detect::Agent] = match name {
        "copilot-instructions.md" => &[Copilot, CopilotVsCode],
        ".hermes.md" | "HERMES.md" | "soul.md" => &[Hermes],
        "opencode.md" => &[OpenCode],
        ".kilocoderules" => &[KiloCode],
        // Devin Desktop (`localRulesFilePathSegments: [".windsurfrules"]`).
        ".windsurfrules" => &[DevinDesktop],
        _ if target
            .components()
            .any(|c| c.as_os_str() == ".roo" || c.as_os_str() == ".kilocode") =>
        {
            &[]
        }
        _ => return None,
    };
    Some(readers)
}

/// The pointer written in place of the block, inside the same markers — they
/// are how update and uninstall find it again.
/// The pointer 1.1.18–1.1.19 wrote; the tests treat it as a leftover to heal.
#[cfg(test)]
pub(crate) fn pointer_block() -> String {
    format!(
        "{SCAFFOLD_MARKER}\n## Project rules\n\
         The `tokenade` CLI is available globally. Its usage rules are loaded \
         from this machine's agent config — if the Tokenade instruction is \
         already in the context, do not open another copy.\n\
         {SCAFFOLD_END_MARKER}"
    )
}

/// Scaffold content version. Bumped whenever [`scaffold_block`]'s
/// text changes in a way users should see (new rule, removed rule,
/// reworded guidance). Stored in `.tokenade/scaffold.flag` so
/// projects that were scaffolded under an older version trigger the
/// in-place upgrade path the next time `ensure()` runs — `tokenade
/// install` and the SessionStart hook. It said "the next `tokenade mcp`
/// session boot" until 2026-08-25, naming the MCP-server entry point the
/// product dropped at the zero-MCP pivot: a version bump would have read
/// as reaching users it could no longer reach.
// Bumped 4 → 5 (#449): scaffold_block() removed the
// `claude_memory_recall` line when that MCP tool was retired, but the
// version stayed at 4 so the in-place upgrade never fired on existing
// repos. Bumping forces every repo's next install / SessionStart to
// splice the new rules block over the stale one.
// Bumped 5 → 6 (#529): some projects scaffolded under the pre-marker
// era still carry legacy un-wrapped prose (`## Codebase exploration`
// + `## Engineering principles`) ABOVE the modern marker block, so
// every session loads the same rules twice. `write_scaffold_to` now
// detects and strips that legacy fingerprint; the version bump is
// what makes the cleanup actually fire on existing repos.
// Bumped 8 → 9 (P8, 2026-06): rules block reworded to v4 — bypass
// guidance inverted (the hook now auto-passes interactive TTY
// commands through, so agents must NOT pre-emptively prefix
// TOKENADE_HOOK_DISABLED "just in case"; bypasses are also measured
// as lost savings now), plus web-tool guidance (serp_compact /
// web_html_to_markdown were near-unused: 0.3% of calls).
// Bumped 9 → 10 (2026-06): added the "batch independent work into one turn"
// rule (read_many / run_batch / parallel tool calls) + "don't re-Read a file
// already read this session" — turn count drives cost (cache re-read every
// turn), the biggest lever; see THOL input-token breakdown.
// Bumped 11 → 12 (2026-06 pivot): hook-first. The MCP server is no longer
// scaffolded by default (bench: MCP manifest is a net parasite at ctc=0; a
// minimal CLI nudge beats a proactive multi-rule block). scaffold_block()
// collapsed to a single bare CLI exploration nudge — no `mcp__tokenade__*`,
// no proactive multi-rule prose (a verbose proactive nudge measured WORSE
// than hook-only). Bumping forces existing repos to splice over the v4 block.
// Bumped 12 → 13: add ONE line advertising `tokenade exec` (think-in-code).
// The think-in-code sandbox survived the MCP retirement as a CLI command
// (`tokenade exec`), but nothing told the agent it exists — the only lever
// that has to be INVOKED rather than firing passively, and historically the
// single biggest saver (it keeps large intermediate data out of context).
// One terse clause, not a proactive block, so we don't regress the
// verbose-nudge-is-worse finding.
// Bumped 13 → 14 (2026-06 #1072): the code-intel CLIs (skeleton/query/
// impact) are now variadic — advertise `<file…>`/`<symbol…>` so the
// agent passes MULTIPLE targets in ONE invocation instead of one per
// turn. Turn count drives cost (full conversation re-read from cache
// every turn); batching N lookups into 1 turn is the same lever as
// read_many/run_batch. The bench showed an agent chaining `tokenade
// map; tokenade skeleton x; tokenade query y` across N sequential
// turns and losing to a control that batched — this closes that gap.
// Still one terse clause, not a proactive block (verbose-nudge-worse).
// Bumped 14 → 15 (2026-06 regression fix): restore the bulk-edit clause
// for `tokenade exec`. The v14 scaffold only advertised exec for
// "filtering or aggregating a large output" (reading data) — it dropped
// the "mechanical edit across many files → ONE script, not Read+Edit
// per file" lever that historically drove code-migration −52% (control
// derails 42 turns → tokenade 8). That clause lived in the retired
// tokenade-mcp MCP_INSTRUCTIONS and was never ported to the scaffold,
// the only channel that still reaches Claude Code (hooks-only). Keeps
// the Read-before-Edit guard so we don't reopen the "don't pre-inspect"
// cascade regression. Still one terse clause.
// Bumped 15 → 16 (2026-06): extend the `tokenade exec` clause to cover
// READ-side fan-out, not just edits — answering several questions about
// the same data, or pulling facts from several files, in ONE script
// instead of one awk/grep/Read per question. The bench showed an agent
// running 4 separate awk passes over one big file (data-bigvolume) and
// 4 sequential Reads to extract from several files (html-extract),
// paying a turn each, when one exec script would do it in one. Still one
// terse clause appended to the existing exec sentence (verbose-worse).
// Bumped 16 → 17 (2026-06 #1072): forceful scaffold batching rule (see
// commit 7e928811) + reactive per-session Read nudge.
// Bumped 17 → 18 (2026-06 regression fix): scope the exploration CLIs.
// The block led with `map`/`skeleton`/`query`/`semantic`, nudging the
// agent to explore even when the task already named a file. rep=1 bench:
// code-debug-cilog +112% (agent ran `tokenade map`+`find`+`Read AGENTS.md`
// on a one-file "read build.log" task before answering), doc-digest +10%
// and code-overview-cobra indirection followed the same shape. Prepend one
// terse scoping sentence: exploration CLIs are for when you DON'T know
// where code lives; a named file or known path → open it directly, no
// `map`/`semantic`/`skeleton` round-trip first. Complements the existing
// "to read/digest one file use Read directly" clause. Still one clause,
// not a proactive block (verbose-nudge-worse holds).
// Bumped 18 → 19 (2026-06): simplify the wording. Same levers (scoping,
// the CLI list, variadic batching, exec for filter/multi-question/
// bulk-edit, the Read-before-Edit guard, "use Read for one file") but
// stripped of redundant qualifiers and run-on phrasing — ~250 → ~150
// tokens. The scaffold is a fixed per-session tax (CLAUDE.md is loaded
// every session, and re-injected via additionalContext the session it's
// written), so a clearer, shorter block both reads better and shrinks
// the tax. Bumping forces existing repos to splice over the v18 block.
// Bumped 21 → 22 (2026-06): add a "## Commands" clause telling hookless
// agents to wrap their commands with `tokenade wrap`. tokenade's auto-
// compaction is hook-only, and only TWO agents expose a usable hook —
// Claude Code and Gemini CLI; every other agent (Codex, Cursor, Copilot,
// Aider, Claude Desktop, a bare shell) got ZERO Bash compaction. The clause
// used to be gated by agent NAME ("if you are not Claude Code or Gemini CLI"),
// which was true when written and false by 2026-08-08: NINE agents carry hooks
// (`coverage()` == `Hook`), so seven of them read "you have no hooks, wrap
// every command by hand" — a stale-roster defect in the surface every agent reads.
// A hardcoded roster in prose rots; the clause now points at `tokenade detect`,
// which answers for the agent actually running. Also splits the old
// `exec` paragraph into a "## Read or edit several files: one turn, not N"
// lever (parallel tool calls — the top batchable opportunity sequence
// mining found, previously taught only by a reactive hook) and a
// recentred "## Compute over data with `tokenade exec`" (exec is compute,
// not a file reader). Bumping forces existing repos to splice over v21.
// Refined (post-bench): the "edit several files in parallel" lever and the
// "one mechanical edit → ONE script" lever gave OPPOSITE advice for a
// migration. A bench run took the parallel-Write-all-12-files path (55.6k
// output tokens vs ~3.9k for the script path — 14×). Added an explicit
// exception so the parallel-edit rule defers to the script for the SAME
// mechanical change across many files.
// Bumped 22 → 23 (2026-06): the "## Commands" clause now tells hookless
// Windows agents whose commands are PowerShell/cmd (not bash) to add
// `--shell powershell` / `--shell cmd` so `tokenade wrap` runs them under
// the right interpreter instead of bash.
// Bumped 23 → 24 (2026-06, bench fix): the v22 "Read OR EDIT several files
// in parallel" lever BACKFIRED on migrations — the agent read+edited all 12
// files one-by-one (13 Read + 12 Edit, huge output) instead of one sed
// script, so code-migration-py went +7.7% → +116% vs control (reps=3).
// Scoping it to reads-only only HALVED it (+60%): the eager-read itself was
// the trap (read-all-up-front → edit-all).
// Bumped 24 → 25 (2026-06, bench fix): REMOVED the whole "## Read several
// files: one turn, not N" section. The static prompt telling the agent to
// mass-read/mass-edit was the culprit; deleting it lets the agent fall into
// the cheaper one-script path on migrations like control does. The
// migration→script guidance stays in the exec section. The REACTIVE hook
// nudge (batching_read_nudge) is KEPT — it's the static section that
// backfired, not the hook.
// Bumped 40 → 41 (2026-08-04, accuracy): the exec section said the script
// "runs in a sandbox". `sandbox.rs`'s own module doc is honest that it is not
// one — "the OS-level process boundary + resource caps + env scrubbing, not a
// real syscall jail […] treat scripts as if your shell ran them" — and a probe
// confirmed it: a script wrote outside the cwd and read `$HOME` freely. The
// caps and the env scrubbing ARE real (12 env vars reach the child). Only the
// word "sandbox" was wrong, and it is the one word an agent would lean on when
// deciding whether some script is safe to run. The bump is what makes the
// correction reach projects that already have a scaffolded CLAUDE.md; without
// it the fix would only ever land on new ones.
// Bumped 43 → 44: the `exec` language list is now DERIVED from
// `sandbox::LANGS` instead of frozen in the sentence — it had lost `bash`.
// Same reasoning as the bump below: without it, only new projects get the fix.
// Bumped 42 → 43: the "Commands" clause named Claude Code and Gemini CLI
// as the only hooked agents. Nine carry hooks today, so seven were told to wrap
// by hand what their hook already folds. Without this bump the corrected text
// reaches new projects only — every already-scaffolded project keeps the wrong
// sentence, which is the "a fix that never reaches the installations" class.
// Bumped 49 → 50: co-loaded files (`CLAUDE.md`, `.cursorrules`, `.grokrules`)
// that already hold a full rules block must splice to `pointer_block()`. The
// flag skip hid them — `scaffold_block()` text is unchanged; the on-disk
// *shape* of co-loaded files is what moves.
// Bumped 50 → 51: the CLI lecture moves to each agent's machine-global
// rules file. Project files splice to `pointer_block()` when that agent
// has a global address; `ensure_global_rules` runs from `ensure` and
// from the post-upgrade refresh so existing installs migrate.
// Bumped 51 → 52: a global write with no `<!-- tokenade-scaffold -->`
// used to APPEND. The unmarked healer now recognises Searching /
// Reading / Fetching, and each global is rendered for the agent that
// *reads* the file (not who launched tokenade).
// Bumped 52 → 53: unmarked healer also drops a truncated
// `## Keep output lean` / « Keep prose terse. » stub left outside the
// marker, and a duplicate `## Project rules` heading (including
// leftover `~/CLAUDE.md`).
// Bumped 53 → 54: nested crate `CLAUDE.md` / `AGENTS.md` splice to
// pointers on CLI start (`heal_project_lectures`), and a
// `<!-- tokenade:start -->` fence without end marker is stripped.
// Bumped 54 → 55: upgrade wipes every Tokenade-owned region (markers,
// old fence, our H2 titles) then writes the current block. Bodies are
// not fingerprinted — covering every prior wording was the class of bug.
// Bumped 55 → 56: no more pointer. A project file carries the complete
// lesson (a present reader has no global) or nothing — our block removed,
// the file deleted if only we remained. OpenCode, Copilot CLI and Kilo
// have a global.
pub const SCAFFOLD_VERSION: u32 = 58;

/// Short 1.1.18 fence. The lesson now lives in the same file.
pub(crate) const OLD_CLAUDE_FENCE_START: &str = "<!-- tokenade:start -->";
pub(crate) const OLD_CLAUDE_FENCE_END: &str = "<!-- tokenade:end -->";

/// Output-brevity + code-minimalism rules. Shared single source of truth,
/// injected in TWO places: the project scaffold below (main agent — loaded
/// once per session via CLAUDE.md) AND the subagent preamble in
/// `handle_subagent_pretool` (per Task/Agent call). Targets the OUTPUT
/// lever — the tokens the agent WRITES (prose and code), which output
/// compaction can't touch. Combines response concision with code
/// minimalism (reuse-before-write, YAGNI) AND safety guards (root-cause
/// fixes, no flimsier algorithm, flag deliberate shortcuts) so brevity
/// never costs quality. The reuse rung points at tokenade's own
/// `query`/`skeleton`/`impact`, which find existing code to reuse. Kept
/// tight: every byte is paid as input on every session/call.
pub const OUTPUT_BREVITY_RULES: &str =
    "Keep prose terse and code minimal — every token you write is billed as output.\n\
- **Prose:** answer directly — no preamble, recap, tool-call narration, summary, \
or emoji. Drop articles, filler (*just/really/basically/simply*) and hedging; \
fragments fine; short word over long.\n\
- **Output:** don't paste long raw output — quote the shortest decisive line. No \
decorative tables.\n\
- **Code:** write the least that works; reuse before adding (`query` / `skeleton` \
/ `impact`, stdlib, platform feature — YAGNI).\n\
- **Verbatim:** keep code, identifiers, API/CLI names and error strings exact — \
never abbreviate or paraphrase. Keep the user's language.\n\
- **Correctness first:** fix root causes not symptoms, don't downgrade the \
algorithm, don't guess APIs/flags/versions — verify.\n\
- **Full prose where terseness could mislead:** security/data-loss warnings, \
irreversible-action confirmations, multi-step sequences.";

/// The tokenade rules block that gets appended/written to each rules
/// file. Hook-first pivot: a single minimal nudge toward the tokenade
/// CLI for code exploration. The marker stays constant so we can
/// match-and-replace the stale block on upgrade.
/// The document formats tokenade can turn into text. Kept as ONE list so the
/// scaffold cannot drift from what `docs::Format` actually extracts — an agent
/// told it can read a format we do not handle will loop on an unreadable
/// binary, which is worse than not advertising it at all.
pub const DOC_EXTENSIONS: &str =
    ".pdf .docx .doc .xlsx .xls .xlsb .pptx .ppt .odt .ods .odp .odg .epub .rtf .fb2 .msg \
(and their flat-XML, macro-enabled and template variants)";

/// Images tokenade can decode. Formats the agent cannot display itself are
/// converted to PNG automatically, so this list is a capability gain, not only
/// a cost one.
pub const IMAGE_EXTENSIONS: &str =
    ".png .jpg .jpeg .gif .webp .bmp .tif .tiff .ico .tga .pnm .pbm .pgm .ppm .qoi .hdr";

/// Audio/video, which tokenade summarises (and transcribes when subtitles or a
/// local whisper are available). Separate from documents because the answer is
/// metadata + transcript, not the file's text.
pub const MEDIA_EXTENSIONS: &str =
    ".mp4 .mkv .mov .webm .avi .mp3 .wav .m4a .flac .ogg .opus (and other common containers)";

/// The one line telling an agent what tokenade adds to its file reading.
///
/// Two different truths, and stating the wrong one is harmful: where we hook
/// the Read tool the capability is INVISIBLE (the agent just Reads a .pdf and
/// gets text), so we say the tool gained the ability. Everywhere else the
/// capability exists but must be CALLED, so we give the command instead — an
/// agent told "your Read handles PDFs" on a shell-only integration would loop
/// on binary garbage.
/// The one line telling an agent how to search the web with us.
///
/// Same two-truths problem as [`read_capability_line`]. Where we intercept the
/// native search tool the capability is invisible and must NOT be advertised as
/// a command — the agent would call it redundantly. Everywhere else it is the
/// only search we offer, and staying silent means the agent burns a native
/// search (or has none at all).
fn search_capability_line(hooks_websearch: bool) -> String {
    if hooks_websearch {
        String::new()
    } else {
        String::from(
            "## Searching the web\n\
`tokenade search \"<query>\"` — one query, fanned out across several \
independent search engines, merged, deduped and ranked by cross-engine \
agreement. Returns title/url/snippet only, so it costs a fraction of reading a \
results page. Add `--json` for machine-readable output.\n\
Follow a result with `tokenade web <url>` to read the page itself.\n\
\n",
        )
    }
}

mod gitignore;
mod global;
mod housekeeping;
mod targets;
mod write;
pub use gitignore::*;
pub use global::*;
pub use housekeeping::*;
pub use targets::*;
pub(crate) use write::SCAFFOLD_END_MARKER;
use write::*;

/// Told to EVERY agent, hooked or not (D13), because batching is always cheaper:
/// for a hooked agent, three native WebFetch calls are three round-trips of
/// tool-call overhead, whereas `tokenade web <u1> <u2> <u3>` (via Bash) is one
/// call fetched concurrently; for a hookless agent it is the only web path
/// anyway. A capability nobody calls is worthless.
pub const BATCH_WEB_LINE: &str = "## Fetching or searching several things\n\
Do them in ONE call — `tokenade web <url1> <url2> …` / \
`tokenade search \"<q1>\" \"<q2>\" …` — they run concurrently, so you pay ONE \
round-trip instead of N and never re-send the context each extra turn would \
have re-sent.\n\
\n";

/// [`BATCH_WEB_LINE`] without the search half, for `TOKENADE_NO_WEBSEARCH=1`.
pub const BATCH_FETCH_LINE: &str = "## Fetching several pages\n\
Do them in ONE call — `tokenade web <url1> <url2> …` — they run concurrently, \
so you pay ONE round-trip instead of N.\n\
\n";

fn read_capability_line(hooks_read: bool) -> String {
    if hooks_read {
        format!(
            "## Reading documents & media\n\
tokenade extends your `Read` tool: reading {DOC_EXTENSIONS} returns extracted \
text instead of failing on the binary; {MEDIA_EXTENSIONS} returns what the file \
is plus a transcript when one is available; and {IMAGE_EXTENSIONS} are decoded \
for you — any image format you cannot display yourself is converted to PNG \
automatically. Just Read the path as usual.\n\
For a big document, asking beats reading it whole — \
`tokenade read <file> --prompt \"q1, q2\"` returns only the passages that answer, \
and putting several questions in ONE comma-separated call is the CHEAPEST \
option in tokens spent: each is answered under its own heading in ONE \
round-trip, instead of re-sending the context once per question.\n\
\n"
        )
    } else {
        format!(
            "## Reading documents & media\n\
tokenade can read {DOC_EXTENSIONS} (extracted text), {MEDIA_EXTENSIONS} (what \
the file is, plus a transcript when available) and {IMAGE_EXTENSIONS} (the text \
they show, when a local tesseract is installed) — your own file reader cannot. \
Use:\n\
`tokenade read <file>` — the document as text\n\
`tokenade read <file> --prompt \"q1, q2\"` — only the passages answering your \
questions; several questions in ONE comma-separated call is the CHEAPEST option \
in tokens spent, answering each under its own heading in ONE round-trip \
instead of re-sending the context once per question.\n\
\n"
        )
    }
}

/// Pulled out of the render so the rules text has ONE definition on every
/// platform: the fingerprint gate below has to see the same bytes whether it
/// runs on Windows or not.
pub(crate) const WINDOWS_SHELL_NOTE: &str =
    " On Windows, if your commands are PowerShell or cmd (not bash), add \
`--shell powershell` or `--shell cmd` so they run under the right interpreter: \
`tokenade wrap --shell powershell '<cmd>'`.";

/// The STYLE line, when the user picked one.
///
/// The style preamble had only one outlet: a prompt hook. Agents that have
/// none — cursor, windsurf, devin, antigravity — therefore never received
/// anything, even though they all read a rules file this install already
/// writes. A rules file IS a prompt channel: it enters the context every
/// session.
///
/// It is NOT in `scaffold_block()` and that is deliberate: that block is
/// addressed by its content (the fingerprint gate compares its bytes from one
/// platform to another), and making it depend on a user setting would make two
/// machines with the same product diverge. The line is therefore added to what
/// is WRITTEN, not to what is compared, and the splice stays idempotent: it
/// compares the current rendering to the current file, so changing style
/// refreshes the block and changing nothing writes nothing.
///
/// `auto` writes NOTHING, and that is the point: `auto` means "classify every
/// prompt", which a static file cannot do. `resolve("")` returns `Off` on an
/// empty prompt, so only a PINNED style (`chat` / `coding`) reaches the file.
fn style_line(root: &Path) -> String {
    let preamble = crate::style::load(root).resolve("").preamble();
    if preamble.is_empty() {
        return String::new();
    }
    format!("\n{preamble}\n")
}

pub fn scaffold_block(subagent_note: bool) -> String {
    scaffold_block_for(crate::detect::from_env(), subagent_note)
}

/// CLI lesson for ONE agent. `from_env()` is who *runs* tokenade (Grok
/// installing, a test, a `--version`) — not who *will read* the file.
/// Without this, `~/.claude/CLAUDE.md` received the sentence "your own file
/// reader cannot" as soon as Grok wrote the global, then "extends your Read
/// tool" at Claude's SessionStart: two contradictory versions glued together.
pub fn scaffold_block_for(agent: crate::detect::Agent, subagent_note: bool) -> String {
    render_block(
        agent.hooks_read(),
        agent.hooks_websearch(),
        subagent_note,
        true,
    )
}

/// Lesson for an agent's GLOBAL file — only it reads it. An agent whose
/// shell hook is wired has nothing to learn from "Commands": the hook
/// compacts without it knowing, and "wrap / absolute path / stderr" cost it
/// ~150 tokens per session to describe a path it does not take.
/// Project files, shared between agents, keep the section.
pub fn global_block_for(agent: crate::detect::Agent, subagent_note: bool) -> String {
    global_block_for_readers(&[agent], subagent_note)
}

/// A global that SEVERAL agents read (`~/.claude/CLAUDE.md`: Claude, Grok in
/// compat, Kilo and OpenCode as fallback). A section only drops if EVERY
/// reader has the hook that makes it useless — rendered for Claude alone, it
/// told Grok "tokenade extends your Read tool", false for it.
pub fn global_block_for_readers(readers: &[crate::detect::Agent], subagent_note: bool) -> String {
    render_block(
        readers.iter().all(|a| a.hooks_read()),
        readers.iter().all(|a| a.hooks_websearch()),
        subagent_note,
        !readers.iter().all(|a| a.integration().shell_hook),
    )
}

fn render_block(
    hooks_read: bool,
    hooks_websearch: bool,
    subagent_note: bool,
    with_commands: bool,
) -> String {
    // The brevity rules must reach subagents too. On agents where tokenade
    // CAN inject into a subagent's context (Claude Code: every subagent
    // inherits CLAUDE.md, and Explore/Plan get the rules in their pretool
    // preamble — see `handle_subagent_pretool`), the static note is pure
    // duplication, so we omit it. For agents where we have no per-subagent
    // injection channel (the cross-agent rule files), the note is the only
    // lever, so callers pass `true`.
    let subagent_note: &str = if subagent_note {
        "\n- Applies to the subagents you spawn."
    } else {
        ""
    };
    // The `--shell powershell|cmd` guidance only matters on Windows (where the
    // agent may emit PowerShell/cmd instead of bash). Emit it ONLY when the
    // scaffolding tokenade runs on Windows — on macOS/Linux it's dead weight,
    // and every byte is paid as input on every session.
    let windows_shell_note: &str = if cfg!(windows) {
        WINDOWS_SHELL_NOTE
    } else {
        ""
    };
    let read_capability = read_capability_line(hooks_read);
    // `TOKENADE_NO_WEBSEARCH=1` keeps the agent on its own web search (an
    // organisation that wants queries to leave only through its vendor): the
    // rules then do not send the agent to ours either.
    let web_search_off = crate::env_compat::is_truthy_one("TOKENADE_NO_WEBSEARCH");
    let search_capability = if web_search_off {
        String::new()
    } else {
        search_capability_line(hooks_websearch)
    };
    let batch_web = if web_search_off {
        BATCH_FETCH_LINE
    } else {
        BATCH_WEB_LINE
    };
    // The `exec` language list, DERIVED from `sandbox::LANGS` rather than
    // frozen in the prose. It read `sh/node/ruby/awk/jq/perl` and had lost
    // `bash`, which `exec --lang zzz` lists among its known languages — the
    // hooked-agents roster defect one size down: a roster in a sentence nobody re-checks.
    // `python` is named in the example just before, so it is excluded here.
    let exec_langs: String = crate::sandbox::LANGS
        .iter()
        .map(|l| l.name)
        .filter(|n| *n != "python")
        .collect::<Vec<_>>()
        .join("/");
    let commands: String = if with_commands {
        format!(
            "## Commands\n\
If you do not have tokenade hooks, use \
`tokenade wrap '<cmd>'` to wrap all your commands — `tokenade health` says \
whether they are INSTALLED, and names the fix when they are not, so you never \
have to guess. If there is an opportunity \
for compacting noisy output, tokenade will find it — and you will waste fewer \
tokens.{windows_shell_note}\n\
An absolute path (`/usr/bin/git`) is intercepted exactly like `git` when hooks \
are installed; where interception goes through your PATH instead, only the \
bare name is seen — so prefer the bare name if you are not sure which you \
have.\n\
`wrap` keeps the exit code exactly, and MERGES stderr into stdout — the two \
streams are compacted as one. `tokenade wrap '\''cmd'\'' 2>err.log` therefore \
writes an empty err.log; redirect from the wrapped command instead \
(`tokenade wrap '\''cmd 2>err.log'\''`).\n\
\n\
"
        )
    } else {
        String::new()
    };
    format!(
        "{SCAFFOLD_MARKER}\n\
## Explore code with the `tokenade` CLI (cheaper than reading whole \
files)\n\
Use these only when you don't yet know where code lives — if you know \
the path, open it directly:\n\
`tokenade map` (repo structure; `map <path>` for one subtree) · `skeleton <file…>` (signatures) · \
`query <symbol…>` (locate a symbol) · `impact <file…>` (dependents) · \
`semantic \"<query>\"` (search by meaning). They take MANY targets per \
call (`tokenade skeleton a.rs b.rs c.rs`) — batch in ONE turn.\n\
\n\
{read_capability}\
{search_capability}\
{batch_web}\
## Compute over data with `tokenade exec`\n\
`tokenade exec --lang python --script '<code>'` (also \
{exec_langs}) runs a capped subprocess with a scrubbed env \
— your permissions, not a jail — and returns ONLY its stdout. Data you PIPE in reaches the script's stdin (`cmd | tokenade exec …`), so a big output is filtered where it is produced instead of entering your context. Use it to COMPUTE over data — filter/aggregate a large or \
structured output, pull facts across SEVERAL files, or apply one \
mechanical edit across many files (migration, find-replace) — in ONE \
script, not one command per item. It is NOT a file reader: to read \
content, use the parallel reads above, not `exec`. Keep scripts SHORT \
(aim ≤ ~20 lines): exec is for throwaway one-shot computation, not for \
code you will edit and iterate on — every script char is billed as \
output, and a long script usually means a simpler command (or a real \
file you Write once and run) does it cheaper. Long or quote-heavy \
script? `--script-file <path>` (or `--script -` on stdin) avoids shell \
quoting entirely.\n\
\n\
{commands}## Keep output lean\n\
{OUTPUT_BREVITY_RULES}{subagent_note}\n\
<!-- /tokenade-scaffold -->\n"
    )
}

/// The scaffold rules as plain text WITHOUT the HTML sentinel lines —
/// for injecting via a SessionStart hook's `additionalContext`.
///
/// Why this exists: Claude Code loads `CLAUDE.md` into the context
/// window at session boot, BEFORE SessionStart hooks run. So a
/// `CLAUDE.md` that `ensure()` *just* wrote (first session in a
/// project, or a `SCAFFOLD_VERSION` bump) is only visible to the NEXT
/// session. To make the rules land in the CURRENT session we re-emit
/// them as `additionalContext` — the same content as
/// [`scaffold_block`], minus the `<!-- tokenade-scaffold -->` /
/// `<!-- /tokenade-scaffold -->` markers (those only matter for the
/// on-disk in-place upgrade path, not for context injection).
pub fn scaffold_block_plain() -> String {
    // Injected as a Claude SessionStart additionalContext; Claude subagents
    // get the brevity rules via CLAUDE.md inheritance / the pretool preamble,
    // so no static subagent note is needed here.
    scaffold_block(false)
        .lines()
        .filter(|l| {
            let t = l.trim();
            t != SCAFFOLD_MARKER && t != SCAFFOLD_END_MARKER
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Result of `ensure`: which files were touched, for telemetry.
#[derive(Debug, Default)]
pub struct ScaffoldResult {
    pub files_written: Vec<String>,
    pub skipped: bool,
}

/// Marker file recording that this project's scaffold blocks were RETRACTED
/// because the machine hit its quota. Its presence means "restore me once
/// the quota is back"; `ensure()` refuses to (re)write scaffold while the
/// machine is gated, so an UPGRADE (SCAFFOLD_VERSION bump, npm postinstall)
/// can never sneak the rules back into a paused machine's projects.
fn retracted_marker(root: &Path) -> PathBuf {
    root.join(".tokenade").join("scaffold.retracted")
}

/// Remove the tokenade rules block (marker → end-marker) from every rules
/// file of this project. User content is never touched — only our marked
/// block is spliced out; unreadable/absent files are skipped. Returns the
/// files actually modified.
pub fn retract(root: &Path) -> Vec<String> {
    let mut candidates: Vec<PathBuf> = PRIMARY_AGENT_FILES.iter().map(|n| root.join(n)).collect();
    candidates.extend(detect_installed_agents(root));
    candidates.extend(AGENT_PROJECT_FILES.iter().map(|n| root.join(n)));
    candidates.extend(leftover_project_files(root));
    let mut touched = Vec::new();
    for path in candidates {
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (Some(start), Some(end_at)) = (
            existing.find(SCAFFOLD_MARKER),
            existing.find(SCAFFOLD_END_MARKER),
        ) else {
            continue;
        };
        if end_at < start {
            continue; // merge-mangled markers — leave alone (same guard as write path)
        }
        let end = end_at + SCAFFOLD_END_MARKER.len();
        let prefix = existing[..start].trim_end();
        let suffix = existing[end..].trim_start_matches('\n');
        let mut out = String::with_capacity(prefix.len() + suffix.len() + 2);
        out.push_str(prefix);
        if !prefix.is_empty() && !suffix.is_empty() {
            out.push_str("\n\n");
        } else if !prefix.is_empty() {
            out.push('\n');
        }
        out.push_str(suffix);
        if crate::paths::write_atomic_repo_scoped(root, &path, &out).is_ok() {
            touched.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    touched
}

/// Keep the on-disk scaffold in step with the quota gate — the PO contract
/// (2026-07-13): past the limit, the NEXT tokenade invocation removes every
/// tokenade prompt from the project's rules files; once quota is back, the
/// next invocation restores them. Wired into `compaction_gate()` so it fires
/// on EVERY channel — all agents (the rules files come from
/// `detect_installed_agents`), all shells (PATH shim / PowerShell / cmd route
/// through the same chokepoint), all OSes (plain fs ops). State lives in
/// `.tokenade/scaffold.retracted` (a JSON list of the files we actually
/// stripped) so both directions are one `stat` when nothing changed, and the
/// restore is SURGICAL: only the files we retracted get their block back —
/// running gated in a non-project dir never seeds a new scaffold on recovery.
/// Returns what happened, if anything ("retracted"/"restored").
pub fn sync_gate_state(root: &Path, gated: bool) -> Option<&'static str> {
    let marker = retracted_marker(root);
    match (gated, marker.exists()) {
        (true, false) => {
            let touched = retract(root);
            if touched.is_empty() {
                // Nothing of ours here (not a scaffolded project) — no state
                // to record, nothing to restore later.
                return None;
            }
            crate::platform::ensure_parent_dir(&marker);
            // `.tokenade/` sits inside the repo too — a clone can ship it
            // pre-populated with symlinks. Same guard, same reason.
            let _ = crate::paths::write_atomic_repo_scoped(
                root,
                &marker,
                &serde_json::to_string(&touched).unwrap_or_else(|_| "[]".into()),
            );
            crate::debug_log::log(
                "project-scaffold",
                "gate_retracted",
                serde_json::json!({ "files": touched }),
            );
            Some("retracted")
        }
        (false, true) => {
            let files: Vec<String> = std::fs::read_to_string(&marker)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            // Remove the marker FIRST: ensure()/write_scaffold_to are guarded
            // on it, and a half-restored project self-heals on the next call.
            let _ = std::fs::remove_file(&marker);
            let mut restored = Vec::new();
            for rel in &files {
                let p = root.join(rel);
                let note = p.file_name().and_then(|n| n.to_str()) != Some("CLAUDE.md");
                if write_scaffold_to(root, &p, note) == Some(true) {
                    restored.push(rel.clone());
                }
            }
            crate::debug_log::log(
                "project-scaffold",
                "gate_restored",
                serde_json::json!({ "files": restored }),
            );
            Some("restored")
        }
        _ => None,
    }
}

#[cfg(test)]
include!("project_scaffold_tests.rs");
