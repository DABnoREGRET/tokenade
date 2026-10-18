//! Fold a PROTOBUF payload without knowing its schema.
//!
//! Four agents in the matrix — Cursor, Windsurf, Devin Desktop, Antigravity —
//! speak their editor's protocol, not JSON. Tier 4 relayed them untouched,
//! and their `Command compaction`, `Documents & media`,
//! `Web search`, `Context optimization` and `Subagent parity`
//! cells stayed `Partial`, with as sole evidence "an unrecognised body is relayed
//! untouched rather than guessed at". Measured in the shipped binaries:
//! `aiserver.v1.*` ×21 in Cursor, `exa.language_server_pb` ×4152 and
//! `google.protobuf` ×651 in Windsurf/Devin.
//!
//! **The schema is not needed.** The protobuf wire format carries the field
//! number AND its type in the same varint, so a message can be walked without
//! a descriptor. This module descends the tree, replaces long UTF-8 strings
//! with their folded version, and re-encodes. Everything else — unknown fields,
//! order, varints, groups — goes back out byte for byte.
//!
//! The safety rule is the same as on the JSON path: at the slightest doubt we
//! return `None`, and the caller relays the original payload. A body that does
//! not re-parse EXACTLY into what we just wrote is never sent.

/// Below this size, a string is not worth the risk of ambiguity.
///
/// This is the central safeguard of this module. A short byte sequence can
/// read BOTH as text and as a valid nested message, and that is exactly the
/// case where being wrong is expensive. A tool result weighs kilobytes:
/// above this threshold the coincidence no longer holds.
const MIN_FOLDABLE: usize = 1024;

/// Maximum descent depth. A legitimate nested message rarely goes beyond a
/// dozen levels; past that we give up rather than burn the stack on a hostile
/// payload.
const MAX_DEPTH: usize = 24;

/// What a pass changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProtoRewrite {
    /// Number of strings replaced.
    pub folded: usize,
    /// Text bytes before folding (sum of the replaced strings).
    pub bytes_before: usize,
    /// Text bytes after folding.
    pub bytes_after: usize,
    /// One of the substitutions redacted a secret. As on the JSON path, this
    /// exempts the body from the "never larger than on arrival" rule.
    pub redacted: bool,
    /// Like [`super::wire::Rewrite::blocks`].
    pub blocks: Vec<(u64, usize, usize, bool, bool)>,
    /// Offset, in the ORIGINAL body, of the first folded string.
    ///
    /// The cache gate needs it to know which part of the prefix already cached
    /// at the provider a rewrite invalidates. Without it we would have to
    /// assume "the whole body", which is the safe side but refuses folds that
    /// cost nothing.
    pub first_fold_at: usize,
}

impl ProtoRewrite {
    pub fn is_noop(&self) -> bool {
        self.folded == 0
    }
}

/// The kind of framing this `content-type` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `application/proto`, `application/x-protobuf`: a single bare message.
    Bare,
    /// `application/connect+proto`, `application/grpc-web+proto`, `application/grpc+proto`:
    /// a sequence of `[flag:1][length:4 BE][payload]` frames.
    Enveloped,
}

/// Recognise a protobuf body by its `content-type`, and say how it is
/// framed. Returns `None` for everything else — including `application/json`,
/// which has its own path and must not go through here.
pub fn framing_of(content_type: &str) -> Option<Framing> {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // Connect has TWO encodings and they are not framed the same way. Unary —
    // `application/connect+proto`, which is what Cursor sends — is a BARE
    // message in the body; only streaming
    // (`application/connect+streaming+proto`) carries `[flag][length]`
    // frames. gRPC and gRPC-web always frame. Mixing the two up makes the
    // first five bytes of the message read as a frame header; the body is
    // then refused and relayed — silently useless.
    if ct.starts_with("application/connect+streaming+proto")
        || ct.starts_with("application/grpc-web+proto")
        || ct.starts_with("application/grpc-web+protobuf")
        || ct.starts_with("application/grpc+proto")
        || ct == "application/grpc-web"
        || ct == "application/grpc"
    {
        return Some(Framing::Enveloped);
    }
    if ct.starts_with("application/connect+proto")
        || ct == "application/proto"
        || ct == "application/protobuf"
        || ct == "application/x-protobuf"
        || ct == "application/octet-stream+proto"
    {
        return Some(Framing::Bare);
    }
    None
}

/// One pass over a protobuf body.
///
/// `fold` receives the text and returns its replacement, or `None` to leave it.
/// This is exactly the JSON path's contract, so that both paths share the same
/// compactor and nothing is counted twice.
///
/// Returns `None` when the body must be relayed VERBATIM: unreadable body,
/// nothing to fold, or unfaithful re-encoding.
pub fn fold_body<F>(raw: &[u8], framing: Framing, mut fold: F) -> Option<(Vec<u8>, ProtoRewrite)>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut r = ProtoRewrite {
        first_fold_at: raw.len(),
        ..Default::default()
    };
    let out = match framing {
        Framing::Bare => rewrite_message(raw, 0, 0, &mut r, &mut fold)?,
        Framing::Enveloped => rewrite_frames(raw, &mut r, &mut fold)?,
    };
    if r.is_noop() {
        return None;
    }
    // The check that makes this module safe: what we are about to send must
    // walk again as valid protobuf. A re-encoding that does not hold together
    // would be relayed to the provider and break the user's session — folding
    // is never important enough for that.
    if !parses_whole(&out, framing) {
        return None;
    }
    Some((out, r))
}

/// Each `[flag][BE length][payload]` frame is folded then re-framed with its
/// NEW length. A non-zero flag (Connect end of stream, gRPC-web trailers)
/// carries metadata, not a message: it is copied as is.
fn rewrite_frames<F>(raw: &[u8], r: &mut ProtoRewrite, fold: &mut F) -> Option<Vec<u8>>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        if raw.len() - i < 5 {
            return None; // truncated frame: we don't guess
        }
        let flag = raw[i];
        let len = u32::from_be_bytes([raw[i + 1], raw[i + 2], raw[i + 3], raw[i + 4]]) as usize;
        let start = i + 5;
        let end = start.checked_add(len)?;
        if end > raw.len() {
            return None;
        }
        let payload = &raw[start..end];
        let new_payload = if flag == 0 {
            rewrite_message(payload, 0, start, r, fold)?
        } else {
            payload.to_vec()
        };
        out.push(flag);
        out.extend_from_slice(&(new_payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&new_payload);
        i = end;
    }
    Some(out)
}

/// Descend a message and return its rewritten version.
///
/// Returns `None` as soon as one byte does not read: the message is then
/// relayed whole by the caller, which is always safer than a half-parse.
fn rewrite_message<F>(
    buf: &[u8],
    depth: usize,
    abs_base: usize,
    r: &mut ProtoRewrite,
    fold: &mut F,
) -> Option<Vec<u8>>
where
    F: FnMut(&str) -> Option<String>,
{
    if depth > MAX_DEPTH {
        return None;
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0usize;
    while i < buf.len() {
        let (tag, next) = read_varint(buf, i)?;
        let field = tag >> 3;
        let kind = (tag & 0x7) as u8;
        if field == 0 {
            return None; // field number 0 does not exist: this is not a message
        }
        let tag_bytes = &buf[i..next];
        i = next;
        match kind {
            // varint
            0 => {
                let (_, after) = read_varint(buf, i)?;
                out.extend_from_slice(tag_bytes);
                out.extend_from_slice(&buf[i..after]);
                i = after;
            }
            // 64 bits
            1 => {
                let end = i.checked_add(8)?;
                if end > buf.len() {
                    return None;
                }
                out.extend_from_slice(tag_bytes);
                out.extend_from_slice(&buf[i..end]);
                i = end;
            }
            // length-delimited: string, bytes, nested message or packed
            // repeated. The only interesting case.
            2 => {
                let (len, after_len) = read_varint(buf, i)?;
                let start = after_len;
                let end = start.checked_add(len as usize)?;
                if end > buf.len() {
                    return None;
                }
                let payload = &buf[start..end];
                let replacement = rewrite_delimited(payload, depth, abs_base + start, r, fold);
                out.extend_from_slice(tag_bytes);
                match replacement {
                    Some(new) => {
                        write_varint(&mut out, new.len() as u64);
                        out.extend_from_slice(&new);
                    }
                    None => {
                        // Nothing changed here: copy the ORIGINAL length as it
                        // was encoded. A varint may be written on more bytes
                        // than necessary, and re-writing it "cleanly" would
                        // change bytes nobody asked us to touch.
                        out.extend_from_slice(&buf[i..start]);
                        out.extend_from_slice(payload);
                    }
                }
                i = end;
            }
            // group (deprecated): start / end. Copied without descending.
            3 => {
                let end = skip_group(buf, i, field, depth)?;
                out.extend_from_slice(tag_bytes);
                out.extend_from_slice(&buf[i..end]);
                i = end;
            }
            4 => return None, // group end without a start
            // 32 bits
            5 => {
                let end = i.checked_add(4)?;
                if end > buf.len() {
                    return None;
                }
                out.extend_from_slice(tag_bytes);
                out.extend_from_slice(&buf[i..end]);
                i = end;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// What to do with a length-delimited field.
///
/// The order of attempts is the heart of this module's correctness:
///
/// 1. **Nested message first.** A message that parses ENTIRELY is almost
///    always a message; reading it as text would lose everything it holds,
///    including the tool results we are looking for.
/// 2. **Text next**, and only above [`MIN_FOLDABLE`]. That is where a tool
///    result lives.
/// 3. **Nothing**, otherwise: binary bytes, truncated protobuf, text too short.
///
/// Returns `None` when nothing changes, so that the caller copies the original
/// bytes without rewriting the length varint.
fn rewrite_delimited<F>(
    payload: &[u8],
    depth: usize,
    abs_at: usize,
    r: &mut ProtoRewrite,
    fold: &mut F,
) -> Option<Vec<u8>>
where
    F: FnMut(&str) -> Option<String>,
{
    if payload.is_empty() {
        return None;
    }
    if looks_like_message(payload) {
        let mut inner = ProtoRewrite {
            first_fold_at: usize::MAX,
            ..Default::default()
        };
        if let Some(new) = rewrite_message(payload, depth + 1, abs_at, &mut inner, fold) {
            if inner.is_noop() {
                return None;
            }
            r.folded += inner.folded;
            r.bytes_before += inner.bytes_before;
            r.bytes_after += inner.bytes_after;
            r.redacted |= inner.redacted;
            r.blocks.extend(inner.blocks);
            r.first_fold_at = r.first_fold_at.min(inner.first_fold_at);
            return Some(new);
        }
        return None;
    }
    let text = std::str::from_utf8(payload).ok()?;
    let short = payload.len() < MIN_FOLDABLE;
    let folded = if short {
        // Too short to be folded without risk of ambiguity — but a secret has
        // no size: `API_KEY=…` fits in 40 bytes. Whatever does not read as a
        // message (tried before) and carries a secret is redacted.
        let clean = tokenade_core::redact::redact_for_model(text);
        (clean != text).then_some(clean)?
    } else {
        fold(text)?
    };
    if folded == text {
        return None;
    }
    if short || tokenade_core::redact::redact_for_model(text) != text {
        r.redacted = true;
    }
    r.folded += 1;
    r.first_fold_at = r.first_fold_at.min(abs_at);
    r.bytes_before += text.len();
    r.bytes_after += folded.len();
    // No known position in this protocol: the credit memory decides.
    let offered = !tokenade_core::fold_marker::already_folded(text)
        && tokenade_core::fold_marker::already_folded(&folded);
    r.blocks.push((
        super::wire::block_identity(text),
        text.len(),
        folded.len(),
        true,
        offered,
    ));
    if folded.len() > text.len() {
        // Longer than on arrival: that is the signature of a redaction
        // (`AWS_SECRET_ACCESS_KEY=…` becomes longer than the key). The JSON
        // path carries the same exemption, for the same reason.
        r.redacted = true;
    }
    Some(folded.into_bytes())
}

/// Do these bytes read as a COMPLETE protobuf message?
///
/// "Complete" is the condition that does all the work: a long text almost
/// always ends on a byte that does not close the last field, so it fails here
/// and goes on to folding. A real message lands exactly.
fn looks_like_message(buf: &[u8]) -> bool {
    let mut i = 0usize;
    let mut fields = 0usize;
    while i < buf.len() {
        let Some((tag, next)) = read_varint(buf, i) else {
            return false;
        };
        let field = tag >> 3;
        let kind = (tag & 0x7) as u8;
        // Field numbers go from 1 to 2^29-1, and 19000..19999 are reserved.
        // Text that "parses" very quickly falls outside these bounds.
        if field == 0 || field > 536_870_911 || (19_000..20_000).contains(&field) {
            return false;
        }
        i = next;
        match kind {
            0 => {
                let Some((_, after)) = read_varint(buf, i) else {
                    return false;
                };
                i = after;
            }
            1 => {
                if i + 8 > buf.len() {
                    return false;
                }
                i += 8;
            }
            2 => {
                let Some((len, after_len)) = read_varint(buf, i) else {
                    return false;
                };
                let Some(end) = after_len.checked_add(len as usize) else {
                    return false;
                };
                if end > buf.len() {
                    return false;
                }
                i = end;
            }
            3 | 4 => return false, // groups: too rare to be worth the bet
            5 => {
                if i + 4 > buf.len() {
                    return false;
                }
                i += 4;
            }
            _ => return false,
        }
        fields += 1;
    }
    fields > 0
}

/// Skip a deprecated group, counting nesting.
fn skip_group(buf: &[u8], mut i: usize, field: u64, depth: usize) -> Option<usize> {
    if depth > MAX_DEPTH {
        return None;
    }
    while i < buf.len() {
        let (tag, next) = read_varint(buf, i)?;
        let f = tag >> 3;
        let kind = (tag & 0x7) as u8;
        i = next;
        match kind {
            0 => i = read_varint(buf, i)?.1,
            1 => i = i.checked_add(8).filter(|e| *e <= buf.len())?,
            2 => {
                let (len, after_len) = read_varint(buf, i)?;
                i = after_len
                    .checked_add(len as usize)
                    .filter(|e| *e <= buf.len())?;
            }
            3 => i = skip_group(buf, i, f, depth + 1)?,
            4 => {
                if f == field {
                    return Some(i);
                }
                return None;
            }
            5 => i = i.checked_add(4).filter(|e| *e <= buf.len())?,
            _ => return None,
        }
    }
    None
}

/// The final check: does what we are going to send re-parse entirely?
fn parses_whole(buf: &[u8], framing: Framing) -> bool {
    match framing {
        Framing::Bare => looks_like_message(buf),
        Framing::Enveloped => {
            let mut i = 0usize;
            while i < buf.len() {
                if buf.len() - i < 5 {
                    return false;
                }
                let flag = buf[i];
                let len =
                    u32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
                let Some(end) = (i + 5).checked_add(len) else {
                    return false;
                };
                if end > buf.len() {
                    return false;
                }
                if flag == 0 && len > 0 && !looks_like_message(&buf[i + 5..end]) {
                    return false;
                }
                i = end;
            }
            true
        }
    }
}

fn read_varint(buf: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(i)?;
        if shift >= 64 {
            return None;
        }
        value |= ((b & 0x7f) as u64) << shift;
        i += 1;
        if b & 0x80 == 0 {
            return Some((value, i));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            return;
        }
    }
}

#[cfg(test)]
#[path = "protowire_tests.rs"]
mod tests;
