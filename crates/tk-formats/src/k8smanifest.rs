//! Kubernetes manifest YAML compaction.
//!
//! `helm template`, `kubectl get -o yaml`, `kustomize build`, and raw `.yaml`
//! files inside `manifests/` / `kubernetes/` directories all emit one or more
//! YAML documents, each describing a k8s resource:
//!
//! ```yaml
//! ---
//! # Source: chart/templates/deployment.yaml
//! apiVersion: apps/v1
//! kind: Deployment
//! metadata:
//!   name: my-app
//!   namespace: default
//! spec:
//!   replicas: 3
//!   template:
//!     spec:
//!       containers:
//!         - name: app
//!           image: myapp:v1.0
//!           …500 lines of spec…
//! ---
//! apiVersion: v1
//! kind: Service
//! …
//! ```
//!
//! A real `helm template` of a moderately-complex chart is 2 000 – 20 000
//! lines; the agent reading it really just needs:
//!
//!   - the **resource list** — `apiVersion`/`kind`/`namespace`/`name`
//!   - **Deployment / StatefulSet / DaemonSet**: replica count + every
//!     container image
//!   - **Service**: type + first ports
//!   - **ConfigMap / Secret**: list of `data:` keys (NOT the values — those
//!     are often KB-MB of inlined config + base64-encoded secrets)
//!   - **Ingress**: list of hosts
//!
//! Detection requires BOTH `apiVersion:` AND `kind:` as column-0 keys (so
//! that a YAML file mentioning `apiVersion` in a value doesn't false-match).

use std::fmt::Write;

pub const RESOURCES_CAP: usize = 200;
pub const IMAGES_CAP_PER_RESOURCE: usize = 20;
pub const KEYS_CAP_PER_CONFIGMAP: usize = 50;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(32_768)).unwrap_or(text);
    // Both keys MUST appear at column 0 (top-level YAML key, not inside a
    // string value). A line starting with whitespace doesn't count.
    let has_apiversion = head.lines().any(|l| {
        l.starts_with("apiVersion:") || l.starts_with("apiVersion ") || l == "apiVersion:"
    });
    let has_kind = head
        .lines()
        .any(|l| l.starts_with("kind:") || l.starts_with("kind ") || l == "kind:");
    has_apiversion && has_kind
}

#[derive(Default, Debug)]
struct Resource {
    api_version: String,
    kind: String,
    name: String,
    namespace: String,
    replicas: Option<String>,
    images: Vec<String>,
    service_type: Option<String>,
    ports: Vec<String>,
    data_keys: Vec<String>,
    ingress_hosts: Vec<String>,
}

/// Split the input into YAML documents separated by `---`. Handles leading
/// `---`, trailing `---`, and `---` followed by a `# Source:` helm comment.
fn split_docs(text: &str) -> Vec<&str> {
    let trimmed = text.trim_start();
    let body = trimmed.strip_prefix("---\n").unwrap_or(trimmed);
    body.split("\n---\n")
        .filter(|d| !d.trim().is_empty())
        .collect()
}

fn parse_resource(doc: &str) -> Option<Resource> {
    let mut r = Resource::default();
    let mut in_metadata = false;
    let mut in_data = false;

    for raw in doc.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        // YAML comments — skip them entirely.
        if line.trim_start().starts_with('#') {
            continue;
        }
        let t = line.trim_start();
        let indent = line.len() - t.len();

        // Column-0 keys are top-level keys for this doc.
        if indent == 0 {
            if let Some(v) = t.strip_prefix("apiVersion: ") {
                r.api_version = v.trim().to_string();
                continue;
            }
            if let Some(v) = t.strip_prefix("apiVersion:") {
                r.api_version = v.trim().to_string();
                continue;
            }
            if let Some(v) = t.strip_prefix("kind: ") {
                r.kind = v.trim().to_string();
                continue;
            }
            if let Some(v) = t.strip_prefix("kind:") {
                r.kind = v.trim().to_string();
                continue;
            }
            // Section toggles.
            if t == "metadata:" || t.starts_with("metadata:") {
                in_metadata = true;
                in_data = false;
                continue;
            }
            if t == "data:" || t == "stringData:" {
                in_data = true;
                in_metadata = false;
                continue;
            }
            // Any other column-0 key terminates active sections.
            in_metadata = false;
            in_data = false;
        }

        // metadata sub-keys at indent 2.
        if in_metadata && indent <= 4 {
            if let Some(v) = t.strip_prefix("name: ") {
                if r.name.is_empty() {
                    r.name = v.trim().to_string();
                }
            } else if let Some(v) = t.strip_prefix("namespace: ") {
                if r.namespace.is_empty() {
                    r.namespace = v.trim().to_string();
                }
            }
        }

        // ConfigMap / Secret data block: keys live at indent 2 (single-block
        // YAML) and have the `key: value` or `key: |` shape.
        if in_data && indent == 2 {
            if let Some((key_raw, _)) = t.split_once(':') {
                let key = key_raw.trim().to_string();
                if !key.is_empty()
                    && !r.data_keys.contains(&key)
                    && r.data_keys.len() < KEYS_CAP_PER_CONFIGMAP
                {
                    r.data_keys.push(key);
                }
            }
        }

        // Once inside a `data:` block, every further line is part of a
        // string VALUE — even an indented `replicas: 99` is just text inside
        // a ConfigMap entry, not a structural field of THIS resource. Skip
        // the per-field extractions that follow.
        if in_data {
            continue;
        }

        // Container images: any line matching `image: …` regardless of nesting.
        if let Some(v) = t.strip_prefix("image: ") {
            if r.images.len() < IMAGES_CAP_PER_RESOURCE {
                r.images.push(v.trim().to_string());
            }
        } else if let Some(v) = t.strip_prefix("- image: ") {
            if r.images.len() < IMAGES_CAP_PER_RESOURCE {
                r.images.push(v.trim().to_string());
            }
        }

        // Replicas — gated by kind to avoid picking up `replicas: …` text
        // accidentally present in a ConfigMap value (we already skip when
        // in_data, but a Secret might use stringData or non-standard layouts).
        if matches!(
            r.kind.as_str(),
            "Deployment" | "StatefulSet" | "ReplicaSet" | "ReplicationController"
        ) {
            if let Some(v) = t.strip_prefix("replicas: ") {
                if r.replicas.is_none() {
                    r.replicas = Some(v.trim().to_string());
                }
            }
        }

        // Service type — must be a Service AND must be the `spec.type` not
        // `metadata.type` (no such thing in standard k8s, so safe heuristic).
        if r.kind == "Service" {
            if let Some(v) = t.strip_prefix("type: ") {
                if r.service_type.is_none() {
                    r.service_type = Some(v.trim().to_string());
                }
            }
        }

        // Ports.
        if r.kind == "Service" {
            if let Some(v) = t.strip_prefix("port: ") {
                if !r.ports.iter().any(|p| p == v.trim()) {
                    r.ports.push(v.trim().to_string());
                }
            }
            if let Some(v) = t.strip_prefix("- port: ") {
                if !r.ports.iter().any(|p| p == v.trim()) {
                    r.ports.push(v.trim().to_string());
                }
            }
        }

        // Ingress hosts.
        if r.kind == "Ingress" {
            if let Some(v) = t.strip_prefix("host: ") {
                if !r.ingress_hosts.iter().any(|h| h == v.trim()) {
                    r.ingress_hosts.push(v.trim().to_string());
                }
            }
            if let Some(v) = t.strip_prefix("- host: ") {
                if !r.ingress_hosts.iter().any(|h| h == v.trim()) {
                    r.ingress_hosts.push(v.trim().to_string());
                }
            }
        }
    }

    if r.api_version.is_empty() || r.kind.is_empty() {
        return None;
    }
    Some(r)
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let docs = split_docs(text);
    let mut resources: Vec<Resource> = Vec::new();
    for doc in &docs {
        if let Some(r) = parse_resource(doc) {
            resources.push(r);
        }
    }

    let mut out = String::with_capacity(2048);
    let total_resources = resources.len();
    let total_docs = docs.len();

    if resources.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "{total_docs} document(s), {total_resources} resource(s)"
    );

    // Compute column widths for the alignment.
    let api_w = resources
        .iter()
        .take(RESOURCES_CAP)
        .map(|r| r.api_version.len() + 1 + r.kind.len())
        .max()
        .unwrap_or(20)
        .min(50);

    out.push_str("\nresources:\n");
    // A generated fleet (`svc-0` … `svc-299`) is ONE line naming its range:
    // the cap taken over raw rows named the first 200 and dropped the rest —
    // a ConfigMap after them included — unnamed.
    let mut rows: Vec<(String, Option<(String, u64)>)> = Vec::new();
    for r in &resources {
        let ns_name = if r.namespace.is_empty() {
            r.name.clone()
        } else {
            format!("{}/{}", r.namespace, r.name)
        };
        let kind_block = format!("{} {}", r.api_version, r.kind);
        let mut extras: Vec<String> = Vec::new();
        if let Some(repl) = &r.replicas {
            extras.push(format!("replicas: {repl}"));
        }
        if !r.images.is_empty() {
            extras.push(format!("images: [{}]", r.images.join(", ")));
        }
        if let Some(t) = &r.service_type {
            extras.push(format!("type: {t}"));
        }
        if !r.ports.is_empty() {
            extras.push(format!("ports: [{}]", r.ports.join(", ")));
        }
        if !r.ingress_hosts.is_empty() {
            extras.push(format!("hosts: [{}]", r.ingress_hosts.join(", ")));
        }
        if !r.data_keys.is_empty() {
            extras.push(format!(
                "data keys ({}): [{}]",
                r.data_keys.len(),
                r.data_keys.join(", ")
            ));
        }
        let extras_str = if extras.is_empty() {
            String::new()
        } else {
            format!("  {}", extras.join(", "))
        };
        let line = |name: &str| {
            format!(
                "  {:<api_w$}  {}{}",
                kind_block,
                name,
                extras_str,
                api_w = api_w
            )
        };
        rows.push((
            line(&ns_name),
            numbered(&ns_name).map(|(stem, n)| (line(&format!("{stem}{{}}")), n)),
        ));
    }
    let mut members: std::collections::HashMap<&str, Vec<u64>> = std::collections::HashMap::new();
    for (_, t) in &rows {
        if let Some((tpl, n)) = t {
            members.entry(tpl.as_str()).or_default().push(*n);
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut done: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (line, t) in &rows {
        match t {
            Some((tpl, _)) if members[tpl.as_str()].len() >= FLEET_MIN => {
                if done.insert(tpl.as_str()) {
                    let ns = &members[tpl.as_str()];
                    let braced = format!("{{{}}} (×{})", ranges(ns), ns.len());
                    lines.push(tpl.replacen("{}", &braced, 1));
                }
            }
            _ => lines.push(line.clone()),
        }
    }
    for l in lines.iter().take(RESOURCES_CAP) {
        out.push_str(l);
        out.push('\n');
    }
    if lines.len() > RESOURCES_CAP {
        let _ = writeln!(
            out,
            "  … {} more resource line(s) elided",
            lines.len() - RESOURCES_CAP
        );
    }

    out.trim_end().to_string()
}

/// Resources named `stem` + a number before this many share one line.
const FLEET_MIN: usize = 3;

/// `prod/svc-12` → (`prod/svc-`, 12). A zero-padded number (`svc-007`) is
/// not split: written back it would lose its padding.
fn numbered(name: &str) -> Option<(&str, u64)> {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 9 {
        return None;
    }
    let (stem, num) = name.split_at(name.len() - digits);
    if num.len() > 1 && num.starts_with('0') {
        return None;
    }
    Some((stem, num.parse().ok()?))
}

/// `0,1,2,3,7,9,10` → `0..3,7,9..10`, in the order given sorted.
fn ranges(ns: &[u64]) -> String {
    let mut v = ns.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        parts.push(if j > i {
            format!("{}..{}", v[i], v[j])
        } else {
            v[i].to_string()
        });
        i = j + 1;
    }
    parts.join(",")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELM_SAMPLE: &str = r#"---
# Source: chart/templates/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: default
  labels:
    app: my-app
spec:
  replicas: 3
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
        - name: app
          image: myapp:v1.0
          ports:
            - containerPort: 8080
        - name: sidecar
          image: envoyproxy/envoy:v1.27.0
---
# Source: chart/templates/service.yaml
apiVersion: v1
kind: Service
metadata:
  name: my-app
  namespace: default
spec:
  type: ClusterIP
  selector:
    app: my-app
  ports:
    - port: 80
      targetPort: 8080
      protocol: TCP
    - port: 443
      targetPort: 8443
      protocol: TCP
---
# Source: chart/templates/configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: my-config
  namespace: default
data:
  config.yaml: |
    server:
      port: 8080
      host: 0.0.0.0
    log:
      level: info
  secrets.env: |
    DB_HOST=db.example.com
    DB_PORT=5432
  feature-flags.json: |
    {"flag-a": true, "flag-b": false}
---
# Source: chart/templates/ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-app
  namespace: default
spec:
  rules:
    - host: example.com
      http:
        paths:
          - path: /
            pathType: Prefix
    - host: api.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
"#;

    #[test]
    fn a_fleet_with_gaps_names_its_ranges_and_padded_names_stay() {
        let mut y = String::new();
        for n in ["0", "1", "2", "3", "7", "9", "10", "007", "008", "009"] {
            y.push_str(&format!(
                "apiVersion: v1\nkind: Service\nmetadata:\n  name: web-{n}\n---\n"
            ));
        }
        y.push_str("apiVersion: v1\nkind: Service\nmetadata:\n  name: db-1\n");
        let out = compact(&y);
        assert!(out.contains("web-{0..3,7,9..10} (×7)"), "{out}");
        assert!(out.contains("web-007") && out.contains("web-009"), "{out}");
        assert!(out.contains("  db-1") || out.contains(" db-1"), "{out}");
        assert!(!out.contains("db-{"), "{out}");
    }

    #[test]
    fn a_fleet_past_the_cap_still_names_every_resource() {
        let mut y = String::new();
        for i in 0..300 {
            y.push_str(&format!(
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: svc-{i}\n  namespace: prod\nspec:\n  replicas: 2\n---\n"
            ));
        }
        y.push_str("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: last-config\n  namespace: prod\ndata:\n  k: v\n");
        let out = compact(&y);
        assert!(out.contains("prod/svc-{0..299} (×300)"), "{out}");
        assert!(out.contains("last-config"), "{out}");
        assert!(!out.contains("elided"), "{out}");
    }

    #[test]
    fn detects_multi_doc_manifest() {
        assert!(detect(HELM_SAMPLE));
    }

    #[test]
    fn detects_single_doc_manifest() {
        let s = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n";
        assert!(detect(s));
    }

    #[test]
    fn rejects_plain_yaml() {
        let s = "foo: bar\nbaz: qux\nlist:\n  - one\n  - two\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_yaml_mentioning_apiversion_in_value() {
        // `apiVersion` appears, but only as a value, not a column-0 key.
        let s = "spec:\n  apiVersion: v1\n  kind: Pod\n";
        // No column-0 `apiVersion:` or `kind:` line — should be rejected.
        assert!(!detect(s));
    }

    #[test]
    fn extracts_all_resources() {
        let out = compact(HELM_SAMPLE);
        assert!(out.contains("4 document(s), 4 resource(s)"));
        assert!(out.contains("apps/v1 Deployment"));
        assert!(out.contains("v1 Service"));
        assert!(out.contains("v1 ConfigMap"));
        assert!(out.contains("networking.k8s.io/v1 Ingress"));
        assert!(out.contains("default/my-app"));
        assert!(out.contains("default/my-config"));
    }

    #[test]
    fn extracts_deployment_replicas_and_images() {
        let out = compact(HELM_SAMPLE);
        assert!(out.contains("replicas: 3"));
        assert!(out.contains("myapp:v1.0"));
        assert!(out.contains("envoyproxy/envoy:v1.27.0"));
    }

    #[test]
    fn extracts_service_type_and_ports() {
        let out = compact(HELM_SAMPLE);
        assert!(out.contains("type: ClusterIP"));
        assert!(out.contains("ports: [80, 443]"), "out: {out}");
    }

    #[test]
    fn extracts_configmap_keys_not_values() {
        let out = compact(HELM_SAMPLE);
        assert!(out.contains("data keys (3): [config.yaml, secrets.env, feature-flags.json]"));
        // Values must NOT leak.
        assert!(!out.contains("server.port: 8080"));
        assert!(!out.contains("DB_HOST=db.example.com"));
        assert!(!out.contains("flag-a"));
    }

    #[test]
    fn extracts_ingress_hosts() {
        let out = compact(HELM_SAMPLE);
        assert!(out.contains("hosts: [example.com, api.example.com]"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a k8s manifest";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_helm_output() {
        // The hand-written HELM_SAMPLE is tight (no boilerplate `selector`,
        // `labels`, `annotations`); on a real `helm template` of a
        // production chart the per-doc spec body is far larger, so this
        // 65 % floor is a conservative lower bound.
        let (out, sav) = compact_with_savings(HELM_SAMPLE);
        assert!(out.contains("4 resource(s)"));
        assert!(sav.percent() > 65.0, "got {sav:?}");
    }

    #[test]
    fn handles_50_resource_manifest() {
        // Synthesise a moderate chart with 50 resources.
        let mut s = String::new();
        for i in 0..50 {
            s.push_str("---\n");
            let _ = write!(
                s,
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app-{i}\n  namespace: ns-{}\nspec:\n  replicas: {}\n  template:\n    spec:\n      containers:\n        - name: c\n          image: registry/app-{i}:v1.{i}\n",
                i % 5,
                (i % 10) + 1
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("50 document(s), 50 resource(s)"));
        // Spot check first + last.
        assert!(out.contains("registry/app-0:v1.0"));
        assert!(out.contains("registry/app-49:v1.49"));
        // Synth fixture has minimal spec body; bench fixture exercises a
        // more realistic compaction ratio (>80 %).
        assert!(sav.percent() > 50.0, "got {sav:?}");
    }
}
