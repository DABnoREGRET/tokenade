//! **Read a child's output without keeping all of it.**
//!
//! `Command::output()` reads ALL bytes into memory: on a multi-gigabyte `cat`
//! that is a bomb. The sequential `read_to_string`
//! after `try_wait` pattern is worse — the pipe buffer fills up, the child blocks
//! on write, and `try_wait` spins until the timeout.
//!
//! A background thread reads up to the ceiling then keeps DRAINING and discarding, so
//! that the child can finish. And truncation is STATED: output cut without a
//! banner reads like complete output, which is the worse of the two
//! errors.

// No `use super::*;`: draining a pipe under a ceiling knows neither the
// creation flags nor the groups.

pub fn drain_capped<R: std::io::Read + Send + 'static>(
    r: R,
    cap: usize,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    drain_capped_inner(r, cap, false)
}

/// As [`drain_capped`], but when the stream overflows the cap it also keeps the
/// END of it. Only the COMMAND-capture path asks for this: a build prints its
/// reason last, so a head-only cap keeps exactly the part that does not say
/// why. A fetched document must NOT use it — splicing a marker into the middle
/// of markup would corrupt what the extractor then reads.
pub fn drain_capped_keeping_tail<R: std::io::Read + Send + 'static>(
    r: R,
    cap: usize,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    drain_capped_inner(r, cap, true)
}

pub(crate) fn drain_capped_inner<R: std::io::Read + Send + 'static>(
    mut r: R,
    cap: usize,
    keep_tail: bool,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        // A build that fails prints its reason LAST, and a head-only cap keeps
        // the first 16 MB — precisely the part that does not say why. MEASURED:
        // 17 MB of noise ending in `ERROR: link failed: undefined symbol`, exit
        // 1: the truncation was disclosed and the error was gone. The product
        // already states this principle for user filters ("tail_lines … so the
        // final outcome line survives"); the capture cap never applied it. So
        // reserve a slice of the cap for a rolling tail and keep the end too.
        let tail_cap = if keep_tail {
            (cap / 16).clamp(64 * 1024, 2 * 1024 * 1024).min(cap / 2)
        } else {
            0
        };
        // The head still fills the WHOLE cap. Reserving space up front would
        // shrink every capture, including the ones with no usable tail (a
        // stream with no newline at all), and the cap is a promise about how
        // much we keep. The head is trimmed back only if a tail actually
        // survives its own line-boundary rule.
        let head_cap = cap;
        let mut buf = Vec::with_capacity(head_cap.min(64 * 1024));
        let mut tail: Vec<u8> = Vec::new();
        let mut dropped: usize = 0;
        let mut chunk = [0u8; 16 * 1024];
        let mut truncated = false;
        loop {
            match std::io::Read::read(&mut r, &mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let remaining = head_cap.saturating_sub(buf.len());
                    let take = n.min(remaining);
                    if take > 0 {
                        buf.extend_from_slice(&chunk[..take]);
                    }
                    if take < n {
                        truncated = true;
                        if tail_cap > 0 {
                            tail.extend_from_slice(&chunk[take..n]);
                        }
                        // Keep the tail bounded without copying on every chunk.
                        if tail.len() > tail_cap * 2 {
                            let cut = tail.len() - tail_cap;
                            dropped += cut;
                            tail.drain(..cut);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if tail.len() > tail_cap {
            let cut = tail.len() - tail_cap;
            dropped += cut;
            tail.drain(..cut);
        }
        // Cut back to a line boundary. The cap lands on an arbitrary BYTE,
        // and redaction runs downstream of this buffer — so a secret that
        // straddles the cut leaves its left half behind, short enough that
        // no fixed-length pattern (`AKIA…`, `ghp_…`, `sk-ant-…`) matches it
        // any more, and it ships unredacted. Dropping the partial line costs
        // one truncated line out of the last 16 MB and closes that.
        //
        // Residual case, deliberately left: output with no newline at all in
        // `cap` bytes (a single enormous line). Rolling back would discard
        // everything, which is worse than a partial line, so the buffer is
        // kept as-is and redaction sees it whole-blob.
        if truncated {
            if let Some(nl) = buf.iter().rposition(|&b| b == b'\n') {
                buf.truncate(nl + 1);
            }
            // Same reasoning on the tail's LEFT edge: it starts mid-line, and a
            // secret sliced there would lose the half that makes it matchable.
            if let Some(nl) = tail.iter().position(|&b| b == b'\n') {
                dropped += nl + 1;
                tail.drain(..nl + 1);
            } else {
                dropped += tail.len();
                tail.clear();
            }
            if !tail.is_empty() {
                let marker =
                    format!("\n[… {dropped} bytes omitted — the end of the output follows …]\n");
                // Stay within `cap`: give the tail its room by trimming the
                // head back, on a line boundary so the same half-secret rule
                // holds there too.
                let want = marker.len() + tail.len();
                if buf.len() + want > cap {
                    let keep = cap.saturating_sub(want);
                    buf.truncate(keep);
                    if let Some(nl) = buf.iter().rposition(|&b| b == b'\n') {
                        buf.truncate(nl + 1);
                    } else {
                        buf.clear();
                    }
                }
                buf.extend_from_slice(marker.as_bytes());
                buf.extend_from_slice(&tail);
            }
        }
        (buf, truncated)
    })
}

/// Default stdout cap for Bash-wrapped commands. 16 MB strikes a
/// balance between accommodating real log dumps and bounding memory
/// on pathological cases (cat /var/log/big.log).
pub const DEFAULT_STDOUT_CAP: usize = 16 * 1024 * 1024;

/// Default stderr cap. Smaller than stdout because stderr is usually
/// status messages + warnings, not bulk data.
pub const DEFAULT_STDERR_CAP: usize = 4 * 1024 * 1024;

/// Format a truncation banner suitable for appending to compacted
/// output. Wording rewritten so the agent's first instinct
/// isn't "re-run raw" (same bytes hit the same cap; nothing gained)
/// but rather "narrow the command" — which is the actual fix. The
/// dropped bytes are gone from tokenade's buffer too, so `expand_ref`
/// doesn't apply here.
pub fn truncation_banner(stdout_cap: usize, stderr_cap: usize) -> String {
    format!(
        "\n[tokenade: output exceeded {} MB stdout / {} MB stderr cap; truncated. \
         Narrow the command to capture the dropped region: `head -N` / `tail -N` / \
         `grep <pattern>` / `jq <filter>` (or `tokenade exec` if you \
         just need a derived value over the full stream). `tokenade raw <cmd>` only \
         helps when you can stream-redirect to a file; capture-cap still applies in \
         hook context.]\n",
        stdout_cap / (1024 * 1024),
        stderr_cap / (1024 * 1024),
    )
}
