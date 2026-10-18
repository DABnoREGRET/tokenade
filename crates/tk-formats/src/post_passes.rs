//! The passes that apply AFTER routing.
//!
//! Once a command compactor has answered, its answer still goes through
//! a ladder of passes that only SHORTEN: replay of cursor
//! redraws, folding of repetitive runs, path relativisation, folding of
//! separators, removal of build-artifact lines. None of them
//! chooses a route; all of them rework the TEXT.
//!
//! They used to live in `filter.rs` in the middle of the router, and that is the
//! reason that file shipped 3,536 lines. The subject is the same
//! as that of the submodules already there (`artifacts`, `cargo`, `git`,
//! `remote_argv`): what `filter` does, filed by what it DOES.
//!
//! The constraint the shape carries: a pass here only returns `Some` if
//! it actually shortened. `shrink_only_post_passes` depends on it — it is
//! the only thing that guarantees a pass cannot reintroduce the
//! lines a user filter had just removed.

mod artifacts;
use artifacts::*;

/// Replay a cursor-up redraw instead of keeping every frame of it.
///
/// A progress display has two spellings. `\r` rewrites ONE line, and
/// `strip_ansi` already collapses that to its final state. `ESC[<n>A` moves the
/// cursor up n lines and rewrites BLOCK of them — what BuildKit, modern npm and
/// cargo's multi-bar do — and nothing here touched it: four lines redrawn thirty
/// times arrived as a hundred and twenty.
///
/// This is not a fold and it removes no information: the terminal showed four
/// lines, the other hundred and sixteen were overwritten before any human or
/// model saw them. Keeping them is not fidelity, it is transcribing frames of an
/// animation.
///
/// Deliberately narrow. The sequence is honoured only at the START of a line,
/// which is where a redraw emits it; a mid-line cursor move belongs to a
/// full-screen application whose output is not a transcript at all. When the
/// text carries no such sequence the input is returned untouched.
pub fn replay_cursor_redraws(text: &str) -> String {
    // The two early returns below are COST, not conditions: the rebuild
    // reproduces its input byte for byte when no redraw fired, so removing
    // them changes nothing a test can see. Both survive mutation green for
    // that reason, and they stay because the overwhelming majority of inputs
    // carry no escape at all and should not pay for a line-by-line rebuild.
    if !text.contains("\x1b[") {
        return text.to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut saw_one = false;
    for raw in text.split_inclusive('\n') {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        let (acts, rest) = read_leading_redraw(body);
        // Only a MOVE licenses the rebuild. Erase-line and cursor-home on their
        // own are stray control bytes, and removing those is `strip_ansi`'s
        // job, not this function's — it replays a redraw or it does nothing.
        if acts
            .iter()
            .any(|a| matches!(a, Redraw::Up(_) | Redraw::ClearAll))
        {
            saw_one = true;
        }
        for act in acts {
            match act {
                Redraw::Up(n) => {
                    let n = n.min(lines.len());
                    // A redraw never overwrites an ERROR.
                    //
                    // This is the one place the replay stops being faithful to
                    // the screen, and the trade is deliberate: the repo's own
                    // rule, written on `compress::is_error_line` ("never
                    // dropped by `smart_fold_indices`") and encoded in the
                    // router fuzzer's fourth invariant, is that for an error
                    // "elision disclosure is NOT enough — the agent needs the
                    // actual location to act". A build that prints
                    // `error[E0433]: …` and then redraws a progress block over
                    // it costs the agent exactly the line it needed.
                    //
                    // The unit is the WINDOW, not the line. `is_error_line`
                    // matches the keyword line and not the ` --> src/mod.rs:41`
                    // under it, so stopping AT the error keeps the word and
                    // drops the location — half a diagnostic, which is the one
                    // outcome this rule exists to prevent. Refusing the whole
                    // pop keeps the frame intact and bounds the over-retention
                    // at `n` lines either way.
                    let overwrites_an_error = lines[lines.len() - n..]
                        .iter()
                        .any(|l| crate::compress::is_error_line(l));
                    if !overwrites_an_error {
                        lines.truncate(lines.len() - n);
                    }
                }
                // Erasing the whole display says everything above is
                // gone from the screen. `watch`, `docker stats` and several
                // test runners redraw that way, and the argument is the one
                // cursor-up already makes: the frames before the clear were
                // overwritten before anyone saw them. Measured on 25 frames of
                // `watch kubectl get pods`, 7 738 bytes for what the terminal
                // showed once.
                Redraw::ClearAll => lines.clear(),
                // Erase-line and cursor-home carry nothing once the redraw has
                // been replayed; they are consumed and drop out.
                Redraw::Consume => {}
            }
        }
        lines.push(rest.to_string());
    }
    if !saw_one {
        return text.to_string();
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// What a leading CSI sequence does to the transcript so far.
enum Redraw {
    /// `ESC[<n>A` — the next lines overwrite the last n.
    Up(usize),
    /// `ESC[2J` / `ESC[3J` — everything above is gone from the screen.
    ClearAll,
    /// `ESC[K` (erase line), `ESC[H` (home): part of the grammar, says nothing
    /// once replayed.
    Consume,
}

/// Read the RUN of redraw directives a line opens on, and return what follows.
///
/// A run, because a clear is spelled `ESC[2J` `ESC[H` — two sequences with
/// nothing between them. Deliberately anchored at the START of the line: that
/// is where a redraw emits them, and a mid-line cursor move belongs to a
/// full-screen application whose output is not a transcript at all.
fn read_leading_redraw(line: &str) -> (Vec<Redraw>, &str) {
    let mut acts = Vec::new();
    let mut rest = line;
    loop {
        let Some(after_csi) = rest.strip_prefix("\x1b[") else {
            break;
        };
        let digits: String = after_csi
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ';')
            .collect();
        let Some(final_byte) = after_csi[digits.len()..].chars().next() else {
            break;
        };
        let tail = &after_csi[digits.len() + final_byte.len_utf8()..];
        let act = match final_byte {
            'A' => {
                let n = if digits.is_empty() {
                    1
                } else {
                    match digits.parse::<usize>() {
                        Ok(n) => n,
                        Err(_) => break,
                    }
                };
                Redraw::Up(n)
            }
            'J' if digits == "2" || digits == "3" => Redraw::ClearAll,
            'K' => Redraw::Consume,
            'H' => Redraw::Consume,
            _ => break,
        };
        acts.push(act);
        rest = tail;
    }
    (acts, rest)
}

/// The only-ever-shrink, lossless post-passes — ONE composition, both doors.
///
/// #1 strip the cwd prefix, #2 fold separator rules, #3 drop always-noise build
/// artifacts from file listings, #4 fold runs of template-equal lines. Benign
/// deletions — no stash / `expand_ref` footer for the first three (per design:
/// the original is one re-run away); #4 carries its own count and pointer.
/// Kept only if it genuinely helped.
///
/// This block used to live inside `filter_command` and therefore ran on the
/// COMMAND door alone. The CONTENT door (`tokenade read`, the Read hook, MCP
/// results, `exec`, the Gemini bridge, `llm_proxy`) reached none of it, and
/// that is the same door gap for the fourth time — after the compactor and
/// the floor, this is the post-passes. Measured on a BuildKit-style build
/// log that redraws four lines with `ESC[4A` thirty times: 4 677 bytes came
/// back as 267 through `wrap` and as 4 678 through `read`.
///
/// `argv` is passed rather than assumed because exactly ONE of the four is
/// command-specific: `drop_build_artifact_lines` gates on
/// `is_file_listing_cmd`, which is what makes dropping `__pycache__` right
/// under `find` and wrong under `cat`. Handed an empty `argv` it returns its
/// input untouched, so the content door gets the other three and not that one
/// — declared as an argument rather than spelled twice, the same shape
/// `Unclaimed` uses for the router disagreement.
pub fn shrink_only_post_passes(argv: &[String], raw: &str, routed: &str) -> Option<String> {
    shrink_only_post_passes_keeping_rows(argv, raw, routed, false)
}

/// [`shrink_only_post_passes`], told whether a command's OWN compactor kept
/// every row on purpose. Then the repetition fold — which re-folds the RAW
/// output into a template and a count — may not undo that decision: `ls -l`
/// keeps every file line by design (`ls_table`: "every file line survives"),
/// and measured 2026-10-02 on `ls -la` of this repository the fold turned 44
/// `…-build-NNN.log` lines into two templates with ONE example name
/// each — 42 file names gone from a listing whose names are the answer. The
/// lossless passes still run.
pub fn shrink_only_post_passes_keeping_rows(
    argv: &[String],
    raw: &str,
    routed: &str,
    rows_kept_on_purpose: bool,
) -> Option<String> {
    let mut best: Option<String> = None;
    // `crate::pem` joins the first group because it is the same kind of pass:
    // it removes nothing a reader could want. A certificate body is base64 that
    // no model can decode, sitting beside the parsed `subject=` / `issuer=` /
    // validity the same command already printed in the clear. It is HERE and
    // nowhere else on purpose — a rule spelled twice is a rule neither copy can
    // be tested for — and here means every command route gets it, which the
    // measurement demanded: on a live `openssl s_client -showcerts` the
    // dedicated openssl compactor and the generic fallback BOTH returned 0.0%.
    //
    // `crate::ssh_keys` sits beside it for the same reason and on the same
    // terms: a public key body in `known_hosts` / `authorized_keys` is base64
    // no two lines of which repeat, and the hosts and key types around it are
    // the whole of what a reader wants. Two passes, not two spellings of one
    // rule — they parse different declared formats and die to different
    // mutations.
    //
    // Command path only, like the repetition fold below and for the same
    // reason: a document handed to `read` comes back as its own bytes.
    let cleaned = drop_build_artifact_lines(
        argv,
        &fold_separator_rules(&relativize_cwd_paths(&replay_cursor_redraws(
            &crate::ssh_keys::fold_ssh_key_bodies(&crate::pem::fold_pem_blocks(routed).0).0,
        ))),
    );
    if cleaned.len() < routed.len() {
        best = Some(cleaned);
    }
    // The key folds again, on the RAW text this time, as a COMPETING candidate.
    //
    // They are the only two passes here whose input a compactor can destroy
    // before they see it: both need a block that OPENS and CLOSES, and a
    // routed answer that trimmed either delimiter leaves nothing to parse.
    // MEASURED 2026-09-06 on a `git log --show-signature` transcript — the
    // fold turns 1 115 bytes into 408, and the routed answer delivered 986
    // with the signature whole, because `auto:git-log` had already touched the
    // block. Folding the raw recovers it.
    //
    // Kept only when it is SMALLER than what routing produced, so a compactor
    // that legitimately summarises a thousand lines still wins: on a large log
    // holding one signature, the folded raw is the big one and loses. And the
    // `raw` handed here is already `fold_source`, so a USER filter's answer is
    // never re-expanded — see the caller.
    {
        let folded_raw =
            crate::ssh_keys::fold_ssh_key_bodies(&crate::pem::fold_pem_blocks(raw).0).0;
        let shortest = best.as_deref().unwrap_or(routed).len();
        if folded_raw.len() < shortest && folded_raw.len() < raw.len() {
            best = Some(folded_raw);
        }
    }
    // #4 competes against whatever the first three left, exactly as it did
    // when it lived in `filter_command`.
    //
    // And it runs for a COMMAND only. The first three remove nothing a reader
    // could want — an absolute prefix, a rule of eighty dashes, build noise a
    // named listing command produced. This one is different in kind: it keeps
    // a template and a COUNT and drops the values. On `git log --oneline` that
    // is 141 distinct hashes and 141 distinct subjects replaced by
    // `distinct HEX=141,N=141`, which is the answer the reader asked for.
    //
    // That is exactly what `Unclaimed::Keep` withholds from an unclaimed
    // DOCUMENT, and the reason `generic` is not applied there either.
    // Handing a document to this fold on the content door would be the same
    // mistake wearing a different name — measured, because an e2e guard on the
    // bridges caught it the hour it was written.
    if !argv.is_empty() {
        let base: &str = best.as_deref().unwrap_or(routed);
        // ...and not over an answer whose VALUES are the answer. The fold
        // keeps a template and a count and drops the values, which is right for
        // receipts and wrong for addresses. MEASURED 2026-09-04 on a
        // 53,084-byte accessibility tree of 500 list items: the a11y compactor
        // answered 45,584 bytes carrying all 500 `[ref=eN]`, this fold turned
        // that into 402 bytes reading `[ref=e<N2>]  [×500 lines]` — **zero
        // refs** — and won on size. A ref is what `browser_click` targets, so
        // the agent was handed an answer that looks complete and cannot be
        // acted on.
        if !carries_interaction_handles(base) && !rows_kept_on_purpose {
            if let Some(folded) = fold_repetitive_runs(raw, base) {
                best = Some(folded);
            }
        }
    }
    best
}

/// Does this answer carry handles an agent has to quote back verbatim?
///
/// Today that means a Playwright `[ref=eN]`, the id `browser_click` and
/// `browser_type` take. They are not values a count can stand in for: an icon
/// button with no accessible name can be addressed by nothing else, and the
/// compactor that produced them keeps every one on purpose.
///
/// Deliberately a HANDLE test and not a kind test — this pass runs after
/// routing and sees only text, and a snapshot reaches it under any command
/// name (every MCP tool result and plugin-bridge result arrives that way).
pub fn carries_interaction_handles(text: &str) -> bool {
    /// Enough of them to be a tree rather than a sentence about one.
    const MIN_HANDLES: usize = 3;
    text.matches("[ref=e").count() >= MIN_HANDLES
}

/// Minimum size before the repetition post-pass is worth attempting.
const REPETITION_MIN_CHARS: usize = 4_000;

/// It must remove at least this share of the output to be kept.
const REPETITION_MIN_SHARE: usize = 4; // → a quarter

/// Fold runs of lines that differ only in their variable parts.
///
/// Every tool that does N units of work prints N receipts —
/// `Setting up libpkg7 (1.0.7-1) ...`, `[412/600] Building CXX object …`,
/// `Applying app.0123... OK`, `Running upgrade rev41 -> rev42`. They are all
/// DISTINCT, so nothing that compares whole lines removes them, and a
/// per-command compactor only helps for the tools someone thought to write one
/// for. Six tools failed the failure-survives corpus on exactly this shape in
/// one batch, which is the signal that it wanted solving once rather than six
/// times.
///
/// Safe because the fold groups RUNS of template-equal lines: a diagnostic is
/// unique, so it forms no run and comes through verbatim. Measured on a 400-line
/// `apt-get` failure — 14 715 chars to 227, with both `E:` lines intact.
///
/// Applied only when it earns its place: the output must still be large, and
/// the fold must remove a quarter of it, otherwise the original stands.
fn fold_repetitive_runs(raw: &str, routed: &str) -> Option<String> {
    if routed.len() < REPETITION_MIN_CHARS {
        return None;
    }
    // Fold the RAW output, not the routed one. A generic fallback that has
    // already capped or de-duplicated its input leaves the repetition broken
    // into pieces no template can span — measured on a 400-line `apt-get`
    // failure: folding the raw text gives 227 chars, folding what the fallback
    // produced gives 8 714. The two are candidates for the same job, and the
    // smaller one wins, which is the same rule the format detector already
    // uses when it competes with the generic pass.
    let folded = crate::logs::compact_generic_logs(raw);
    (folded.len() + folded.len() / (REPETITION_MIN_SHARE - 1) < routed.len()).then_some(folded)
}

/// Strip the current working directory prefix from absolute paths, turning
/// them into workspace-relative paths. The agent's cwd IS the workspace, so
/// relative paths resolve identically — this is lossless for the agent and
/// recoverable via `expand_ref`. Many tools (`unittest`, compilers, linters)
/// embed the full absolute path on every line; `pytest`/`cargo` already print
/// relative, so this just normalizes the noisy ones to that convention.
///
/// Conservative: bails out when cwd is unobtainable, the root, or too short
/// to be a real project dir (avoids mangling on `/` or `/tmp`).
// `pub` and not `pub(crate)`: the router's tests, which stayed in
// `tokenade-core`, call this pass by its bare name — `filter.rs` does
// `pub use crate::post_passes::*` and a glob does not propagate what is less
// visible than itself.
pub fn relativize_cwd_paths(text: &str) -> String {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return text.to_string(),
    };
    let cwd = cwd.to_string_lossy();
    if cwd.len() < 8 || cwd.as_ref() == "/" {
        return text.to_string();
    }
    let needle = format!("{cwd}/");
    if !text.contains(&needle) {
        return text.to_string();
    }
    text.replace(&needle, "")
}

/// Collapse a line that is nothing but a run of >5 (i.e. >=6) identical
/// separator characters (`= - _ * # ~`) down to 3 of that character,
/// preserving any leading indentation and the original line ending
/// (CRLF-safe). Any such rule carries one bit of information ("boundary
/// here"); 3 chars keep that boundary. Lines containing spaces (e.g. the
/// `--- ---` markers a Markdown/dash-table compactor emits) are left
/// untouched, as are runs of 5 or fewer (`=====` and shorter stay verbatim).
// `pub` and not `pub(crate)`: the router's tests, which stayed in
// `tokenade-core`, call this pass by its bare name — `filter.rs` does
// `pub use crate::post_passes::*` and a glob does not propagate what is less
// visible than itself.
pub fn fold_separator_rules(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for seg in text.split_inclusive('\n') {
        let (body, nl) = match seg.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (seg, ""),
        };
        let (content, cr) = match body.strip_suffix('\r') {
            Some(c) => (c, "\r"),
            None => (body, ""),
        };
        let trimmed = content.trim();
        let folded = if trimmed.len() >= 6 {
            let fc = trimmed.chars().next().unwrap();
            if matches!(fc, '=' | '-' | '_' | '*' | '#' | '~') && trimmed.chars().all(|c| c == fc) {
                let indent: String = content
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                Some(format!("{indent}{fc}{fc}{fc}"))
            } else {
                None
            }
        } else {
            None
        };
        match folded {
            Some(f) => {
                out.push_str(&f);
                out.push_str(cr);
                out.push_str(nl);
            }
            None => out.push_str(seg),
        }
    }
    out
}

/// #3 — drop always-noise build-artifact paths from a file listing.
///
/// A project inventory (`find . -type f`, `ls -R`, `tree`, `git ls-files`)
/// is dominated, every debug cycle, by paths that carry zero signal for a
/// reader: `__pycache__/*.pyc`, `*.pyo`, `.DS_Store`, `*.egg-info/*` and
/// `.git/` internals. These are benign to drop — the original is one re-run
/// away — so no footer / stash / expand_ref is emitted (per design: tiny
/// benign deletions don't warrant the recovery machinery).
///
/// Two guards keep it safe:
///   - it fires ONLY for known file-listing commands. Otherwise a log or
///     grep line that merely *mentions* a `.pyc` could be deleted.
///   - it passes through untouched when the command EXPLICITLY targets such
///     an artifact (`find -name '*.pyc'`, anything under `.git`/`node_modules`)
///     — there the "noise" is the actual subject of the query.
// `pub` and not `pub(crate)`: the router's tests, which stayed in
// `tokenade-core`, call this pass by its bare name — `filter.rs` does
// `pub use crate::post_passes::*` and a glob does not propagate what is less
// visible than itself.
pub fn drop_build_artifact_lines(argv: &[String], text: &str) -> String {
    if !is_file_listing_cmd(argv) || argv_targets_artifact(argv) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut dropped = false;
    for seg in text.split_inclusive('\n') {
        let body = seg.strip_suffix('\n').unwrap_or(seg);
        let content = body.strip_suffix('\r').unwrap_or(body);
        if line_is_build_artifact(content) {
            dropped = true;
            continue;
        }
        out.push_str(seg);
    }
    if dropped {
        out
    } else {
        text.to_string()
    }
}
