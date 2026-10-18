//! Images on the Read path: the formats an agent cannot display, and the
//! second read of an image it has already been shown.

use super::*;

/// True for raster image extensions that agents load as VISION blocks (the
/// model SEES them — it does not parse the bytes as text). SVG is excluded
/// (it's text/XML and flows through the normal text path). Used by the image
/// re-read dedup below.
/// **"You have already seen this image": a single wording.**
///
/// Two paths refuse an identical image re-read — the conversation-marker one
/// and the dedup-store one — and each wrote its own sentence. This is not a
/// matter of tone: this text REPLACES the tool result
/// (`permissionDecision: deny`), so it is the only thing the agent receives,
/// and the value ranking credits `image-downscale` with 823 bytes per call
/// over 156 calls.
///
/// One of the two was BROKEN and shipped since 2026-08-05: three runs of
/// twenty-two spaces in the middle of the sentence, the literal's line
/// continuations having been lost. 66 bytes of blank, and a message that
/// reads like corrupted output.
///
/// `identity` carries what identifies the file when the caller has it
/// (`12 KB, size=12345 mtime=1699…`); without it the sentence stays true.
pub(crate) fn already_seen_note(file_path: &str, identity: Option<&str>) -> String {
    let detail = match identity {
        Some(d) => format!(" ({d})"),
        None => String::new(),
    };
    format!(
        "[tokenade:image-dedup] `{file_path}`{detail} is byte-identical to an image \
         already delivered earlier in this conversation — it is still visible in your \
         context above, so it was not re-loaded. Re-read only if you edited the file since."
    )
}

pub(crate) fn is_image_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tiff", ".tif",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// PreToolUse `Read` sub-handler: when the target is an oversized image, stage
/// a downscaled copy and rewrite the Read's `file_path` to it via
/// `permissionDecision: allow` + `updatedInput` (same cache-stable temp-redirect
/// the compacted-view path uses), with an `additionalContext` hint pointing at
/// a full-resolution copy in case the agent needs the detail. Returns
/// `Some(exit_code)` when it handled the read, `None` to fall through to the
/// normal text-read path. Shared by Claude Code / Codex / Grok (all routed
/// through this handler). Disabled when `TOKENADE_IMAGE_MAX_EDGE=0`.
/// Stage a PNG copy of an image the calling agent cannot render, and redirect
/// the Read to it. Returns `Some(0)` when it served, `None` to fall through to
/// the ordinary downscale path (conversion failed — better a raw file than a
/// broken redirect).
pub(super) fn convert_unrenderable_image(
    parsed: &serde_json::Value,
    file_path: &str,
    ext: &str,
    base: &std::path::Path,
    started: std::time::Instant,
) -> Option<i32> {
    use tokenade_core::image_downscale as img;
    let src = std::path::Path::new(file_path);
    let (png, w, h) = img::convert_to_png(src)?;
    let dir = base.join(".tokenade").join("reads");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = src.file_name().and_then(|s| s.to_str()).unwrap_or("image");
    let out = dir.join(format!("{stem}.converted.png"));
    std::fs::write(&out, &png).ok()?;
    let hint = format!(
        "tokenade: `{file_path}` is a .{ext}, which you cannot display — converted to PNG \
         ({w}×{h}) so you can actually see it. The original is untouched at `{file_path}`."
    );
    log_emitted_additional_context("hookwrap-pretool-image", "PreToolUse", &hint);
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason":
                    "tokenade: image converted to a format this agent can render",
                "updatedInput": { "file_path": out.to_string_lossy() },
                "additionalContext": hint,
            }
        })
    );
    // Capability, not economy: without this the agent could not read the image
    // at all, so there is no baseline cost to claim as saved (the media lesson).
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "image_converted",
        serde_json::json!({
            "file": file_path,
            "from": ext,
            "w": w,
            "h": h,
            "elapsed_ms": started.elapsed().as_millis() as u64
        }),
    );
    let _ = parsed;
    Some(0)
}

/// Read this path WHOLE to hash it as a possible image — or refuse to.
///
/// Two conditions, and the first is the one that matters. A character device or
/// a FIFO reports `metadata.len() == 0`, so every ceiling keyed on size lets one
/// straight through — and the read below loads the entire path before anything
/// has established it is an image at all (`image_dimensions` is consulted
/// afterwards). MEASURED on the published 1.1.8 under a real user HOME: a
/// PreToolUse `Read` of `/dev/urandom` held the device open and grew to 2.1 GB
/// of RSS in four seconds, still climbing; `/dev/zero` was SIGKILLed after 28 s.
/// A PreToolUse hook that does not return stalls the tool call it exists to
/// speed up.
///
/// The size ceiling is the other half of the same waste: an ordinary
/// several-hundred-megabyte file was being read whole and hashed here on every
/// Read, only to be dismissed as "not an image".
pub(crate) fn read_image_candidate(file_path: &str) -> Option<Vec<u8>> {
    let md = std::fs::metadata(file_path).ok()?;
    if !image_candidate_ok(&md) {
        return None;
    }
    std::fs::read(file_path).ok()
}

/// The decision alone, over metadata that is already in hand.
///
/// Split out so it can be TESTED without performing the read: an assertion that
/// calls the reader on `/dev/zero` would itself hang the moment the predicate
/// regressed, and a test that hangs takes the whole suite with it — which is
/// exactly what happened on the first attempt at this guard (a mutation left a
/// `cargo test` running for seven minutes before it was killed).
///
/// Production still goes through [`read_image_candidate`], where the decision
/// and the read are one call and cannot drift apart.
fn image_candidate_ok(md: &std::fs::Metadata) -> bool {
    const MAX_IMAGE_HASH_BYTES: u64 = 50 * 1024 * 1024;
    md.is_file() && md.len() <= MAX_IMAGE_HASH_BYTES
}

pub(crate) fn handle_image_downscale_pretool(
    parsed: &serde_json::Value,
    file_path: &str,
    started: std::time::Instant,
) -> Option<i32> {
    use tokenade_core::image_downscale as img;
    // Fleet forensics (100 escaped image Reads on ONE user, ~15MB): agents
    // re-Read the SAME image repeatedly (iterating on slides/screenshots).
    // Identical bytes re-read in the SAME session = the vision block is
    // already in context — answer with a stub instead of re-shipping ~40k
    // vision tokens. mtime+size+hash gate: any touch to the file falls
    // through to the normal downscale path. Sidechain window disables the
    // claim (sibling contexts don't share ours).
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    // Context-scoped marker (class-1 endgame): the tag itself IS the proof
    // this context saw the bytes — a sibling context has a different id.
    // A REGULAR file, and one small enough to plausibly be an image. This read
    // loads the WHOLE path to hash it, and it runs BEFORE anything has
    // established that the path is an image at all — `image_dimensions` is
    // only consulted further down.
    //
    // MEASURED: a PreToolUse `Read` of `/dev/urandom` grew this process to
    // 2.6 GB of RSS after five seconds and 5.2 GB after ten, and never
    // returned; `/dev/zero` was SIGKILLed after 28 s. A hook that does not
    // return stalls the tool call it exists to speed up.
    //
    // The file-TYPE test is what closes it: `metadata.len()` is 0 for a
    // character device, so a ceiling keyed on size alone lets one straight
    // through. The size ceiling is there for the other half of the same
    // problem — an ordinary multi-hundred-MB file was being read whole and
    // hashed here on every Read, only to be dismissed as "not an image".
    if !session_id.is_empty() && !compaction_recent(session_id) {
        // The guard and the read are ONE call on purpose. Held apart, the
        // condition can be dropped and the read left behind — which is exactly
        // how the unbounded read got here. Now there is no separate condition
        // to lose.
        if let Some(bytes) = read_image_candidate(file_path) {
            let h = tokenade_core::dedup_ref::hash_content(&bytes);
            let ctx = ctx_file_id(parsed);
            let marker = super::read_staging::read_pending_dir().join(format!("imgseen-{ctx}-{h}"));
            if marker.exists() {
                let reason = already_seen_note(file_path, None);
                println!(
                    "{}",
                    serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": reason,
                        }
                    })
                );
                let before = (bytes.len() / 750).max(1_000);
                crate::log_gain(
                    file_path,
                    "image_dedup",
                    tokenade_core::Savings::new(
                        before,
                        tokenade_core::tokenizer::estimate(&reason),
                    ),
                );
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "image_dedup_hit",
                    serde_json::json!({"file": file_path, "hash": h, "elapsed_ms": started.elapsed().as_millis() as u64}),
                );
                return Some(0);
            }
            // Class-2 discipline (register-before-delivery): only mark
            // "seen" AFTER the downscale handler below actually stages a
            // response (or falls through to a normal read, which small
            // images survive). Corrupt/unreadable images must never mark.
            if tokenade_core::image_downscale::image_dimensions(std::path::Path::new(file_path))
                .is_some()
            {
                tokenade_core::platform::ensure_parent_dir(&marker);
                let _ = std::fs::write(&marker, b"1");
            }
        }
    }
    let base = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(file_path)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
        });
    // Formats this agent cannot RENDER must be converted, not merely resized:
    // `prepare_downscaled_read` returns None for an image already within
    // bounds, which would hand the model a .tiff/.bmp/.ico it cannot display at
    // all. Convert those to PNG regardless of size — capability first, economy
    // second (the same order the document path follows).
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let agent = tokenade_core::detect::from_env();
    if img::is_downscalable_ext(ext) && !agent.renders_image_ext(ext) {
        if let Some(rc) = convert_unrenderable_image(parsed, file_path, ext, &base, started) {
            return Some(rc);
        }
    }
    let staged = img::prepare_downscaled_read(std::path::Path::new(file_path), &base)?;
    let before = img::vision_token_estimate(staged.orig_w, staged.orig_h) as usize;
    let after = img::vision_token_estimate(staged.new_w, staged.new_h) as usize;
    crate::log_gain(
        file_path,
        "image-downscale",
        tokenade_core::Savings::new(before, after).billed(),
    );
    let saved = before.saturating_sub(after);
    let hint = format!(
        "tokenade: downscaled this image {}×{} → {}×{} (~{} fewer vision tokens). \
         If you need full resolution, Read `{}`.",
        staged.orig_w,
        staged.orig_h,
        staged.new_w,
        staged.new_h,
        saved,
        staged.full_path.to_string_lossy(),
    );
    let redirect = staged.redirect_path.to_string_lossy().into_owned();
    log_emitted_additional_context("hookwrap-pretool-image", "PreToolUse", &hint);
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason":
                "tokenade: image downscaled to cut vision tokens; full-res copy noted in context",
            "updatedInput": { "file_path": redirect },
            "additionalContext": hint,
        }
    });
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "image_downscale",
        serde_json::json!({
            "before": before,
            "after": after,
            "elapsed_ms": started.elapsed().as_millis() as u64
        }),
    );
    Some(0)
}

/// Image re-read dedup (extracted phase of `handle_read_pretool`). When the
/// Read targets an image, a byte-identical re-read in the same session
/// (same path+mtime+size identity already encoded in `hash`) is denied so the
/// model is not re-shipped a base64 image it already sees upthread; the first
/// sighting registers the identity and passes through. Returns `Some(0)` on
/// either arm (dedup-deny or first-sighting register), `None` when the path is
/// not an image (caller continues).
pub(crate) fn handle_image_dedup_or_register(
    parsed: &serde_json::Value,
    file_path: &str,
    cwd: &std::path::Path,
    hash: &str,
    offset: u64,
    limit: u64,
    size: u64,
    mtime: u64,
    started: std::time::Instant,
) -> Option<i32> {
    if is_image_path(file_path) {
        if tokenade_core::dedup_ref::lookup(cwd, hash).is_some() && ctx_claim_ok(parsed, hash) {
            // DENY the Read (not the head-slice/additionalContext path: Claude
            // Code ignores `limit` on images and would re-load the whole image
            // ANYWAY, then add our note on top = worse). A `permissionDecision:
            // deny` substitutes the reason for the tool result — the image is
            // already a vision block upthread, so the agent loses nothing.
            let reason = already_seen_note(
                file_path,
                Some(&format!("{} KB, size={size} mtime={mtime}", size / 1024)),
            );
            let decision = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            });
            println!("{decision}");
            // Realised saving: the VISION tokens the re-read would have cost,
            // minus the short reason. NOT bytes-based: agents never inline
            // images as base64 text — they go through the vision pipeline,
            // ~(w×h)/750 tokens, capped by the platform downscale (≈1.6-2k
            // for a typical screenshot). The old `bytes×1.37/4` fiction
            // credited up to ~583k tokens for ONE image (seen in prod).
            let before =
                tokenade_core::image_downscale::image_dimensions(std::path::Path::new(file_path))
                    .map(|(w, h)| {
                        tokenade_core::image_downscale::vision_token_estimate(w, h) as usize
                    })
                    // Dims unreadable → conservative flat estimate of a typical
                    // already-downscaled vision block.
                    .unwrap_or(1_600);
            let after = tokenade_core::tokenizer::estimate(
                decision.get("hookSpecificOutput").unwrap()["permissionDecisionReason"]
                    .as_str()
                    .unwrap_or(""),
            );
            gain_log::record(
                "hook",
                "read_image_dedup",
                file_path,
                tokenade_core::Savings::new(before, after),
            );
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_image_dedup_hit",
                serde_json::json!({
                    "file": file_path,
                    "size": size,
                    "hash": hash,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
        // First sighting — register the identity (empty body: the marker path
        // is never used for images) so a re-read dedups, then pass through so
        // Claude Code loads the image for the vision model.
        tokenade_core::dedup_ref::register(cwd, hash, "");
        ctx_tag(parsed, hash);
        let pk = tokenade_core::dedup_ref::path_key(file_path, offset, limit);
        tokenade_core::dedup_ref::register_path_pointer(cwd, &pk, hash);
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_image_dedup_registered",
            serde_json::json!({
                "file": file_path,
                "size": size,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return Some(0);
    }
    None
}

#[cfg(test)]
mod hashable_tests {
    use super::{already_seen_note, image_candidate_ok};

    fn ok(path: &str) -> bool {
        std::fs::metadata(path)
            .map(|m| image_candidate_ok(&m))
            .unwrap_or(false)
    }

    /// A path whose length metadata cannot bound it must never be read whole.
    ///
    /// MEASURED on the published 1.1.8 under a real user HOME: a PreToolUse
    /// `Read` of `/dev/urandom` held the device open and grew to 2.1 GB of RSS
    /// in four seconds, still climbing; `/dev/zero` was SIGKILLed after 28 s.
    /// The image-dedup gate hashed the file BEFORE anything established it was
    /// an image, and `metadata.len()` is 0 for a device, so every ceiling keyed
    /// on size let one through.
    ///
    /// Asserted on the DECISION, never by calling the reader: a test that reads
    /// the device to prove the reader refuses it hangs as soon as the guard
    /// regresses.
    ///
    /// A second arm keeps the CALL SITE honest, from the source: the decision
    /// and the read must stay one call. Comment lines are stripped before
    /// matching, because a comment naming the guard would satisfy a bare
    /// `contains` while the code had dropped it.
    #[test]
    fn a_non_regular_path_is_never_hashed_whole() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-hashable-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Anti-vacuity: an ordinary small file IS a candidate. Without this a
        // predicate answering "no" to everything would pass, and image dedup
        // would be silently dead.
        let real = dir.join("shot.png");
        std::fs::write(&real, b"a regular file").unwrap();
        assert!(
            ok(&real.to_string_lossy()),
            "an ordinary file stays hashable"
        );

        // Sparse, so it costs no disk: above the ceiling, so it is refused.
        let big = dir.join("huge.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(60 * 1024 * 1024).unwrap();
        drop(f);
        assert!(
            !ok(&big.to_string_lossy()),
            "a file past the ceiling must not be read whole to be dismissed as \
             'not an image'"
        );

        // A unix socket rather than a FIFO: also a non-regular file, also
        // length 0, and it needs no subprocess — `mkfifo` would be a spawn,
        // and `every_spawn_either_hides_its_console_or_is_listed_as_wanting_one`
        // rightly refuses an undeclared one.
        #[cfg(unix)]
        {
            let sock = dir.join("s.sock");
            if std::os::unix::net::UnixListener::bind(&sock).is_ok() {
                assert!(
                    !ok(&sock.to_string_lossy()),
                    "a socket is not a regular file and must not be read whole"
                );
            }
        }
        if std::path::Path::new("/dev/zero").exists() {
            assert!(!ok("/dev/zero"), "a character device never ends");
        }
        assert!(
            !ok(&dir.join("absent").to_string_lossy()),
            "missing path answers no"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reader must consult the decision, and nothing else in this module
    /// may read a caller-supplied path whole.
    #[test]
    fn the_reader_still_consults_the_decision() {
        // Only the PRODUCTION half. The assertion below contains the very
        // string it looks for, so scanning the whole file made this guard
        // match its own source and pass on any mutation — a known
        // self-matching trap, in a new dress.
        let whole = include_str!("images.rs");
        let src = tokenade_core::srcsweep::production_lines(whole)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n");
        let src = src.as_str();
        let code: Vec<&str> = src
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with("///")
            })
            .collect();
        assert!(
            code.iter()
                .any(|l| l.contains("if !image_candidate_ok(&md)")),
            "`read_image_candidate` must refuse before it reads"
        );
        // Anti-vacuity: the scan must have seen real code, not an empty filter.
        assert!(
            code.len() > 100,
            "source scan found only {} lines",
            code.len()
        );
    }

    /// **The message carries no gap.**
    ///
    /// Shipped since 2026-08-05: three runs of twenty-two spaces in the middle
    /// of the sentence, the literal's line continuations having been lost.
    /// 66 bytes for nothing, in the only text the agent receives instead of
    /// the image.
    #[test]
    fn the_already_seen_note_carries_no_gap() {
        for note in [
            already_seen_note("/a/b.png", None),
            already_seen_note("/a/b.png", Some("12 KB, size=1 mtime=2")),
        ] {
            assert!(
                !note.contains("  "),
                "run of spaces in the message returned to the agent:\n{note}"
            );
            assert!(note.contains("/a/b.png"), "{note}");
            assert!(note.contains("byte-identical"), "{note}");
        }
        // The identity, when the caller has it, goes into the sentence.
        let with = already_seen_note("/a/b.png", Some("12 KB, size=1 mtime=2"));
        assert!(with.contains("12 KB, size=1 mtime=2"), "{with}");
        // …and its absence leaves no empty parentheses.
        let without = already_seen_note("/a/b.png", None);
        assert!(!without.contains("()"), "{without}");
        assert!(without.len() < with.len());
    }

    /// **A single spelling.** The two refusal paths each wrote their own
    /// sentence, and that is how one of them could carry sixty-six spaces for
    /// a month without the other showing it.
    #[test]
    fn the_note_is_spelled_in_exactly_one_place() {
        let src = std::fs::read_to_string(file!())
            .or_else(|_| {
                std::fs::read_to_string(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("src/hooks/read_pretool/images.rs"),
                )
            })
            .expect("the source of this module");
        let code = tokenade_core::srcsweep::production_lines(&src)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n");
        let code = code.as_str();
        assert_eq!(
            code.matches("[tokenade:image-dedup").count(),
            1,
            "the image-duplicate message is spelled more than once"
        );
    }
}
