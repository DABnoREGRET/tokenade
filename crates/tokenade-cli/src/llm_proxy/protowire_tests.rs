use super::*;

/// Write a length-delimited field, the way a real encoder would.
fn field_len(field: u64, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    write_varint(&mut v, (field << 3) | 2);
    write_varint(&mut v, payload.len() as u64);
    v.extend_from_slice(payload);
    v
}

fn field_varint(field: u64, value: u64) -> Vec<u8> {
    let mut v = Vec::new();
    write_varint(&mut v, field << 3);
    write_varint(&mut v, value);
    v
}

fn big_text(n: usize) -> String {
    // A real tool result: repeating lines, which the compactor knows how to
    // fold.
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "drwxr-xr-x  2 someuser someuser 4096 file_{i:05}.rs\n"
        ));
    }
    s
}

/// The nominal case: a tool result buried two message levels deep.
#[test]
fn a_long_string_nested_two_levels_deep_is_folded() {
    let text = big_text(400);
    assert!(text.len() > MIN_FOLDABLE * 4);
    let inner = [field_varint(1, 7), field_len(2, text.as_bytes())].concat();
    let outer = [field_len(3, &inner), field_varint(4, 42)].concat();

    let (out, r) = fold_body(&outer, Framing::Bare, |s| {
        assert_eq!(s, text);
        Some("FOLDED".to_string())
    })
    .expect("the body must be folded");

    assert_eq!(r.folded, 1);
    assert_eq!(r.bytes_before, text.len());
    assert_eq!(r.bytes_after, 6);
    assert!(out.len() < outer.len(), "{} vs {}", out.len(), outer.len());
    // The rest of the message survives: field 4's varint is still there.
    assert!(out.ends_with(&field_varint(4, 42)));
}

/// What the module PROMISES: what it does not fold, it returns byte for byte.
#[test]
fn nothing_to_fold_means_the_body_is_left_alone() {
    let body = [
        field_varint(1, 1),
        field_len(2, b"court"),
        field_len(3, &[0xff, 0xfe, 0x00, 0x01]),
    ]
    .concat();
    assert!(fold_body(&body, Framing::Bare, |_| Some("X".into())).is_none());
}

/// A field holding a real nested message must NEVER be read as text, even if
/// it is long and even if it is valid UTF-8.
#[test]
fn a_nested_message_is_descended_not_swallowed() {
    let text = big_text(200);
    let inner = field_len(1, text.as_bytes());
    let outer = field_len(9, &inner);
    let mut seen: Vec<usize> = Vec::new();
    let (_, r) = fold_body(&outer, Framing::Bare, |s| {
        seen.push(s.len());
        Some("Z".into())
    })
    .expect("folded");
    assert_eq!(r.folded, 1, "a single string, the innermost one");
    assert_eq!(
        seen,
        vec![text.len()],
        "the nested message was not folded as a block"
    );
}

/// The threshold is the safeguard against text/message ambiguity. A short
/// string is never a candidate, whatever the compactor says.
#[test]
fn a_short_string_is_never_folded() {
    let short = "a".repeat(MIN_FOLDABLE - 1);
    let body = field_len(1, short.as_bytes());
    assert!(fold_body(&body, Framing::Bare, |_| Some("Q".into())).is_none());

    let long = "a".repeat(MIN_FOLDABLE + 1);
    let body = field_len(1, long.as_bytes());
    assert!(fold_body(&body, Framing::Bare, |_| Some("Q".into())).is_some());
}

/// Bytes that are not UTF-8 are not text.
#[test]
fn binary_bytes_are_not_treated_as_text() {
    let mut blob = vec![0xffu8; MIN_FOLDABLE * 2];
    blob[0] = 0x08; // a plausible message start, to make the case hostile
    let body = field_len(1, &blob);
    assert!(fold_body(&body, Framing::Bare, |_| Some("Q".into())).is_none());
}

/// A truncated body is relayed, not guessed.
#[test]
fn a_truncated_body_is_refused() {
    let text = big_text(200);
    let mut body = field_len(1, text.as_bytes());
    body.truncate(body.len() - 10);
    assert!(fold_body(&body, Framing::Bare, |_| Some("Q".into())).is_none());
}

/// Field 0 does not exist: a body that starts with it is not protobuf.
#[test]
fn field_zero_is_refused() {
    let body = vec![0x00, 0x01, 0x02];
    assert!(fold_body(&body, Framing::Bare, |_| Some("Q".into())).is_none());
}

/// Connect / gRPC-web frames: the length is REWRITTEN, not copied.
/// That is the mistake that would break the session most silently.
#[test]
fn enveloped_frames_get_their_new_length() {
    let text = big_text(300);
    let msg = field_len(1, text.as_bytes());
    let mut body = vec![0u8];
    body.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    body.extend_from_slice(&msg);

    let (out, r) = fold_body(&body, Framing::Enveloped, |_| Some("SHORT".into()))
        .expect("the frame must be folded");
    assert_eq!(r.folded, 1);
    let declared = u32::from_be_bytes([out[1], out[2], out[3], out[4]]) as usize;
    assert_eq!(
        declared,
        out.len() - 5,
        "the declared length must be the real one"
    );
    assert_eq!(out[0], 0);
}

/// A metadata frame (non-zero flag: Connect end of stream, gRPC-web trailers)
/// is not a message and goes back out as is.
#[test]
fn a_flagged_frame_is_copied_verbatim() {
    let text = big_text(300);
    let msg = field_len(1, text.as_bytes());
    let trailer = b"grpc-status: 0\r\n";
    let mut body = vec![0u8];
    body.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    body.extend_from_slice(&msg);
    body.push(0x80);
    body.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
    body.extend_from_slice(trailer);

    let (out, _) = fold_body(&body, Framing::Enveloped, |_| Some("S".into())).expect("folded");
    assert!(out.ends_with(trailer), "the trailer must survive intact");
    assert_eq!(out[out.len() - trailer.len() - 5], 0x80);
}

/// A truncated frame is refused rather than completed.
#[test]
fn a_truncated_frame_is_refused() {
    let text = big_text(300);
    let msg = field_len(1, text.as_bytes());
    let mut body = vec![0u8];
    body.extend_from_slice(&((msg.len() + 50) as u32).to_be_bytes()); // lies about the length
    body.extend_from_slice(&msg);
    assert!(fold_body(&body, Framing::Enveloped, |_| Some("S".into())).is_none());
}

/// A redaction can LENGTHEN the text, and that must be flagged — otherwise the
/// "never larger than on arrival" rule throws away the safe body and sends the
/// key in clear to the provider. The JSON path already paid for this mistake.
#[test]
fn a_longer_replacement_marks_the_body_as_redacted() {
    let text = big_text(200);
    let body = field_len(1, text.as_bytes());
    let longer = format!("{text}{}", "x".repeat(50));
    let (_, r) = fold_body(&body, Framing::Bare, |_| Some(longer.clone())).expect("rewritten");
    assert!(r.redacted, "a longer replacement is a redaction");
    assert!(r.bytes_after > r.bytes_before);
}

/// A compactor that returns `None` leaves the string intact, and a body with
/// no change at all is not sent.
#[test]
fn a_declining_compactor_leaves_the_body_alone() {
    let text = big_text(200);
    let body = field_len(1, text.as_bytes());
    assert!(fold_body(&body, Framing::Bare, |_| None).is_none());
}

/// The recognised `content-type`s, and above all those that must NOT be.
#[test]
fn the_content_types_are_read_the_way_the_editors_send_them() {
    assert_eq!(framing_of("application/proto"), Some(Framing::Bare));
    assert_eq!(framing_of("application/x-protobuf"), Some(Framing::Bare));
    // UNARY Connect: a bare message. This is what Cursor sends.
    assert_eq!(framing_of("application/connect+proto"), Some(Framing::Bare));
    // STREAMING Connect: frames.
    assert_eq!(
        framing_of("application/connect+streaming+proto"),
        Some(Framing::Enveloped)
    );
    assert_eq!(
        framing_of("application/grpc-web+proto"),
        Some(Framing::Enveloped)
    );
    // With parameters, and in mixed case: both happen for real.
    assert_eq!(
        framing_of("Application/Connect+Streaming+Proto; charset=utf-8"),
        Some(Framing::Enveloped)
    );
    // JSON has its own path. Letting it in here means folding it twice —
    // exactly the double declaration this project refuses.
    assert_eq!(framing_of("application/json"), None);
    assert_eq!(framing_of("application/connect+json"), None);
    assert_eq!(framing_of("text/event-stream"), None);
    assert_eq!(framing_of(""), None);
}

/// A string whose bytes ALSO read as a valid message goes to the "message"
/// branch. That is the accepted trade-off, and it loses nothing: the descent
/// finds the text at the level below if there is one.
#[test]
fn an_ambiguous_payload_is_read_as_a_message() {
    let text = big_text(200);
    let inner = field_len(1, text.as_bytes());
    assert!(looks_like_message(&inner));
    // The same buffer seen as text would be invalid UTF-8 anyway (the field
    // header is not printable), so nothing is lost.
    assert!(std::str::from_utf8(&inner).is_err());
}

/// Depth is bounded: a hostile payload does not blow the stack, it is simply
/// refused.
#[test]
fn deep_nesting_is_bounded_not_fatal() {
    let text = big_text(200);
    let mut buf = field_len(1, text.as_bytes());
    for _ in 0..(MAX_DEPTH + 5) {
        buf = field_len(1, &buf);
    }
    // No panic, no overflow: a verdict.
    let _ = fold_body(&buf, Framing::Bare, |_| Some("S".into()));
}

/// The length varint of an UNTOUCHED field is copied as is, including when it
/// is encoded on more bytes than necessary. Rewriting those bytes would change
/// a body nobody asked us to touch.
#[test]
fn an_untouched_field_keeps_its_original_length_encoding() {
    let text = big_text(200);
    // field 1: length written on 2 bytes where one would do
    let mut padded = Vec::new();
    write_varint(&mut padded, 1 << 3 | 2);
    padded.push(0x85);
    padded.push(0x00); // 5, encoded on two bytes
    padded.extend_from_slice(b"court");
    // field 2: the real tool result
    padded.extend_from_slice(&field_len(2, text.as_bytes()));

    let (out, r) = fold_body(&padded, Framing::Bare, |_| Some("S".into())).expect("folded");
    assert_eq!(r.folded, 1);
    assert!(
        out.starts_with(&padded[..padded.len() - text.len() - 4]),
        "the preceding bytes are intact, length encoding included"
    );
}
