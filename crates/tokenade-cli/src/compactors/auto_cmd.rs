//! **`tokenade auto`: guess the format, then fold.**
//!
//! The command that does not know what it receives. Detector ORDER carries
//! meaning and every exception is commented in place — an over-greedy
//! detector placed first disarms all those after it, and that has happened.

use super::*;
// See the note in `compactors.rs`: the explicit import resolves the ambiguity.
use super::delivery::slice_lines;

pub(crate) fn cmd_auto(rest: &[String]) -> i32 {
    // A question is the difference between retrieval and truncation. `web`
    // already accepts one (WebFetch always carried the agent's `prompt`); the
    // catch-all entry point could only ever say "all of it, folded". Same flag,
    // same engine, same budget — RAG needs a query, and this is where the agent
    // can give one.
    let mut prompt: Option<String> = None;
    let mut cmd_hint: Option<String> = None;
    let mut frames: Option<usize> = None;
    let mut line_range: Option<(usize, usize)> = None;
    let mut rest_owned: Vec<String> = Vec::with_capacity(rest.len());
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--prompt" | "--question" | "-q" => match it.next() {
                Some(v) => prompt = Some(v.clone()),
                None => {
                    eprintln!("tokenade read: --prompt requires a value");
                    return 2;
                }
            },
            // `--frames [N]` — video only, opt-in (frames are the expensive
            // way to read a video). Bare `--frames` uses the default cap.
            // The command that PRODUCED this text, when the caller knows it.
            // Content sniffing alone cannot pick a per-command compactor:
            // measured 2026-08-07, `git log --oneline` and `git status` piped
            // through `tokenade read -` came back at 100 % of their size while
            // the same output through `tokenade wrap` folded to 5 % and 79 %.
            // The plugin bridges (opencode/hermes/cline/kilo/pi/openclaw) all
            // call `read -`, so every plugin agent was getting NO compaction on
            // exactly the output the command compactors exist for.
            "--cmd" | "--command" => match it.next() {
                Some(v) => cmd_hint = Some(v.clone()),
                None => {
                    eprintln!("tokenade read: --cmd requires a value");
                    return 2;
                }
            },
            // A RANGE, not a truncation: the caller names the region and gets
            // exactly it, with a line count it can page through.
            "--lines" | "--line" | "-L" => match it.next() {
                Some(v) => match parse_line_spec(v) {
                    Ok(r) => line_range = Some(r),
                    Err(why) => {
                        eprintln!("tokenade read: --lines {why}");
                        return 2;
                    }
                },
                None => {
                    eprintln!("tokenade read: --lines requires a value (e.g. `--lines 120-260`)");
                    return 2;
                }
            },
            "--frames" => {
                let n = it.clone().next().and_then(|v| v.parse::<usize>().ok());
                if n.is_some() {
                    it.next();
                }
                frames = Some(n.unwrap_or(tokenade_core::media::DEFAULT_MAX_FRAMES));
            }
            _ => rest_owned.push(a.clone()),
        }
    }
    let rest: &[String] = &rest_owned;
    // Every detector below sniffs by REOPENING the path. That is free on a
    // regular file and destructive on anything else: reading a prefix from a
    // pipe consumes it, and the read that follows finds an empty stream. Three
    // detectors in a row each ate part of it, so `tokenade read <(git diff)` —
    // an ordinary way to hand a command's output over — answered with a single
    // newline. For a non-regular path there is exactly one safe move: read it
    // ONCE, then decide from the bytes, which is what the piped-stdin branch
    // further down has always done.
    let mut piped_text: Option<String> = None;
    let named_path = rest
        .first()
        .filter(|a| a.as_str() != "-" && !a.starts_with('-'))
        .map(|a| std::path::PathBuf::from(a.as_str()));
    let path_is_regular = named_path
        .as_deref()
        .map(|p| std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(true))
        .unwrap_or(true);
    if let Some(p) = named_path.as_deref().filter(|_| !path_is_regular) {
        match crate::bounded_input::read_path_bounded(p) {
            Ok(bytes) => {
                if let Some(kind) = tokenade_core::disclose::detect_binary(&bytes) {
                    return serve_binary_named_bytes(bytes, &kind, Some(p));
                }
                piped_text = Some(
                    tokenade_core::text_encoding::decode(&bytes)
                        .map(|(t, _)| t)
                        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned()),
                );
            }
            Err(e) => {
                eprintln!("tokenade read: {}: {e}", p.display());
                return 1;
            }
        }
    }
    // A document is not text, and this path reads text. `read` is advertised as
    // the entry point for a blob whose format you do not know, but a .docx /
    // .pdf / .epub went through a lossy UTF-8 decode BEFORE detection ran —
    // measured: 1 of 10 planted facts recoverable, the rest served back as raw
    // deflate bytes. `web` already owns the byte-accurate document path
    // (extraction, spreadsheet skeletons, per-format budgets), so hand over to
    // it rather than grow a second one. Flags pass through untouched, `--prompt`
    // included.
    if let Some(first) = rest.first().filter(|a| a.as_str() != "-") {
        if !first.starts_with('-')
            && path_is_regular
            && tokenade_core::docs::detect_format_at(std::path::Path::new(first.as_str())).is_some()
        {
            // Rebuild argv for `web`: it parses --prompt itself.
            let mut fwd: Vec<String> = rest_owned.clone();
            if let Some(q) = prompt.as_ref() {
                fwd.push("--prompt".to_string());
                fwd.push(q.clone());
            }
            return cmd_web(&fwd);
        }
    }
    // Media is not text either, and the text path is WORSE than useless on it:
    // a media file is no `docs::Format`, so it used to reach the UTF-8
    // compactor, which lossy-decoded the bytes and INFLATED them — measured, a
    // 30 s mp3 served 107 138 tokens of garbage and a 10 s mp4 58 450, more
    // output than input for zero information. Intercept before any read.
    if let Some(first) = rest.first().filter(|a| a.as_str() != "-") {
        if !first.starts_with('-') {
            let p = std::path::Path::new(first.as_str());
            if let Some(kind) = path_is_regular
                .then(|| tokenade_core::media::detect_media_at(p))
                .flatten()
            {
                return serve_media(p, kind, prompt.as_deref(), frames);
            }
        }
    }
    /// Emit a named, byte-free view of a file that is not text.
    ///
    /// Two families reached the text path and printed their own bytes as mojibake:
    /// images at any size, and any binary small enough to sit under the stash
    /// threshold — a threshold written for text, applied to bytes. Both are the
    /// same mistake, so both go through one function: once a file is known not to
    /// be text, its bytes are never printed, whatever its size.
    fn serve_binary_named(p: &std::path::Path, label: &str) -> i32 {
        match crate::bounded_input::read_path_bounded(p) {
            Ok(b) => serve_binary_named_bytes(b, label, Some(p)),
            Err(e) => {
                eprintln!("tokenade read: {}: {e}", p.display());
                1
            }
        }
    }

    /// Same rule, for bytes that have no path — stdin. `curl -s <url> | tokenade
    /// read -` is a documented pattern, and a piped PNG came back as 2.2x its own
    /// size in mojibake: the stdin branch decodes lossily with no look at the
    /// bytes, so the sniff that protects the file path never ran here.
    fn serve_binary_named_bytes(bytes: Vec<u8>, label: &str, p: Option<&std::path::Path>) -> i32 {
        let n = bytes.len();
        // The hint carries the PATH when there is one, and `SourceHint::File`
        // already says why: "files have authoritative extensions, we shouldn't
        // second-guess them". Passing `Unknown` from here threw that away, and
        // the sniffer only knows the formats `infer` has a magic number for.
        // Measured 2026-09-04 on a 120x80 TGA and a Radiance HDR: both were
        // DECODED — the line above says `[tokenade:image 120x80]` — and then
        // stashed as `<hash>.bin` under "Binary content … Inspect with: file,
        // xxd, hexdump", the advice an unrecognised blob gets. Two surfaces,
        // one truth: the half that knew it was an image did not tell the half
        // that names the file.
        let summary = tokenade_core::disclose::maybe_stash_bytes(
            bytes,
            match p {
                Some(path) => tokenade_core::disclose::SourceHint::File(path.to_path_buf()),
                None => tokenade_core::disclose::SourceHint::Unknown,
            },
        );
        if summary.starts_with("[disclose:") {
            println!("[tokenade:{label}] {summary}");
        } else {
            // Under the stash threshold: say what it is, and where it is when
            // there is a where. Piped input has no path, and saying so is better
            // than printing bytes nobody can read.
            match p {
                Some(path) => println!(
                    "[tokenade:{label} {n} bytes] not shown — this file is not text. \
                 It is at {}.",
                    path.display()
                ),
                None => println!(
                    "[tokenade:{label} {n} bytes] not shown — the piped input is not text."
                ),
            }
        }
        0
    }

    // Images are the THIRD family this function has had to intercept before the
    // text read, after documents and after media — and each earlier fix carried
    // the same measurement: a lossy UTF-8 decode of binary INFLATES it and
    // returns nothing. Measured here on a 2 931-byte PNG: 2 910 bytes of
    // mojibake, byte-for-byte useless. A big one escaped only because the
    // binary sniffer caught it further down; small ones went straight through.
    //
    // A terminal cannot show pixels, so the honest answer is the one an
    // unrecognised binary already gets: say what it is, keep the exact bytes,
    // and hand back a pointer. (The Read-tool interception is a different
    // surface and keeps converting images for the model.)
    if let Some(first) = rest
        .first()
        .filter(|a| a.as_str() != "-" && !a.starts_with('-'))
    {
        let p = std::path::Path::new(first.as_str());
        if let Some((dims, note)) = tokenade_core::media::image_read(p) {
            let Some(note) = note else {
                return serve_binary_named(p, &dims);
            };
            println!("{note}");
            return 0;
        }
    }
    // Anything else that is not text. The stash sniffer already knows every
    // family (archive, database, wasm, class, ELF, font…) — it simply was not
    // consulted on this path until the content was big enough to reach it, so
    // a 30-byte gzip printed itself while a 123-byte one was named correctly.
    // Sniffing a prefix costs one small read and closes the family at every
    // size instead of one format at a time.
    if let Some(first) = rest.first().filter(|a| a.as_str() != "-") {
        if !first.starts_with('-') && path_is_regular {
            let p = std::path::Path::new(first.as_str());
            let head = std::fs::File::open(p).ok().and_then(|mut f| {
                use std::io::Read as _;
                let mut buf = Vec::new();
                f.by_ref()
                    .take(8192)
                    .read_to_end(&mut buf)
                    .ok()
                    .map(|_| buf)
            });
            if let Some(kind) = head
                .as_deref()
                .and_then(tokenade_core::disclose::detect_binary)
            {
                return serve_binary_named(p, &kind);
            }
        }
    }
    let reading_stdin = !rest
        .iter()
        .any(|a| !a.starts_with('-') && a.as_str() != "-");
    if reading_stdin {
        use std::io::{IsTerminal as _, Read as _};
        if !std::io::stdin().is_terminal() {
            // Capped: `yes | tokenade read -` reached 50 GB and was
            // OOM-killed before this.
            let Ok(bytes) = crate::bounded_input::read_stdin_bytes() else {
                eprintln!("tokenade read: failed to read stdin");
                return 1;
            };
            if let Some(kind) = tokenade_core::disclose::detect_binary(&bytes) {
                return serve_binary_named_bytes(bytes, &kind, None);
            }
            // Zero bytes is not input. `is_terminal()` above already catches
            // the interactive case, but an agent harness hands us a non-tty
            // stdin that is simply empty (`/dev/null`, a closed pipe), and the
            // empty string then flowed all the way through: `tokenade read`
            // with no argument answered
            // `§ref:e3b0c44298fc§ (~0 tokens / 0 bytes, identical to a prior
            // call — ask it what you need)` — a recovery pointer to nothing,
            // and an invitation to spend a turn asking it questions. That is
            // the shape already refused elsewhere: a summary that counts
            // several things and finds none of them is not served.
            //
            // Which of the two answers depends on whether stdin was ASKED for.
            // An explicit `-` means "here is my stdin", and a producer that
            // legitimately emitted nothing (`grep pat f | tokenade read -`
            // with no match) is not a usage error — it gets silence and a
            // success code. A bare `tokenade read` is a forgotten path.
            if bytes.is_empty() {
                if matches!(rest.first().map(String::as_str), Some("-")) {
                    return 0;
                }
                eprintln!(
                    "usage: tokenade read <file>|-\n  \
                     nothing arrived on stdin and no file was named."
                );
                return 2;
            }
            piped_text = Some(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    let src = match piped_text {
        Some(t) => t,
        None => match read_stdin_or_file_with_tty_hint(
        rest,
        "read",
        "  detect format and dispatch to the right compactor (the catch-all entry point)\n  \
         pipe input:    `curl -s api/x | tokenade read -`\n  \
         from file:     `tokenade read blob.txt`\n  \
         ask a question: `tokenade read big.log --prompt \"why did the deploy fail\"`\n  \
                         (retrieves the relevant passages instead of folding the whole thing;\n  \
                         ask several at once with commas: `--prompt \"why did it fail, which step, how to retry\"`)\n  \
         audio/video:    `tokenade read talk.mp4` → what it is + transcript (sidecar/embedded\n  \
                         subtitles, else local whisper when installed); `--prompt` questions it,\n  \
                         `--frames [N]` adds deduplicated still frames (video only, ~1.5k tok each)",
    ) {
            Ok(s) => s,
            Err(code) => return code,
        },
    };
    // The slice happens HERE, at the one place the text exists and before any
    // detector sees it: a compactor asked to fold lines 120-260 must be given
    // lines 120-260, not the whole file and a note.
    let src = match line_range {
        None => src,
        Some((from, to)) => match slice_lines(&src, from, to) {
            Some((body, first, last, total)) => {
                format!("[tokenade: lines {first}-{last} of {total}]\n{body}\n")
            }
            None => {
                let total = src.lines().count();
                eprintln!("tokenade read: line {from} is past the end of a {total}-line file");
                return 2;
            }
        },
    };
    // "Was the question actually applied?" is asked ONCE, downstream, by the
    // code that ran the retrieval and therefore knows — see `unanswered` and
    // the `None` arms below.
    //
    // It used to be answered TWICE. An advisory stood here and re-decided the
    // question from the source's FORMAT: `--prompt` on plain text was declared
    // a no-op, on the ground that retrieval lived on the document path
    // (`docs::detect_format_at` → pdf/docx/xlsx/epub…) and on `web`. That was
    // true when it was measured on 2026-08-07 — the same file came back at
    // 38 352 bytes with no prompt, with one question and with two.
    //
    // It stopped being true, and the note stayed. The scoped-extract dispatch
    // below is gated on nothing but a non-empty prompt: no format filter, no
    // document check. Measured 2026-08-26 on a 14 487-byte markdown handbook:
    // 3 084 bytes without the prompt against 678 WITH it, ending on the
    // passage that answers — a real retrieval, 4.5x smaller. And the agent was
    // told, on stderr, that its question had been ignored and that what
    // followed was "not an answer".
    //
    // That is worse than the silence this advisory was added to break: an
    // agent that believes it was not answered re-reads the whole file. The
    // decline case it existed for is already covered downstream, by the code
    // that watched the retrieval decline instead of guessing from an
    // extension.
    // Already folded by another tokenade channel → pass it through.
    //
    // One authority per payload. The plugin
    // bridges guard on "did it get shorter", which is not the same question —
    // MEASURED 2026-08-07, folding a git log twice went 16 198 → 4 124 → 4 070,
    // and a third pass came back to 4 071. Each extra pass is a second
    // authority chewing on a summary, and it does not even converge. The
    // shim-then-plugin path on Linux hits exactly this: the shim folds the
    // command output, then the plugin folds the fold.
    //
    // `fold_marker` answers the right question, and the hash-length rule keeps
    // prose that merely MENTIONS `expand-ref` from tripping it.
    //
    // Two bounds on that passthrough, both measured 2026-08-08 once
    // `fold_marker` learnt to recognise banners and elision footers — the
    // signal it was missing on most real folds:
    //
    // * **stdin only.** A FILE the agent asked to read is a document, and a
    //   document that quotes a tokenade banner (this repo's own docs do) must
    //   still be compacted on request.
    // * **redact before printing.** The old trigger was an `expand-ref` or
    //   `§ref:` hash, which only tokenade emits, so the bytes had already been
    //   through `redact_for_model`. A banner line has not: any input can carry
    //   one. Serving `src` verbatim leaked a live `sk-ant-api03-…` out of a
    //   document that the same binary redacted one commit earlier. Same class
    //   as #903, and the same fix — scrub at the print site.
    let from_stdin = matches!(rest.first().map(String::as_str), Some("-") | None);
    if from_stdin && tokenade_core::fold_marker::already_folded(&src) {
        tokenade_core::debug_log::log(
            "read",
            "already_folded_passthrough",
            serde_json::json!({ "bytes": src.len() }),
        );
        print!("{}", passthrough_text(&src));
        return 0;
    }
    // #571 — when a file path is on argv (i.e. NOT stdin), pass it to
    // compact_with_path_hint so the #570 source-code-extension reject
    // fires. Without this, `tokenade read src/foo.rs` falls through to
    // bare auto::compact and source code can still be mis-detected as
    // SERP/INI/cmake etc. (the same class as the Read pretool fix).
    let file_path: Option<&str> = match rest.first().map(String::as_str) {
        Some("-") | None => None,
        Some(p) => Some(p),
    };
    // A named command wins over sniffing: it is the one thing the detector
    // cannot infer from bytes, and it is what selects the git/cargo/docker/…
    // compactors. Falls through to detection when the program has no
    // dedicated compactor or when its compactor declines this text.
    // Source code reaches this command as bytes with a KNOWN extension, and
    // `compact_with_path_hint` deliberately rejects source from the
    // format-specific compactors (#570) — which left `tokenade read foo.go`
    // returning the file whole while READING the same path through the hook
    // stripped its comments. Same rule, same two stages, one declarer.
    //
    // The hook's editable-source exception is carried over: a source file
    // inside a git work tree and under the ceiling is what the agent is about
    // to EDIT, and an Edit is matched against the bytes it was shown.
    if let Some(rc) = try_source_strip(file_path, &src, prompt.as_deref()) {
        return rc;
    }
    let (kind, out, sav) = match file_path {
        Some(p) => auto::compact_with_path_hint(&src, p),
        None => auto::compact(&src),
    };
    // Let the competing candidates run, exactly as the COMMAND path does.
    // They lived inside `filter::auto_or_generic`, which this path never
    // reaches, so the same bytes folded very differently depending on whether
    // tokenade RAN the command or was handed the file. Measured 2026-08-26 on
    // a 94 890-byte tar listing of 1 500 entries: 39 538 bytes here against
    // ~1 600 through `wrap`. See `filter::best_auto_candidate`.
    let (auto_label, out, sav) = tokenade_core::filter::structured_or_generic(
        &src,
        sav.tokens_before,
        kind,
        out,
        sav,
        tokenade_core::filter::Unclaimed::Keep,
    );
    // A named command does NOT win by default — it competes. Measured
    // 2026-08-07 on the same corpus: the hint takes `git log --oneline` from
    // 16 199 bytes (detection folds nothing) down to 4 124, and takes
    // `ls -la` from 332 UP to 5 875, because the generic archetype pass beats
    // the `ls` compactor on that shape. Picking the smaller of the two is the
    // only rule that never regresses — the same lesson as the "greedy detect"
    // negative: compare DELIVERED output, never trust a module to be better
    // because it is more specific.
    // Which program won, not just what it produced. The ledger row is written
    // ~100 lines below from `kind`, and `kind` is the CONTENT-detected format —
    // so a `git log` folded by the hint path was booked as `auto:unknown`, with
    // no `cmd` field at all. Measured 2026-08-08 through the opencode bridge:
    // 29 115 -> 4 569 bytes, credited to agent `opencode`, labelled
    // `auto:unknown`. That is the six plugin-bridge agents' entire traffic
    // arriving in the Commands panel as one anonymous bucket — the same shape
    // as the 14.6k Codex shim events that were invisible there until the
    // basename was threaded through `record_cmd`.
    let mut winning_prog: Option<String> = None;
    let (out, sav) = match cmd_hint.as_deref().and_then(|c| {
        // The same reader `wrap` would have applied to this command — its
        // meaningful segment through `filter_command` — beside each program's
        // own compactor. `dispatch_named` alone misses every arm of the
        // command dispatcher: `dotnet build` (57 kB, 122 warnings) was left to
        // content sniffing, which took it for an archive listing (2026-10-05).
        let seg = tokenade_core::command_rewrite::meaningful_segment(
            tokenade_core::filter::strip_leading_comment_lines(c),
        );
        let argv: Vec<String> = seg.split_whitespace().map(String::from).collect();
        let by_command = Some(tokenade_core::filter::filter_command_fail_open(&argv, &src))
            .filter(|r| r.is_command_specific() && r.output.len() < src.len())
            .map(|r| {
                (
                    tokenade_core::command_rewrite::first_program_token(seg),
                    r.output,
                    true,
                )
            });
        hint_programs(c)
            .into_iter()
            .filter_map(|prog| {
                tokenade_core::cmd::dispatch_named(prog, &src).map(|f| (prog, f, false))
            })
            .chain(by_command)
            .min_by_key(|(_, f, _)| f.len())
    }) {
        // Against what `wrap` would have delivered, sniffing the bytes wins
        // only by being shorter WITHOUT losing a word it kept — the rule
        // `filter::safety` applies to the same two candidates. Shorter alone
        // let a misread win: 556 bytes of "archive listing" against 1 372 of
        // warnings, files and verdict. A bare program compactor keeps the
        // plain size contest (`ls -la /usr/bin`: a 6 kB head-and-tail cut
        // against the 1 kB census — measured, unchanged).
        Some((prog, folded, as_wrap))
            if folded.len() < out.len()
                || (as_wrap && !tokenade_core::filter::keeps_every_word_of(&out, &folded)) =>
        {
            let sav = Savings::new(
                tokenade_core::tokenizer::estimate(&src),
                tokenade_core::tokenizer::estimate(&folded),
            );
            winning_prog = Some(prog.to_string());
            (folded, sav)
        }
        _ => (out, sav),
    };
    // The post-passes both doors owe. They lived inside
    // `filter_command`, so this path reached none of them: measured on a
    // BuildKit-style log that redraws four lines with `ESC[4A` thirty times,
    // `wrap` delivered 267 bytes and `read` delivered 4 678. No `argv` here,
    // which is exactly right — the one command-specific pass
    // (`drop_build_artifact_lines`) self-disables on an empty argv and the
    // other three apply. See `filter::shrink_only_post_passes`.
    let (out, sav) = match tokenade_core::filter::shrink_only_post_passes(&[], &src, &out) {
        Some(folded) => {
            let after = tokenade_core::tokenizer::estimate(&folded);
            (folded, Savings::new(sav.tokens_before, after))
        }
        None => (out, sav),
    };
    // L81 — anti-inflation guard via shared helper. User explicitly
    // asked `tokenade read` to compact; serving inflated output would
    // be opposite of intent. Same pattern as MCP auto_compact tool.
    let raw_owned = src.clone();
    let (mut effective, effective_sav) =
        tokenade_core::fold_count::pick_compact_or_raw(raw_owned, out, sav, || {
            tokenade_core::tokenizer::estimate(&src)
        });
    // #903 — second leak vector. The #902 fix scrubs the
    // inflation-fallback path inside the helper, but Kind::Unknown
    // passthrough (input too tiny / content not detected) returns
    // raw bytes through the equal-tokens branch where the helper
    // doesn't intervene. Repro: a 1-line `.env` below the env_dump
    // detector's ≥3-line threshold → Unknown → passthrough →
    // cleartext API_KEY served back. Belt-and-suspenders scrub
    // here at the print site; idempotent on already-redacted
    // compactor output (redact() is a no-op when no patterns
    // match — Cow-based, see #895).
    let redaction_fired = {
        let scrubbed = tokenade_core::redact::redact_for_model(&effective);
        let fired = scrubbed != effective;
        effective = scrubbed;
        fired
    };
    // Tell the caller that the difference is a REMOVED SECRET, not a fold.
    //
    // Every plugin bridge accepts our stdout only when it is strictly SHORTER
    // than what it sent — a sound rule for compaction and a wrong one for
    // redaction, because redaction routinely makes text LONGER: `API_KEY=k0`
    // (10 chars) becomes `API_KEY=<redacted>` (18). Measured 2026-08-08 on
    // three ~2 kB payloads well above every bridge's floor, the scrubbed
    // output grew by ~10% each time, so all five bridges discarded it and the
    // model received the raw key. The bridges cannot tell "longer because we
    // failed" from "longer because we removed something" without this line.
    //
    // stderr, so stdout stays byte-identical (no dedup or cache disturbance) —
    // same posture as the other advisories in this function.
    if redaction_fired {
        eprintln!(
            "{REDACTION_SIGNAL} this output has secrets removed. Take it even if it is \
             LONGER than what you sent."
        );
    }
    let inflated = sav.tokens_after > sav.tokens_before;
    // Retrieve, if asked. Runs AFTER compaction so the extractor sees the same
    // distilled text the agent would otherwise read, and after redaction so a
    // scoped extract can never surface a secret the full serve would have
    // hidden. Declining (vague question, no lexical anchor, embedder offline)
    // falls back to the compacted text — never below it.
    // Was a question actually asked? The `None` arms below serve the document
    // instead of an answer, and until now they did it silently — so a question
    // the source cannot answer came back looking exactly like one it could,
    // and the passages read as the answer. Saying "this is the document, not
    // your answer" is the whole difference between a miss and a wrong answer.
    // Source id = the path when reading a file; stdin has no stable identity
    // across calls, so key on the content instead.
    let source = rest
        .first()
        .filter(|a| a.as_str() != "-")
        .cloned()
        .unwrap_or_else(|| tokenade_core::dedup_ref::hash_content(effective.as_bytes()));
    let asked = prompt.as_deref().is_some_and(|p| !p.trim().is_empty());
    // The question is asked of the SOURCE (redacted), not of the fold: a
    // 4 000-item JSON array folded to its schema and three examples cannot
    // say what `svc-1234`'s timeout is, and the reader was sent the schema —
    // under a footer that recommended this very `--prompt` (2026-10-03).
    // Same defect as `web --prompt` on the capped view; see
    // `feedback_a_retriever_must_read_the_source_not_a_reduced_view`.
    let search = asked.then(|| tokenade_core::redact::redact_for_model(&src));
    let (effective, scoped) = retrieve_or_note(
        effective,
        search.as_deref(),
        prompt.as_deref(),
        &source,
        effective_sav.tokens_after,
    );
    // The numbers must be the FILE's, so they are matched against `src` — the
    // bytes as they are on disk — and not against the compacted view the
    // extractor worked on. Measured 2026-09-03 while writing this: annotating
    // against the view put a passage at "lines 124-134" of PATCHNOTES.md,
    // where both of those lines are blank. A passage the file cannot confirm
    // is left unlabelled.
    let effective = if scoped {
        let labelled = annotate_passage_lines(&effective, &src);
        if labelled.contains("[line") {
            hint_line_range_once(rest.first().map(String::as_str).unwrap_or("<file>"));
        }
        labelled
    } else {
        effective
    };
    let (effective_sav, label) = if scoped {
        let mut s = effective_sav;
        s.tokens_after = tokenade_core::tokenizer::estimate(&effective);
        (s, format!("{auto_label}:scoped"))
    } else if inflated {
        (effective_sav, "auto:passthrough_inflated".to_string())
    } else {
        (effective_sav, auto_label.clone())
    };
    match winning_prog {
        // The hint won: name the program, and book it through the cmd-aware
        // record so the Commands panel can see it. `covered = true` because a
        // named compactor is exactly what "command-specific" means here.
        Some(ref prog) => {
            if tokenade_core::gain::worth_recording(
                effective_sav.tokens_before as u64,
                effective_sav.tokens_after as u64,
                "auto-compact",
            ) {
                // `op` stays "auto-compact" on purpose. It is what every row
                // this path has ever written carries, the dashboard groups by
                // it, and `worth_recording` has op-keyed exemptions — moving
                // the taxonomy to buy a label would trade one reporting bug
                // for another. The program goes where the Commands panel
                // actually reads it: the `cmd` field.
                gain_log::record_cmd(
                    "cli",
                    "auto-compact",
                    &redact::redact(cmd_hint.as_deref().unwrap_or(prog)),
                    effective_sav,
                    prog,
                    true,
                );
            }
        }
        None => log_gain(&label, "auto-compact", effective_sav),
    }
    // UNIVERSAL SIZE BACKSTOP. Every compactor above is a bet
    // that we recognised the format; when the bet loses, this path used to
    // `println!` whatever came back, however large. Measured: a 23 MB sitemap
    // returned 23 352 815 bytes — ~5.8M tokens from one call — because the
    // generic XML pass only strips whitespace.
    //
    // A per-format compactor cannot be the answer: sitemaps were today's case
    // and the next unrecognised format is unknowable. `maybe_stash` already is
    // the answer — it writes the full text to the stash and returns a pointer
    // with `expand-ref --prompt / --grep / --raw` instructions — and it was
    // simply not wired here. Its own doc-comment records that this guard "lived
    // in 3 of 8+ call paths and every new path re-created the bug"; this was
    // one of the paths.
    //
    // `read_threshold_tokens` (not the lower incidental one) because
    // `tokenade read <file>` is a DELIBERATE read: the agent chose this file
    // and should get it inline up to a higher bar.
    let hint = match file_path {
        Some(p) => tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(p)),
        None => tokenade_core::disclose::SourceHint::web(),
    };
    // Measured BEFORE the stash, so the fold below can be credited. Same exact
    // byte floor as `cmd_wrap`: a token is at least one
    // byte, so a string shorter than the threshold cannot exceed it in tokens
    // and `maybe_stash` cannot fire.
    // Byte fidelity, before anything is bounded: a rebuild that changed only
    // the line endings has compacted nothing, and `tokenade read x.md > y.md`
    // would silently rewrite the file. Same rule as `read`'s trailing-newline
    // fidelity, one layer up. MEASURED 2026-09-04: a 29-byte CRLF markdown came
    // back as 26 bytes of the same words, and a 15-byte CRLF csv the same way.
    let effective: String = if tokenade_core::filter::differs_only_in_line_endings(&src, &effective)
    {
        src.clone()
    } else {
        effective
    };
    let bounded = bound_delivery(effective, hint.clone(), "auto-compact");
    // `bound_delivery` REBUILDS the text — for markdown it returns an outline
    // of it — so the note added above does not survive into what the agent
    // reads. Measured on a 127 824-byte markdown asked a question nothing in
    // it answers: `mode=md-outline`, 4 699 bytes, and no note, while the same
    // question on a document served whole says so. An outline of section
    // titles reads even more like a curated answer than the document does,
    // which is exactly the confusion the note exists to prevent — "a question
    // the source cannot answer came back looking exactly like one it could".
    let bounded = if asked
        && !label.ends_with(":scoped")
        && bounded.len() >= UNANSWERED_NOTE_MIN_CHARS
        && !bounded.contains(UNANSWERED_NOTE)
    {
        format!("{UNANSWERED_NOTE}\n{bounded}")
    } else {
        bounded
    };
    // Recovery for what the fold DROPPED, in the shape the SOURCE allows.
    // Piped input has no fallback at all, and this is where the loss is
    // largest: 1.5 MB of log through `tokenade read -` comes back as one shaped
    // line, which is the right answer and was, until now, the only surviving
    // copy — so it gets a stash and a hash.
    //
    // A FILE still needs no stash; the path the agent typed holds every byte.
    // But this branch used to justify saying NOTHING with "the disclose footer
    // already says to grep or re-read it", and that footer does not exist here:
    // measured 2026-09-04 at 44 KB, 454 KB and 4.6 MB, the file route printed
    // no recovery line at any size, while the installed guidance sends the
    // agent looking for a hash in a banner that is never emitted.
    let dropped = effective_sav
        .tokens_before
        .saturating_sub(effective_sav.tokens_after);
    let bounded = match file_path {
        None => crate::wrap::attach_drop_recovery(
            bounded,
            &src,
            dropped,
            effective_sav.tokens_before,
            &hint,
            // The ledger's name for this route, not a second vocabulary.
            "auto-compact",
        ),
        // A file needs no stash, but it does need the sentence: this route
        // printed no footer at all, at any size.
        Some(p) => crate::wrap::nudges::attach_path_recovery(
            bounded,
            p,
            dropped,
            effective_sav.tokens_before,
            "auto-compact",
        ),
    };
    // The same hole again, on the OTHER command — and this is the one the six plugin
    // bridges call. `cmd_wrap` was fixed on 2026-08-08; `cmd_auto` has the same
    // shape and was not. Measured the same day on a fresh HOME: 841 232 bytes
    // of unrecognised text through `tokenade read -` came out at 4 843 (183 397
    // tokens down to ~1 200) and wrote **zero** ledger rows — the generic pass
    // saved nothing, so `worth_recording` closed the gate, and then the stash
    // folded 40x with nobody counting.
    //
    // Worse than the `wrap` case, which at least booked the compactor's own
    // row. Here a bridge folds a large tool result and the dashboard shows
    // nothing at all: a channel that works and shows up as a zero.
    //
    // The two rows tile and cannot overlap: the compactor's spans raw ->
    // compacted, this one compacted -> delivered, and `after` is taken on the
    // FINAL string so the recovery pointer's own bytes are never credited.
    // DEDUP OF RE-DELIVERIES — the half of the dedup layer `read` never had.
    //
    // `tokenade read` is the command every plugin BRIDGE calls (opencode,
    // cline, openclaw, pi, kilo, hermes), and it was the only delivery path in
    // the product that never spoke to `dedup_ref`. Measured on the compat bench
    // 2026-08-18, all three OS: the same bytes through `tokenade read -` twice
    // came back the same size both times, on eighteen of the twenty-four
    // agents. The dedup layer was not missing there — its PRECONDITION was
    // never supplied, because nothing on the bridge path set a session.
    //
    // The gate is `wrap`'s, unchanged and unduplicated: fold to a §ref only
    // when THIS session provably delivered these exact bytes and no /compact
    // has dropped them since. Without `TOKENADE_SESSION` it never folds, which
    // is why the bridges now set one — a bridge module is loaded once per agent
    // process, so its own key proves exactly what the rule asks.
    //
    // Skipped when the delivery is already a pointer: `maybe_stash` above may
    // have turned this into one, and a pointer to a pointer buys nothing.
    let bounded = if bounded.contains("§ref:") {
        bounded
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Stdin has no stable identity across calls, so the key is what came
        // IN. A file has one, and it is the path.
        let key = match file_path {
            Some(p) => format!("read:{p}"),
            None => format!(
                "read:-:{}",
                tokenade_core::dedup_ref::hash_content(src.as_bytes())
            ),
        };
        let safe_label = redact::redact(file_path.unwrap_or("-"));
        // The bytes as READ, before this route compacted them.
        let raw_fp = tokenade_core::dedup_ref::hash_content(src.as_bytes());
        crate::wrap::fold_or_register_dedup(
            bounded,
            crate::wrap::DedupSite {
                channel: "read",
                op: "read_dedup",
                cwd: &cwd,
                key: &key,
                shell_cmd: "",
                safe_label: &safe_label,
                filter_label: &label,
                session: None,
                // Keyed on the bytes as read, not on what we delivered.
                raw_fingerprint: &raw_fp,
            },
        )
    };
    // The caller's own bytes need no terminator; a VIEW does. `println!` gave
    // one to both: measured 2026-09-04, `tokenade read <file>` returned 26
    // bytes for a 25-byte file with no trailing newline, and 28 for a 27-byte
    // file that HAD one — a blank line appended to every ordinary read, and a
    // corrupted copy under `> file`. `wrap` and `filter` were fixed to ask
    // this question through one declarant; `read`, the content entry point,
    // was still answering it with `println!`.
    let passed_through = bounded == src;
    print!("{bounded}");
    if crate::wrap::needs_terminator(&bounded, passed_through) {
        println!();
    }
    0
}
