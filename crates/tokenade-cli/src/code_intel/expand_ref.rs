//! `expand-ref` / `expand-fold`: recovering the raw bytes a compactor folded.
//!
//! Split out of `code_intel.rs`, which had grown to hold two unrelated
//! families — recovery (here) and code intelligence (query/impact/map/
//! skeleton, still there).
use super::*;
use crate::*;

/// Tokens this run printed — what the recovery COST, whichever branch served it.
static DELIVERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Set when the post-compaction credit booked this run: that row already nets
/// what was delivered, so the cost must not be charged a second time.
static NETTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `print!` that also counts what reached the agent. Every print of this
/// module goes through it, so the cost booked by [`cmd_expand_ref`] cannot
/// miss a branch.
macro_rules! out {
    ($($t:tt)*) => {{
        let s = format!($($t)*);
        DELIVERED.fetch_add(tokenade_core::tokenizer::estimate(&s), std::sync::atomic::Ordering::Relaxed);
        print!("{s}");
    }};
}
macro_rules! outln {
    () => { out!("\n") };
    ($($t:tt)*) => {{
        let s = format!($($t)*);
        out!("{s}\n");
    }};
}

mod args;
mod budget;
mod grep;
mod missing;
pub(crate) use args::*;
pub(crate) use budget::*;
pub(crate) use grep::*;
pub(crate) use missing::*;

/// Post-compaction recovery credit: a fresh compact-digest marker means this
/// expand-ref recovered bytes the auto-summary had squashed — the alternative
/// was re-RUNNING the original command (whose output was ~the stash size).
/// Estimated, input-side, once per hash per digest window.
///
/// Shared by both expand-ref paths on purpose: the filtered path prints far
/// less, but it recovers the same BLOB, and two paths crediting the same
/// recovery differently is how a ledger stops meaning anything. What differs
/// between them is `delivered` — the tokens this round actually cost — and
/// netting that is not "crediting the same recovery differently": it is
/// charging each path for what it spent. The filtered path legitimately nets
/// out better, because it is better.
///
/// `after` was 0 until 2026-09-02, i.e. "the alternative cost the whole blob
/// and this cost nothing". Both halves were wrong. The recovery round is
/// itself a turn the fold caused, and its output is real tokens; and a re-run
/// in a world that HAS tokenade would have been compacted too, so the honest
/// alternative is never the raw blob at full price. Measured on a dogfood
/// ledger: 43 rows, one of them crediting 393 740 tokens for a single
/// `expand-ref` — 17x the bound every measured row obeys.
fn record_recovery_credit(hash: &str, toks: usize, delivered_toks: usize) {
    let Some(h) = tokenade_core::platform::home_dir() else {
        return;
    };
    let ts_path = h.join(".tokenade").join("compact-digest-ts");
    let fresh = std::fs::read_to_string(&ts_path)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .map(|t| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().saturating_sub(t) < 1800)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if !fresh {
        return;
    }
    let once = h
        .join(".tokenade")
        .join("read-pending")
        .join(format!("digestcredit-{hash}"));
    if once.exists() {
        return;
    }
    tokenade_core::platform::ensure_parent_dir(&once);
    let _ = std::fs::write(&once, b"1");
    NETTED.store(true, std::sync::atomic::Ordering::Relaxed);
    // class is the CANONICAL bucket name ("recovery") — the old literal
    // "digest_recovery" matched no reader arm, so these rows were silently
    // dropped from every live total (migration audit). Readers keep an alias
    // for the legacy rows already on disk.
    tokenade_core::gain::record_estimated(
        "cli",
        "digest_recovery",
        hash,
        "recovery",
        tokenade_core::Savings::new(toks, delivered_toks),
    );
}

/// Dense RAG (+ multi-prompt) over a recovered stash blob — the shared muscle
/// behind `expand-ref <hash> --prompt` AND `search-stash`. A ref is a document
/// you ASK, not a dump handle: this redacts the blob then routes the question(s)
/// through the SAME extractor `read`/`web` use (`prompt_scoped_extract_dispatch`,
/// which splits comma-separated questions into labelled `### Q` blocks). Prints
/// the targeted passages or a no-match note; returns the rc.
fn answer_from_blob(hash: &str, text: &str, prompt: &str) -> i32 {
    let red = tokenade_core::redact::redact_for_model(text);
    let blob_toks = tokenade_core::tokenizer::estimate(&red);
    // Shared core: small blob → whole, large → dense RAG (multi-question aware).
    match tokenade_core::gain::answer_over_blob_asked(&red, prompt, "expand-ref") {
        Some(scoped) => {
            out!("{scoped}");
            // The blob is what the re-run would have cost; `scoped` is what
            // this round did cost. The credit is the difference, bounded like
            // any other row.
            let delivered = tokenade_core::tokenizer::estimate(&scoped);
            record_recovery_credit(hash, blob_toks, delivered);
            0
        }
        // Declining has TWO causes and they deserve opposite answers.
        //
        // The question may be absent from the blob — then "no passage matched,
        // widen it" is right. Or every one of its words may sit on every line,
        // which is what a uniform log looks like: `--prompt "worker status
        // path"` against 4 000 lines of `INFO worker=<N> status=ok
        // path=/var/run/app/<N>` was answered "no passage matched — widen the
        // question", when the true answer is that they ALL match. Widening
        // makes that worse, and the only remaining options were `--grep` or
        // dumping 241 KB — on the single most foldable input there is.
        //
        // So when the blob folds down small, hand over the fold. It says the
        // shape and the count, which IS the answer to a question about what
        // the whole blob says. When the fold is still large the passages
        // really are the better route and the refusal stands.
        None => {
            const FOLD_SERVES_UP_TO_TOKENS: usize = 1_500;
            let folded = tokenade_core::auto::compact(&red).1;
            let folded_toks = tokenade_core::tokenizer::estimate(&folded);
            if !folded.trim().is_empty()
                && folded_toks <= FOLD_SERVES_UP_TO_TOKENS
                && folded_toks < blob_toks
            {
                outln!(
                    "[tokenade] no passage in {hash} singles out {prompt:?} — every line \
                     answers it, or none does. What follows is the whole ref, folded."
                );
                out!("{folded}");
                // The banner one line up says this is the ref FOLDED, so it is
                // a view. `--raw` is the byte-exact path and prints elsewhere.
                if crate::wrap::needs_terminator(&folded, false) {
                    outln!();
                }
                let delivered = tokenade_core::tokenizer::estimate(&folded);
                record_recovery_credit(hash, blob_toks, delivered);
                return 0;
            }
            eprintln!(
                "[tokenade] no passage in {hash} matched {prompt:?} — widen the question, \
                 `--grep <pattern>` for an exact line, or {RAW_ADVICE}"
            );
            1
        }
    }
}

/// The bare hashes, with every flag and every flag VALUE dropped.
fn collect_hashes(rest: &[String]) -> Vec<&str> {
    let mut skip_next = false;
    let hashes: Vec<&str> = rest
        .iter()
        .filter(|s| {
            if skip_next {
                skip_next = false;
                return false;
            }
            // A flag that TAKES a value hides it from this pass, or the value
            // is read as a hash: `--from-line 900` reported "'900' is not a ref
            // hash" the moment the flag was added and the list was not. The
            // list is now `EXPAND_FLAGS`, shared with the parser.
            if expand_flag_takes_value(s) == Some(true) {
                skip_next = true;
                return false;
            }
            !looks_like_a_flag(s)
        })
        .map(|s| unkey(s))
        .filter(|s| !s.is_empty())
        .collect();
    hashes
}

/// Which hash the lines below belong to. Printed only in a batch run — a
/// single hash keeps byte-identical output.
fn print_run_header(hi: usize, multi: bool, hash: &str) {
    if multi {
        if hi > 0 {
            outln!();
        }
        outln!("==> {hash} <==");
    }
}

/// A resolved blob, served the way the flags ask: a RAG answer, grep lines, or
/// the size-gated raw dump. `needs_redact` says whether the bytes still carry
/// secrets — a stash read already redacted them on the way out.
fn serve_blob(hash: &str, content: String, needs_redact: bool, args: &ExpandArgs) -> i32 {
    // A QUESTION takes precedence: RAG (+ multi-prompt) over the raw
    // blob, small docs served whole. This is the primary path.
    if let Some(p) = &args.prompt {
        return answer_from_blob(hash, &content, p);
    }
    let red = if needs_redact {
        tokenade_core::redact::redact(&content)
    } else {
        content
    };
    if !args.grep_pats.is_empty() {
        print_grep_over_text(&red, &args.grep_pats, args.ignore_case);
    } else {
        print_or_gate(hash, &red, args);
    }
    0
}

/// One hash, start to finish: its own exit code, never a shared `rc`.
fn expand_one(
    hash: &str,
    hi: usize,
    multi: bool,
    args: &ExpandArgs,
    root: &std::path::Path,
) -> i32 {
    let ExpandArgs {
        grep_pats, prompt, ..
    } = args;
    let from_dedup = tokenade_core::dedup_ref::lookup(root, hash);
    // A `--grep` against a STASH filters before redacting — see
    // `disclose::grep_stash_by_hash`. Worth the extra arm: it takes this
    // command from ~25 s to well under a second on a 26 MB stash, and
    // `--grep` is the path the spreadsheet skeleton points agents at.
    // A dedup hit falls through: those blobs are small, and the shared
    // path below already filters them.
    // Skipped when a question is set (RAG takes precedence) or on a dedup
    // hit (small blob → in-memory grep below). Repeatable `--grep`: each
    // pattern's matches are printed under its own header.
    if prompt.is_none() && from_dedup.is_none() && !grep_pats.is_empty() {
        if let Some(rc) = serve_stash_grep(hash, grep_pats, args.ignore_case) {
            return rc;
        }
    }
    // `dedup_ref::lookup` returns raw bytes; `read_stash_by_hash` redacts on
    // read. Track which, so the blob is redacted exactly ONCE: the previous
    // unconditional pass below re-redacted the stash a second time, ~12 s of
    // duplicate work on a 26 MB blob for a pass that is idempotent.
    let mut needs_redact = true;
    let content = from_dedup.or_else(|| {
        needs_redact = false;
        tokenade_core::disclose::read_stash_by_hash(hash).map(|(c, _ext)| c)
    });
    // NOTE: the credit is booked per-branch below, NOT here. Booking it
    // up front credited "saved the whole blob" even on the branches that
    // SHIP the whole blob (`--raw`, and any blob under the threshold) — a
    // 50k-token dump both reached the agent and added a 50k saving. The
    // anti-inflation guard cannot catch it because the caller passes
    // `after = 0`.
    match content {
        Some(content) => {
            // The other half of `recovery:offered`: a pointer was followed
            // and this much came back. Recorded on RESOLUTION, not per
            // print site — the branches below serve the same blob three
            // ways (RAG answer, grep lines, raw dump) and the question the
            // pair answers is whether the content was reached at all.
            tokenade_core::gain::record_recovery(
                "expanded",
                tokenade_core::tokenizer::estimate(&content),
            );
            print_run_header(hi, multi, hash);
            serve_blob(hash, content, needs_redact, args)
        }
        None => report_missing_hash(hash),
    }
}

/// `tokenade expand-ref <hash>` — CLI twin of the MCP `expand_ref` tool.
/// Recovers the raw bytes a compactor/hook folded, keyed by the `hash`
/// printed in its banner. Mirrors the MCP handler: dedup.db first, then
/// the disclose stash dir, redact-on-return (defense in depth, #488).
/// This is the no-quality-loss recovery net on agents where the MCP
/// server is no longer registered (2026-06 hook-first pivot).
pub(crate) fn cmd_expand_ref(rest: &[String]) -> i32 {
    // #1072 — batchable: collect every hash so the agent can recover
    // several folded blobs in ONE turn (`tokenade expand-ref a b c`).
    // A single hash keeps byte-identical output (no header).
    //
    // A ref is an interrogable document, not a dump handle. The primary path is
    // `--prompt "q1, q2"` → dense RAG (+ multi-prompt), just the passages that
    // answer. `--grep <pattern>` filters to exact lines. Bare (no question) is
    // the RAW-BYTES escape — and it is size-gated: above the RAG threshold it
    // refuses to re-inflate context and points at `--prompt` instead (`--raw`
    // forces the dump).
    //
    // Field data, and it is the ONE number here that has to be re-read rather
    // than remembered — the gate was built to change it, so quoting the
    // before-figure as if it were current says the opposite of what happened.
    // Re-measured 2026-08-29 over every recorded `expand-ref` invocation
    // (467), split by month:
    //
    //   2026-06   75 bare,   0 raw,  0 grep,  0 prompt   — 100 % bare
    //   2026-07   89 bare,   4 raw, 10 grep, 13 prompt   —  77 % bare
    //   2026-08   64 bare, 102 raw, 36 grep, 74 prompt   —  23 % bare
    //
    // The 107/111 (96 %) that justified the gate is the pre-gate reading. The
    // targeted verbs now carry three calls in four, and `--grep` is the
    // cheapest of all of them: 1 657 bytes on average against 4 996 for a bare
    // dump and 4 424 for `--prompt`. Two figures that did NOT move: 3 of 64
    // August bare calls were above the threshold (the rest are genuinely small
    // blobs, dumping as intended), and 16 % of bare calls re-dump a hash the
    // same session already dumped — which is the caller's to decide, since a
    // context compaction between the two makes the second ask legitimate.
    // ONE hoisting, read by both halves. `parse_expand_args` asks what to do
    // with a flag and `collect_hashes` asks what is NOT a hash; hoisting for
    // only one of them made `--grep -i alpha <hash>` parse correctly and then
    // report `'alpha' is not a ref hash`.
    let rest: &[String] = &hoist_misplaced_flags(rest);
    let args = parse_expand_args(rest);
    if let Some(flag) = args.unknown.first() {
        eprintln!("tokenade expand-ref: unknown flag '{flag}'. Try `tokenade expand-ref --help`.");
        return 2;
    }
    let hashes = collect_hashes(rest);
    if hashes.is_empty() {
        eprintln!(
            "usage: tokenade expand-ref <hash...> [--prompt \"q1, q2\" | --grep <regex> [-i] | --raw]"
        );
        return 2;
    }
    let multi = hashes.len() > 1;
    let root = root();
    let mut rc = 0;
    for (hi, hash) in hashes.iter().enumerate() {
        rc = rc.max(expand_one(hash, hi, multi, &args, &root));
    }
    book_recovery_cost();
    rc
}

/// **A recovery hands back part of the fold's gain.** The fold was credited
/// `raw − folded`; without tokenade the raw output was already in context, so
/// what `expand-ref` prints now is a cost the fold caused, and the net saving
/// is `raw − folded − delivered`. MEASURED 2026-10-05 over 30 days of local
/// transcripts: 1 158 `expand-ref` calls delivered ≈ 0.52 M tokens against
/// 16.4 M credited by folds (3.2 %), none of it ever deducted.
///
/// Booked as `overhead` — the class the dashboard and the server already
/// subtract — priced as fresh input. Not booked when the post-compaction
/// credit fired: that row is `blob − delivered` and has netted it already.
/// What it prints then stays in the context and is re-read on every later
/// call until the conversation compacts: it is logged as injected, and the
/// re-bill charges it like tokenade's own injected text. And the call itself
/// re-read the whole conversation (`record_recovery_turn`).
fn book_recovery_cost() {
    let delivered = DELIVERED.load(std::sync::atomic::Ordering::Relaxed);
    if delivered == 0 || NETTED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    tokenade_core::gain::record_estimated_io(
        "cli",
        "recovery_cost",
        "expand-ref",
        "overhead",
        "input",
        tokenade_core::Savings::new(delivered, 0),
    );
    tokenade_core::gain::note_injected_into_context(delivered as u64);
    tokenade_core::gain::record_recovery_turn("cli", "expand-ref");
}

/// `tokenade expand-fold <template_id> [content_hash] [--max N]` — CLI
/// twin of MCP `expand_fold`. Expands a folded repeated-run marker back
/// to its matched lines. With no content_hash, resolves the most recent
/// fold for the template_id (the common case from a fold marker).
pub(crate) fn cmd_expand_fold(rest: &[String]) -> i32 {
    let pos: Vec<&str> = rest
        .iter()
        .filter(|s| !s.starts_with('-'))
        .map(|s| unkey(s))
        .filter(|s| !s.is_empty())
        .collect();
    let template_id = match pos.first() {
        Some(t) if !t.trim().is_empty() => *t,
        _ => {
            eprintln!("usage: tokenade expand-fold <template_id> [content_hash] [--max N]");
            return 2;
        }
    };
    let content_hash = pos.get(1).copied();
    let max = flag_usize(rest, "--max").filter(|&n| n > 0);
    let slot_filter: std::collections::BTreeMap<String, String> = Default::default();
    let resolved_hash = match content_hash {
        Some(h) => h.to_string(),
        None => match tokenade_core::fold_store::latest_content_hash(template_id) {
            Some(h) => h,
            None => {
                eprintln!("expand-fold: no fold matches template_id={template_id}");
                return 1;
            }
        },
    };
    match tokenade_core::fold_store::expand_fold(&resolved_hash, template_id, &slot_filter, max) {
        Ok(Some(r)) => {
            let mut body = format!(
                "[expand-fold] content_hash={} template_id={}\n  template: {}\n  total fold lines: {} · matched: {}{}\n",
                r.content_hash,
                r.template_id,
                r.template,
                r.total_lines,
                r.matched_lines,
                if r.truncated {
                    " (TRUNCATED — refine the slot_filter)"
                } else {
                    ""
                },
            );
            if r.lines.is_empty() {
                body.push_str("  (no lines match the filter)\n");
            } else {
                body.push('\n');
                for line in &r.lines {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            out!("{}", tokenade_core::redact::redact(&body));
            0
        }
        Ok(None) => {
            eprintln!(
                "expand-fold: no fold for content_hash={resolved_hash} template_id={template_id} \
                 (stale or unindexed)"
            );
            1
        }
        Err(e) => {
            eprintln!("expand-fold: {e}");
            1
        }
    }
}

#[cfg(test)]
mod a_failing_recovery_says_what_to_do {
    /// **41 recoveries out of 436 come back empty-handed** (measured over 758
    /// real transcripts), 15 of them on a missing hash. That sentence named NO
    /// next step — and the same lesson had already been paid for once: a
    /// recourse is judged where the agent reads, not in `--help`.
    ///
    /// The test targets the FUNCTION that composes the message rather than the
    /// binary's output: the function is what decides, and the message goes to
    /// stderr, which the binary harness does not return line by line.
    #[test]
    fn a_missing_hash_names_the_only_next_step_left() {
        let said = super::missing_hash_report("deadbeefdead");
        assert!(
            said.contains("re-run the command"),
            "no next step named:\n{said}"
        );
        assert!(
            said.contains("tokenade raw"),
            "the only command that replays is not named:\n{said}"
        );
        // **And it INFERS nothing the store contradicts.** The first wording
        // concluded "an older hash was swept"; the machine answered "the
        // oldest entry is 17 days old" for a TTL of 7.
        assert!(
            !said.contains("has been swept") && !said.contains("may have expired"),
            "the message infers a rule instead of saying what it knows:\n{said}"
        );
    }

    /// What is not a hash is not an expiry: blaming retention sends the reader
    /// chasing a problem they do not have.
    #[test]
    fn what_is_not_a_hash_is_said_as_such() {
        let said = super::missing_hash_report("pasunhash");
        assert!(said.contains("is not a ref hash"), "{said}");
        assert!(
            !said.contains("re-run the command"),
            "a typo is not fixed by replaying the command:\n{said}"
        );
    }
}

#[cfg(test)]
mod raw_budget_tests {
    use super::*;

    /// Every value-taking flag must be skipped when hashes are collected, or
    /// the value is read as a hash. `--from-line 900` did exactly that.
    #[test]
    fn a_flag_value_is_never_mistaken_for_a_hash() {
        let rest: Vec<String> = [
            "abc123abc123",
            "--raw",
            "--from-line",
            "900",
            "--budget",
            "0",
            "--grep",
            "deadbeefdead",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(collect_hashes(&rest), vec!["abc123abc123"]);
    }

    #[test]
    fn the_flags_parse_into_the_shape_the_printer_reads() {
        let rest: Vec<String> = ["--raw", "--budget", "77", "--from-line", "12"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_expand_args(&rest);
        assert!(a.raw);
        assert_eq!(a.budget, Some(77));
        assert_eq!(a.from_line, 12);
        // A missing `--from-line` starts at the first line, never at 0.
        assert_eq!(parse_expand_args(&[]).from_line, 1);
        assert_eq!(
            parse_expand_args(&["--from-line".into(), "0".into()]).from_line,
            1
        );
    }

    /// `--grep -i "poll"` greps for "poll", ignoring case.
    ///
    /// It used to grep for `-i`. A value-taking flag consumes whatever follows
    /// it, so the pattern became `-i`, `"poll"` fell through to
    /// [`collect_hashes`] as a second hash, and the caller was told
    /// `0 matching line(s)` plus "nothing matches \"-i\" … ignoring case
    /// either" over a stash whose every line held the word, then
    /// `'poll' is not a ref hash`. Reproduced 2026-09-05 from a transcript on
    /// this machine (`tokenade expand-ref 4e17ec351e3c --grep -i "poll"`, and
    /// a second one with seven `--grep -i` in a row): on the RECOVERY command,
    /// where a wrong "it is not there" is the most expensive answer there is.
    #[test]
    fn a_flag_wedged_before_a_value_is_read_where_it_was_meant() {
        let a = parse_expand_args(&["--grep".into(), "-i".into(), "poll".into()]);
        assert_eq!(a.grep_pats, vec!["poll".to_string()]);
        assert!(a.ignore_case, "the hoisted `-i` must still take effect");

        // A whole run of them, and the long spellings too.
        let a = parse_expand_args(&["--grep".into(), "-i".into(), "--raw".into(), "poll".into()]);
        assert_eq!(a.grep_pats, vec!["poll".to_string()]);
        assert!(a.ignore_case && a.raw);

        // Every value-taking flag, not just `--grep`.
        let a = parse_expand_args(&[
            "--prompt".into(),
            "--ignore-case".into(),
            "what does it say".into(),
        ]);
        assert_eq!(a.prompt.as_deref(), Some("what does it say"));
        assert!(a.ignore_case);

        // The escape hatch, for a pattern that really is a flag spelling.
        let a = parse_expand_args(&["--grep".into(), "--".into(), "-i".into()]);
        assert_eq!(
            a.grep_pats,
            vec!["-i".to_string()],
            "`--` must still buy a literal pattern"
        );
        assert!(
            !a.ignore_case,
            "`-i` after `--` is the pattern, not the flag"
        );

        // Correct order is untouched, and a pattern that merely LOOKS like a
        // flag but is none of ours stays the pattern: hoisting only ever moves
        // a flag this command knows.
        let a = parse_expand_args(&["--grep".into(), "poll".into(), "-i".into()]);
        assert_eq!(a.grep_pats, vec!["poll".to_string()]);
        assert!(a.ignore_case);
        let a = parse_expand_args(&["--grep".into(), "-notaflag".into()]);
        assert_eq!(a.grep_pats, vec!["-notaflag".to_string()]);

        // Only VALUELESS flags are hoisted. Two value-taking flags in a row
        // are genuinely ambiguous — `--grep --prompt "q" poll` could mean
        // either — so the conservative reading stands and `--grep` keeps the
        // token after it. Hoisting `--prompt` instead would let it swallow the
        // pattern and leave the grep empty, which is a worse answer than an
        // odd one.
        let a = parse_expand_args(&["--grep".into(), "--prompt".into(), "q".into()]);
        assert_eq!(
            a.grep_pats,
            vec!["--prompt".to_string()],
            "a value-taking flag must not be hoisted over another one"
        );
    }

    /// A flag this command does not accept was dropped in silence, on the one
    /// command an agent reaches for AFTER it has already lost bytes. `--raww`
    /// returned the 313-byte "too large" banner where `--raw` returns 187 550
    /// bytes, exit 0 either way.
    #[test]
    fn a_flag_we_do_not_know_is_named_not_dropped() {
        for typo in ["--raww", "--gerp", "--prompts", "--zzbogus", "-x"] {
            let a = parse_expand_args(&[typo.into(), "abcdef123456".into()]);
            assert_eq!(a.unknown, vec![typo.to_string()], "for {typo:?}");
        }
    }

    /// ...and every flag the command DOES accept stays accepted. A refusal
    /// that also refuses the real flags would be worse than the silence.
    #[test]
    fn every_flag_in_the_table_is_accepted() {
        for (name, takes) in EXPAND_FLAGS {
            let mut argv: Vec<String> = vec![(*name).into()];
            if *takes {
                argv.push("7".into());
            }
            argv.push("abcdef123456".into());
            let a = parse_expand_args(&argv);
            assert!(
                a.unknown.is_empty(),
                "{name} must be accepted, got {:?}",
                a.unknown
            );
            // ...and the hash survives, which is the other half: a flag the
            // collector does not know turns its VALUE into a bogus hash.
            assert_eq!(
                collect_hashes(&argv),
                vec!["abcdef123456"],
                "{name} must not eat the hash, nor leave its value behind"
            );
        }
    }

    /// Every flag the command ACCEPTS must appear in the help an agent reads.
    /// The miss note names `--ignore-case`, and a message pointing at a flag
    /// nobody can find is the defect it was written to avoid.
    #[test]
    fn every_accepted_flag_appears_in_the_help() {
        let help = crate::expand_ref_help_text();
        let mut checked = 0;
        for (name, _) in EXPAND_FLAGS {
            // Word boundary: `--raw` must not be satisfied by `--raww`, nor
            // `-i` by any word holding those two characters.
            let found = help
                .split(|c: char| !(c.is_alphanumeric() || c == '-'))
                .any(|w| w == *name);
            assert!(found, "{name} is accepted but undocumented");
            checked += 1;
        }
        assert!(checked >= 8, "the table went empty, so this proved nothing");
    }

    /// Folding one side of the comparison passes every lowercase fixture and
    /// fails on an uppercase pattern, which is how a half-done fold survives
    /// its own test.
    #[test]
    fn matching_folds_both_sides_or_neither() {
        assert!(line_matches("row 1 Alpha", "Alpha", false), "literal hit");
        assert!(
            !line_matches("row 1 Alpha", "alpha", false),
            "literal is literal"
        );
        assert!(
            !line_matches("row 1 Alpha", "ALPHA", false),
            "literal is literal"
        );
        assert!(
            line_matches("row 1 Alpha", "alpha", true),
            "haystack folded"
        );
        assert!(
            line_matches("row 1 Alpha", "ALPHA", true),
            "needle folded too"
        );
        assert!(
            !line_matches("row 1 Alpha", "zeta", true),
            "absent stays absent"
        );
    }

    /// The flag the miss note points at must exist, in both spellings — a
    /// message naming a door that does not open is worse than silence.
    #[test]
    fn ignore_case_is_a_flag_in_both_spellings() {
        for spelling in ["--ignore-case", "-i"] {
            let a = parse_expand_args(&[spelling.into(), "abcdef123456".into()]);
            assert!(a.ignore_case, "{spelling} must set it");
            assert!(a.unknown.is_empty(), "{spelling} must not be refused");
            assert_eq!(
                collect_hashes(&[spelling.into(), "abcdef123456".into()]),
                vec!["abcdef123456"],
                "{spelling} takes no value, so it must not eat the hash"
            );
        }
        // Absent by default: a literal grep is what a copied token wants.
        assert!(!parse_expand_args(&["abcdef123456".into()]).ignore_case);
    }

    /// `--` ends the flags, as it does everywhere else. It was accepted here
    /// before only by being ignored, so refusing unknown flags must not turn
    /// it into an error.
    #[test]
    fn the_double_dash_ends_the_flags() {
        let a = parse_expand_args(&["--".into(), "abcdef123456".into()]);
        assert!(a.unknown.is_empty(), "`--` is a separator, not a flag");
        assert_eq!(
            collect_hashes(&["--".into(), "abcdef123456".into()]),
            vec!["abcdef123456"]
        );
        // Past it, a dash-leading token is an argument and not a refusal.
        let a = parse_expand_args(&["--".into(), "--raww".into()]);
        assert!(a.unknown.is_empty(), "nothing after `--` is a flag");
    }

    /// A lone `-` is the stdin spelling, not a flag: refusing it as an unknown
    /// flag would be wrong. It is not a hash either, so it reaches the hash
    /// pass and is answered there — "'-' is not a ref hash" says more than a
    /// bare usage line.
    #[test]
    fn a_bare_dash_is_not_a_flag() {
        let a = parse_expand_args(&["-".into()]);
        assert!(a.unknown.is_empty(), "`-` must stay a positional");
        assert_eq!(
            collect_hashes(&["-".into()]),
            vec!["-"],
            "`-` reaches the hash pass rather than vanishing"
        );
    }

    /// The two passes read ONE table. A value-taking flag must be invisible to
    /// the hash collector, or its value is reported as a bad hash — which is
    /// what `--gerp foo <hash>` did: it blamed `foo` for not being a hash and
    /// never mentioned the flag.
    #[test]
    fn a_flag_value_is_never_read_as_a_hash() {
        let argv: Vec<String> = vec![
            "--from-line".into(),
            "900".into(),
            "--grep".into(),
            "deadbeefcafe".into(),
            "abcdef123456".into(),
        ];
        assert_eq!(
            collect_hashes(&argv),
            vec!["abcdef123456"],
            "only the real hash may survive"
        );
    }

    /// A budget smaller than the first line still prints that line: a slice
    /// that shows nothing is not a slice, it is a silent empty answer.
    #[test]
    fn a_budget_below_one_line_still_yields_one_line() {
        let blob = "aaaa bbbb cccc dddd eeee\nsecond line\nthird line\n";
        let lines: Vec<&str> = blob.lines().collect();
        let (end, _) = raw_slice(&lines, 0, 1);
        assert_eq!(end, 1, "the first line must always survive the budget");
    }

    /// A generous budget reaches the end, and `start` is honoured so a
    /// continuation does not re-print what the caller already has.
    #[test]
    fn the_slice_respects_the_budget_and_the_resume_point() {
        let lines: Vec<&str> = (0..50).map(|_| "one two three four").collect();
        let (end, used) = raw_slice(&lines, 0, 100_000);
        assert_eq!(end, 50, "a generous budget must reach the end");
        assert!(used > 0);

        let (end2, _) = raw_slice(&lines, 40, 100_000);
        assert_eq!(end2, 50, "resuming must not restart");

        let (small, _) = raw_slice(&lines, 0, 12);
        assert!(
            small > 0 && small < 50,
            "a tight budget must stop short without stopping dead: {small}"
        );
    }

    /// The fallback advice no longer promises the entirety — and no longer
    /// pushes `--raw` (107/111 bare dumps). `--prompt` / `--grep` remain.
    #[test]
    fn the_advice_never_promises_the_whole_blob() {
        let record = "a 1\nb 2\nc 3\n".repeat(200);
        let prose = "Une phrase de prose qui parle de choses et d'autres. ".repeat(200);
        for blob in [record.as_str(), prose.as_str()] {
            let a = too_large_advice(9_000, "abcdef123456", blob);
            assert!(
                !a.contains("whole stored blob") && !a.contains("for all bytes"),
                "the advice still promises the entirety: {a}"
            );
            assert!(
                !a.contains("--raw"),
                "the advice must not push the dump: {a}"
            );
            assert!(
                a.contains("--prompt") && a.contains("--grep"),
                "the cheap recourses are missing: {a}"
            );
        }
    }

    /// **The cheap recourses are named, everywhere.** `--raw` is no longer in
    /// the menu (the File banner had already dropped it); it remains in
    /// `--help` / the refusal.
    #[test]
    fn every_banner_names_all_three_recoveries() {
        let record = "path/a.rs:12:fn one()\npath/b.rs:44:fn two()\n".repeat(200);
        let prose = "Une phrase de prose qui parle de choses et d'autres. ".repeat(200);
        for blob in [record.as_str(), prose.as_str()] {
            for menu in [
                recovery_menu("abcdef123456", blob),
                too_large_advice(9_000, "abcdef123456", blob),
            ] {
                for flag in ["--prompt", "--grep"] {
                    assert!(menu.contains(flag), "{flag} is not offered: {menu}");
                }
                assert!(
                    !menu.contains("--raw"),
                    "the menu no longer pushes the dump: {menu}"
                );
                assert!(menu.contains("abcdef123456"), "the hash is missing: {menu}");
            }
        }
        // The ORDER follows the blob's shape: over a record stream a
        // natural-language question retrieves badly (measured: 5 561
        // characters without the answer versus the exact line via `--grep`).
        let rec = recovery_menu("abcdef123456", record.as_str());
        assert!(
            rec.find("--grep").unwrap() < rec.find("--prompt").unwrap(),
            "over a line stream, `--grep` comes first: {rec}"
        );
        let pr = recovery_menu("abcdef123456", prose.as_str());
        assert!(
            pr.find("--prompt").unwrap() < pr.find("--grep").unwrap(),
            "over prose, `--prompt` comes first: {pr}"
        );
    }
}
