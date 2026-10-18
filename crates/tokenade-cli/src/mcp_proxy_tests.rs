// The tests of `mcp_proxy`.
//
// Spliced in by `include!` from `mcp_proxy.rs`: the same module, hence the
// same private access and the same `super::` paths, without a single line of
// visibility changed. This is the form this repository already uses for 33
// other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod lean_schema_tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> serde_json::Value {
        json!({
            "type": "object",
            "description": "verbose tool prose that the agent doesn't need to dispatch",
            "properties": {
                "path": {"type": "string", "description": "the file path", "examples": ["/a/b"]},
                // a property LITERALLY named "description" must survive.
                "description": {"type": "string", "title": "Desc field"},
                "mode": {"type": "string", "enum": ["r", "w"], "description": "drop me"},
                // The constraint that lives ONLY in the prose — the reason the
                // description strip is no longer a default.
                "severity": {"type": "string", "description": "Minimum severity to block (high, medium, low)"}
            },
            "required": ["path"]
        })
    }

    #[test]
    fn the_lean_strip_keeps_the_callable_skeleton() {
        let mut schema = manifest();
        strip_schema_prose(&mut schema, true);
        assert!(schema.get("description").is_none());
        let props = schema.pointer("/properties").unwrap();
        assert!(props.pointer("/path/description").is_none());
        assert!(props.pointer("/path/examples").is_none());
        assert_eq!(props.pointer("/path/type").unwrap(), "string");
        assert_eq!(schema.pointer("/required/0").unwrap(), "path");
        assert_eq!(props.pointer("/mode/enum/1").unwrap(), "w");
        // the PROPERTY named "description" is intact (not treated as a keyword).
        assert!(props.get("description").is_some());
        assert_eq!(props.pointer("/description/type").unwrap(), "string");
        assert!(props.pointer("/description/title").is_none());
    }

    /// The DEFAULT pass: inert metadata goes, every `description` stays.
    ///
    /// Measured 2026-08-26 over 337 real `inputSchema` subtrees: 462 of 2 415
    /// per-field descriptions (19.1 %) state a constraint no `enum` or `const`
    /// expresses. `severity` below is one of them, copied from the corpus.
    #[test]
    fn by_default_a_constraint_that_lives_only_in_the_prose_survives() {
        let mut schema = manifest();
        strip_schema_prose(&mut schema, false);
        let props = schema.pointer("/properties").unwrap();
        assert_eq!(
            props
                .pointer("/severity/description")
                .and_then(|v| v.as_str()),
            Some("Minimum severity to block (high, medium, low)"),
            "the only place the allowed values are written was dropped"
        );
        assert_eq!(
            props.pointer("/path/description").and_then(|v| v.as_str()),
            Some("the file path")
        );
        // …while the inert half still goes, flag or no flag.
        assert!(props.pointer("/path/examples").is_none());
        assert!(props.pointer("/description/title").is_none());
    }

    /// The flag is genuinely OFF by default, and the doc no longer says the
    /// opposite of the code.
    #[test]
    fn the_flag_is_off_unless_it_is_set_to_one() {
        // Read the source rather than the env: a test that mutates a process
        // -wide variable races every other test in the binary.
        // The FAMILY, not the trunk: `lean_schema_enabled` moved down into
        // `mcp_proxy/response.rs`, and reading only the trunk would fail this
        // test on an absence that is not one.
        let root = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        let src = tokenade_core::source_guard::module_family(&root, "mcp_proxy");
        let src = src.as_str();
        let body = src
            .split("fn lean_schema_enabled() -> bool {")
            .nth(1)
            .expect("the gate is declared here");
        let body = &body[..body.find('}').unwrap()];
        assert!(
            body.contains("== \"1\""),
            "the gate must be opt-IN, found: {body}"
        );
    }

    /// The keywords that were not descended into kept their prose.
    #[test]
    fn every_schema_bearing_keyword_is_descended_into() {
        for kw in [
            "if",
            "then",
            "else",
            "propertyNames",
            "additionalItems",
            "unevaluatedItems",
            "unevaluatedProperties",
        ] {
            let mut schema = json!({ kw: {"type": "string", "title": "gone", "examples": [1]} });
            strip_schema_prose(&mut schema, true);
            assert!(
                schema.pointer(&format!("/{kw}/title")).is_none(),
                "`{kw}` was not descended into"
            );
        }
        for kw in ["patternProperties", "dependentSchemas"] {
            let mut schema = json!({ kw: {"^a": {"type": "string", "title": "gone"}} });
            strip_schema_prose(&mut schema, true);
            assert!(
                schema.pointer(&format!("/{kw}/^a/title")).is_none(),
                "`{kw}` was not descended into"
            );
        }
    }
}

#[cfg(test)]
mod image_tests {
    /// The image policy has THREE publishers — the resize itself, the
    /// `mcp-wrap --help`, and the CLAUDE.md block scaffolded into every
    /// project — and the scaffold was the one left saying "Image results pass
    /// through untouched" after the other two had been corrected. A user
    /// reading their own CLAUDE.md was told the opposite of what the tool does.
    #[test]
    fn every_publisher_of_the_image_policy_says_the_same_thing() {
        let help = super::mcp_proxy_help_text();
        let scaffold = crate::install::GLOBAL_CLAUDE_MD_BLOCK;
        // The global fence is a short "Tokenade is installed" pointer — it
        // must not contradict the policy, and it no longer publishes it.
        assert!(
            !scaffold.contains("Image results pass through untouched"),
            "CLAUDE.md block still claims images are untouched"
        );
        assert!(
            !help.contains("Image results pass through untouched"),
            "mcp-wrap --help still claims images are untouched"
        );
        assert!(
            help.contains("resized"),
            "mcp-wrap --help does not say images are resized"
        );
        assert!(
            help.contains("TOKENADE_IMAGE_MAX_EDGE=0"),
            "the help does not name the opt-out"
        );
    }

    #[test]
    fn mcp_image_item_downscales_big_png() {
        use base64::Engine as _;
        // 2000x1400 gradient PNG — big enough to trip the max-edge cap.
        let mut img = image::RgbImage::new(2000, 1400);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x % 256) as u8, 40, 80]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut item = serde_json::json!({
            "type": "image", "mimeType": "image/png", "data": b64,
        });
        let notice = super::downscale_mcp_image_item(&mut item, "test");
        let new_len = item.get("data").and_then(|v| v.as_str()).unwrap().len();
        assert!(
            new_len < b64.len(),
            "downscaled payload must shrink: {new_len} vs {}",
            b64.len()
        );

        // Resizing in silence is the defect this returns a notice for: the
        // Read hook has always said what it did, and this channel said
        // nothing, so an agent handed a screenshot whose text is no longer
        // legible could not tell that from a bad screenshot.
        let notice = notice.expect("a resize must return a notice");
        assert!(
            notice.contains("image-downscale"),
            "the notice does not identify itself: {notice}"
        );
        for named in ["1024", "resized to"] {
            assert!(
                notice.contains(named),
                "the notice does not say `{named}`: {notice}"
            );
        }
        assert!(
            notice.contains("TOKENADE_IMAGE_MAX_EDGE"),
            "the notice does not name the way out: {notice}"
        );
        // The saving it claims must be the saving it made, in the unit that
        // reaches the model. An image item goes through the VISION pipeline,
        // so its cost is area, not the base64 characters that carried it:
        // crediting `b64.len() / 4` was the bytes-as-tokens fiction the Read
        // hook removed for crediting ~583k tokens on ONE image. Parse the
        // dimensions back out of the notice and recompute.
        let dims = notice
            .split("resized to ")
            .nth(1)
            .and_then(|t| t.split(' ').next())
            .expect("the notice states the new dimensions");
        let (nw, nh) = dims.split_once('×').expect("dimensions are W×H");
        let vt = tokenade_core::image_downscale::vision_token_estimate;
        let expected = vt(2000, 1400) - vt(nw.parse().unwrap(), nh.parse().unwrap());
        assert!(
            notice.contains(&format!("~{expected} fewer vision tokens")),
            "the notice claims a different saving than it made ({expected}): {notice}"
        );
        // …and that figure is nowhere near the character count. This is the
        // assertion that fails if the fiction comes back: the payload delta
        // here is tens of thousands where the vision saving is hundreds.
        let char_fiction = ((b64.len() - new_len) / 4) as u64;
        assert!(
            expected * 4 < char_fiction,
            "the saving is being credited in base64 characters, not vision \
             tokens: claimed {expected}, chars/4 would be {char_fiction}"
        );

        // An image already within the cap is not resized, and says nothing.
        let mut small = image::RgbImage::new(64, 48);
        for (x, _y, px) in small.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, 7, 9]);
        }
        let mut sbytes: Vec<u8> = Vec::new();
        small
            .write_to(
                &mut std::io::Cursor::new(&mut sbytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let sb64 = base64::engine::general_purpose::STANDARD.encode(&sbytes);
        let mut sitem = serde_json::json!({
            "type": "image", "mimeType": "image/png", "data": sb64.clone(),
        });
        assert!(
            super::downscale_mcp_image_item(&mut sitem, "test").is_none(),
            "an image within the cap produced a notice"
        );
        assert_eq!(
            sitem.get("data").and_then(|v| v.as_str()).unwrap(),
            sb64,
            "an image within the cap was rewritten"
        );

        // And the notice must reach the RESPONSE, not just the function: it
        // is appended to `result.content` after the pass, which a test on
        // the helper alone cannot see.
        let line = serde_json::json!({
            "jsonrpc": "2.0", "id": 7,
            "result": {"content": [{"type": "image", "mimeType": "image/png", "data": b64}]}
        })
        .to_string();
        let out = super::transform_mcp_response_line(&line, "test", Some("screenshot"), None);
        let val: serde_json::Value = serde_json::from_str(&out).expect("valid json out");
        let content = val
            .pointer("/result/content")
            .and_then(|v| v.as_array())
            .expect("content array survived");
        let kinds: Vec<&str> = content
            .iter()
            .filter_map(|i| i.get("type").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec!["image", "text"],
            "the response carries {kinds:?}, not the image plus its notice"
        );
        assert!(
            content[1]
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("image-downscale")),
            "the appended item is not the notice: {:?}",
            content[1]
        );
    }

    /// A tool call is often not repeatable — a paginated fetch, a search over
    /// moving data, a report on a mutation that already happened. So when the
    /// fold removes most of the payload, the pointer is the only copy left.
    #[test]
    fn a_heavily_folded_mcp_result_leaves_a_pointer_that_returns_the_bytes() {
        // Isolate the stash. This test writes a fold and then reads it back by
        // hash; against the REAL stash it raced the opportunistic TTL sweep and
        // failed once in a full run while passing alone. `cfg!(test)` does not
        // silence that sweep from here — it is FALSE inside `tokenade-core`
        // when this crate's test binary links it.
        // The fold this asserts reads process env, and the env-rewriting
        // tests serialise on `ENV_RW_LOCK`, not on `ENV_LOCK` alone. Hold
        // BOTH. The READ side suffices: it excludes the writers, which is the
        // whole hazard, and stays parallel with the other readers.
        // ONE order everywhere: `ENV_LOCK` outermost, `ENV_RW_LOCK`
        // inside it. Two orders in one tree is an inversion waiting to happen.
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _rw = crate::tests::env_guard();
        let stash = std::env::temp_dir().join(format!("tok-mcp-stash-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&stash);
        // SAFETY: process-wide; ENV_LOCK is held for the whole test.
        unsafe {
            std::env::set_var("TOKENADE_STASH_DIR", &stash);
        }
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: same lock still held by the caller's guard.
                unsafe {
                    std::env::remove_var("TOKENADE_STASH_DIR");
                }
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _restore = Restore(stash);

        let payload: String = (0..8_000)
            .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/srv/app/{i}\n"))
            .collect();
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {"content": [{"type": "text", "text": payload}]}
        })
        .to_string();
        let out = super::transform_mcp_response_line(&line, "test-server", None, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.len() < line.len() / 10,
            "the fold happened: {}",
            text.len()
        );
        let hash = text
            .split("expand-ref ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .map(|s| s.trim_end_matches('`').to_string())
            .unwrap_or_default();
        assert_eq!(hash.len(), 12, "a recovery pointer rides along:\n{text}");
        let (back, _) = tokenade_core::disclose::read_stash_by_hash(&hash).expect("stash resolves");
        assert!(back.contains("worker=7999"), "the dropped tail came back");
    }

    /// A base64 image payload is a very large, very uniform blob of text — the
    /// exact shape every text compactor is built to eat. Routing images away
    /// from that path is what keeps an image out of the TEXT compactors (it is
    /// resized on its own path, which the neighbouring test covers), and
    /// nothing else in the suite watches the *whole* response transform do it:
    /// the neighbouring tests call the image helper directly.
    #[test]
    fn an_image_result_is_never_compacted_as_text() {
        // The text half of this comparison must actually fold, and folding
        // reads process env — see `tests::env_guard`.
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _rw = crate::tests::env_guard();
        use base64::Engine as _;
        let png = |w: u32, h: u32| {
            let mut img = image::RgbImage::new(w, h);
            for (x, y, p) in img.enumerate_pixels_mut() {
                *p = image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]);
            }
            let mut bytes: Vec<u8> = Vec::new();
            img.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        let through = |b64: &str| -> String {
            let line = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {"content": [{
                    "type": "image", "mimeType": "image/png", "data": b64,
                }]}
            })
            .to_string();
            let out = super::transform_mcp_response_line(&line, "test-server", None, None);
            let v: serde_json::Value = serde_json::from_str(&out).expect("still JSON-RPC");
            v["result"]["content"][0]["data"]
                .as_str()
                .expect("still an image item")
                .to_string()
        };

        // Under the downscale cap: byte-identical, however big the base64 is.
        let small = png(320, 200);
        assert!(
            small.len() > 1_000,
            "a payload worth compacting: {}",
            small.len()
        );
        assert_eq!(
            through(&small),
            small,
            "an in-cap image comes back verbatim"
        );

        // Over the cap it shrinks — but into a smaller *image*, never into a
        // fold banner or a template line.
        let big = png(2000, 1400);
        let out = through(&big);
        assert!(out.len() < big.len(), "an over-cap image shrinks");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&out)
            .expect("still valid base64");
        assert!(
            image::load_from_memory(&bytes).is_ok(),
            "what comes back is still a decodable image"
        );
    }

    #[test]
    fn mcp_image_item_leaves_foreign_mime_alone() {
        let mut item = serde_json::json!({
            "type": "image", "mimeType": "image/tiff", "data": "AAAA",
        });
        let before = item.clone();
        super::downscale_mcp_image_item(&mut item, "test");
        assert_eq!(item, before);
    }

    /// JSON-RPC ids may be a number OR a string. The armed fallback is written
    /// from an ALREADY-stringified id, so a string id emitted bare would give
    /// the client `"id":abc` — unparseable, in the one place it cannot recover.
    #[test]
    fn the_fault_fallback_is_valid_json_for_both_id_shapes() {
        for (id, expect) in [
            ("42", serde_json::json!(42)),
            ("req-7", serde_json::json!("req-7")),
            ("a\"b", serde_json::json!("a\"b")),
        ] {
            let line = super::fault_fallback_response(id);
            let v: serde_json::Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|e| panic!("id {id:?} produced unparseable JSON: {e} in {line:?}"));
            assert_eq!(v["id"], expect, "id round-trip changed shape for {id:?}");
            assert_eq!(v["jsonrpc"], "2.0");
            assert!(v["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("re-run the call"));
            assert!(line.ends_with('\n'), "the client reads line-delimited JSON");
        }
    }

    /// The salvage must carry no content: this is the one path that skips the
    /// redaction every other branch goes through.
    #[test]
    fn the_fault_fallback_carries_nothing_from_the_server() {
        let line = super::fault_fallback_response("1");
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert!(
            v.get("result").is_none(),
            "a `result` field here would be server content that skipped redaction: {line}"
        );
        assert_eq!(
            v.as_object().map(|o| o.len()),
            Some(3),
            "unexpected extra field: {line}"
        );
        assert!(
            line.len() < 250,
            "the fallback should be a sentence, not a payload: {line}"
        );
    }
}
