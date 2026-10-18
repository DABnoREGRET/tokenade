//! `kubectl` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_kubectl_pod(n_events: usize, n_warnings: usize) -> String {
    // Realistic shape: real `kubectl describe pod` output is dominated by
    // the `last-applied-configuration` JSON annotation (often 1-5 KB) plus
    // Environment / Mounts / Volumes blocks for each container.
    // Real last-applied-configuration JSON for a pod with multiple containers
    // and ConfigMap/Secret references is routinely 5-10 KB. Use 8 KB here.
    let last_applied = "x".repeat(8000);
    let mut s = format!(
        "Name:           my-pod\nNamespace:      default\nPriority:       0\nNode:           node-1/10.0.0.1\nStart Time:     Mon, 15 Jan 2024 10:30:00 +0000\nLabels:         app=my-app\n                version=v1\n                tier=backend\n                environment=production\nAnnotations:    deployment.kubernetes.io/revision: 5\n                kubectl.kubernetes.io/last-applied-configuration: |\n                  {{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{{\"name\":\"my-pod\"}},\"data\":\"{last_applied}\"}}\n                prometheus.io/scrape: true\n                prometheus.io/port: 8080\nStatus:         Running\nIP:             10.244.0.5\nIPs:\n  IP:           10.244.0.5\nControlled By:  ReplicaSet/my-app-7d9f\nContainers:\n  app:\n    Container ID:   docker://abc123def456789abcdef0123456789\n    Image:          myapp:v1.0\n    Image ID:       docker-pullable://myapp@sha256:abc123def456789abcdef0123456789abc123def456789abcdef0123456789\n    Port:           8080/TCP\n    Host Port:      0/TCP\n    State:          Running\n      Started:      Mon, 15 Jan 2024 10:30:15 +0000\n    Last State:     Terminated\n      Reason:       Error\n      Exit Code:    137\n      Started:      Mon, 15 Jan 2024 10:25:00 +0000\n      Finished:     Mon, 15 Jan 2024 10:29:55 +0000\n    Ready:          True\n    Restart Count:  3\n    Limits:\n      cpu:     500m\n      memory:  512Mi\n    Requests:\n      cpu:     250m\n      memory:  256Mi\n    Liveness:   http-get http://:8080/health delay=10s timeout=1s\n    Environment:\n      DB_HOST:           db.example.com\n      DB_PORT:           5432\n      API_KEY:           <set to the key 'api-key' in secret 'app-secrets'>\n      LOG_LEVEL:         info\n      FEATURE_FLAG_A:    true\n      FEATURE_FLAG_B:    false\n      CACHE_HOST:        cache.example.com\n      CACHE_PORT:        6379\n    Mounts:\n      /etc/config from app-config (ro)\n      /var/secrets from app-secrets (ro)\n      /var/run/secrets/kubernetes.io/serviceaccount from default-token-xxx (ro)\nConditions:\n  Type             Status\n  Initialized      True\n  Ready            True\n  ContainersReady  True\n  PodScheduled     True\nVolumes:\n  app-config:\n    Type:       ConfigMap\n    Name:       my-config\n    Optional:   false\n  app-secrets:\n    Type:        Secret\n    SecretName:  app-secrets\n    Optional:    false\n  default-token-xxx:\n    Type:        Secret\n    SecretName:  default-token-xxx\n    Optional:    false\nQoS Class:                   Burstable\nNode-Selectors:              <none>\nTolerations:                 node.kubernetes.io/not-ready:NoExecute for 300s\n                             node.kubernetes.io/unreachable:NoExecute for 300s\nEvents:\n  Type     Reason     Age    From               Message\n  ----     ------     ----   ----               -------\n"
    );
    for i in 0..n_warnings {
        s.push_str(&format!(
            "  Warning  Unhealthy  {i}m     kubelet            Liveness probe failed: HTTP probe failed with statuscode: 500\n"
        ));
    }
    let n_normal = n_events.saturating_sub(n_warnings);
    for i in 0..n_normal {
        s.push_str(&format!(
            "  Normal   Created    {i}m     kubelet            Created container app-{i}\n"
        ));
    }
    s
}

pub(crate) fn task_kubectl_real(results: &mut Vec<RunResult>) {
    // 30 events + 5 warnings is realistic for a pod that has been restarting
    // a few times (the kind of pod an agent debugs).
    let raw = synth_kubectl_pod(30, 5);
    let (out, sav, ms) = measure(&raw, kubectl::compact_with_savings);
    let quality = if out.contains("Name=my-pod")
        && out.contains("Namespace=default")
        && out.contains("Status=Running")
        && out.contains("QoS=Burstable")
        && out.contains("app: Running")
        && out.contains("restarts: 3")
        && out.contains("last state: Terminated")
        && out.contains("reason=Error")
        && out.contains("exit=137")
        && out.contains("Initialized=True")
        && out.contains("Liveness probe failed")
        && !out.contains("DB_HOST")
        && !out.contains("default-token")
        && !out.contains("last-applied-configuration")
        && sav.percent() > 50.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:kubectl-compact",
        "kubectl",
        "synth-pod-30ev-5warn",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #55: 1 000-event pod description (real flapping/restarting
// pods can hit this). Must compact >90% in under 1 s and KEEP every Warning.
pub(crate) fn adv_kubectl_huge_events(results: &mut Vec<RunResult>) {
    let raw = synth_kubectl_pod(1_000, 50);
    let (out, sav, ms) = measure(&raw, kubectl::compact_with_savings);
    let quality = if out.contains("events (1000 total: 50 Warning")
        && out.contains("Liveness probe failed")
        // Last warning must be there (we keep all warnings up to cap 50).
        && sav.percent() > 90.0
        && ms < 1000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:kubectl:huge-1k-events",
        "kubectl",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #56: hostile content — the Annotations block contains JSON
// with text like "Events:", "Status:", "Containers:" embedded inside.
// The compactor MUST NOT treat those as section headers (they're indented,
// so our top-level-only detection should reject them).
pub(crate) fn adv_kubectl_hostile(results: &mut Vec<RunResult>) {
    let raw = r#"Name:           sneaky-pod
Namespace:      default
Annotations:    foo: |
                  {"FakeEvents:": "Type Reason Age",
                   "FakeContainers:": "should not be picked up",
                   "FakeStatus:": "Running",
                   "QoS Class:": "MUST NOT MATCH AT COL 0"}
                bar: |
                  Containers:
                    fake-app:
                      State: Running
                      Image: fake-image:v1
Status:         Running
Node:           node-2/10.0.0.2
IP:             10.244.0.99
Controlled By:  StatefulSet/real-app
Containers:
  real-app:
    Image:          real-image:v2
    State:          Running
    Ready:          True
    Restart Count:  0
Conditions:
  Type             Status
  Ready            True
QoS Class:        BestEffort
Node-Selectors:   <none>
Events:
  Type     Reason     Age   From      Message
  ----     ------     ----  ----      -------
  Warning  RealAlarm  1m    kubelet   This is a real warning
"#;
    let (out, sav, ms) = measure(raw, kubectl::compact_with_savings);
    // Must classify as kubectl describe, extract REAL container/status/events,
    // and NOT leak fake annotation content.
    let quality = if out.contains("Name=sneaky-pod")
        && out.contains("Status=Running")
        && out.contains("Controlled By=StatefulSet/real-app")
        && out.contains("real-app: Running")
        && out.contains("image=real-image:v2")
        && out.contains("This is a real warning")
        && out.contains("QoS=BestEffort")
        // Critical: fake content must NOT leak.
        && !out.contains("fake-app")
        && !out.contains("FakeEvents")
        && !out.contains("FakeContainers")
        && !out.contains("fake-image")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:kubectl:hostile",
        "kubectl",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
