//! **What must not go back through as is**: secrets, and images that are too
//! large.
//!
//! Two dangers of opposite nature on the same path. A secret that gets through
//! is permanent damage — it is in the model's context, and it stays there. An
//! oversized image is only a cost, but a cost nobody sees go by: it is resized
//! and NOT removed, with the note that says how to opt out. Skipping the second
//! would be the worse of the two answers, because a missing result gets noticed
//! and an expensive one does not.

// No `use super::*;`: what we refuse to let through is decided on the
// CONTENT of a response, without knowing anything about the pump that read it.

/// Redact EVERY string in an MCP response, wherever it sits.
///
/// The previous pass walked `result.content[].text` and nothing else, which is
/// one of at least five places a server puts text. Measured against a server
/// built to leak, a credential came back in clear through an embedded
/// `resource`, through `structuredContent`, through an `isError` result and
/// through `resources/read`'s `contents[]` — four different shapes, one missing
/// guard each.
///
/// So the walk is exhaustive rather than a list of pointers: a new content type
/// in a future revision of the protocol is covered the day it appears, and
/// `redact` is a no-op on a string that holds no secret, so the cost on an
/// ordinary response is a walk and no allocation.
///
/// The one exception is the base64 payload of an image or audio item: it is not
/// text, no pattern can match it, and it is the one field big enough for the
/// scan to be worth skipping.
pub(crate) fn redact_mcp_response(line: &str) -> String {
    fn walk(v: &mut serde_json::Value, changed: &mut bool) {
        match v {
            serde_json::Value::String(s) => {
                let safe = tokenade_core::redact::redact_for_model(s);
                if &safe != s {
                    *s = safe;
                    *changed = true;
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|i| walk(i, changed)),
            serde_json::Value::Object(map) => {
                let binary_payload = matches!(
                    map.get("type").and_then(|t| t.as_str()),
                    Some("image") | Some("audio")
                );
                for (k, val) in map.iter_mut() {
                    if binary_payload && (k == "data" || k == "blob") {
                        continue;
                    }
                    // In structured output the key IS the context: a bare
                    // 40-character blob is unmatchable on its own, and what
                    // makes it a credential is the field it sits in.
                    if tokenade_core::redact::key_names_a_secret(k) {
                        if let serde_json::Value::String(sv) = val {
                            if !sv.is_empty() && !sv.starts_with("<redacted") {
                                *sv = "<redacted>".to_string();
                                *changed = true;
                                continue;
                            }
                        }
                    }
                    walk(val, changed);
                }
            }
            _ => {}
        }
    }

    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return line.to_string();
    };
    let mut changed = false;
    walk(&mut val, &mut changed);
    if !changed {
        return line.to_string();
    }
    let mut out = serde_json::to_string(&val).unwrap_or_else(|_| line.to_string());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Downscale one `{type:"image", data:<b64>, mimeType}` content item
/// in-place. Silent no-op on foreign shapes, decode failures, disabled
/// downscaling, or when the result wouldn't shrink.
/// Downscale an MCP image payload in place, returning the notice the agent
/// must be given — or `None` when nothing was changed.
///
/// The Read hook has always SAID what it did: "downscaled this image 877×2948
/// → 305×1024 (~N fewer vision tokens). If you need full resolution, Read
/// `<path>`." This channel applied the same policy and said nothing, so an
/// agent handed a screenshot whose text is no longer legible had no way to
/// know it had been resized, let alone why. Measured 2026-09-03 through
/// `mcp-wrap` on a real 877×2948 screenshot: it arrived at 305×1024 with no
/// marker of any kind in the response. One rule, two publishers, one mute.
pub(crate) fn downscale_mcp_image_item(
    item: &mut serde_json::Value,
    target_label: &str,
) -> Option<String> {
    use base64::Engine as _;
    let max_edge = tokenade_core::image_downscale::configured_max_edge();
    if max_edge == 0 {
        return None;
    }
    let mime = item.get("mimeType").and_then(|v| v.as_str())?;
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        _ => return None,
    };
    // Owned, because the payload is replaced further down and the borrow
    // cannot still be alive at that point.
    let b64 = item.get("data").and_then(|v| v.as_str())?.to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .ok()?;
    let (small, w, h) = tokenade_core::image_downscale::downscale_image_bytes(
        &bytes,
        ext,
        max_edge,
        tokenade_core::image_downscale::DEFAULT_MAX_PIXELS,
    )?;
    if small.len() >= bytes.len() {
        return None;
    }
    let new_b64 = base64::engine::general_purpose::STANDARD.encode(&small);
    // Vision tokens, not base64 characters. An image item reaches the model
    // through the vision pipeline: the characters are transport, the cost is
    // area. `b64.len() / 4` was the same bytes-as-tokens fiction the Read hook
    // removed for crediting ~583k tokens on ONE image; it survived here because
    // the rule was corrected in one publisher and not in the other.
    // Dimensions unreadable (they are not, once the decode above succeeded)
    // credits nothing rather than guessing.
    let (orig_w, orig_h) =
        tokenade_core::image_downscale::image_dimensions_bytes(&bytes, ext).unwrap_or((w, h));
    let before_tok = tokenade_core::image_downscale::vision_token_estimate(orig_w, orig_h) as usize;
    let after_tok = tokenade_core::image_downscale::vision_token_estimate(w, h) as usize;
    if let Some(field) = item.get_mut("data") {
        *field = serde_json::Value::String(new_b64);
    }
    tokenade_core::gain::record(
        "wrap-mcp",
        &format!("mcp-image:{target_label}"),
        &format!("mcp-image:{target_label}"),
        tokenade_core::Savings::new(before_tok, after_tok),
    );
    tokenade_core::debug_log::log(
        "mcp-proxy",
        "mcp_image_downscaled",
        serde_json::json!({"target": target_label, "w": w, "h": h,
                           "bytes_before": bytes.len(), "bytes_after": small.len()}),
    );
    // Names the VISION tokens, the same figure just credited, and the same
    // unit the Read hook's hint uses. Base64 characters are transport: an
    // agent reading "N fewer characters" would read a saving five times the
    // one it got.
    Some(format!(
        "[tokenade:image-downscale] the image above was resized to {w}×{h} \
         (long edge capped at {max_edge}px), ~{} fewer vision tokens. Fine \
         detail and small text may no longer be legible; re-run the tool with \
         TOKENADE_IMAGE_MAX_EDGE=0 in the server's environment for the \
         original.",
        before_tok.saturating_sub(after_tok)
    ))
}
