//! **A field the agent already has, from this command's previous run.**
//!
//! Exec-dedup folds a re-run whose output is byte-identical. A poll is rarely
//! that: a CLI's `inbound --json` answers `{"count", "items", "principles"}`,
//! and between two calls only `count` and `items` move — `principles`, 4.4 KB
//! of standing guidance, is reprinted every time (8 calls in the ledger,
//! 2026-10-02; a figure of "336 calls" first written here was the whole
//! same CLI's `… --json` family, grouped under its first label — since
//! corrected). A top-level value EQUAL to the one the same command
//! delivered last time, in this session, is replaced by a pointer to that
//! delivery.

use serde_json::Value;

/// Values smaller than this stay: the pointer would cost about as much.
const MIN_FIELD_BYTES: usize = 300;

/// `cur` with every large top-level field equal to `prev`'s replaced by a
/// pointer string naming `prev_ref`, and the list of the fields replaced.
/// `None` when either side is not a JSON object or nothing large is unchanged.
pub fn fold_unchanged_fields(
    prev: &str,
    cur: &str,
    prev_ref: &str,
) -> Option<(String, Vec<String>)> {
    let (Ok(Value::Object(p)), Ok(Value::Object(mut c))) = (
        serde_json::from_str::<Value>(prev),
        serde_json::from_str::<Value>(cur),
    ) else {
        return None;
    };
    let mut folded = Vec::new();
    for (k, v) in c.iter_mut() {
        if p.get(k) == Some(v) && serde_json::to_string(v).map_or(0, |s| s.len()) >= MIN_FIELD_BYTES
        {
            *v = Value::String(format!(
                "<unchanged since this command's previous output — tokenade expand-ref {prev_ref}>"
            ));
            folded.push(k.clone());
        }
    }
    if folded.is_empty() {
        return None;
    }
    let out = serde_json::to_string(&Value::Object(c)).ok()?;
    Some((out, folded))
}

#[cfg(test)]
mod tests {
    use super::fold_unchanged_fields;

    fn inbound(count: u32) -> String {
        let principles: Vec<String> = (0..12)
            .map(|i| format!("principe {i} : un énoncé assez long pour peser"))
            .collect();
        serde_json::json!({ "count": count, "items": (0..count).collect::<Vec<_>>(), "principles": { "how": "relire avec eux", "list": principles } }).to_string()
    }

    #[test]
    fn the_standing_guidance_is_a_pointer_the_moving_parts_stay() {
        let (out, folded) = fold_unchanged_fields(&inbound(0), &inbound(2), "abc123def456")
            .expect("principles unchanged");
        assert_eq!(folded, ["principles"]);
        assert!(
            out.contains("\"count\":2") && out.contains("\"items\":[0,1]"),
            "{out}"
        );
        assert!(out.contains("expand-ref abc123def456") && !out.contains("principe 7"));
    }

    #[test]
    fn a_changed_or_small_field_is_left_alone() {
        let mut changed: serde_json::Value = serde_json::from_str(&inbound(0)).unwrap();
        changed["principles"]["how"] = "autrement".into();
        assert!(fold_unchanged_fields(&inbound(0), &changed.to_string(), "h").is_none());
        assert!(fold_unchanged_fields(r#"{"a":1,"b":"x"}"#, r#"{"a":2,"b":"x"}"#, "h").is_none());
        assert!(fold_unchanged_fields("[1,2]", "[1,2]", "h").is_none());
    }
}
