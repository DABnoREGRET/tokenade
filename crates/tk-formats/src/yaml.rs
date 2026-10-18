//! YAML compaction (and kubectl-style manifests in particular).
//!
//! A real `kubectl get pod -o yaml` is a 200-line manifest with **5000 lines
//! of `managedFields`** plus `resourceVersion`, `uid`, `creationTimestamp`,
//! `kubectl.kubernetes.io/last-applied-configuration` (an inlined JSON blob),
//! and every `status.conditions[].lastTransitionTime` / `lastProbeTime`. The
//! signal the agent needs is `spec` + a few `status` fields; the rest is
//! Kubernetes bookkeeping.
//!
//! We don't pull a YAML parser dependency. Instead [`compact`] operates at
//! the **line level** with indentation tracking, which is enough to:
//!
//!   - drop a known noise key and every line at deeper indent under it,
//!   - truncate long single-line scalar values,
//!   - cap repeated list items at N + summary marker,
//!   - never panic on malformed indentation.
//!
//! Survives malformed input: any line we can't classify is passed through.

use crate::tokenizer;

/// Noise keys whose entire block is dropped **in a Kubernetes manifest**.
///
/// These names are bookkeeping only where Kubernetes wrote them. Elsewhere
/// they are ordinary field names, and dropping them cost real values: an
/// application config with `uid:`, `creationTimestamp:` or `resourceVersion:`
/// had them replaced by a marker, in a file that has nothing to do with
/// kubectl. The loss was announced rather than silent, but it was still a
/// field the reader asked for. Gated on the document actually being a
/// manifest — see the `is_k8s_doc` test in [`compact`].
pub const DROP_BLOCKS: &[&str] = &[
    // Kubernetes bookkeeping
    "managedFields",
    "resourceVersion",
    "uid",
    "generation",
    "creationTimestamp",
    "deletionTimestamp",
    "selfLink",
    "ownerReferences",
    "finalizers",
    // Per-condition timestamps (kept the condition itself, dropped the times)
    "lastTransitionTime",
    "lastProbeTime",
    "lastUpdateTime",
    "lastHeartbeatTime",
    "observedGeneration",
    // Helm bookkeeping
    "helm.sh/hook",
    "helm.sh/hook-weight",
    // Inline kubectl applied-config (a JSON blob, useless to a human reader)
    "kubectl.kubernetes.io/last-applied-configuration",
];

/// Truncate single-line scalar VALUES longer than this many chars.
pub const STR_CAP: usize = 240;
/// Keep at most this many items in any explicit list before folding.
pub const LIST_CAP: usize = 12;

pub fn compact(yaml: &str) -> String {
    let is_k8s_doc = yaml.contains("apiVersion:") && yaml.contains("kind:");
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());

    // When we drop a block, skip every subsequent line whose indent is
    // strictly greater than the dropped key's indent. The block ends when
    // we hit a non-empty line whose indent is ≤ that.
    let mut skip_above_indent: Option<usize> = None;

    // Track sequence-item count at each indent depth. Items are siblings
    // when they share an indent — the counter accumulates across the
    // children of each item and only resets when we actually dedent past
    // the sequence's parent.
    let mut seq_count: std::collections::HashMap<usize, usize> = Default::default();

    for raw_line in &lines {
        let line = *raw_line;
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        // Resolve any pending block-skip.
        if let Some(limit) = skip_above_indent {
            if trimmed.is_empty() || indent > limit {
                continue;
            }
            skip_above_indent = None;
        }

        // 1) Detect `key:` (with or without inline value) → check noise list.
        //    Only inside a real manifest: outside one these are just field
        //    names, and the flag below was computed for exactly this and then
        //    never consulted.
        if let Some(key) = key_before_colon(trimmed) {
            if is_k8s_doc && DROP_BLOCKS.contains(&key) {
                out.push(format!("{}{}: <dropped>", " ".repeat(indent), key));
                skip_above_indent = Some(indent);
                continue;
            }
        }

        // 2) Dedent: drop any sequence counters at deeper indent than this
        // line. (Skip blank lines so they don't false-trigger a dedent.)
        if !trimmed.is_empty() {
            seq_count.retain(|k, _| *k <= indent);
        }

        // 3) Cap list items: lines like `<indent>- ...` are sequence entries.
        if trimmed.starts_with("- ") {
            let n = seq_count.entry(indent).or_insert(0);
            *n += 1;
            if *n > LIST_CAP {
                // Skip this list-item subtree (everything until we dedent).
                skip_above_indent = Some(indent);
                if *n == LIST_CAP + 1 {
                    out.push(format!("{}- … more items …", " ".repeat(indent)));
                }
                continue;
            }
        }

        // 4) Truncate long scalar values.
        out.push(truncate_inline_value(line));
    }

    // Collapse multi-blank runs.
    let mut squeezed = String::with_capacity(out.iter().map(|l| l.len() + 1).sum());
    let mut blank = 0;
    for l in &out {
        if l.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                squeezed.push('\n');
            }
        } else {
            blank = 0;
            squeezed.push_str(l);
            squeezed.push('\n');
        }
    }
    squeezed.trim_end().to_string()
}

pub fn compact_with_savings(yaml: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(yaml);
    let out = compact(yaml);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

/// Return the bare key name from a line like `  metadata.name: foo` or
/// `managedFields:` (without surrounding indent). YAML keys can be quoted
/// (`"weird key": value`); we handle the common unquoted form and the
/// fully-quoted form. Anything else returns `None` and is passed through.
fn key_before_colon(trimmed: &str) -> Option<&str> {
    // Skip sequence markers.
    let s = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    if let Some(rest) = s.strip_prefix('"') {
        // Quoted key.
        let end = rest.find('"')?;
        let after = &rest[end + 1..];
        if after.starts_with(':') {
            return Some(&rest[..end]);
        }
        return None;
    }
    let colon = s.find(':')?;
    let key = &s[..colon];
    // A key must not contain whitespace or special YAML chars.
    if key.is_empty() || key.chars().any(|c| c.is_whitespace() || c == '#') {
        return None;
    }
    Some(key)
}

/// If the line is `<key>: <very long scalar>`, truncate the scalar with a
/// `…[+N chars]` marker so the structure is preserved.
fn truncate_inline_value(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let Some(colon) = trimmed.find(": ") else {
        // No inline value (block-scalar markers `:|`, `:>`, or a key-only line).
        return line.to_string();
    };
    let (key, rest) = trimmed.split_at(colon + 2);
    let n = rest.chars().count();
    if n <= STR_CAP {
        return line.to_string();
    }
    let kept: String = rest.chars().take(STR_CAP).collect();
    format!(
        "{}{key}{kept}…[+{} chars]",
        " ".repeat(indent_len),
        n - STR_CAP
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn drops_managed_fields_block() {
        let y = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n  managedFields:\n    - manager: kubectl\n      apiVersion: v1\n      fieldsType: FieldsV1\n      operation: Update\n  namespace: default\nspec:\n  containers: []\n";
        let out = compact(y);
        assert!(out.contains("name: foo"));
        assert!(out.contains("namespace: default"));
        assert!(out.contains("kind: Pod"));
        assert!(out.contains("managedFields: <dropped>"));
        assert!(!out.contains("FieldsV1"));
    }

    #[test]
    fn drops_resource_version_and_uid() {
        // A real manifest — the fixture carries the markers that make these
        // names kubectl bookkeeping rather than ordinary fields.
        let y = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: x\n  uid: aaa-bbb-ccc\n  resourceVersion: \"12345\"\n  creationTimestamp: \"2024-01-01T00:00:00Z\"\n";
        let out = compact(y);
        assert!(out.contains("name: x"));
        assert!(!out.contains("aaa-bbb-ccc"));
        assert!(!out.contains("12345"));
        assert!(!out.contains("2024-01-01T00:00:00Z"));
    }

    /// Outside a manifest these are just field names, and an application
    /// config that happens to use them was losing real values to a list
    /// written for kubectl.
    #[test]
    fn an_ordinary_config_keeps_fields_that_only_kubectl_calls_noise() {
        let y = "account:\n  uid: 4c2f9a1e-legacy-account-key\n  creationTimestamp: \"2019-04-02T10:00:00Z\"\n  resourceVersion: v3-schema\n  generation: 7\n";
        let out = compact(y);
        for kept in [
            "4c2f9a1e-legacy-account-key",
            "2019-04-02T10:00:00Z",
            "v3-schema",
            "7",
        ] {
            assert!(
                out.contains(kept),
                "dropped {kept} from a non-manifest:\n{out}"
            );
        }
        assert!(!out.contains("<dropped>"), "nothing to drop here:\n{out}");
    }

    #[test]
    fn truncates_long_inline_scalar() {
        let big = "X".repeat(1000);
        let y = format!("annotation:\n  config: {big}\n");
        let out = compact(&y);
        assert!(out.contains("[+760 chars]"));
        assert!(out.len() < y.len() / 2);
    }

    #[test]
    fn caps_long_list() {
        let mut y = String::from("items:\n");
        for i in 0..30 {
            let _ = write!(y, "  - id: {i}\n    name: item-{i}\n");
        }
        let out = compact(&y);
        assert!(out.contains("- id: 0"));
        assert!(out.contains("more items"));
        // Should not contain item 25 (cap is 12)
        assert!(!out.contains("id: 25"));
    }

    #[test]
    fn malformed_does_not_panic() {
        let y = "key:\n\tweirdly-indented:\n   inconsistent\n   - mix\n\twith-tabs";
        let _ = compact(y);
    }

    #[test]
    fn savings_real_shape() {
        // Build a fake "kubectl get pod" with a massive managedFields tree.
        let mut y = String::from(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: real-pod\n  namespace: default\n  managedFields:\n",
        );
        for i in 0..200 {
            let _ = write!(
                y,
                "    - manager: kubectl-{i}\n      apiVersion: v1\n      fieldsType: FieldsV1\n      operation: Update\n      time: \"2024-01-01T00:00:00Z\"\n"
            );
        }
        y.push_str("spec:\n  containers:\n    - name: app\n      image: my-app:latest\n");
        let (out, sav) = compact_with_savings(&y);
        assert!(out.contains("real-pod"));
        assert!(out.contains("name: app"));
        assert!(sav.percent() > 80.0, "got {:?}", sav);
    }
}
