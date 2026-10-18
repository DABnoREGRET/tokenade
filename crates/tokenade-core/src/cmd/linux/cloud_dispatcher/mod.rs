//! Dispatcher for `cloud`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::argv::flag_value;

pub fn dispatch(argv: &[String], output: &str) -> String {
    let out_fmt = flag_value(argv, "--output")
        .or_else(|| flag_value(argv, "-o"))
        .or_else(|| flag_value(argv, "--format"))
        .unwrap_or("");
    if out_fmt == "json" || out_fmt == "yaml" || out_fmt == "yml" {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    if out_fmt == "text" {
        return generic(output);
    }
    // Default: auto detects whichever format the cloud CLI emitted.
    let (_, c, _) = crate::auto::compact(output);
    if c.len() < output.len() { c } else { generic(output) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn output_json_routes_to_auto() {
        // `aws ec2 describe-instances --output json` style — auto's
        // json compactor takes over.
        let body = r#"{"Reservations":[{"Groups":[],"Instances":[{"InstanceId":"i-abc","State":{"Code":16,"Name":"running"},"Tags":[{"Key":"Name","Value":"web1"}]}]}]}"#;
        for flag in &["--output", "-o", "--format"] {
            let out = dispatch(&argv(&["aws", "ec2", "describe-instances", flag, "json"]), body);
            assert!(out.contains("Instance") || out.contains("running"));
        }
    }

    #[test]
    fn output_yaml_routes_to_auto() {
        // gcloud / openstack often produce yaml.
        let body = "instances:\n  - name: web1\n    status: RUNNING\n    zone: us-central1-a\n    image: debian-11\n    cpu: 2\n";
        for fmt in &["yaml", "yml"] {
            let out = dispatch(&argv(&["gcloud", "compute", "instances", "list", "--format", fmt]), body);
            assert!(out.contains("web1") || out.contains("RUNNING"));
        }
    }

    #[test]
    fn output_text_routes_to_generic() {
        // `--format text` → generic (dedup + truncate).
        let body = "repeated line\nrepeated line\nrepeated line\nuseful info\n";
        let out = dispatch(
            &argv(&["aws", "s3", "ls", "--output", "text"]),
            body,
        );
        // generic dedups runs of identical lines.
        assert!(out.contains("useful info"));
        assert_eq!(
            out.matches("repeated line").count(),
            1,
            "text mode must dedup; got: {out:?}"
        );
    }

    #[test]
    fn default_uses_auto_when_structured() {
        // No -o flag, but content IS JSON → auto wins on size; the
        // dispatcher returns auto's output (smaller than raw).
        let body = format!(
            "{{\"items\":[{}]}}\n",
            (0..40).map(|i| format!("{{\"id\":{i}}}")).collect::<Vec<_>>().join(",")
        );
        let out = dispatch(&argv(&["aws", "s3", "ls"]), &body);
        // auto's JSON compactor folds the array into a lossless TOON block.
        assert!(out.contains("items") && out.contains("toon"));
    }

    #[test]
    fn default_falls_back_to_generic_when_auto_did_not_shrink() {
        // No -o flag, content is prose-y → auto returns same length →
        // dispatcher uses generic instead. We can't easily check WHICH
        // path fired, but we can verify a result is returned and that
        // it preserves recognizable content.
        let body = "alpha line\nbeta line\nbeta line\nbeta line\ngamma line\n";
        let out = dispatch(&argv(&["aws", "s3", "ls"]), body);
        assert!(!out.is_empty());
        assert!(out.contains("alpha") && out.contains("gamma"));
    }
}
