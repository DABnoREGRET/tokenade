//! **Serving an image, a sound, a video.**
//!
//! Media is not folded like text: it is resized, or described. What matters
//! is never to SKIP it silently — an image above the cap comes back smaller
//! WITH the note that says so and how to opt out.

use super::*;

/// `tokenade read <file|->` — auto-detect format and dispatch to the right
/// compactor. The single content entry point an agent should reach for when
/// the blob's format isn't statically known. (Formerly `tokenade auto`; the
/// per-format compactor commands were removed 2026-07 — `read` reaches the
/// same core compactors via format detection.)
/// Serve an audio/video file. Never decodes the bytes as text (that inflation
/// is the bug this path exists to kill). What a model needs is what the file IS
/// and what is SAID in it:
///
/// 1. `ffprobe` summary — ~30 tokens, when ffprobe is installed.
/// 2. A transcript when one can be had for FREE: a sidecar `.srt`/`.vtt`, or a
///    subtitle track already embedded in the container. No ASR is ever run
///    implicitly — that is the agent's call, not ours.
/// 3. The transcript is ordinary text, so it flows into the machinery the rest
///    of tokenade already has: `answer_over_blob` serves it whole under the
///    tout-vs-RAG threshold and answers questions against it above (multi-
///    question included). A media file becomes an interrogable document.
///
/// Missing optional binaries are reported as ONE actionable next step, never a
/// hard failure: without any of them this still identifies the file and refuses
/// to inflate it, which is the bulk of the win.
pub(super) fn serve_media(
    path: &std::path::Path,
    kind: tokenade_core::media::MediaKind,
    prompt: Option<&str>,
    frames: Option<usize>,
) -> i32 {
    use tokenade_core::media;
    let name = path.display();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut out = format!(
        "[tokenade:{kind} {name} · {size} bytes]\n",
        kind = kind.as_str()
    );
    match media::probe_summary(path) {
        Some(p) => {
            out.push_str(&p);
            out.push('\n');
        }
        None => out.push_str("(no ffprobe: duration/codecs/tracks unavailable)\n"),
    }

    // A transcript we can get for free — sidecar first (no tooling at all),
    // then an embedded track (ffmpeg, still free relative to ASR).
    let transcript = media::sidecar_subtitle(path)
        .and_then(|s| std::fs::read_to_string(s).ok())
        .map(|raw| media::subtitle_to_text(&raw))
        .filter(|t| !t.trim().is_empty())
        .or_else(|| media::embedded_subtitles(path))
        // Nothing free available → transcribe locally with whisper when it is
        // installed. Only here, never in a hook: ASR is minutes-scale and hooks
        // have hard timeouts. The result is cached by CONTENT hash, so this is
        // paid once and every later question against the same media is free.
        .or_else(|| {
            if !media::asr_available() {
                return None;
            }
            let hash = tokenade_core::media::cheap_media_identity(path)?;
            if media::cached_transcript(&hash).is_none() {
                eprintln!(
                    "[tokenade] no subtitles found — transcribing locally with whisper \
                     (first time for this file; the transcript is cached afterwards)…"
                );
            }
            media::transcribe(path, &hash)
        });

    match transcript {
        Some(text) => {
            let text = tokenade_core::redact::redact_for_model(&text);
            let toks = tokenade_core::tokenizer::estimate(&text);
            match prompt.filter(|p| !p.trim().is_empty()) {
                // A question: the shared retrieval core decides whole-vs-RAG,
                // exactly as it does for a stash or a document.
                Some(q) => match tokenade_core::gain::answer_over_blob_asked(&text, q, "read") {
                    Some(ans) => {
                        out.push_str("\n--- TRANSCRIPT (answering your question) ---\n");
                        out.push_str(&ans);
                    }
                    None => out.push_str(
                        "\n[no passage of the transcript matched your question — widen it]",
                    ),
                },
                // No question: serve it whole when small, otherwise point at
                // the question path rather than dumping an hour of speech.
                None if toks <= tokenade_core::disclose::threshold_tokens() => {
                    out.push_str("\n--- TRANSCRIPT ---\n");
                    out.push_str(&text);
                }
                None => {
                    let hint = tokenade_core::disclose::stash_for_reference(
                        &text,
                        &tokenade_core::disclose::SourceHint::File(path.with_extension("txt")),
                    );
                    out.push_str(&format!(
                        "\n[transcript available: ~{toks} tokens — too large to dump whole. \
                         Ask it what you need (RAG): `tokenade read {name} --prompt \"q1, q2\"` \
                         answers each question under its own heading, in one call."
                    ));
                    if let Some((h, _)) = hint {
                        out.push_str(&format!(" Full text also stashed as `{h}`."));
                    }
                    out.push(']');
                }
            }
        }
        None => {
            // Name the cause that actually applied. This arm listed two
            // reasons and is reached for three: the branch above ALSO runs
            // local whisper when it is installed, so a user who just watched
            // `[tokenade] … transcribing locally with whisper …` scroll past
            // was then told the file had no sidecar and no embedded track —
            // true, and not why there is no transcript. MEASURED 2026-08-08 on
            // a speechless WAV with whisper on PATH.
            out.push_str(if media::asr_available() {
                "\n[no transcript: no sidecar .srt/.vtt, no embedded subtitle track, \
                 and local whisper produced none for this file]"
            } else {
                "\n[no transcript: no sidecar .srt/.vtt, no embedded subtitle track, \
                 and no local ASR installed — install whisper to transcribe audio]"
            });
        }
    }

    // Frames: opt-in only. Each one the agent Reads costs ~1.5k vision tokens,
    // so we never produce them by default — and when asked, we sample SCENE
    // CHANGES (a static slide yields one frame, not ninety) and state the cost
    // so the agent chooses knowingly.
    if let Some(want) = frames {
        if matches!(kind, media::MediaKind::Video) {
            let hash = tokenade_core::media::cheap_media_identity(path);
            match hash
                .as_deref()
                .and_then(|h| media::extract_scene_frames(path, h, want))
            {
                Some(fs) if !fs.is_empty() => {
                    out.push_str(&format!(
                        "\n\n--- {} FRAME(S) (evenly sampled, duplicates dropped, \
                         downscaled) ---\nReading all of them costs roughly {}k vision \
                         tokens; Read only the ones you need:\n",
                        fs.len(),
                        (fs.len() * 3) / 2
                    ));
                    for f in &fs {
                        out.push_str(&format!("{}\n", f.display()));
                    }
                }
                _ => out.push_str("\n[no frames extracted: ffmpeg missing or no video track]"),
            }
        } else {
            out.push_str("\n[--frames applies to video, not audio]");
        }
    }

    // One actionable next step when tooling is missing — capability, not a
    // shopping list.
    if let Some(nudge) = media::install_nudge(kind) {
        out.push_str(&format!("\n[tokenade] {nudge}."));
        tokenade_core::gain::record_nudge(
            "media-tooling",
            "fire",
            tokenade_core::gain::NudgeMetric::None,
        );
    }

    println!("{out}");
    log_gain(&format!("media:{}", kind.as_str()), "media-summary", {
        let (b, a) = tokenade_core::media::plausible_media_credit(
            size,
            tokenade_core::tokenizer::estimate(&out),
        );
        Savings::new(b, a)
    });
    0
}
