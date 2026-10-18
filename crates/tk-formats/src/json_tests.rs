// The tests of `crate::json`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `json`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_noise_keys() {
        // Wrap the single object in a parent so it's not array-folded.
        let raw = r#"{"name":"x","etag":"abc","_links":{"self":"/x"},"node_id":"n"}"#;
        let out = compact(raw);
        assert!(out.contains("\"name\":\"x\""));
        assert!(!out.contains("etag"));
        assert!(!out.contains("_links"));
        assert!(!out.contains("node_id"));
    }

    #[test]
    fn schema_fold_homogeneous_array() {
        let raw = r#"[
            {"id":1,"user":"alice","score":95},
            {"id":2,"user":"bob","score":82},
            {"id":3,"user":"carol","score":77},
            {"id":4,"user":"dave","score":91},
            {"id":5,"user":"eve","score":68}
        ]"#;
        let out = schema_fold(raw).expect("should fold");
        // A clean scalar table folds LOSSLESSLY to TOON: keys once in the
        // header (columns sorted, like serde_json's object order), every row
        // kept (no elision).
        assert!(out.contains("[×5 rows · toon]"), "got: {out}");
        assert!(out.contains("id,score,user"), "got: {out}");
        // Every value survives — all five users present, nothing elided.
        for u in ["alice", "bob", "carol", "dave", "eve"] {
            assert!(out.contains(u), "row {u} dropped: {out}");
        }
        assert!(!out.contains("elided"), "TOON is lossless: {out}");
        // Lossless: header + exactly five data rows.
        assert_eq!(out.lines().count(), 6, "header + 5 rows: {out}");
    }

    #[test]
    fn toon_quotes_strings_leaves_scalars_bare() {
        // Strings JSON-quoted; numbers/bools/null bare. A string that looks
        // like a number must stay quoted so it round-trips. (`note` is null
        // in every row so the column is uniform — a column that mixed null
        // and string would split the structural signature and never fold.)
        let raw = r#"[
            {"id":1,"name":"a,b","ok":true,"note":null,"code":"007"},
            {"id":2,"name":"c","ok":false,"note":null,"code":"42"},
            {"id":3,"name":"d","ok":true,"note":null,"code":"99"},
            {"id":4,"name":"e","ok":false,"note":null,"code":"01"},
            {"id":5,"name":"f","ok":true,"note":null,"code":"55"}
        ]"#;
        let out = schema_fold(raw).expect("scalar table should TOON-fold");
        assert!(out.contains("[×5 rows · toon]"), "got: {out}");
        // bare scalars
        assert!(out.contains(",1,"), "id bare: {out}");
        assert!(out.contains("true"), "bool bare: {out}");
        assert!(out.contains("null"), "null bare: {out}");
        // string with a comma stays quoted (delimiter unambiguous)
        assert!(out.contains("\"a,b\""), "comma string quoted: {out}");
        // numeric-looking string stays quoted (won't read as bare 7)
        assert!(out.contains("\"007\""), "numeric string quoted: {out}");
    }

    #[test]
    fn nested_values_fall_back_to_lossy_summary() {
        // An array of objects with a NESTED array value can't be a flat TOON
        // table, so it must fall through to the `[×N items fit schema]` block.
        // All `tags` are non-empty so every row shares one signature (an empty
        // `[]` would read as a different shape and skew the dominant count).
        let raw = r#"[
            {"id":1,"tags":["a","b"]},
            {"id":2,"tags":["c"]},
            {"id":3,"tags":["z"]},
            {"id":4,"tags":["d","e"]},
            {"id":5,"tags":["f"]}
        ]"#;
        let out = schema_fold(raw).expect("should still fold via summary");
        assert!(out.contains("[×5 items fit schema]"), "got: {out}");
        assert!(
            !out.contains("· toon"),
            "nested values must not TOON: {out}"
        );
    }

    #[test]
    fn schema_fold_skips_small_arrays() {
        let raw = r#"[{"x":1},{"x":2}]"#;
        assert!(schema_fold(raw).is_none(), "small arrays should not fold");
    }

    #[test]
    fn schema_fold_skips_heterogeneous_arrays() {
        let raw = r#"[
            {"x":1},
            {"x":2},
            "a string",
            42,
            null,
            {"x":3}
        ]"#;
        // 6 items but only 3 match the object shape — pct = 50% < 80%.
        assert!(schema_fold(raw).is_none());
    }

    #[test]
    fn schema_fold_tolerates_few_anomalies() {
        // 5 matching + 1 anomalous = 5/6 ≈ 83% > 80%. Should fold.
        let raw = r#"[
            {"id":1,"u":"a"},
            {"id":2,"u":"b"},
            {"id":3,"u":"c"},
            {"id":4,"u":"d"},
            {"id":5,"u":"e"},
            {"_meta":"envelope"}
        ]"#;
        let out = schema_fold(raw).expect("should fold with 5/6 majority");
        assert!(out.contains("[×5 items fit schema]"));
        // The off-schema item is reported AND shown: naming its count was what
        // this used to check, and a count alone sends the reader away for the
        // one record that differs.
        assert!(
            out.contains("+1 item(s) with a DIFFERENT shape"),
            "got: {out}"
        );
        assert!(
            out.contains("_meta"),
            "the odd item was counted, not shown: {out}"
        );
    }

    #[test]
    fn schema_fold_compact_integration() {
        // The public `compact` should pick schema_fold when applicable.
        let raw = r#"[
            {"id":1,"v":1},{"id":2,"v":2},{"id":3,"v":3},
            {"id":4,"v":4},{"id":5,"v":5},{"id":6,"v":6}
        ]"#;
        let out = compact(raw);
        assert!(
            out.contains("[×6 rows · toon]"),
            "compact should route through fold: {out}"
        );
    }

    #[test]
    fn drops_url_keys_except_keepers() {
        let raw = r#"{"html_url":"keep","url":"keep","avatar_url":"drop","events_url":"drop","name":"n"}"#;
        let out = compact(raw);
        assert!(out.contains("html_url"));
        assert!(out.contains("\"url\""));
        assert!(!out.contains("avatar_url"));
        assert!(!out.contains("events_url"));
        assert!(out.contains("\"name\":\"n\""));
    }

    #[test]
    fn caps_arrays() {
        let raw: String = format!(
            "{{\"items\":[{}]}}",
            (0..100)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let out = compact(&raw);
        assert!(out.contains("more items"));
        // First 20 preserved
        assert!(out.contains("19"));
        assert!(!out.contains(",50,"));
    }

    #[test]
    fn truncates_long_strings() {
        let long: String = "x".repeat(1000);
        let raw = format!("{{\"big\":\"{long}\"}}");
        let out = compact(&raw);
        assert!(out.contains("[+800 chars]"));
    }

    #[test]
    fn survives_malformed() {
        let raw = "{not: valid json,";
        let out = compact(raw);
        assert_eq!(out, raw);
    }

    #[test]
    fn extremely_deep_does_not_panic() {
        // 10_000 levels — serde_json refuses to parse this (its own recursion
        // limit kicks in first); compact() must return the original safely,
        // never panic, never stack-overflow.
        let mut raw = String::new();
        for _ in 0..10_000 {
            raw.push_str("{\"x\":");
        }
        raw.push('1');
        for _ in 0..10_000 {
            raw.push('}');
        }
        // The point: this line completes without panicking.
        let out = compact(&raw);
        assert!(!out.is_empty());
    }

    #[test]
    fn moderately_deep_hits_depth_cap() {
        // 300 levels — within serde_json's parse limit (~128 for default
        // deserializer is too low actually; testing at a safer 60 levels).
        let mut raw = String::new();
        for _ in 0..60 {
            raw.push_str("{\"x\":");
        }
        raw.push('1');
        for _ in 0..60 {
            raw.push('}');
        }
        // With MAX_DEPTH = 256, this parses and walks fully (no marker).
        let out = compact(&raw);
        assert!(!out.is_empty());
        // Compose a value just above MAX_DEPTH using a CompactOptions override.
        let opts = CompactOptions {
            max_depth: 5,
            ..Default::default()
        };
        let capped = compact_with(&raw, &opts);
        assert!(capped.contains("deep"), "got: {}", capped);
    }

    #[test]
    fn savings_real() {
        // Simulate a GitHub-style PR object with lots of noise.
        let pr = r#"{
            "number": 1, "title": "Fix something", "state": "open",
            "html_url":"https://github.com/x/y/pull/1",
            "comments_url":"...","events_url":"...","issue_url":"...","commits_url":"...",
            "review_comments_url":"...","review_comment_url":"...","statuses_url":"...",
            "_links":{"self":{},"html":{},"comments":{}},
            "etag":"abc","node_id":"PRABC=="
        }"#;
        let (out, sav) = compact_with_savings(pr);
        assert!(out.contains("\"number\":1"));
        assert!(out.contains("Fix something"));
        assert!(sav.percent() > 30.0, "got {:?}", sav);
    }

    // ---- Adversarial: schema-aware (kubectl + GitHub) --------------------

    #[test]
    fn kubectl_pod_drops_managed_fields_and_keeps_signal() {
        // Skinny realistic `kubectl get pod -o json`.
        let pod = r#"{
            "apiVersion":"v1","kind":"Pod",
            "metadata":{
                "name":"redis-0","namespace":"prod",
                "uid":"a1b2c3","resourceVersion":"999","generation":7,
                "creationTimestamp":"2026-01-01T00:00:00Z",
                "labels":{"app":"redis","tier":"cache"},
                "annotations":{
                    "kubectl.kubernetes.io/last-applied-configuration":"{\"some\":\"huge json blob duplicating the entire manifest body that no agent ever needs to read\"}",
                    "deployment.kubernetes.io/revision":"42",
                    "prometheus.io/scrape":"true"
                },
                "managedFields":[
                    {"manager":"kubectl","operation":"Update","apiVersion":"v1","time":"2026-01-01T00:00:00Z","fieldsType":"FieldsV1","fieldsV1":{}},
                    {"manager":"kubelet","operation":"Update","apiVersion":"v1","time":"2026-01-01T00:00:01Z","fieldsType":"FieldsV1","fieldsV1":{}},
                    {"manager":"kube-controller","operation":"Update","apiVersion":"v1","time":"2026-01-01T00:00:02Z","fieldsType":"FieldsV1","fieldsV1":{}}
                ],
                "ownerReferences":[{"apiVersion":"apps/v1","kind":"StatefulSet","name":"redis"}]
            },
            "spec":{"containers":[{"name":"redis","image":"redis:7.2"}]},
            "status":{"phase":"Running"}
        }"#;
        let (out, sav) = compact_with_savings(pod);
        // Signal preserved.
        assert!(out.contains("\"name\":\"redis-0\""), "name dropped: {out}");
        assert!(out.contains("\"namespace\":\"prod\""));
        assert!(out.contains("\"phase\":\"Running\""));
        assert!(out.contains("\"image\":\"redis:7.2\""));
        // Noise dropped.
        assert!(!out.contains("managedFields"), "managedFields not dropped");
        assert!(!out.contains("resourceVersion"));
        assert!(
            !out.contains("last-applied-configuration"),
            "kubectl applied-config blob not dropped"
        );
        assert!(!out.contains("ownerReferences"));
        // Useful annotation preserved.
        assert!(out.contains("prometheus.io/scrape"));
        assert!(
            sav.percent() > 40.0,
            "expected >40% savings on noisy pod, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn github_pr_drops_head_base_repo_blobs() {
        // GH PR API returns `head.repo` and `base.repo` as full Repository
        // objects (each ~3 KB). On a 200-PR list that's 1.2 MB of duplication.
        let pr = r#"{
            "number":42,"title":"Add feature","state":"open",
            "node_id":"PR_abc","html_url":"https://github.com/x/y/pull/42",
            "head":{
                "ref":"feature","sha":"abc123",
                "repo":{"id":1,"node_id":"R_x","name":"y","full_name":"x/y","owner":{"login":"x"},"description":"yet another tool","fork":false,"url":"...","clone_url":"...","ssh_url":"...","html_url":"...","language":"Rust","stargazers_count":99}
            },
            "base":{
                "ref":"main","sha":"def456",
                "repo":{"id":1,"node_id":"R_x","name":"y","full_name":"x/y","owner":{"login":"x"}}
            },
            "user":{"login":"alice","id":1234,"node_id":"U_alice","type":"User"}
        }"#;
        let (out, sav) = compact_with_savings(pr);
        assert!(out.contains("\"number\":42"));
        assert!(out.contains("Add feature"));
        assert!(out.contains("\"login\":\"alice\""));
        assert!(!out.contains("\"head\""), "head.repo not dropped");
        assert!(!out.contains("\"base\""), "base.repo not dropped");
        assert!(!out.contains("stargazers_count"));
        assert!(
            sav.percent() > 50.0,
            "expected >50% savings, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn schema_aware_never_panics_on_truncated_kubectl_json() {
        // Malformed kubectl output (process killed mid-stream): returns
        // input verbatim, never panics.
        let bad = r#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"x","managedFields":["#;
        let out = compact(bad);
        // Either the original or a no-op string; never a panic.
        assert!(!out.is_empty());
    }

    #[test]
    fn schema_lookalike_object_is_not_misclassified() {
        // A plain JSON that happens to have `apiVersion`/`kind` keys but not
        // a `metadata` object — must not trigger k8s-mode drops.
        let v = r#"{"apiVersion":"1.0","kind":"feature-toggle","value":true,"metadata":"just a string"}"#;
        let out = compact(v);
        assert!(out.contains("apiVersion"));
        assert!(out.contains("kind"));
        // "metadata" is a string here, NOT an object — schema detection
        // requires metadata to be present (which it is). But our walker
        // treats the metadata path generically since detect_shape only
        // requires presence, so this is fine; we just ensure no panic.
        assert!(out.contains("value"));
    }

    // ---- Cookie-export `value` redaction (gated by cookie signature) -----

    #[test]
    fn redacts_cookie_export_value_field() {
        // EditThisCookie / Cookie-Editor shape: value + cookie attributes.
        let raw = r#"[{"name":"sid","value":"SUPERSECRETVALUE","domain":".x.com","path":"/","httpOnly":true,"secure":true}]"#;
        let out = compact(raw);
        assert!(
            !out.contains("SUPERSECRETVALUE"),
            "cookie value leaked: {out}"
        );
        assert!(out.contains("<redacted"), "no redaction marker: {out}");
        // Non-value fields preserved.
        assert!(out.contains("sid"), "name lost: {out}");
        assert!(out.contains(".x.com"), "domain lost: {out}");
    }

    #[test]
    fn bare_name_value_object_not_redacted() {
        // FALSE-POSITIVE GUARD: `{"name","value"}` with NO cookie attribute is
        // a generic key/value pair and must keep its value.
        let raw = r#"[{"name":"greeting","value":"hello world"}]"#;
        let out = compact(raw);
        assert!(
            out.contains("hello world"),
            "generic value wrongly redacted: {out}"
        );
        assert!(!out.contains("<redacted"), "no redaction expected: {out}");
    }

    #[test]
    fn cookie_array_not_leaked_via_toon_fold() {
        // A ≥5-item cookie array would otherwise TOON-fold and dump every
        // value losslessly. The pre-pass must redact BEFORE folding.
        let raw = r#"[
            {"name":"a","value":"SECRETA","domain":".x.com","httpOnly":true},
            {"name":"b","value":"SECRETB","domain":".x.com","httpOnly":true},
            {"name":"c","value":"SECRETC","domain":".x.com","httpOnly":false},
            {"name":"d","value":"SECRETD","domain":".x.com","httpOnly":true},
            {"name":"e","value":"SECRETE","domain":".x.com","httpOnly":false}
        ]"#;
        let out = compact(raw);
        for s in ["SECRETA", "SECRETB", "SECRETC", "SECRETD", "SECRETE"] {
            assert!(!out.contains(s), "cookie value {s} leaked via fold: {out}");
        }
        assert!(out.contains("<redacted"), "no redaction marker: {out}");
        // Cookie names still visible.
        assert!(out.contains('a') && out.contains('e'), "names lost: {out}");
    }

    #[test]
    fn cookie_value_redaction_idempotent() {
        let raw = r#"[{"name":"sid","value":"SECRET","domain":".x.com","httpOnly":true}]"#;
        let once = compact(raw);
        // The compacted output isn't re-fed as JSON, but the structural pass
        // itself must be a no-op on an already-redacted value.
        let mut v: Value = serde_json::from_str(raw).unwrap();
        assert!(crate::redact::redact_cookie_values(&mut v), "first pass should redact");
        assert!(!crate::redact::redact_cookie_values(&mut v), "second pass must be a no-op");
        let _ = once;
    }

    #[test]
    fn generic_unschematic_json_unchanged_by_shape_rules() {
        // Plain user-defined JSON: must not be touched by k8s or GH drops.
        let v = r#"{"head":{"a":1},"base":{"b":2},"managedFields":[1,2,3]}"#;
        let out = compact(v);
        assert!(out.contains("head"));
        assert!(out.contains("base"));
        assert!(out.contains("managedFields"));
    }
}
#[cfg(test)]
mod anomaly_tests {
    /// The item that does not fit is the one the reader came for.
    #[test]
    fn an_off_schema_item_is_shown_not_merely_counted() {
        let mut items: Vec<String> = (0..300)
            .map(|i| format!(r#"{{"job":"job-{i}","state":"succeeded"}}"#))
            .collect();
        items[150] =
            r#"{"job":"job-150","state":"failed","message":"disk full on shard 7"}"#.to_string();
        let doc = format!(r#"{{"results":[{}]}}"#, items.join(","));
        let out = crate::json::compact(&doc);
        assert!(
            out.contains("disk full on shard 7"),
            "the one failing record was folded away: {out}"
        );
        // …and the uniformity claim must not contradict it.
        assert!(
            !out.contains("distinct: ") || !out.contains("state=1"),
            "the summary claims a single state while one record differs: {out}"
        );
    }

    /// A heterogeneous array must not reprint itself under the fold.
    #[test]
    fn showing_anomalies_stays_bounded() {
        let items: Vec<String> = (0..300)
            .map(|i| {
                if i % 2 == 0 {
                    format!(r#"{{"job":"job-{i}","state":"ok"}}"#)
                } else {
                    format!(
                        r#"{{"job":"job-{i}","other":{i},"extra":"{}"}}"#,
                        "x".repeat(50)
                    )
                }
            })
            .collect();
        let doc = format!(r#"{{"results":[{}]}}"#, items.join(","));
        let out = crate::json::compact(&doc);
        assert!(
            out.len() < doc.len() / 3,
            "the fold gave back {} of {} bytes",
            out.len(),
            doc.len()
        );
    }
}
#[cfg(test)]
mod the_fold_wrapper_is_not_a_pretty_print {
    use super::{compact_with, schema_fold, CompactOptions};
    use crate::tokenizer::estimate;

    /// The fold replaces ONE array with a pointer and re-emits everything
    /// around it. On a large document that surround is the bulk of the file, so
    /// a pretty-printed wrapper kept every space of the original AND paid for
    /// the fold block on top — the fold then cost more than doing nothing and
    /// `auto`'s anti-inflation guard threw it away, delivering the file whole.
    #[test]
    fn the_wrapper_carries_no_indentation() {
        let raw = format!(
            "{{\n  \"name\": \"svc\",\n  \"items\": [{}]\n}}",
            (0..30)
                .map(|i| format!("{{\"id\":{i},\"k\":\"v\"}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let folded = schema_fold(&raw).expect("a payload array folds");
        let wrapper = folded.split("\n\n[fold @").next().unwrap_or_default();
        assert!(
            !wrapper.contains("\n  "),
            "the wrapper must be minified: {wrapper}"
        );
    }

    /// And the fold must now be worth choosing on a pretty input, which is the
    /// whole point: it was losing to the raw file.
    #[test]
    fn the_fold_beats_the_input_it_replaces() {
        let mut raw = String::from("{\n  \"resources\": [\n");
        for i in 0..60 {
            raw.push_str(&format!(
                "    {{\n      \"id\": \"r-{i}\",\n      \"kind\": \"thing\",\n      \"note\": \"a description long enough to matter\"\n    }},\n"
            ));
        }
        raw.truncate(raw.len() - 2);
        raw.push_str("\n  ]\n}\n");
        let folded = schema_fold(&raw).expect("fold");
        assert!(
            estimate(&folded) < estimate(&raw),
            "folded {} vs input {}",
            estimate(&folded),
            estimate(&raw)
        );
    }

    /// #293 — a key that is chrome in one document and the payload in another.
    /// The object arm drops a key whose walked value is an empty container, so
    /// dropping the ONLY key drops the document.
    #[test]
    fn a_generic_key_name_no_longer_empties_the_document() {
        for raw in [
            r#"{"users":[{"id":1},{"id":2}]}"#,
            r#"{"definitions":{"Pet":{"type":"object"}}}"#,
        ] {
            let out = compact_with(raw, &CompactOptions::default());
            assert_ne!(out, "{}", "{raw} was emptied");
        }
    }

    /// A schema reference IS the payload; without it the object says nothing.
    #[test]
    fn a_schema_reference_survives() {
        let raw = r##"{"$ref":"#/components/schemas/Pet","description":"a pet"}"##;
        assert!(compact_with(raw, &CompactOptions::default()).contains("$ref"));
    }

    /// …and npm registry chrome is still dropped, but only inside an npm
    /// packument, which is what makes `users` chrome in the first place.
    #[test]
    fn npm_chrome_is_still_dropped_where_it_is_chrome() {
        let packument = r#"{"dist-tags":{"latest":"1.0.0"},"versions":{"1.0.0":{"name":"p"}},"users":{"bob":true}}"#;
        let out = compact_with(packument, &CompactOptions::default());
        assert!(!out.contains("\"users\""), "{out}");
        assert!(out.contains("versions"), "{out}");
    }
}
#[cfg(test)]
mod a_counter_and_a_fact_are_not_the_same_key {
    use super::{compact_with, CompactOptions};

    fn out(raw: &str) -> String {
        compact_with(raw, &CompactOptions::default())
    }

    /// `version` was the most-removed key of the whole list — 5 851 of 44 008
    /// real files, 1 507 201 bytes — and it is exactly what a reader looks up.
    #[test]
    fn a_version_survives_where_a_reader_would_look_for_it() {
        assert!(out(r#"{"name":"tokenade","version":"1.1.6"}"#).contains("1.1.6"));
        assert!(
            out(r#"{"openapi":"3.0.0","info":{"title":"A","version":"2.4.1"}}"#).contains("2.4.1")
        );
        assert!(out(r#"{"tool":"cargo","version":"1.90.0"}"#).contains("1.90.0"));
    }

    /// The internal counters the comment was written for are still dropped.
    #[test]
    fn an_internal_counter_is_still_dropped() {
        for (raw, gone) in [
            (r#"{"a":1,"__v":3}"#, "__v"),
            (r#"{"a":1,"_version":3}"#, "_version"),
        ] {
            assert!(!out(raw).contains(gone), "{raw}");
        }
    }

    /// #296 — `resourceVersion` and `generation` are Kubernetes metadata, so
    /// they are dropped INSIDE a Kubernetes object and nowhere else. They used
    /// to sit in the generic list as well, which made the k8s entry dead weight
    /// and the generic one over-reach: a `generation` in a domain model is not
    /// a Kubernetes counter.
    #[test]
    fn a_kubernetes_counter_is_dropped_only_in_a_kubernetes_object() {
        let k8s = r#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"p","resourceVersion":"999","generation":7}}"#;
        let o = out(k8s);
        assert!(!o.contains("resourceVersion"), "{o}");
        assert!(!o.contains("generation"), "{o}");
        assert!(
            o.contains("\"name\":\"p\""),
            "the object itself must survive: {o}"
        );

        let plain = r#"{"widget":"gauge","generation":4,"resourceVersion":"7"}"#;
        let p = out(plain);
        assert!(p.contains("generation"), "not a Kubernetes object: {p}");
        assert!(
            p.contains("resourceVersion"),
            "not a Kubernetes object: {p}"
        );
    }

    /// A telemetry key that HOLDS an id is chrome…
    #[test]
    fn a_telemetry_id_is_still_dropped() {
        for raw in [
            r#"{"requestId":"abc-123","status":"ok"}"#,
            r#"{"traceId":"deadbeef","ok":true}"#,
            r#"{"span_id":42,"ok":true}"#,
        ] {
            let o = out(raw);
            assert!(!o.contains("requestId"), "{o}");
            assert!(!o.contains("traceId"), "{o}");
            assert!(!o.contains("span_id"), "{o}");
        }
    }

    /// …and one that DESCRIBES an id is a documented parameter. Measured:
    /// `requestId` carried 1 009 771 bytes over 324 files, a median of 405 per
    /// occurrence, because in an API schema it is a property definition.
    #[test]
    fn a_telemetry_key_that_describes_a_parameter_survives() {
        let raw = r#"{"parameters":{"requestId":{"description":"An optional request ID.","type":"string"}}}"#;
        let o = out(raw);
        assert!(o.contains("requestId"), "{o}");
        assert!(o.contains("optional request ID"), "{o}");
    }
}
#[cfg(test)]
mod one_home_per_key {
    /// #296 — a key name may live in the GENERIC list or in a SHAPE list, never
    /// in both. Two homes means the shape entry is dead weight while the generic
    /// entry over-reaches, and a later edit fixes one copy while the other keeps
    /// the old behaviour. Three names were in that position: `generation` and
    /// `resourceVersion` (also in the Kubernetes list) and `_links` (also in the
    /// GitHub one).
    ///
    /// Read off the source rather than from a hand-kept mirror, so the guard
    /// cannot itself drift from the lists it checks.
    /// The payload walk must hop through a wrapper ARRAY, not only objects.
    ///
    /// `aws ec2 describe-instances` — the most common AWS response shape there
    /// is — returns `Reservations[0].Instances`, and the walk descended through
    /// objects only. Measured 2026-08-28 on 121 instances, one terminated with
    /// a `StateTransitionReason`: the object shapes folded and surfaced the odd
    /// item; the array-wrapped one fell back to "first 3 items + …118 more
    /// items…" and the terminated instance — the only one a reader is looking
    /// for — was in the 118.
    #[test]
    fn a_payload_behind_a_wrapper_array_still_folds() {
        let items: Vec<String> = (0..120)
            .map(|i| format!("{{\"InstanceId\": \"i-{i:05}\", \"State\": \"running\"}}"))
            .collect();
        let odd = "{\"InstanceId\": \"i-odd\", \"State\": \"terminated\", \"Why\": \"NEEDLE\"}";
        let list = format!("[{}, {odd}]", items.join(", "));

        // Three spellings of the same payload. The first two already worked.
        let flat = format!("{{\"Instances\": {list}}}");
        let nested_obj = format!("{{\"Reservations\": {{\"Instances\": {list}}}}}");
        let nested_arr = format!("{{\"Reservations\": [{{\"Instances\": {list}}}]}}");
        for (name, src) in [
            ("flat", &flat),
            ("object wrapper", &nested_obj),
            ("array wrapper", &nested_arr),
        ] {
            let out = super::schema_fold(src)
                .unwrap_or_else(|| panic!("{name}: the payload must be folded"));
            assert!(
                out.contains("items fit schema"),
                "{name}: no schema fold:\n{out}"
            );
            assert!(
                out.contains("NEEDLE"),
                "{name}: the odd item must be surfaced, not counted away:\n{out}"
            );
        }
        // …and the path is written the way a reader indexes.
        let out = super::schema_fold(&nested_arr).unwrap();
        assert!(
            out.contains("[fold @ Reservations[0].Instances]"),
            "the path must name the index it walked through:\n{out}"
        );
    }

    /// Above the cap, the distinct count must say it stopped counting.
    ///
    /// The set stopped at [`DISTINCT_CAP`] and the line printed `id=200`
    /// whatever the truth. Measured 2026-08-28 on arrays of 100 / 200 / 300 /
    /// 400 uniform items, the reported count read 100, 200, **200, 200** — so
    /// above the cap the number was not an approximation but a false fact about
    /// the data, in the one line the fold exists to state it: "400 items, 200
    /// distinct ids" reads as "each id appears twice".
    ///
    /// The boundary is asserted too. A dataset with EXACTLY 200 distinct values
    /// is not saturated and must print the exact figure, which is why the set
    /// is allowed one element past the cap.
    #[test]
    fn a_distinct_count_over_the_cap_says_so() {
        let arr = |n: usize| -> String {
            let items: Vec<String> = (0..n)
                .map(|i| format!("{{\"id\": {i}, \"ok\": true}}"))
                .collect();
            // One off-schema item, so the fold takes the schema branch.
            format!(
                "[{}, {{\"id\": 999, \"ok\": false, \"why\": \"odd\"}}]",
                items.join(", ")
            )
        };
        let distinct_line = |n: usize| -> String {
            super::compact(&arr(n))
                .lines()
                .find(|l| l.contains("distinct"))
                .unwrap_or_default()
                .to_string()
        };
        assert!(
            distinct_line(100).contains("id=100"),
            "{}",
            distinct_line(100)
        );
        assert!(
            distinct_line(super::DISTINCT_CAP).contains(&format!("id={}", super::DISTINCT_CAP))
                && !distinct_line(super::DISTINCT_CAP).contains("+"),
            "exactly at the cap is EXACT, not saturated: {}",
            distinct_line(super::DISTINCT_CAP)
        );
        let over = distinct_line(super::DISTINCT_CAP * 2);
        assert!(
            over.contains(&format!("id={}+", super::DISTINCT_CAP)),
            "over the cap the count must say it stopped: {over}"
        );
    }

    #[test]
    fn no_key_lives_in_two_lists() {
        // The FAMILY: the array fold and the descent moved down into
        // `json/`, and the trunk only carries the noise tables.
        let src = tk_base::source_guard::library_module_family("json");
        let src = src.as_str();
        let names = |name: &str| -> Vec<String> {
            let i = src.find(&format!("const {name}")).expect(name);
            let j = src[i..].find("];").expect("list end") + i;
            src[i..j]
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .filter_map(|l| l.strip_prefix('"').and_then(|r| r.split('"').next()))
                .map(str::to_string)
                .collect()
        };
        let generic = names("DROP_KEYS");
        assert!(generic.len() > 20, "the generic list must have been parsed");
        for shape in [
            "K8S_METADATA_DROPS",
            "GH_PR_NESTED_DROPS",
            "NPM_REGISTRY_DROPS",
        ] {
            for k in names(shape) {
                assert!(
                    !generic.contains(&k),
                    "`{k}` is in DROP_KEYS and in {shape}: give it one home"
                );
            }
        }
    }
}

#[cfg(test)]
mod a_json_that_is_a_document {
    /// `{"context": "<markdown>"}` — a file whose whole object is one text.
    #[test]
    fn the_document_is_not_cut_but_a_blob_is() {
        let doc: String = (0..120)
            .map(|i| format!("- [l-{i:03}] (2026-09-28, source : learn 30 j) constat numéro {i} et ce qu'on change\n"))
            .collect();
        let wrapped = serde_json::json!({ "context": doc }).to_string();
        assert_eq!(super::compact(&wrapped), wrapped, "a wrapped document came back cut");

        let blob = "QUJD".repeat(1_000);
        let record = serde_json::json!({ "id": 7, "data": blob }).to_string();
        assert!(super::compact(&record).contains("chars]"), "a dominant base64 blob must still be cut");
    }

    /// A CLI's `show --json`: the payload sits in THREE
    /// text leaves, each cut at 200 characters, and the agent's next call
    /// was `expand-ref`.
    #[test]
    fn several_text_leaves_carrying_the_record_are_not_cut() {
        let w = "gouvernance de la data et des actions, journalisation ";
        let rec = serde_json::json!({
            "id": 104, "lang": "en", "status": "writing",
            "instructions": w.repeat(24),
            "source": { "author": "strategist", "owner_instructions": w.repeat(23), "text": w.repeat(8) },
            "then": "acme-cli repurpose done 104 --draft <id>",
            // The rest of the real record: ~⅓ of the bytes, hence a text at ~⅔.
            "runs": (3000..3300).collect::<Vec<u32>>(),
        })
        .to_string();
        assert_eq!(super::compact(&rec), rec, "the record's text leaves came back cut");

        // The same leaves INSIDE a list: the cut makes the overview, it stays.
        let list = serde_json::json!({ "items": [ { "body": w.repeat(24) }, { "body": w.repeat(23) }, { "body": w.repeat(22) } ] }).to_string();
        assert!(super::compact(&list).contains("chars]"), "a list of records keeps its overview cuts");

        // A `plan --json`: ONE target in `targets`, that is
        // not a list to skim.
        let one = serde_json::json!({
            "guidance": w.repeat(24),
            "targets": [ { "article": w.repeat(60), "url": "https://x.org/a", "target": 9 } ],
        })
        .to_string();
        assert_eq!(super::compact(&one), one, "a one-target plan came back cut");
    }
}

#[cfg(test)]
mod small_cuts_and_small_arrays {
    use super::*;

    /// A cut that saves less than a quarter of the cap is not made: the
    /// 201-character command came back as 200 and `…[+1 chars]`.
    #[test]
    fn a_string_just_over_the_cap_comes_back_whole() {
        let cmd = format!("acme-cli draft add --platform linkedin {}", "--flag value ".repeat(13));
        assert!(cmd.chars().count() > DEFAULT_STR_CAP && cmd.chars().count() < DEFAULT_STR_CAP * 5 / 4);
        assert_eq!(super::walk::truncate_string(&cmd, DEFAULT_STR_CAP), cmd);
        let long = "x".repeat(DEFAULT_STR_CAP * 2);
        assert!(super::walk::truncate_string(&long, DEFAULT_STR_CAP).ends_with(&format!("…[+{DEFAULT_STR_CAP} chars]")));
    }

    /// A log held in one JSON field keeps its LAST lines, where the result is.
    #[test]
    fn a_multi_line_string_keeps_its_end() {
        let log: String = (1..=60).map(|i| format!("step {i:02} ok\n")).collect::<String>() + "RESULT: 3 failed, 57 passed";
        let out = super::walk::truncate_string(&log, DEFAULT_STR_CAP);
        assert!(out.starts_with("step 01 ok"), "{out}");
        assert!(out.ends_with("RESULT: 3 failed, 57 passed"), "{out}");
        assert!(out.contains("line(s)"), "{out}");
    }

    /// Twelve records carrying a distinct sentence each, 4 KB together: all
    /// twelve are written, none "elided · use jq".
    #[test]
    fn a_dozen_distinct_records_are_all_written() {
        let topics = ["billing", "proof", "latency", "trust", "audit", "refunds", "onboarding", "limits", "privacy", "pricing", "support", "exports"];
        let items: Vec<Value> = topics
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({
                "id": i + 1,
                "facet": if i % 3 == 0 { "objection" } else { "belief" },
                "statement": format!("A claim about {t} holds only if the reader can check it without the vendor (#{i})"),
                "evidence": [{"kind": "comment", "quote": format!("seen in thread {} about {t}", 40 + i * 7)}],
            }))
            .collect();
        let block = super::fold::fold_array_to_block(&items, Some("principles")).expect("folds");
        for t in topics {
            assert!(block.contains(&format!("about {t} holds")), "{t} missing:\n{block}");
        }
        assert!(!block.contains("elided"), "{block}");
        let many: Vec<Value> = (0..400).map(|i| serde_json::json!({"id": i, "state": "ok", "steps": [{"name": format!("job-{i}")}]})).collect();
        assert!(super::fold::fold_array_to_block(&many, None).expect("folds").contains("397 more items elided"));
    }
}

#[cfg(test)]
mod a_constant_column_is_said_once {
    /// 378 real `out/*.json` payloads: 4,649 columns with the same value
    /// on every row (`"seniority":""`), 2.3 % of the bytes.
    #[test]
    fn the_header_carries_it_and_the_rows_do_not() {
        let leads: Vec<serde_json::Value> = (0..12)
            .map(|i| serde_json::json!({ "name": format!("lead {i}"), "seniority": "", "country": "US" }))
            .collect();
        let doc = serde_json::json!({ "leads": leads, "shown": 12 }).to_string();
        let out = super::compact(&doc);
        assert!(out.contains("same in every row: \"country\"=\"US\", \"seniority\"=\"\"") || out.contains("same in every row: country=\"US\", seniority=\"\""), "{out}");
        assert!(out.contains("\n  \"lead 7\"\n"), "a row keeps only what varies:\n{out}");
    }
}

#[cfg(test)]
mod records_fallback {
    use super::*;

    /// Items where a key is MISSING on some — the schema fold refuses them.
    fn videos() -> Vec<serde_json::Value> {
        (1..=30)
            .map(|i| {
                let mut v = serde_json::json!({"title": format!("Challenge {i}"), "id": format!("vid{i:04}"), "duration": 600 + i, "playlist": "Coding",
                    "playlist_url": "https://www.youtube.com/playlist?list=PLRqwX-V7Uu6ZiZxtDDRCi6uhfTH4FilpH", "uploader": "The Coding Train",
                    "extractor": "youtube:tab", "availability": "public"});
                if i % 2 == 0 {
                    v["channel"] = serde_json::json!("Train");
                } else {
                    v["thumbs"] = serde_json::json!([1, 2]);
                }
                v
            })
            .collect()
    }

    #[test]
    fn a_refused_array_reads_as_a_record_table() {
        let json = serde_json::to_string_pretty(&videos()).unwrap();
        let out = compact(&json);
        assert!(out.contains("30 records · "), "{out}");
        let row = out.lines().find(|l| l.contains("Challenge 17 ")).unwrap_or("");
        assert!(row.contains("| vid0017 |") && row.contains("| 617 |"), "{out}");
    }

    #[test]
    fn a_refused_payload_array_reads_as_a_record_table_too() {
        let json = serde_json::to_string(&serde_json::json!({"playlist": "PL1", "entries": videos()})).unwrap();
        let out = compact(&json);
        assert!(out.contains("[fold @ entries] 30 records · "), "{out}");
        assert!(out.contains("Challenge 29"), "{out}");
    }

    /// A REST issue list: the author is `user.login`, the labels are
    /// `labels[].name`, the issue NUMBER leads, and what only wraps it (`url`)
    /// or only renames the author (`user.id`) is named, not repeated.
    #[test]
    fn an_issue_list_shows_number_author_and_labels() {
        let issues: Vec<serde_json::Value> = (0..20)
            .map(|i| {
                let n = 163700 + i;
                serde_json::json!({
                    "url": format!("https://api.github.com/repos/o/r/issues/{n}"),
                    "id": 5696600000u64 + i,
                    "number": n,
                    "title": format!("Bug number {i}"),
                    "user": {"login": format!("dev{}", i % 4), "id": 1000 + i % 4, "type": "User"},
                    "labels": [{"name": "T-libs", "color": "fff"}, {"name": format!("S-{}", i % 7), "color": "000"}],
                    "repository_url": "https://api.github.com/repos/o/r",
                    "state": "open",
                    "created_at": format!("2026-10-0{}T05:{:02}:00Z", 1 + i % 3, i),
                    "updated_at": format!("2026-10-04T06:{:02}:00Z", i),
                    "author_association": (["NONE", "MEMBER", "CONTRIBUTOR"][(i % 3) as usize]),
                    "milestone": {"title": format!("1.{}", 90 + i % 5)},
                    "assignee": {"login": format!("rev{}", i % 6)},
                    "draft": i % 2 == 0,
                    "body": "x".repeat(300)
                })
            })
            .collect();
        let out = compact(&serde_json::to_string_pretty(&issues).unwrap());
        let header = out.lines().find(|l| l.starts_with("| ") && !l.starts_with("| ---")).unwrap_or("");
        assert!(header.contains("| number |") && header.contains("| user.login |") && header.contains("| labels[].name |"), "{out}");
        assert!(!header.contains("| url |") && !header.contains("| user.id |"), "{out}");
        // A constant long value is said once, and short.
        assert!(!out.contains(&"x".repeat(80)), "{out}");
        let row = out.lines().find(|l| l.contains("| Bug number 7 |")).unwrap_or("");
        assert!(row.contains("| 163707 |") && row.contains("| dev3 |"), "{out}");
        assert!(out.contains("T-libs, S-6"), "{out}");
    }
}

