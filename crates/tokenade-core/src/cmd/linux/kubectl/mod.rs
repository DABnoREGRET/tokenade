//! `kubectl` / `helm` / `nomad` / `consul` / `vault` — variant
//! dispatcher.
//!
//! Subcommands and their output shapes:
//!
//! | Sub                | Output shape                | Strategy             |
//! |--------------------|------------------------------|----------------------|
//! | `get` (default)    | tabular                      | tabular              |
//! | `get -o json`      | JSON                         | auto JSON            |
//! | `get -o yaml`      | YAML                         | auto YAML            |
//! | `get -o wide`      | wider tabular                | tabular              |
//! | `get -o name`      | `<kind>/<name>` per line     | filename list w/ cap |
//! | `describe`         | freeform with sections       | section-aware        |
//! | `logs`             | application stdout/stderr    | generic dedup        |
//! | `top`              | tabular                      | tabular              |
//! | `apply` / `delete` | per-resource status lines    | pass-through         |
//! | `diff`             | unified diff                 | git diff compactor   |
//! | `version`          | client+server info           | pass-through         |
//!
//! Extracted from `filter.rs::kubectl_dispatch` in the L92 refactor.

pub mod get_name;

use crate::argv::{find_subcommand, flag_value};
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    let o = flag_value(argv, "-o")
        .or_else(|| flag_value(argv, "--output"))
        .unwrap_or("");
    let sub = find_subcommand("kubectl", argv);
    match (sub, o) {
        // Structured formats — auto picks json/yaml compactor.
        (_, "json" | "yaml") => {
            let (_, c, _) = crate::auto::compact(output);
            c
        }
        // `get -o name`: one `<kind>/<name>` per line, with cap.
        (_, "name") => get_name::compact(output),
        // Tabular `get` / `top`: reuse the docker-style tabular
        // compactor (same shape: header row + space-padded columns).
        ("get" | "top", _) => super::docker::ps::compact(output),
        // `describe` and `logs`: long-form sections / app stdout.
        ("describe" | "logs", _) => super::docker::logs::compact(output),
        // `diff`: unified diff — reuse the git diff compactor.
        ("diff", _) => super::git::diff::dispatch(&[], output),
        // Status-only commands: minimal trim — plus a fold for the one shape
        // that is not status-only at all. `kubectl apply -f dir/` prints one
        // `<kind>/<name> configured` line per resource, and a real manifest
        // directory holds hundreds: measured 13 617 chars in, 13 616 out.
        // Those lines are a receipt; the errors among them are the message.
        ("apply" | "delete" | "create" | "patch" | "version", _) => {
            let trimmed = strip_ansi(output);
            fold_resource_receipts(trimmed.trim())
        }
        // Fallback to auto detector.
        _ => {
            let (_, c, _) = crate::auto::compact(output);
            c
        }
    }
}

/// Is this a `<kind>/<name> <verb>` receipt line from `apply`/`delete`/`create`?
///
/// Deliberately keyed on the trailing VERB rather than on the slash: an error
/// line can also contain a slash, and folding one would take the only thing
/// worth reading. The verb list is closed, so anything kubectl says that is not
/// a plain success receipt survives verbatim.
fn is_resource_receipt(line: &str) -> bool {
    let t = line.trim();
    let Some((head, verb)) = t.rsplit_once(' ') else {
        return false;
    };
    matches!(
        verb,
        "created" | "configured" | "unchanged" | "deleted" | "serverside-applied" | "patched"
    ) && head.contains('/')
        && !head.contains(' ')
}

/// Fold the receipts into a count, keep everything else byte-for-byte.
fn fold_resource_receipts(text: &str) -> String {
    const MIN: usize = 12;
    let mut kept: Vec<&str> = Vec::new();
    let mut folded = 0usize;
    for line in text.lines() {
        if is_resource_receipt(line) {
            folded += 1;
        } else {
            kept.push(line);
        }
    }
    if folded < MIN {
        return text.to_string();
    }
    let mut out = format!("[tokenade:kubectl] {folded} resource(s) applied without error\n");
    out.push_str(kept.join("\n").trim_end());
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {

    #[test]
    fn apply_folds_its_receipts_but_never_an_error() {
        let mut input: String = (0..400)
            .map(|i| format!("deployment.apps/app{i} configured\n"))
            .collect();
        input.push_str("Error from server (Invalid): error when creating \"svc.yaml\":\n");
        input.push_str("  Service \"api\" is invalid: spec.ports[0].port: must be between 1 and 65535\n");
        let out = dispatch(&argv(&["kubectl", "apply", "-f", "."]), &input);
        assert!(out.contains("400 resource(s) applied without error"), "got:\n{out}");
        assert!(out.contains("Error from server (Invalid)"), "the error survives:\n{out}");
        assert!(
            out.contains("must be between 1 and 65535"),
            "and the line that says WHY:\n{out}"
        );
        assert!(out.len() * 10 < input.len(), "a real fold: {} vs {}", out.len(), input.len());
    }

    #[test]
    fn a_small_apply_is_left_alone() {
        let input = "deployment.apps/api configured\nservice/api unchanged\n";
        let out = dispatch(&argv(&["kubectl", "apply", "-f", "api.yaml"]), input);
        assert_eq!(out, input.trim());
    }
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn get_json_routes_to_auto_compact() {
        // `-o json` lets auto pick the JSON compactor. Sanity: the JSON
        // body survives the dispatch (auto recognizes the shape).
        let body = r#"{"items":[{"name":"a"},{"name":"b"},{"name":"c"}]}"#;
        let out = dispatch(&argv(&["kubectl", "get", "pods", "-o", "json"]), body);
        assert!(
            out.contains("items"),
            "JSON dispatch must preserve `items` key; got: {out}"
        );
    }

    #[test]
    fn get_yaml_routes_to_auto_compact() {
        let body = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n  namespace: default\nstatus:\n  phase: Running\n";
        let out = dispatch(&argv(&["kubectl", "get", "pods", "-o", "yaml"]), body);
        assert!(out.contains("Pod"), "YAML dispatch must preserve `Pod` kind; got: {out}");
    }

    #[test]
    fn get_o_name_routes_to_dedicated_compactor() {
        // `get -o name` produces `<kind>/<name>` per line.
        let body = "pod/a\npod/b\npod/c\npod/d\npod/e\n";
        let out = dispatch(&argv(&["kubectl", "get", "pods", "-o", "name"]), body);
        assert!(out.contains("pod/a") && out.contains("pod/e"));
    }

    #[test]
    fn get_default_routes_to_tabular() {
        // No `-o` → header + space-padded rows (docker ps-style).
        let body = "NAME   READY   STATUS    AGE\n\
                    foo    1/1     Running   3d\n\
                    bar    1/1     Running   5d\n";
        let out = dispatch(&argv(&["kubectl", "get", "pods"]), body);
        assert!(out.contains("foo") && out.contains("bar"));
    }

    #[test]
    fn top_uses_tabular() {
        let body = "NAME   CPU(cores)   MEMORY(bytes)\n\
                    foo    12m          64Mi\n\
                    bar    8m           32Mi\n";
        let out = dispatch(&argv(&["kubectl", "top", "pods"]), body);
        assert!(out.contains("foo") && out.contains("bar"));
    }

    #[test]
    fn describe_routes_to_logs_compactor() {
        // `describe` is long-form sections (Name:, Status:, Events:).
        let body = "Name:         foo\nNamespace:    default\n\
                    Labels:       app=foo\nStatus:       Running\n\
                    Events:\n  pulled image\n  started container\n";
        let out = dispatch(&argv(&["kubectl", "describe", "pod", "foo"]), body);
        assert!(out.contains("foo"));
    }

    #[test]
    fn diff_routes_to_git_diff_compactor() {
        let body = "diff -u -N /tmp/live /tmp/wanted\n\
                    --- /tmp/live\n+++ /tmp/wanted\n@@ -1,3 +1,3 @@\n \
                    apiVersion: v1\n-replicas: 1\n+replicas: 2\n";
        let out = dispatch(&argv(&["kubectl", "diff", "-f", "x.yaml"]), body);
        assert!(out.contains("replicas") || out.contains("@@"));
    }

    #[test]
    fn apply_delete_create_patch_pass_through_trimmed() {
        // Status commands: ANSI strip + trim, no structural transform.
        for sub in &["apply", "delete", "create", "patch"] {
            let body = format!("\x1b[32mpod/foo {sub}d\x1b[0m\n");
            let out = dispatch(&argv(&["kubectl", sub, "-f", "x.yaml"]), &body);
            assert!(
                out.contains("pod/foo") && !out.contains("\x1b["),
                "{sub} must strip ANSI + preserve content; got: {out:?}"
            );
        }
    }

    #[test]
    fn version_passes_through() {
        let body = "Client Version: v1.29.0\nKustomize Version: v5.0.4-0.20230601\nServer Version: v1.29.0\n";
        let out = dispatch(&argv(&["kubectl", "version"]), body);
        assert!(out.contains("Client Version") && out.contains("Server Version"));
    }

    #[test]
    fn unknown_subcommand_falls_back_to_auto() {
        // `exec` isn't handled explicitly — must fall through to auto
        // so structured payloads still get the right compactor.
        let body = "{\"foo\":1,\"bar\":2,\"baz\":3,\"qux\":4,\"quux\":5,\"corge\":6}\n";
        let out = dispatch(&argv(&["kubectl", "exec", "pod", "--", "cat", "x.json"]), body);
        assert!(out.contains("foo"));
    }
}
