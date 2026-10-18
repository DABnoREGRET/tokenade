//! `kubectl describe` block-format output compaction.
//!
//! `kubectl describe pod/deployment/service/node/configmap/…` emits a
//! key:value + indented-section block format that runs 200 – 800 lines on
//! real workloads. Most of those lines are noise the agent doesn't need:
//!
//!   * `Annotations:` — multi-line JSON dump of the last-applied config
//!   * `Tolerations:` — long list of node-condition tolerations
//!   * `Volumes:` — verbose mount-path detail
//!   * `Mounts:` and `Environment:` — per-container secret references
//!   * the `Events:` table itself, when there are 100+ scheduler events
//!
//! What the agent reading this actually wants is:
//!
//!   - resource identity (name, namespace, kind, controlled-by)
//!   - current status + IP + node placement
//!   - container summary (image, state, restart count, last-state reason)
//!   - the `Conditions` row
//!   - every `Warning` event + the most recent `Normal` events
//!   - `QoS Class`
//!
//! Real-world ratio: ≥80 % on a healthy describe, much higher on long
//! event streams.

use std::fmt::Write;

pub const EVENT_NORMAL_CAP: usize = 5;
pub const EVENT_WARNING_CAP: usize = 50;
pub const CONTAINER_CAP: usize = 20;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);

    // Strong signal: the kubectl Events table header, with either of the
    // common kubectl column widths.
    // The Events table header, ANCHORED at the start of its line. It was
    // three `contains` over the whole head until 2026-08-26, and a kubectl
    // sample QUOTED inside source code carries it just as well as kubectl
    // does: this file's own `POD_SAMPLE` is one escaped `\n`-joined string,
    // so the header sits mid-line, and `Pare/…/parsers.ts` has it inside a
    // JSDoc block behind ` * `. Both files were claimed, and the compactor
    // returned the EMPTY STRING for each. Real output puts the header on its
    // own line, indented at most by the table's own two spaces.
    if head.lines().any(|l| {
        let t = l.trim_start();
        if t.starts_with('*') || t.starts_with("//") || t.starts_with("/*") {
            return false;
        }
        t.starts_with("Type") && t[4..].starts_with("   ") && t.contains("Reason")
    }) {
        return true;
    }

    // K8s-distinctive keys that don't appear in generic key:value text.
    let k8s_specific: &[&str] = &[
        "QoS Class:",
        "Node-Selectors:",
        "Controlled By:",
        "Service Account:",
        "Containers:",
        "PodScheduled",
        "ContainersReady",
    ];
    let has_k8s_specific = head.lines().any(|l| {
        let t = l.trim_start();
        k8s_specific.iter().any(|p| t.starts_with(p))
    });

    // Generic structure check: ≥3 distinct top-level Key: lines.
    let key_patterns: &[&str] = &[
        "Name:",
        "Namespace:",
        "Status:",
        "Events:",
        "Conditions:",
        "Containers:",
        "Volumes:",
        "Node:",
    ];
    has_k8s_specific
        && head
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                key_patterns.iter().any(|p| t.starts_with(p))
            })
            .count()
            >= 3
}

#[derive(Default)]
struct Container {
    name: String,
    image: String,
    state: String,
    state_reason: Option<String>,
    state_exit_code: Option<String>,
    last_state: Option<String>,
    last_state_reason: Option<String>,
    last_state_exit_code: Option<String>,
    ready: String,
    restart_count: String,
}

#[derive(Default)]
struct EventRow {
    kind: String,
    reason: String,
    age: String,
    from: String,
    message: String,
}

/// Walk the `kubectl describe` body once and collect what the summary needs:
/// the containers, the conditions table and the events table.
///
/// Split from `compact`, which held a 183-line parser and a 114-line renderer in
/// one body — two jobs, and only the second one decides what the agent sees.
fn parse_described(text: &str) -> (Vec<Container>, Vec<(String, String)>, Vec<EventRow>) {
    #[derive(PartialEq, Eq)]
    enum Section {
        None,
        Containers,
        Conditions,
        Events,
        Skip,
    }
    let mut section = Section::None;
    let mut containers: Vec<Container> = Vec::new();
    let mut current: Option<Container> = None;
    let mut in_state_block = false;
    let mut in_last_state_block = false;
    let mut conditions: Vec<(String, String)> = Vec::new();
    let mut conditions_header_seen = false;
    let mut events: Vec<EventRow> = Vec::new();
    let mut events_header_seen = false;

    for line in text.lines() {
        let line_no_cr = line.trim_end_matches('\r');

        // A top-level new section header is any line that starts at column 0
        // and ends with `:` (no value on the same line) OR has its value
        // empty/trailing-only.
        let is_top_level = !line_no_cr.is_empty() && !line_no_cr.starts_with(' ');

        if is_top_level {
            // Flush in-progress container.
            if let Some(c) = current.take() {
                containers.push(c);
            }
            in_state_block = false;
            in_last_state_block = false;

            // Detect section change.
            section = if line_no_cr.starts_with("Containers:") {
                Section::Containers
            } else if line_no_cr.starts_with("Conditions:") {
                conditions_header_seen = false;
                Section::Conditions
            } else if line_no_cr.starts_with("Events:") {
                events_header_seen = false;
                Section::Events
            } else if line_no_cr.starts_with("Annotations:")
                || line_no_cr.starts_with("Tolerations:")
                || line_no_cr.starts_with("Volumes:")
                || line_no_cr.starts_with("Mounts:")
            {
                Section::Skip
            } else {
                Section::None
            };
            continue;
        }

        match section {
            Section::Containers => {
                // Container-name line: exactly 2-space indent + name + ':' at end.
                // Properties: 4-space+ indent.
                if line_no_cr.starts_with("  ") && !line_no_cr.starts_with("   ") {
                    let t = line_no_cr.trim();
                    if let Some(name) = t.strip_suffix(':') {
                        // Save previous container.
                        if let Some(c) = current.take() {
                            containers.push(c);
                        }
                        current = Some(Container {
                            name: name.to_string(),
                            ..Container::default()
                        });
                        in_state_block = false;
                        in_last_state_block = false;
                        continue;
                    }
                }
                let Some(c) = current.as_mut() else {
                    continue;
                };
                let t = line_no_cr.trim_start();
                if let Some(v) = t.strip_prefix("Image:") {
                    c.image = v.trim().to_string();
                    in_state_block = false;
                    in_last_state_block = false;
                } else if let Some(v) = t.strip_prefix("State:") {
                    c.state = v.trim().to_string();
                    in_state_block = true;
                    in_last_state_block = false;
                } else if let Some(v) = t.strip_prefix("Last State:") {
                    c.last_state = Some(v.trim().to_string());
                    in_state_block = false;
                    in_last_state_block = true;
                } else if let Some(v) = t.strip_prefix("Ready:") {
                    c.ready = v.trim().to_string();
                    in_state_block = false;
                    in_last_state_block = false;
                } else if let Some(v) = t.strip_prefix("Restart Count:") {
                    c.restart_count = v.trim().to_string();
                    in_state_block = false;
                    in_last_state_block = false;
                } else if in_state_block || in_last_state_block {
                    if let Some(v) = t.strip_prefix("Reason:") {
                        if in_state_block && c.state_reason.is_none() {
                            c.state_reason = Some(v.trim().to_string());
                        } else if in_last_state_block && c.last_state_reason.is_none() {
                            c.last_state_reason = Some(v.trim().to_string());
                        }
                    } else if let Some(v) = t.strip_prefix("Exit Code:") {
                        if in_state_block && c.state_exit_code.is_none() {
                            c.state_exit_code = Some(v.trim().to_string());
                        } else if in_last_state_block && c.last_state_exit_code.is_none() {
                            c.last_state_exit_code = Some(v.trim().to_string());
                        }
                    }
                }
            }
            Section::Conditions => {
                let t = line_no_cr.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("Type") && t.contains("Status") {
                    conditions_header_seen = true;
                    continue;
                }
                if !conditions_header_seen {
                    continue;
                }
                // Each row: `Type    Status` with multi-space gap.
                let parts: Vec<&str> = t.split_whitespace().collect();
                if parts.len() >= 2 {
                    conditions.push((parts[0].to_string(), parts[1].to_string()));
                }
            }
            Section::Events => {
                let t = line_no_cr.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("Type") && t.contains("Reason") {
                    events_header_seen = true;
                    continue;
                }
                if t.starts_with("----") {
                    continue;
                }
                if !events_header_seen {
                    continue;
                }
                // Split on double-space to respect the table's variable widths.
                let parts: Vec<&str> = t
                    .split("  ")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() < 4 {
                    continue;
                }
                // parts: [type, reason, age, from, message...]
                let kind = parts[0].to_string();
                let reason = parts[1].to_string();
                let age = parts[2].to_string();
                let from = parts[3].to_string();
                let message = if parts.len() > 4 {
                    parts[4..].join(" ")
                } else {
                    String::new()
                };
                events.push(EventRow {
                    kind,
                    reason,
                    age,
                    from,
                    message,
                });
            }
            _ => {}
        }
    }
    if let Some(c) = current.take() {
        containers.push(c);
    }

    // ---- Render ----
    (containers, conditions, events)
}

/// Render the collected pieces as the compact summary.
fn render_described(
    top: &[(&'static str, String)],
    labels_inline: Option<&str>,
    containers: &[Container],
    conditions: &[(String, String)],
    events: &[EventRow],
) -> String {
    let mut out = String::with_capacity(2048);

    // Header line: pack the most important top-level fields onto one line.
    for (k, v) in top {
        let _ = write!(out, " {k}={v},");
    }
    if out.ends_with(',') {
        out.pop();
    }
    out.push('\n');
    if let Some(l) = &labels_inline {
        let one = l.lines().next().unwrap_or("").trim();
        if !one.is_empty() {
            let _ = writeln!(out, "labels: {one}");
        }
    }

    if !containers.is_empty() {
        let _ = write!(out, "\ncontainers ({}):\n", containers.len());
        for c in containers.iter().take(CONTAINER_CAP) {
            let restarts = if c.restart_count.is_empty() {
                String::new()
            } else {
                format!(", restarts: {}", c.restart_count)
            };
            let ready = if c.ready.is_empty() {
                String::new()
            } else {
                format!(", ready: {}", c.ready)
            };
            let image = if c.image.is_empty() {
                String::new()
            } else {
                format!(", image={}", c.image)
            };
            let _ = writeln!(
                out,
                "  {}: {}{}{}{}",
                c.name, c.state, restarts, ready, image
            );
            // Per-state diagnostics.
            if c.state_reason.is_some() || c.state_exit_code.is_some() {
                let r = c.state_reason.as_deref().unwrap_or("");
                let e = c.state_exit_code.as_deref().unwrap_or("");
                let _ = writeln!(out, "    state: reason={r}, exit={e}");
            }
            if c.last_state.is_some()
                || c.last_state_reason.is_some()
                || c.last_state_exit_code.is_some()
            {
                let s = c.last_state.as_deref().unwrap_or("");
                let r = c.last_state_reason.as_deref().unwrap_or("");
                let e = c.last_state_exit_code.as_deref().unwrap_or("");
                let _ = writeln!(out, "    last state: {s}, reason={r}, exit={e}");
            }
        }
        if containers.len() > CONTAINER_CAP {
            let _ = writeln!(
                out,
                "  … {} more container(s) elided",
                containers.len() - CONTAINER_CAP
            );
        }
    }

    if !conditions.is_empty() {
        let s: Vec<String> = conditions.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = write!(out, "\nconditions: {}\n", s.join(", "));
    }

    if !events.is_empty() {
        let n_warning = events.iter().filter(|e| e.kind == "Warning").count();
        let n_normal = events.iter().filter(|e| e.kind == "Normal").count();
        let _ = write!(
            out,
            "\nevents ({} total: {} Warning, {} Normal):\n",
            events.len(),
            n_warning,
            n_normal
        );
        // Warnings first, then most-recent Normals (last in input).
        let warnings: Vec<&EventRow> = events
            .iter()
            .filter(|e| e.kind == "Warning")
            .take(EVENT_WARNING_CAP)
            .collect();
        for e in &warnings {
            let _ = writeln!(
                out,
                "  {:<8}{:<12}{:<10}{:<25}{}",
                e.kind, e.reason, e.age, e.from, e.message
            );
        }
        // Most-recent Normal events are typically last in the input array.
        let normals: Vec<&EventRow> = events
            .iter()
            .filter(|e| e.kind == "Normal")
            .rev()
            .take(EVENT_NORMAL_CAP)
            .collect();
        for e in normals.iter().rev() {
            let _ = writeln!(
                out,
                "  {:<8}{:<12}{:<10}{:<25}{}",
                e.kind, e.reason, e.age, e.from, e.message
            );
        }
        let elided = events.len() - warnings.len() - normals.len();
        if elided > 0 {
            let _ = writeln!(out, "  … {elided} more Normal event(s) elided");
        }
    }

    out.trim_end().to_string()
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // L42 — strip ANSI before parsing. `kubectl describe` colourises
    // status fields (Ready=green, NotReady=red) and event severity.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    // ---- Pass 1: top-level fields (column 0, `Key: value`) ----
    let mut top: Vec<(&'static str, String)> = Vec::new();
    let push_top = |key: &'static str, v: String, dest: &mut Vec<(&'static str, String)>| {
        if !v.is_empty() && !dest.iter().any(|(k, _)| *k == key) {
            dest.push((key, v));
        }
    };
    // Extract a `Key: value` line whose key is at column 0 (no leading
    // space) — that's how kubectl emits top-level fields. Avoids false-
    // matching `Started:` inside a container state block.
    let extract_top = |line: &str, prefix: &str| -> Option<String> {
        if !line.starts_with(prefix) {
            return None;
        }
        let after = &line[prefix.len()..];
        Some(after.trim().to_string())
    };
    let mut labels_inline: Option<String> = None;
    for line in text.lines() {
        if let Some(v) = extract_top(line, "Name:") {
            push_top("Name", v, &mut top);
        } else if let Some(v) = extract_top(line, "Namespace:") {
            push_top("Namespace", v, &mut top);
        } else if let Some(v) = extract_top(line, "Kind:") {
            push_top("Kind", v, &mut top);
        } else if let Some(v) = extract_top(line, "Status:") {
            push_top("Status", v, &mut top);
        } else if let Some(v) = extract_top(line, "Node:") {
            push_top("Node", v, &mut top);
        } else if let Some(v) = extract_top(line, "IP:") {
            push_top("IP", v, &mut top);
        } else if let Some(v) = extract_top(line, "Controlled By:") {
            push_top("Controlled By", v, &mut top);
        } else if let Some(v) = extract_top(line, "QoS Class:") {
            push_top("QoS", v, &mut top);
        } else if let Some(v) = extract_top(line, "Priority:") {
            push_top("Priority", v, &mut top);
        } else if let Some(v) = extract_top(line, "Start Time:") {
            push_top("Start Time", v, &mut top);
        } else if let Some(v) = extract_top(line, "Labels:") {
            // Labels can be multi-line; we keep just the first inline value.
            if !v.is_empty() {
                labels_inline = Some(v);
            }
        }
    }

    // ---- Pass 2: section-aware parsing ----
    let (containers, conditions, events) = parse_described(text);
    render_described(
        &top,
        labels_inline.as_deref(),
        &containers,
        &conditions,
        &events,
    )
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POD_SAMPLE: &str = "Name:           my-pod-abc123\nNamespace:      default\nPriority:       0\nNode:           node-1/10.0.0.1\nStart Time:     Mon, 15 Jan 2024 10:30:00 +0000\nLabels:         app=my-app\n                version=v1\nAnnotations:    kubectl.kubernetes.io/last-applied-configuration: |\n                  {\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{...}}\n                deployment.kubernetes.io/revision: 5\nStatus:         Running\nIP:             10.244.0.5\nIPs:\n  IP:           10.244.0.5\nControlled By:  ReplicaSet/my-app-7d9f\nContainers:\n  app:\n    Container ID:   docker://abc123def\n    Image:          myapp:v1.0\n    Image ID:       docker-pullable://myapp@sha256:abc\n    Port:           8080/TCP\n    Host Port:      0/TCP\n    State:          Running\n      Started:      Mon, 15 Jan 2024 10:30:15 +0000\n    Last State:     Terminated\n      Reason:       Error\n      Exit Code:    137\n      Started:      Mon, 15 Jan 2024 10:25:00 +0000\n      Finished:     Mon, 15 Jan 2024 10:29:55 +0000\n    Ready:          True\n    Restart Count:  3\n    Limits:\n      cpu:     500m\n      memory:  512Mi\n    Requests:\n      cpu:     250m\n      memory:  256Mi\n    Liveness:   http-get http://:8080/health delay=10s timeout=1s\n    Environment:\n      DB_HOST:  db.example.com\n      DB_PORT:  5432\n      API_KEY:  <set to the key 'api-key' in secret 'app-secrets'>\n    Mounts:\n      /etc/config from app-config (ro)\n      /var/run/secrets/kubernetes.io/serviceaccount from default-token-xxx (ro)\nConditions:\n  Type             Status\n  Initialized      True\n  Ready            True\n  ContainersReady  True\n  PodScheduled     True\nVolumes:\n  app-config:\n    Type:       ConfigMap\n    Name:       my-config\n    Optional:   false\n  default-token-xxx:\n    Type:       Secret\n    SecretName: default-token-xxx\nQoS Class:                   Burstable\nNode-Selectors:              <none>\nTolerations:                 node.kubernetes.io/not-ready:NoExecute for 300s\nEvents:\n  Type     Reason     Age    From               Message\n  ----     ------     ----   ----               -------\n  Normal   Scheduled  10m    default-scheduler  Successfully assigned default/my-pod-abc123 to node-1\n  Warning  BackOff    5m     kubelet            Back-off restarting failed container\n  Normal   Pulling    10m    kubelet            Pulling image \"myapp:v1.0\"\n  Normal   Pulled     9m45s  kubelet            Successfully pulled image\n  Normal   Created    9m44s  kubelet            Created container app\n  Normal   Started    9m43s  kubelet            Started container app\n  Warning  Unhealthy  6m     kubelet            Liveness probe failed: HTTP probe failed with statuscode: 500\n  Normal   Killing    6m     kubelet            Container app failed liveness probe\n";

    #[test]
    fn detects_pod_describe() {
        assert!(detect(POD_SAMPLE));
    }

    #[test]
    fn rejects_yaml_manifest() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n  namespace: default\nspec:\n  containers:\n  - name: app\n";
        assert!(!detect(yaml));
    }

    #[test]
    fn rejects_plain_keyvalue_text() {
        // Configuration-like text WITHOUT k8s-specific keys.
        assert!(!detect(
            "Server: nginx\nPort: 80\nLog Level: info\nWorkers: 4\n"
        ));
    }

    #[test]
    fn extracts_top_fields() {
        let out = compact(POD_SAMPLE);
        assert!(out.contains("Name=my-pod-abc123"), "no Name: {out}");
        assert!(out.contains("Namespace=default"));
        assert!(out.contains("Status=Running"));
        assert!(out.contains("Node=node-1/10.0.0.1"));
        assert!(out.contains("Controlled By=ReplicaSet/my-app-7d9f"));
        assert!(out.contains("QoS=Burstable"));
    }

    #[test]
    fn extracts_containers() {
        let out = compact(POD_SAMPLE);
        assert!(out.contains("containers (1):"));
        assert!(out.contains("app: Running"));
        assert!(out.contains("restarts: 3"));
        assert!(out.contains("ready: True"));
        assert!(out.contains("image=myapp:v1.0"));
        assert!(out.contains("last state: Terminated"));
        assert!(out.contains("reason=Error"));
        assert!(out.contains("exit=137"));
    }

    #[test]
    fn extracts_conditions() {
        let out = compact(POD_SAMPLE);
        assert!(out.contains("conditions: "));
        assert!(out.contains("Initialized=True"));
        assert!(out.contains("Ready=True"));
        assert!(out.contains("ContainersReady=True"));
        assert!(out.contains("PodScheduled=True"));
    }

    #[test]
    fn extracts_events() {
        let out = compact(POD_SAMPLE);
        assert!(out.contains("events (8 total: 2 Warning, 6 Normal):"));
        // Both warnings present.
        assert!(out.contains("BackOff"));
        assert!(out.contains("Unhealthy"));
        assert!(out.contains("Liveness probe failed"));
        // At least one Normal event preserved.
        assert!(out.contains("Scheduled") || out.contains("Pulled"));
        // Annotations / Mounts / Tolerations / Environment must NOT leak.
        assert!(!out.contains("DB_HOST"));
        assert!(!out.contains("default-token-xxx"));
        assert!(!out.contains("kubectl.kubernetes.io/last-applied-configuration"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a kubectl describe";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_pod() {
        let (out, sav) = compact_with_savings(POD_SAMPLE);
        assert!(out.contains("Name=my-pod-abc123"));
        assert!(sav.percent() > 50.0, "got {sav:?}");
    }

    #[test]
    fn handles_no_containers_section() {
        // `kubectl describe configmap` doesn't have Containers/Events sections.
        let cm = "Name:         my-cm\nNamespace:    default\nLabels:       app=foo\nAnnotations:  <none>\nQoS Class:    Burstable\n\nData\n====\nkey1: value1\nkey2: value2\n\nEvents:  <none>\n";
        let out = compact(cm);
        assert!(out.contains("Name=my-cm"));
        assert!(out.contains("Namespace=default"));
    }
}

#[cfg(test)]
mod source_quoting_kubectl_output {
    //! A kubectl sample embedded in source carries the Events header just as
    //! well as kubectl does. Four real files were claimed on it — two of them
    //! ours — and every one came back from the compactor EMPTY.

    #[test]
    fn an_escaped_one_line_fixture_is_not_kubectl_output() {
        // The shape of this file's own `POD_SAMPLE`: one Rust string literal,
        // newlines escaped, so the Events header sits mid-line.
        let src = "const POD_SAMPLE: &str = \"Name: my-pod\\nEvents:\\n  Type     Reason     Age   From   Message\\n  Normal   Scheduled  10m   sched  ok\\n\";\n";
        assert!(
            !super::detect(src),
            "a Rust string literal is not kubectl output"
        );
    }

    #[test]
    fn a_jsdoc_sample_is_not_kubectl_output() {
        let src = "/**\n * Parses `kubectl describe pod`:\n *\n *   Type     Reason     Age   From               Message\n *   ----     ------     ----  ----               -------\n *   Normal   Scheduled  5m    default-scheduler  assigned\n */\nexport function parse(s: string) {}\n";
        assert!(!super::detect(src), "a JSDoc block is not kubectl output");
    }

    #[test]
    fn real_kubectl_output_still_detects() {
        let out = "Name:         my-pod\nNamespace:    default\nEvents:\n  Type     Reason     Age   From               Message\n  ----     ------     ----  ----               -------\n  Normal   Scheduled  10m   default-scheduler  assigned\n";
        assert!(super::detect(out));
    }

    #[test]
    fn a_header_at_byte_zero_still_detects() {
        let out = "Type     Reason     Age   From   Message\n----     ------     ----  ----   -------\nNormal   Pulled     9m    kubelet  pulled\n";
        assert!(super::detect(out));
    }

    #[test]
    fn a_conditions_table_alone_is_not_the_events_header() {
        // `Conditions:` also opens with a `Type` column, and it must not be
        // mistaken for the Events table on its own.
        let out = "some notes about a cluster\nConditions:\n  Type             Status\n  Ready            True\n";
        assert!(!super::detect(out));
    }
}
