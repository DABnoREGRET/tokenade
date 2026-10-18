//! Binary blobs: what they are, and what the agent can run to read them.
//!
//! `html_to_markdown` on binary bytes produces garbage, so these bypass the
//! text path entirely and answer with metadata plus the recovery command.

use super::*;

/// True when `bytes` look like a non-text blob. Three checks in
/// priority order:
///
///   1. `infer::get` — magic-byte database (~1 500 patterns).
///   2. `sniff_binary_magic` — ASCII-prefix fallback for cases
///      `infer` doesn't cover (the OggS/GIF87a paths).
///   3. Density heuristic: ≥5 % control-character bytes (`< 0x20`
///      excluding `\n` / `\r` / `\t`) in the first 4 KB → almost
///      certainly binary (typical text < 0.1 %).
///
/// Public for callers higher up (the Bash wrapper, for example) that
/// short-circuit BEFORE launching compactors that can only read
/// text.
pub fn detect_binary(bytes: &[u8]) -> Option<String> {
    if let Some(t) = infer::get(bytes) {
        // `infer` recognises a few TEXT formats by signature
        // (html `<!DOCTYPE`/`<html`, xml `<?xml`, shellscript `#!`).
        // Those are NOT binary — routing them through the binary
        // stash hides readable content (e.g. an HTML file the agent
        // wanted to read) and forces an extra Read. Let the text
        // compactor handle them.
        if t.matcher_type() == infer::MatcherType::Text {
            return None;
        }
        let ext = map_infer_extension(t.extension());
        // A signature made of printable ASCII cannot tell a header from a
        // sentence that opens the same way. `MAC ` is Monkey's Audio and also
        // `MAC host=127.0.0.1`; `MThd`, `MSCF`, `8BPS` and `SQLite format 3`
        // all fired on prose on the shipped 1.1.6 binary. The bytes AFTER the
        // signature settle it, so ask them — except for the formats that are
        // legitimately ASCII throughout, which have nothing to answer with.
        if !crate::textshape::is_text_encoded_format(ext)
            && crate::textshape::looks_like_text(bytes)
        {
            return None;
        }
        return Some(ext.to_string());
    }
    if let Some(b) = sniff_binary_magic(bytes) {
        if !crate::textshape::is_text_encoded_format(b) && crate::textshape::looks_like_text(bytes)
        {
            return None;
        }
        return Some(b.to_string());
    }
    let sample_len = bytes.len().min(4096);
    if sample_len < 64 {
        return None; // too short to be sure
    }
    let sample = &bytes[..sample_len];
    // Terminal escape sequences are presentation, not evidence of binary — see
    // `textshape::control_census`, which both this heuristic and
    // `looks_like_text` now ask, so the two can never disagree about what a
    // control byte is. Before it, `grep --color=always` (11.9 % escape bytes)
    // came back to the agent as `(Binary content — not shown as text)`.
    let census = crate::textshape::control_census(sample);
    if census.payload == 0 {
        // Nothing but escape sequences: a screen clear, not a binary.
        return None;
    }
    if census.control * 100 / census.payload >= 5 {
        return Some("bin".to_string());
    }
    None
}

/// Persist `bytes` to the stash and emit a metadata-only summary.
/// Common path for both binary entry points.
pub(super) fn stash_as_binary(bytes: &[u8], bin_kind: &str, hint: &SourceHint) -> String {
    // Compose the extension: for Web hint the result is
    // `api.rest.<kind>` / `graphql.json` / etc.; otherwise the raw
    // kind. We pass the empty string as `text` because
    // compose_extension only consults `text` to disambiguate JSON /
    // GraphQL — for binaries the hint+kind combination is enough.
    let ext = match hint {
        SourceHint::File(p) => p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| bin_kind.to_string()),
        SourceHint::Web { .. } => {
            // Don't bury binary kinds under api.rest.bin — that loses
            // the actual extension. Use the detected one directly.
            bin_kind.to_string()
        }
        SourceHint::GraphQl => "graphql.json".to_string(),
        SourceHint::Shell | SourceHint::Mcp | SourceHint::Unknown => bin_kind.to_string(),
    };
    let hash = crate::dedup_ref::hash_content(bytes);
    let path = match ensure_stashed_bytes(bytes, &hash, &ext) {
        Ok(p) => p,
        Err(e) => {
            crate::debug_log::log(
                "disclose",
                "stash_write_failed",
                serde_json::json!({
                    "hash": hash, "bytes": bytes.len(), "ext": ext,
                    "error": e.to_string(), "binary": true,
                }),
            );
            // Stash failed — emit a placeholder so the agent at least
            // knows it was binary. Returning the raw lossy text would
            // pollute the context with garbage.
            return format!(
                "[disclose:binary kind={bin_kind} bytes={n} stash=failed] \
                 (binary content cannot be displayed as text; the stash directory \
                 is unwritable — re-fetch the source for the actual bytes)",
                n = bytes.len(),
            );
        }
    };
    build_binary_metadata(bytes, bin_kind, &ext, &path)
}

/// Metadata-only summary for a binary stash. No snippet — every byte
/// past the magic prefix is noise to the model. Includes parsing
/// hints tailored to the kind so the agent picks the right tool
/// (xxd / pdftotext / unzip / ffprobe / file).
pub(super) fn build_binary_metadata(
    bytes: &[u8],
    bin_kind: &str,
    ext: &str,
    path: &Path,
) -> String {
    let n = bytes.len();
    // The kind the SNIFFER found, unless the extension names an image format
    // this build can decode. `infer` has no magic number for TGA (its
    // signature is optional and at the END of the file) or for Radiance HDR,
    // so both came back `bin` — while the line above them read
    // `[tokenade:image 120x80]`, because the decoder had already opened them.
    // Two surfaces, one truth: the half that knew handed nothing to the half
    // that names the file and picks the advice, and the reader was told to run
    // `xxd` on a picture.
    let bin_kind = if bin_kind == "bin" && crate::image_downscale::is_downscalable_ext(ext) {
        ext
    } else {
        bin_kind
    };
    let tools = tools_for_binary_kind(bin_kind);
    // A format tokenade EXTRACTS only reaches the binary summary once the
    // extractor has already run and come back with nothing usable. Saying only
    // "binary content, try pdftotext" there is misleading in the one direction
    // that costs the reader time: it reads as "tokenade does not do PDFs" —
    // which is the opposite of what the scaffolded instructions promise — when
    // the fact on hand is that THIS file yielded no text. Name that, and keep
    // the tool hint for the reader who wants a second opinion.
    let attempted = if crate::docs::detect_format(Some(&format!("x.{ext}")), bytes).is_some() {
        match declared_password(bytes) {
            Some(open) => format!(
                "\ntokenade reads {ext} files; THIS one is password-protected, so no text \
                 can be read without its password. {open}"
            ),
            None => format!(
                "\ntokenade reads {ext} files; no text could be extracted from THIS one, \
                 so it is most likely truncated, empty, or encrypted."
            ),
        }
    } else {
        String::new()
    };
    format!(
        "[disclose:binary kind={bin_kind} ext={ext} bytes={n} path={}]\n\
         (Binary content — not shown as text. Inspect with: {tools}){attempted}",
        path.display(),
    )
}

/// The file SAYS it is encrypted — no need to guess among "truncated, empty, or
/// encrypted" (pdf-mcp answers `password_required`). A PDF names its
/// `/Encrypt` dictionary; an encrypted Office file is not a zip at all but a
/// CFB container holding a stream named `EncryptedPackage` (UTF-16). Only
/// reached once extraction failed, so a PDF encrypted with an owner password
/// alone — which reads fine — never lands here. Returns how to open it.
fn declared_password(bytes: &[u8]) -> Option<&'static str> {
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    if bytes.starts_with(b"%PDF") && has(b"/Encrypt") {
        return Some("With the password: `qpdf --password=<pw> --decrypt <in>.pdf <out>.pdf`, then read <out>.pdf.");
    }
    let utf16: Vec<u8> = "EncryptedPackage".bytes().flat_map(|b| [b, 0]).collect();
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) && has(&utf16) {
        return Some("Open it with the password in its office app and save a copy without one, then read that copy.");
    }
    None
}

/// Tool-hint string for a binary kind. Tells the agent which CLI to
/// reach for; without this the default reflex is often "Read the
/// file" which dumps the same garbage we just kept out of context.
pub(super) fn tools_for_binary_kind(kind: &str) -> &'static str {
    match kind {
        "pdf" => "pdftotext <path> -, mupdf-tools, pdfinfo",
        "png" | "jpg" | "gif" | "webp" | "bmp" | "tif" | "heif" | "avif" | "jxr" | "psd"
        | "ico" => "file <path>, identify <path> (ImageMagick), exiftool <path>",
        // Every other image format this build can decode, asked of the
        // decoder's own list rather than repeated here — the two lists had
        // already drifted: `pnm` is a kind the sniffer emits and was not in the
        // one above, so a Netpbm image was told to try `xxd`.
        k if crate::image_downscale::is_downscalable_ext(k) => {
            "file <path>, identify <path> (ImageMagick), exiftool <path>"
        }
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "z" | "lz" | "zst" | "ar" => {
            "unzip -l <path>, tar tf <path>, 7z l <path>"
        }
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus" | "amr" | "mid" | "aiff" => {
            "ffprobe -hide_banner <path>, mediainfo <path>"
        }
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "flv" | "wmv" | "m4v" | "mpg" | "3gp" => {
            "ffprobe -hide_banner <path>, mediainfo <path>"
        }
        "ttf" | "otf" | "woff" | "woff2" | "eot" => "fc-query <path>, otfinfo -i <path>",
        "exe" | "elf" | "class" | "dex" => "file <path>, readelf -h <path>, objdump -p <path>",
        "wasm" => "wasm-objdump -x <path>, wasm2wat <path>",
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => {
            "libreoffice --headless --cat <path>, unzip -l <path> (Office files are zip)"
        }
        "epub" => "unzip -l <path>, calibredb",
        "deb" | "rpm" | "apk" => "ar t <path>, rpm -qpi <path>, dpkg-deb -I <path>",
        _ => "file <path>, xxd <path> | head, hexdump -C <path> | head",
    }
}

#[cfg(test)]
mod image_kind_tests {
    use super::*;

    /// Every image format this build can DECODE is named as one when it is
    /// stashed, and gets the advice for a picture.
    ///
    /// The two lists had drifted: `infer` has no magic number for TGA (its
    /// signature is optional and sits at the END of the file) or for Radiance
    /// HDR, so both were stashed as `kind=bin` under "Inspect with: file, xxd,
    /// hexdump" — on the very line below `[tokenade:image 120x80]`, which the
    /// decoder had just printed. `pnm`, a kind the sniffer DOES emit, was
    /// missing from the image advice for the same reason: a second hand-written
    /// list.
    #[test]
    fn every_decodable_image_is_advised_as_a_picture() {
        for ext in [
            "png", "jpg", "gif", "webp", "bmp", "tif", "ico", "tga", "pnm", "qoi", "hdr",
        ] {
            assert!(
                crate::image_downscale::is_downscalable_ext(ext),
                "{ext} must be in the decoder's own list for this test to mean anything"
            );
            assert!(
                tools_for_binary_kind(ext).contains("identify"),
                "{ext} is decodable, so the advice must be the one for a picture"
            );
        }
        // Anti-vacuity: the fallback still exists for what really is opaque.
        assert!(tools_for_binary_kind("dat").contains("xxd"));
        assert!(tools_for_binary_kind("zip").contains("unzip"));
    }

    /// An extension the sniffer cannot confirm names the kind — but only when
    /// the sniffer had nothing, and only for a format we can decode.
    #[test]
    fn the_extension_names_the_kind_only_where_the_sniffer_was_blank() {
        let png = b"\x89PNG\r\n\x1a\n and then some bytes";
        let dir = std::env::temp_dir().join(format!("tkn-binkind-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("x.png");

        let out = build_binary_metadata(png, "bin", "tga", &p);
        assert!(
            out.contains("kind=tga"),
            "a decodable extension names it: {out}"
        );
        assert!(
            out.contains("identify"),
            "and brings the picture advice: {out}"
        );

        let out = build_binary_metadata(png, "png", "jpg", &p);
        assert!(
            out.contains("kind=png"),
            "the sniffer knew, so it wins over the extension: {out}"
        );

        let out = build_binary_metadata(png, "bin", "dat", &p);
        assert!(
            out.contains("kind=bin"),
            "an extension we cannot decode changes nothing: {out}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod a_password_protected_document_says_so {
    use super::build_binary_metadata;
    use std::path::Path;

    /// pdf-mcp answers `password_required`; we answered "most likely
    /// truncated, empty, or encrypted" — a guess where the file says which.
    #[test]
    fn an_encrypted_document_is_named_and_an_unreadable_one_is_not() {
        let p = Path::new("/x/doc");
        let pdf = b"%PDF-1.7\n1 0 obj<<>>endobj\ntrailer<</Root 1 0 R/Encrypt 5 0 R>>\n%%EOF\n";
        let out = build_binary_metadata(pdf, "pdf", "pdf", p);
        assert!(
            out.contains("password-protected") && out.contains("qpdf --password"),
            "{out}"
        );
        let plain = b"%PDF-1.7\n1 0 obj<<>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
        let out = build_binary_metadata(plain, "pdf", "pdf", p);
        assert!(
            out.contains("most likely truncated") && !out.contains("password"),
            "{out}"
        );
        let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        cfb.resize(512, 0);
        cfb.extend("EncryptedPackage".bytes().flat_map(|b| [b, 0]));
        let out = build_binary_metadata(&cfb, "doc", "docx", p);
        assert!(out.contains("password-protected"), "{out}");
    }
}
