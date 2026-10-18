//! **Nothing an agent chose is read without a bound.**
//!
//! A regular file announces its size before it is read. A device file, an
//! open pipe or a socket does not — and `read_to_end` on one of them grows
//! until the kernel kills the process. MEASURED on `/dev/zero`: ~3.2 GB of
//! RSS per second, ~10 GB in three seconds, then SIGKILL. MEASURED on the
//! released binary with `yes |`: `tokenade read -` reached **50 189 MB**
//! before being killed at 28 s, `tokenade search --html -` 48 943 MB at
//! 19 s — while `filter` and `exec --script -`, which already went through
//! the capped reader, refused in under half a second.
//!
//! That is why these four readers live TOGETHER rather than scattered across
//! the crate root: bounding only one of them leaves the hole open, and that
//! is exactly what had happened — `read /dev/zero` reached the path that
//! serves BYTES, not the one that serves text.
//!
//! Decoding is the other half of the subject: bytes taken from a pipe are
//! lost once the pipe is drained, unlike a file that can be re-read. A
//! compactor handed Latin-1, Windows-1252 or UTF-16 must therefore return
//! its best compaction rather than drop the whole pipe. Replacement with
//! U+FFFD is the LAST resort, not the rule.
//!
//! **This is not the hooks' reader.** `hooks::stdin` carries the other one,
//! and the difference is a CONTRACT, not a detail: above the cap, this one
//! REFUSES loudly (`Err`), because an input half-read and answered as whole
//! is the very defect it exists to prevent; the hooks' one returns `None` and
//! lets the command through without a decision, logging the drop — because
//! a failing hook must be transparent. Hence the two names:
//! `read_whole_or_refuse` here, `read_capped` there.

use std::io::Read;

/// Read all of stdin into `buf` as UTF-8, decoding a legacy encoding where one
/// is detectable and replacing what is left with U+FFFD instead of failing.
/// The claim this comment used to make — that `cmd_proxy` / `cmd_wrap` use
/// `from_utf8_lossy` on captured child output, so this path should too — was
/// true and was the wrong direction: command output was corrected to decode
/// as well (`process_util::decode_child_output`), and lossy is now the last
/// resort on every path rather than the rule on some. A compactor handed
/// Latin-1 / Windows-1252 / UTF-16-ish
/// bytes — legacy HTML charsets piped from `curl`, Excel-exported CSV,
/// non-UTF-8 server logs — must still produce its best-effort compaction
/// rather than drop the whole pipe with a hard "failed to read stdin".
/// Mojibake-best-effort beats total loss, and the bytes the agent piped in
/// are gone once the pipe drains (unlike a file it can re-read).
///
/// Only genuine I/O errors (a broken pipe mid-read) propagate as `Err`, so
/// callers keep their existing `.is_err()` short-circuit. For valid UTF-8
/// this is byte-identical to `read_to_string`, so it is safe drop-in
/// everywhere — including the harness JSON hook readers.
/// A regular file states its size before we read it. A character device, an
/// open pipe or a socket does not — and `read_to_end` on one of those grows
/// until the kernel kills the process. MEASURED on `/dev/zero`: ~3.2 GB of RSS
/// per second, ~10 GB in three seconds, then SIGKILL. An agent that passes a
/// device path should get a sentence back, not take the machine down with it.
///
/// The bound is far above any real input, so nothing legitimate meets it;
/// process substitution (`tokenade read <(git diff)`) reads a pipe and is
/// untouched. Override with `TOKENADE_MAX_INPUT_BYTES` if you really mean it.
pub(crate) fn max_unsized_input_bytes() -> u64 {
    std::env::var("TOKENADE_MAX_INPUT_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(1024 * 1024 * 1024)
}

/// Read a user-supplied PATH into bytes, bounding it when the path is not a
/// regular file. Every entry point that slurps a path an agent chose must go
/// through here: `read /dev/zero` reached the byte-serving path, not the text
/// one, so bounding a single reader left the hole open.
pub(crate) fn read_path_bounded(p: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let regular = std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(true);
    if regular {
        std::fs::read(p)
    } else {
        read_whole_or_refuse(std::fs::File::open(p)?, max_unsized_input_bytes())
    }
}

/// Read at most `cap` bytes, and fail loudly rather than silently truncating —
/// a half-read input answered as if whole is the failure this exists to avoid.
fn read_whole_or_refuse<R: std::io::Read>(mut r: R, cap: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let n = (&mut r).take(cap).read_to_end(&mut bytes)?;
    if n as u64 == cap && r.bytes().next().transpose()?.is_some() {
        return Err(std::io::Error::other(format!(
            "input has no end in sight (over {cap} bytes and still going) — \
             an endless source such as a device file or an open pipe cannot be \
             read whole. Set TOKENADE_MAX_INPUT_BYTES to raise the limit."
        )));
    }
    Ok(bytes)
}

/// Bytes twin of [`read_stdin_lossy`], under the same ceiling.
///
/// Every entry point that slurps STDIN must go through one of these two, for
/// the same reason [`read_path_bounded`] exists on the path side: an agent
/// pipes whatever its own command produced, and a command that never ends is
/// indistinguishable from a slow one until the machine is gone. Measured
/// 2026-09-06 on the published binary with `yes |`: `tokenade read -` reached
/// **50 189 MB** and was OOM-killed at 28 s, `tokenade search --html -` 48 943
/// MB at 19 s — while `filter` and `exec --script -`, which already used the
/// capped reader, refused in under half a second.
pub(crate) fn read_stdin_bytes() -> std::io::Result<Vec<u8>> {
    read_whole_or_refuse(std::io::stdin(), max_unsized_input_bytes())
}

pub(crate) fn read_stdin_lossy(buf: &mut String) -> std::io::Result<()> {
    let bytes = read_whole_or_refuse(std::io::stdin(), max_unsized_input_bytes())?;
    // Same correction as `read_file_lossy`: piped Shift-JIS is still Shift-JIS,
    // and "best effort" has to mean decoding it, not replacing it with U+FFFD.
    match tokenade_core::text_encoding::decode(&bytes) {
        Some((text, _enc)) => buf.push_str(&text),
        None => buf.push_str(&String::from_utf8_lossy(&bytes)),
    }
    Ok(())
}

/// File-path twin of `read_stdin_lossy`: read a compactor's input *file* as
/// UTF-8, replacing any invalid byte sequence with U+FFFD instead of failing.
/// The stdin branch of every `validate_stdin_or_single_file` compactor already
/// tolerates non-UTF-8 (legacy HTML charsets, Excel-exported CSV in cp1252 /
/// UTF-16, Latin-1 server logs) via `read_stdin_lossy`; this keeps the *file*
/// branch of the very same `match` consistent, so `tokenade csv export.csv`
/// behaves like `cat export.csv | tokenade csv -` rather than dying with a hard
/// "Is a directory"-style read error on a perfectly readable, merely-non-UTF-8
/// file. Genuine I/O errors (missing file, permission denied, a directory —
/// `fs::read` still surfaces error 21) propagate as `Err`, so callers keep
/// their existing `Err(e) => eprintln!(...)` short-circuit and the
/// directory hint stays intact. For valid UTF-8 this is byte-identical to
/// `fs::read_to_string`.
pub(crate) fn read_file_lossy<P: AsRef<std::path::Path>>(path: P) -> std::io::Result<String> {
    let path = path.as_ref();
    // A regular file knows its length; anything else has to be bounded. See
    // `read_capped`.
    let regular = std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(true);
    let bytes = if regular {
        std::fs::read(path)?
    } else {
        read_whole_or_refuse(std::fs::File::open(path)?, max_unsized_input_bytes())?
    };
    // DECODE before falling back to lossy. The comment above argues that
    // tolerating "legacy HTML charsets" means not *failing* on them — but
    // `from_utf8_lossy` does not tolerate them, it destroys them: every
    // undecodable byte becomes U+FFFD. Measured on the CrUX Japan top 1 000,
    // where **15 of 165 homepages are Shift-JIS / EUC-JP / Windows-31J**:
    // `tokenade read www_sony_jp.html` returned
    // `> �u���r�A�A�T�C�o�[�V���b�g` for the entire page.
    //
    // This is the same defect already fixed in the WebFetch hook and `cmd_web`;
    // the FILE path never got it. Binary still falls through to the old
    // best-effort behaviour, since `decode` returns `None` for non-text.
    if let Some((text, _enc)) = tokenade_core::text_encoding::decode(&bytes) {
        return Ok(text);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
