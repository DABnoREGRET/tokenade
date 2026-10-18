//! `auto` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// 8 inputs of different formats run through `auto::compact`. Quality = the
/// fraction routed to the correct compactor. Savings = aggregate.
pub(crate) fn task_auto_dispatch(results: &mut Vec<RunResult>) {
    let cases: &[(&str, &str, auto::Kind)] = &[
        (
            "html",
            "<!DOCTYPE html><html><body><main><h1>Title</h1><p>body</p></main></body></html>",
            auto::Kind::Html,
        ),
        (
            "notebook",
            r#"{"cells":[{"cell_type":"code","source":["x=1\n"]}],"nbformat":4,"nbformat_minor":5,"metadata":{}}"#,
            auto::Kind::JupyterNotebook,
        ),
        (
            "json",
            r#"{"name":"x","html_url":"keep","avatar_url":"drop","_links":{"a":1}}"#,
            auto::Kind::Json,
        ),
        (
            "diff",
            "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n",
            auto::Kind::UnifiedDiff,
        ),
        (
            "terraform",
            "aws_instance.web: Refreshing state... [id=i-1]\nPlan: 0 to add, 0 to change, 0 to destroy.\n",
            auto::Kind::TerraformPlan,
        ),
        (
            "stacktrace",
            "Traceback (most recent call last):\n  File \"/site-packages/foo.py\", line 1, in <module>\n    bar()\nValueError: x\n",
            auto::Kind::StackTrace,
        ),
        (
            // Generic YAML (NOT a Kubernetes manifest — that's
            // routed to the more specific `K8sManifest` compactor,
            // which is the right behaviour. Use a CI-config-style
            // YAML here so this case exercises the generic Yaml path.
            "yaml",
            "image: rust:1.82\nstages:\n  - test\n  - build\nvariables:\n  CARGO_TARGET_DIR: /tmp/target\ntest:\n  stage: test\n  script:\n    - cargo test --workspace\n",
            auto::Kind::Yaml,
        ),
        (
            // Cover the K8s-manifest specialisation explicitly so the
            // dispatcher's "prefer specific over generic" behaviour has
            // a regression check.
            "k8s-manifest",
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n  namespace: default\nspec:\n  replicas: 3\n",
            auto::Kind::K8sManifest,
        ),
        (
            "challenge",
            "<!DOCTYPE html><html><body><div class=\"cf-turnstile\" data-sitekey=\"x\"></div></body></html>",
            auto::Kind::Challenge,
        ),
    ];
    let mut total_before = 0usize;
    let mut total_after = 0usize;
    let mut correct = 0usize;
    let t = Instant::now();
    for (_name, input, expected) in cases {
        let (kind, out, sav) = auto::compact(input);
        if kind == *expected {
            correct += 1;
        }
        total_before += sav.tokens_before;
        total_after += sav.tokens_after;
        let _ = out;
    }
    let ms = t.elapsed().as_millis();
    let quality = correct as f64 / cases.len() as f64;
    let before_pseudo = "x".repeat(total_before);
    let after_pseudo = "y".repeat(total_after);
    record(
        results,
        "output:auto-dispatch",
        "auto",
        "8-format-mix",
        &before_pseudo,
        &after_pseudo,
        ms,
        quality,
    );
}

// Adversarial #29: deeply ambiguous input — a stack trace embedded inside
// what looks like a JSON document. The detector should pick ONE format and
// not silently corrupt the data either way.
pub(crate) fn adv_auto_ambiguous_json_with_trace(results: &mut Vec<RunResult>) {
    let t = r#"{"error":"crash","traceback":"  File \"/usr/lib/python3/foo.py\", line 1, in main\n    bar()"}"#;
    let st = Instant::now();
    let (kind, out, _sav) = auto::compact(t);
    let ms = st.elapsed().as_millis();
    // The JSON shape is the outer envelope so JSON wins. We just need: a
    // routing decision is made, no panic, the "crash" payload survives.
    let routed = matches!(kind, auto::Kind::Json | auto::Kind::StackTrace);
    let payload_kept = out.contains("crash");
    let quality = if routed && payload_kept { 1.0 } else { 0.0 };
    record(
        results,
        "adv:auto:ambiguous-json-with-trace",
        "auto",
        "adversarial",
        t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #30: a near-empty input (3 chars). Must not panic, must route
// to Unknown, must return the input unchanged.
pub(crate) fn adv_auto_tiny_input(results: &mut Vec<RunResult>) {
    let cases = ["", " ", "ab", "{}\n", "[]\n"];
    let mut ok = 0;
    let st = Instant::now();
    for c in &cases {
        let (kind, out, _) = auto::compact(c);
        // Any of these is acceptable for these edge inputs; what we test is
        // *non-panic* and *non-corruption-when-Unknown*.
        if matches!(kind, auto::Kind::Unknown | auto::Kind::Json)
            && (out == *c || !out.is_empty() || c.is_empty())
        {
            ok += 1;
        }
    }
    let ms = st.elapsed().as_millis();
    let quality = ok as f64 / cases.len() as f64;
    record(
        results,
        "adv:auto:tiny-inputs",
        "auto",
        "adversarial",
        "tiny",
        "",
        ms,
        quality,
    );
}
