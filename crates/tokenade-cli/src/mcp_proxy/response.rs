//! **What the server sends back, folded.**
//!
//! An MCP response is a line of JSON-RPC: a `result` whose content is free
//! text, JSON, logs. That is exactly the material this product knows how to
//! fold — and the only one agent hooks do NOT see, because no matcher
//! catches `mcp__<server>__<tool>`.
//!
//! The fold applies to the LINE, not the connection: a malformed response
//! or a failing fold must return the original line, never interrupt the
//! pump. That is what `fault_fallback_response` guarantees.

use super::*;

/// #4 — drop per-field `description` from a `tools/list` reply.
///
/// **OFF by default since 2026-08-26 (`TOKENADE_MCP_LEAN_SCHEMA=1` to enable),
/// and the doc used to say "off by default" while the code said the opposite.**
/// It was on, and the only evidence behind that default was a savings A/B.
///
/// The saving is real: measured over **337 `inputSchema` subtrees from 92 real
/// manifests**, dropping every per-field description takes a manifest to
/// **0.431** of its bytes. It is also more than half the manifest — the
/// descriptions alone are 56.8 % of it.
///
/// What it costs is what nobody had measured. Of **2 415** per-field
/// descriptions in that corpus, **462 (19.1 %) state a constraint that no
/// `enum` or `const` in the same schema expresses** — `severity_threshold:
/// "Minimum severity to block (high, medium, low)"` with no enum,
/// `pattern_redaction_format: "use {pattern_name} placeholder"`,
/// `start_time_utc: "… in UTC"`. Drop those and the agent has to guess the
/// allowed values of about one parameter in five, on a product whose claim is
/// savings *with no quality loss*. `caveman`, a competitor solving this exact
/// problem, keeps every `description` and drops only `$schema`, `title` and
/// `examples` — the same conclusion from the other side.
///
/// Three retention rules were measured and none is good enough to be a default:
///
/// | rule                                   | ratio | constraint-bearing descriptions lost |
/// |----------------------------------------|-------|--------------------------------------|
/// | drop every description                 | 0.431 | 197 of 197                            |
/// | keep on `required` params              | 0.569 | 168 of 197                            |
/// | keep when it has no `enum`/`format`/…  | 0.738 | 105 of 197                            |
/// | keep when the text states a constraint | 0.584 | 0 — but the rule and the measurement share one English regex, so this is circular, not validated |
///
/// The `required` rule fails because most constrained params are OPTIONAL. So
/// the flag stays, exact as it was, and the DEFAULT changes. A user who wants
/// the 57 % can still ask for it.
///
/// The inert keywords (`title`, `$comment`, `examples`, `$schema`, …) are
/// stripped either way — see [`strip_schema_prose`]. That alone is worth only
/// 0.5 %, which is precisely why it is safe.
pub(crate) fn lean_schema_enabled() -> bool {
    std::env::var("TOKENADE_MCP_LEAN_SCHEMA").is_ok_and(|v| v == "1")
}

/// Recursively strip the prose keywords from a JSON-Schema node, descending
/// only through schema-bearing keywords so a *property named* "description"
/// is never mistaken for the `description` keyword and dropped.
pub(crate) fn strip_schema_prose(node: &mut serde_json::Value, drop_descriptions: bool) {
    use serde_json::Value;
    match node {
        Value::Array(arr) => arr
            .iter_mut()
            .for_each(|v| strip_schema_prose(v, drop_descriptions)),
        Value::Object(map) => {
            // Inert metadata: never callable, never a constraint. Always gone.
            for k in [
                "title",
                "$comment",
                "examples",
                "example",
                "markdownDescription",
            ] {
                map.remove(k);
            }
            // `description` is the one that can carry a constraint nothing else
            // expresses, so it goes only when the operator asked. Passed IN
            // rather than read from the env here: a pure function is the only
            // kind a test can pin without racing every other test in the
            // process on one variable. See [`lean_schema_enabled`].
            if drop_descriptions {
                map.remove("description");
            }
            for kw in ["properties", "patternProperties", "dependentSchemas"] {
                if let Some(props) = map.get_mut(kw).and_then(|v| v.as_object_mut()) {
                    props
                        .values_mut()
                        .for_each(|v| strip_schema_prose(v, drop_descriptions));
                }
            }
            // `propertyNames`, `if`/`then`/`else`, `unevaluated*` and draft-07's
            // `additionalItems` are schema-bearing too and were not descended
            // into, so their prose survived. Adding them removes bytes and
            // changes nothing else.
            for kw in [
                "items",
                "additionalItems",
                "additionalProperties",
                "unevaluatedItems",
                "unevaluatedProperties",
                "propertyNames",
                "not",
                "contains",
                "if",
                "then",
                "else",
            ] {
                if let Some(sub) = map.get_mut(kw) {
                    strip_schema_prose(sub, drop_descriptions);
                }
            }
            for kw in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(arr) = map.get_mut(kw).and_then(|v| v.as_array_mut()) {
                    arr.iter_mut()
                        .for_each(|v| strip_schema_prose(v, drop_descriptions));
                }
            }
            for kw in ["$defs", "definitions"] {
                if let Some(defs) = map.get_mut(kw).and_then(|v| v.as_object_mut()) {
                    defs.values_mut()
                        .for_each(|v| strip_schema_prose(v, drop_descriptions));
                }
            }
        }
        _ => {}
    }
}

/// A minimal JSON-RPC error carrying `id`, armed while a `tools/call` result is
/// being compacted. If the proxy dies in that window the client would otherwise
/// wait out its timeout on a request the server has already answered.
///
/// `id` arrives stringified (JSON-RPC allows a number OR a string), so it is
/// re-encoded by shape: a bare number stays a number, anything else is emitted
/// as a properly escaped JSON string. Getting this wrong would hand the client
/// unparseable JSON in the one place it is least able to recover.
pub(crate) fn fault_fallback_response(id: &str) -> String {
    let id_json = if id.parse::<i64>().is_ok() {
        id.to_string()
    } else {
        serde_json::to_string(id).unwrap_or_else(|_| "null".to_string())
    };
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id_json},\"error\":{{\"code\":-32603,\"message\":\
         \"tokenade: this result could not be delivered (the proxy faulted while compacting it); \
         re-run the call\"}}}}\n"
    )
}

/// Every response leaves through here, so the redaction is applied HERE rather
/// than on each of the branches below — several of which return the server's
/// bytes verbatim (a JSON-RPC error, an unparseable line, a gated session) and
/// each of which was a way out for a secret.
pub(crate) fn transform_mcp_response_line(
    line: &str,
    target_label: &str,
    tool: Option<&str>,
    args: Option<&serde_json::Value>,
) -> String {
    let out = transform_mcp_response_line_inner(line, target_label, tool, args);
    redact_mcp_response(&out)
}

pub(crate) fn transform_mcp_response_line_inner(
    line: &str,
    target_label: &str,
    tool: Option<&str>,
    args: Option<&serde_json::Value>,
) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return line.to_string();
    }
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return line.to_string();
    };
    // JSON-RPC error path — preserve verbatim (the agent reads
    // `error.code` / `error.message` for diagnostics).
    if val.get("error").is_some() {
        return line.to_string();
    }
    // tools/list result shape: `result.tools[]` each carry an `inputSchema`.
    // Progressively disclose — strip schema prose, keep the callable skeleton
    // (#4). Mutually exclusive with the tools/call `result.content` path below.
    // NOT gated on `lean_schema_enabled`: the inert-keyword strip inside
    // `strip_schema_prose` is always safe and always wanted, and only the
    // `description` removal reads the flag. Gating the whole pass on the flag —
    // as this did while the flag was on by default — meant switching the
    // default off would have silently switched the safe half off with it.
    {
        let lean = lean_schema_enabled();
        if let Some(tools) = val
            .pointer_mut("/result/tools")
            .and_then(|v| v.as_array_mut())
        {
            let mut before = 0usize;
            let mut after = 0usize;
            for t in tools.iter_mut() {
                if let Some(schema) = t.get_mut("inputSchema") {
                    before += tokenade_core::tokenizer::estimate(&schema.to_string());
                    strip_schema_prose(schema, lean);
                    after += tokenade_core::tokenizer::estimate(&schema.to_string());
                }
            }
            // Only rewrite if it genuinely shrank (token-accounted).
            if after < before {
                return serde_json::to_string(&val).unwrap_or_else(|_| line.to_string());
            }
            return line.to_string();
        }
    }
    // tools/call result shape: `result.content` is an array of
    // `{type: "text"|"image"|..., ...}` objects.
    let mut total_before: usize = 0;
    let mut total_after: usize = 0;
    let mut bytes_before: usize = 0;
    let mut bytes_after: usize = 0;
    let mut had_text = false;
    // Collected during the pass, appended after it: the array cannot grow
    // while it is being iterated, and a notice must sit NEXT TO the image it
    // describes rather than replace it.
    let mut image_notices: Vec<String> = Vec::new();
    if let Some(content) = val
        .pointer_mut("/result/content")
        .and_then(|v| v.as_array_mut())
    {
        for item in content.iter_mut() {
            let is_text = item
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s == "text")
                .unwrap_or(false);
            if !is_text {
                // MCP image results (Playwright screenshots and friends) were
                // the ONE image channel nothing downscaled — a single user's
                // report bundle carried 1.88M chars of full-res sidechain
                // screenshots. Same policy as the Read hook: cap the long
                // edge (TOKENADE_IMAGE_MAX_EDGE, 0 disables), re-encode, and
                // only swap the payload when it actually shrank.
                let is_image = item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "image")
                    .unwrap_or(false);
                if is_image {
                    if let Some(n) = downscale_mcp_image_item(item, target_label) {
                        image_notices.push(n);
                    }
                }
                continue;
            }
            let Some(text) = item.get("text").and_then(|v| v.as_str()).map(String::from) else {
                continue;
            };
            had_text = true;
            let before_tok = tokenade_core::tokenizer::estimate(&text);
            // 1) TOOL-SPECIFIC compactor first — a dedicated fold for a known
            //    tool's output shape (e.g. browser console logs). Returns None
            //    to defer. 2) else the generic shape-aware compactor (logs,
            //    JSON, ndjson, tables, search output) the Bash/Read paths use.
            //    Either way, 3) stash if still large.
            // Same rule as the Copilot hook and `tokenade read`: a result that
            // already carries a tokenade marker was folded by another channel
            // (an MCP server that shells out through the shim, a nested proxy).
            // One authority per payload.
            if tokenade_core::fold_marker::already_folded(&text) {
                return line.to_string();
            }
            let null = serde_json::Value::Null;
            // Screenshot captures in `computer` / `browser_batch` bodies:
            // compact_for_tool returns None (the tool name is not
            // "screenshot"), and the generic fold would chew the capture
            // notice. leave_uncompacted is the body-fingerprint gate.
            let folded = if tokenade_core::mcp_compact::leave_uncompacted(tool.unwrap_or(""), &text)
            {
                text.clone()
            } else {
                tool.and_then(|t| {
                    tokenade_core::mcp_compact::compact_for_tool(
                        target_label,
                        t,
                        args.unwrap_or(&null),
                        &text,
                    )
                })
                .unwrap_or_else(|| tokenade_core::auto::compact(&text).1)
            };
            // SECURITY: the generic fold does not scrub — same print-site
            // redact as cmd_auto (#902) / cmd_filter (#904). Runs on the
            // model-visible text (the stash itself redacts independently).
            //
            // DELIBERATE-retrieval threshold, not the 5k incidental one. A
            // `tools/call` result is NOT incidental output the way a curl body
            // or a command's stdout is: the agent EXPLICITLY invoked this MCP
            // tool because it wants the content back (a QMD "second brain"
            // search, a docs lookup). Pointer-ifying a normal-size retrieval at
            // 5k strands the agent with a stash ref its host may never expand —
            // the search's whole payload vanishes and the MCP looks "broken"
            // (Silvio/QMD report). Same reasoning as a deliberate `Read`, so it
            // shares the Read bar (`read_threshold_tokens`, ~25k): the fold
            // still runs (savings kept) and only a genuinely huge dump stashes.
            let new_text = tokenade_core::redact::redact_for_model(
                &tokenade_core::disclose::maybe_stash_with_threshold(
                    folded,
                    tokenade_core::disclose::SourceHint::Mcp,
                    tokenade_core::disclose::read_threshold_tokens(),
                ),
            );
            // The threshold above asks "is what remains still huge?". It never
            // asks what we REMOVED — so a 200 KB result folded to a few hundred
            // bytes passed through with no way back. An MCP call is the worst
            // place for that: it is often not repeatable (a paginated fetch, a
            // search against moving data, a report on a mutation), so the folded
            // copy is the only one.
            //
            // This APPENDS a pointer, it does not replace the content, so the
            // concern that motivated the high threshold — stranding a host that
            // cannot expand refs — does not apply: the fold is still all there.
            let dropped = before_tok.saturating_sub(tokenade_core::tokenizer::estimate(&new_text));
            let new_text = crate::wrap::attach_drop_recovery(
                new_text,
                &text,
                dropped,
                before_tok,
                &tokenade_core::disclose::SourceHint::Mcp,
                "mcp-proxy",
            );
            let after_tok = tokenade_core::tokenizer::estimate(&new_text);
            total_before += before_tok;
            total_after += after_tok;
            bytes_before += text.len();
            bytes_after += new_text.len();
            if let Some(field) = item.get_mut("text") {
                *field = serde_json::Value::String(new_text);
            }
        }
        // One text item per image resized, appended after the pass so it sits
        // beside the payload it describes. `content` is an array of typed
        // items by construction, so a `text` item is the protocol's own way
        // to say something about what came back.
        for n in &image_notices {
            content.push(serde_json::json!({"type": "text", "text": n}));
        }
    }
    // Per-tool MCP stats. `op` carries the specific tool when known
    // (`mcp:<tool>`) so the dashboard + server see which MCP tools were
    // compacted and by how much; falls back to the server label.
    let op = match tool {
        Some(t) if !t.is_empty() => format!("mcp:{t}"),
        _ => format!("mcp-proxy:{target_label}"),
    };
    if total_before > 0 && total_after < total_before {
        tokenade_core::gain::record_mcp_tool(
            &op,
            tokenade_core::Savings::new(total_before, total_after),
            bytes_before,
            bytes_after,
        );
    } else if had_text {
        // Seen but NOT compacted (already terse / image-only / shape we don't
        // fold). Record it so the server can report "which MCP tools weren't
        // compacted" — a no-savings row carrying the volume, gated to MCP.
        tokenade_core::gain::record_mcp_uncompacted(&op, total_before);
    }
    let mut out = serde_json::to_string(&val).unwrap_or_else(|_| line.to_string());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}
