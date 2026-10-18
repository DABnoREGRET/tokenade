//! `k8smanifest` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_k8s_chart(
    n_deploys: usize,
    n_configmaps: usize,
    configmap_size: usize,
) -> String {
    let mut s = String::new();
    for i in 0..n_deploys {
        s.push_str("---\n");
        s.push_str(&format!(
            "# Source: chart/templates/deployment-{i}.yaml\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app-{i}\n  namespace: default\n  labels:\n    app: app-{i}\n    chart: my-chart-1.0.0\n    release: my-release\n    heritage: Helm\n  annotations:\n    deployment.kubernetes.io/revision: \"1\"\n    meta.helm.sh/release-name: my-release\nspec:\n  replicas: 3\n  selector:\n    matchLabels:\n      app: app-{i}\n  strategy:\n    type: RollingUpdate\n    rollingUpdate:\n      maxSurge: 1\n      maxUnavailable: 0\n  template:\n    metadata:\n      labels:\n        app: app-{i}\n        version: v1.0.0\n      annotations:\n        prometheus.io/scrape: \"true\"\n        prometheus.io/port: \"8080\"\n    spec:\n      serviceAccountName: app-{i}\n      securityContext:\n        runAsUser: 1000\n        fsGroup: 2000\n      containers:\n        - name: main\n          image: registry.example.com/app-{i}:v1.0.{i}\n          imagePullPolicy: IfNotPresent\n          ports:\n            - name: http\n              containerPort: 8080\n              protocol: TCP\n          env:\n            - name: SERVICE_NAME\n              value: app-{i}\n            - name: LOG_LEVEL\n              value: info\n            - name: DB_HOST\n              valueFrom:\n                secretKeyRef:\n                  name: db-secret\n                  key: host\n          resources:\n            limits:\n              cpu: 500m\n              memory: 512Mi\n            requests:\n              cpu: 100m\n              memory: 256Mi\n          livenessProbe:\n            httpGet:\n              path: /health\n              port: http\n            initialDelaySeconds: 10\n            periodSeconds: 5\n          readinessProbe:\n            httpGet:\n              path: /ready\n              port: http\n        - name: sidecar\n          image: envoyproxy/envoy:v1.27.0\n          ports:\n            - containerPort: 9901\n              protocol: TCP\n"
        ));
    }
    let body = "abcdefghij".repeat(configmap_size / 10);
    for i in 0..n_configmaps {
        s.push_str("---\n");
        s.push_str(&format!(
            "# Source: chart/templates/configmap-{i}.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm-{i}\n  namespace: default\ndata:\n  config.yaml: |\n    {body}\n  secrets.env: |\n    {body}\n  schema.json: |\n    {body}\n"
        ));
    }
    s
}

pub(crate) fn task_k8smanifest_real(results: &mut Vec<RunResult>) {
    // 6 deployments + 4 configmaps × 1 KB each = realistic small chart.
    let raw = synth_k8s_chart(6, 4, 1_000);
    let (out, sav, ms) = measure(&raw, k8smanifest::compact_with_savings);
    let quality = if out.contains("10 document(s), 10 resource(s)")
        && out.contains("apps/v1 Deployment")
        && out.contains("v1 ConfigMap")
        && out.contains("default/app-0")
        && out.contains("default/app-5")
        && out.contains("default/cm-0")
        && out.contains("default/cm-3")
        && out.contains("replicas: 3")
        && out.contains("registry.example.com/app-0:v1.0.0")
        && out.contains("envoyproxy/envoy:v1.27.0")
        && out.contains("data keys")
        && !out.contains("abcdefghij")  // configmap values not leaked
        && !out.contains("prometheus.io/scrape") // annotations dropped
        && !out.contains("livenessProbe") // verbose spec dropped
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:k8smanifest-compact",
        "k8smanifest",
        "synth-6deploy-4cm",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #63: large helm-template output — 100 deployments + 50
// configmaps × 5 KB each (300 KB-ish total). >95% savings, <1s.
pub(crate) fn adv_k8smanifest_huge(results: &mut Vec<RunResult>) {
    let raw = synth_k8s_chart(100, 50, 5_000);
    let (out, sav, ms) = measure(&raw, k8smanifest::compact_with_savings);
    let quality =
        if out.contains("150 document(s), 150 resource(s)") && sav.percent() > 95.0 && ms < 1000 {
            1.0
        } else {
            0.0
        };
    record(
        results,
        "adv:k8smanifest:huge-helm-150",
        "k8smanifest",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #64: hostile YAML with several traps:
//   - a ConfigMap whose `data:` block contains a NESTED YAML manifest as the
//     value (apiVersion + kind appearing in a quoted multi-line literal) —
//     must NOT be parsed as a separate resource
//   - non-ASCII names (`Müller`) and Unicode in image references
//   - a `kind:` value as a string (e.g. `description: "Resource kind: Pod"`)
//   - missing `---` separator between docs (some `kustomize` outputs)
//   - empty document `---\n---\n`
pub(crate) fn adv_k8smanifest_hostile(results: &mut Vec<RunResult>) {
    let raw = r#"---
apiVersion: v1
kind: ConfigMap
metadata:
  name: tricky-cm
  namespace: Müller
data:
  embedded.yaml: |
    apiVersion: apps/v1
    kind: Deployment
    metadata:
      name: should-NOT-be-extracted
    spec:
      replicas: 99
  description: |
    Resource kind: Pod (in a value, not a key)
    apiVersion is mentioned here too but not as a key
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: real-deploy
  namespace: default
spec:
  replicas: 5
  template:
    spec:
      containers:
        - name: app
          image: ghcr.io/münchen/myapp:v1.0
---
---
apiVersion: v1
kind: Service
metadata:
  name: real-service
  namespace: default
spec:
  type: LoadBalancer
  ports:
    - port: 443
"#;
    let (out, sav, ms) = measure(raw, k8smanifest::compact_with_savings);
    let quality = if out.contains("ConfigMap")
        && out.contains("tricky-cm")
        && out.contains("Müller/tricky-cm")
        && out.contains("Deployment")
        && out.contains("default/real-deploy")
        && out.contains("replicas: 5")
        && out.contains("ghcr.io/münchen/myapp:v1.0")
        && out.contains("Service")
        && out.contains("default/real-service")
        && out.contains("type: LoadBalancer")
        && out.contains("ports: [443]")
        // CRITICAL: the embedded "Deployment" inside the ConfigMap data MUST
        // NOT have been extracted as a separate resource. We accept that the
        // current parser may pick up the embedded image lines (no way to
        // distinguish without a real YAML parser), but the resource COUNT
        // must be 3, not 4.
        && out.contains("3 resource(s)")
        && !out.contains("should-NOT-be-extracted")
        && !out.contains("replicas: 99")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:k8smanifest:hostile",
        "k8smanifest",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
