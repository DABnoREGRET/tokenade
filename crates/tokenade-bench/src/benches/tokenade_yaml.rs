//! `tokenade_yaml` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Build a realistic kubectl-style Deployment manifest with a heavy
/// `managedFields` tree + status conditions — what `kubectl get -o yaml`
/// actually returns from a real cluster.
pub(crate) fn synth_kubectl_yaml() -> String {
    let mut y = String::from(
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: super-app\n  namespace: prod\n  uid: 11111111-2222-3333-4444-555555555555\n  resourceVersion: \"98765432\"\n  generation: 7\n  creationTimestamp: \"2024-06-01T12:00:00Z\"\n  labels:\n    app: super-app\n    tier: backend\n  annotations:\n    kubectl.kubernetes.io/last-applied-configuration: |\n      {\"apiVersion\":\"apps/v1\",\"kind\":\"Deployment\",\"metadata\":{\"annotations\":{},\"name\":\"super-app\",\"namespace\":\"prod\"},\"spec\":{\"replicas\":3,\"selector\":{\"matchLabels\":{\"app\":\"super-app\"}},\"template\":{\"metadata\":{\"labels\":{\"app\":\"super-app\"}},\"spec\":{\"containers\":[{\"image\":\"super-app:v1.2.3\",\"name\":\"app\"}]}}}}\n  managedFields:\n",
    );
    for i in 0..50 {
        y.push_str(&format!(
            "    - manager: kubectl-client-side-apply\n      operation: Update\n      apiVersion: apps/v1\n      time: \"2024-06-01T12:00:{:02}Z\"\n      fieldsType: FieldsV1\n      fieldsV1:\n        f:metadata:\n          f:annotations:\n            .: {{}}\n            f:kubectl.kubernetes.io/last-applied-configuration: {{}}\n        f:spec:\n          f:replicas: {{}}\n          f:selector: {{}}\n          f:strategy:\n            f:rollingUpdate:\n              .: {{}}\n              f:maxSurge: {{}}\n              f:maxUnavailable: {{}}\n            f:type: {{}}\n          f:template:\n            f:metadata:\n              f:labels:\n                .: {{}}\n                f:app: {{}}\n            f:spec:\n              f:containers:\n                k:{{\"name\":\"app\"}}:\n                  .: {{}}\n                  f:image: {{}}\n                  f:name: {{}}\n                  f:resources: {{}}\n                  f:terminationMessagePath: {{}}\n                  f:terminationMessagePolicy: {{}}\n",
            i % 60
        ));
    }
    y.push_str("spec:\n  replicas: 3\n  selector:\n    matchLabels:\n      app: super-app\n  template:\n    metadata:\n      labels:\n        app: super-app\n    spec:\n      containers:\n        - name: app\n          image: super-app:v1.2.3\n          ports:\n            - containerPort: 8080\n              protocol: TCP\n          resources:\n            limits:\n              cpu: 500m\n              memory: 512Mi\n            requests:\n              cpu: 100m\n              memory: 128Mi\nstatus:\n  observedGeneration: 7\n  replicas: 3\n  updatedReplicas: 3\n  readyReplicas: 3\n  availableReplicas: 3\n  conditions:\n    - type: Available\n      status: \"True\"\n      lastUpdateTime: \"2024-06-01T12:01:00Z\"\n      lastTransitionTime: \"2024-06-01T12:01:00Z\"\n      reason: MinimumReplicasAvailable\n      message: Deployment has minimum availability.\n    - type: Progressing\n      status: \"True\"\n      lastUpdateTime: \"2024-06-01T12:01:00Z\"\n      lastTransitionTime: \"2024-06-01T12:00:30Z\"\n      reason: NewReplicaSetAvailable\n      message: ReplicaSet \"super-app-7d4b8c\" has successfully progressed.\n");
    y
}

pub(crate) fn task_yaml_kubectl(results: &mut Vec<RunResult>) {
    let raw = synth_kubectl_yaml();
    let (out, sav, ms) = measure(&raw, tokenade_yaml::compact_with_savings);
    // Quality: signal preserved (name, namespace, image, replica counts) AND
    // noise dropped (managedFields, resourceVersion, last-applied-config blob).
    let signal_kept = out.contains("name: super-app")
        && out.contains("namespace: prod")
        && out.contains("image: super-app:v1.2.3")
        && out.contains("availableReplicas: 3");
    let noise_dropped = out.contains("managedFields: <dropped>")
        && !out.contains("kubectl-client-side-apply")
        && !out.contains("98765432");
    let quality = if signal_kept && noise_dropped {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:yaml-compact",
        "yaml-compact",
        "kubectl-deployment",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #15: a 10 000-entry managedFields tree (real clusters can
// accumulate this with many controllers). Must compress > 90 % AND complete
// in < 1 second AND keep every spec/status field intact.
pub(crate) fn adv_yaml_managed_fields_bomb(results: &mut Vec<RunResult>) {
    let mut y = String::from(
        "apiVersion: v1\nkind: Pod\nmetadata:\n  name: bomb-pod\n  namespace: chaos\n  managedFields:\n",
    );
    for i in 0..10_000 {
        y.push_str(&format!(
            "    - manager: ctrl-{i}\n      operation: Update\n      time: \"2024-01-01T00:00:00Z\"\n      fieldsType: FieldsV1\n      fieldsV1:\n        f:spec:\n          f:containers: {{}}\n"
        ));
    }
    y.push_str("spec:\n  containers:\n    - name: app\n      image: my-app:latest\nstatus:\n  phase: Running\n");
    let (out, sav, ms) = measure(&y, tokenade_yaml::compact_with_savings);
    let signal = out.contains("name: app")
        && out.contains("image: my-app:latest")
        && out.contains("phase: Running")
        && out.contains("name: bomb-pod");
    let compressed = sav.percent() > 90.0;
    let fast = ms < 1000;
    let quality = match (signal, compressed, fast) {
        (true, true, true) => 1.0,
        (true, _, _) => 0.5,
        _ => 0.0,
    };
    record(
        results,
        "adv:yaml:managed-fields-bomb",
        "yaml-compact",
        "adversarial",
        &y,
        &out,
        ms,
        quality,
    );
}

// Adversarial #16: pathological indentation — mixed tabs/spaces, irregular
// step sizes. Must not panic; must keep at least the visible scalar values.
pub(crate) fn adv_yaml_pathological_indent(results: &mut Vec<RunResult>) {
    let y = "root:\n\t key1: value-one\n   key2: value-two\n      \tkey3: value-three\nmanagedFields:\n  - garbage\n  - more\nname: keep-me\n";
    let (out, _sav, ms) = measure(y, tokenade_yaml::compact_with_savings);
    let quality = if out.contains("keep-me") && out.contains("managedFields: <dropped>") {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:yaml:pathological-indent",
        "yaml-compact",
        "adversarial",
        y,
        &out,
        ms,
        quality,
    );
}
