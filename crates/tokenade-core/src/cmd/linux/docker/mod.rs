//! `docker` / `podman` / `buildah` / `skopeo` — container CLIs.
//!
//! Subcommands share most output shapes. Each shape has its own
//! file so future per-subcommand optimisations don't need to
//! refactor a giant `match`.

pub mod build;
pub mod inspect;
pub mod logs;
pub mod ps;
pub mod system_df;

use crate::compress::generic;
use crate::argv::{find_subcommand, flag_value};

pub fn dispatch(argv: &[String], output: &str) -> String {
    let fmt = flag_value(argv, "--format").unwrap_or("");
    if fmt == "json" || fmt.contains("json") {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    let sub = find_subcommand("docker", argv);
    match sub {
        "ps" | "images" | "image" | "container" | "network" | "volume" | "stats" => {
            ps::compact(output)
        }
        "build" | "buildx" => build::compact(output),
        "logs" => logs::compact(output),
        "inspect" => inspect::compact(output),
        "system" if argv.iter().any(|a| a == "df") => system_df::compact(output),
        "system" | "info" | "version" => {
            let (_, c, _) = crate::auto::compact(output);
            c
        }
        "exec" | "run" => {
            let (_, c, _) = crate::auto::compact(output);
            c
        }
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn system_df_reaches_its_compactor() {
        let mut body = String::from("Local Volumes space usage:\n\nVOLUME NAME   LINKS   SIZE\n");
        for i in 0..20u64 {
            body.push_str(&format!("{:064x}   0   19.67MB\n", i * 7919 + 3));
        }
        let out = dispatch(&argv(&["docker", "system", "df", "-v"]), &body);
        assert!(out.contains("[20 anonymous volume(s)"), "{out}");
    }

    #[test]
    fn format_json_short_circuits_to_auto() {
        // `--format json` skips the subcommand dispatch entirely and
        // hands off to auto so the JSON compactor takes over.
        let body = r#"[{"ID":"a"},{"ID":"b"},{"ID":"c"}]"#;
        let out = dispatch(&argv(&["docker", "ps", "--format", "json"]), body);
        assert!(out.contains("ID"), "JSON short-circuit must preserve content");
    }

    #[test]
    fn format_contains_json_template_short_circuits() {
        // `--format '{{json .}}'` is the Go template variant that also
        // emits JSON; same short-circuit.
        let body = r#"{"Name":"box","Image":"alpine"}"#;
        let out = dispatch(
            &argv(&["docker", "inspect", "--format", "{{json .}}", "box"]),
            body,
        );
        assert!(out.contains("Name"));
    }

    #[test]
    fn ps_routes_to_ps_compactor() {
        // `docker ps` is tabular CONTAINER ID + IMAGE + STATUS + ...
        // The tabular compactor preserves data rows but may drop
        // low-signal columns. Sanity: at least one container name
        // survives the compaction.
        let body = "CONTAINER ID   IMAGE     STATUS    NAMES\n\
                    abc123         alpine    Running   box1\n\
                    def456         debian    Running   box2\n";
        let out = dispatch(&argv(&["docker", "ps"]), body);
        assert!(
            out.contains("box1") || out.contains("box2") || out.contains("alpine"),
            "ps dispatch must preserve some signal column; got: {out}"
        );
    }

    #[test]
    fn images_uses_ps_compactor() {
        // Same tabular shape (REPOSITORY + TAG + IMAGE ID + ...).
        let body = "REPOSITORY   TAG       IMAGE ID       CREATED\n\
                    alpine       latest    abc1234567ab   2 days ago\n\
                    debian       latest    def4567890cd   5 days ago\n";
        let out = dispatch(&argv(&["docker", "images"]), body);
        assert!(out.contains("alpine") && out.contains("debian"));
    }

    #[test]
    fn container_network_volume_stats_use_ps_compactor() {
        // All tabular subcommands route through ps::compact.
        for sub in &["container", "network", "volume", "stats"] {
            let body = "NAME   STATUS\nfoo    Running\nbar    Running\n";
            let out = dispatch(&argv(&["docker", sub, "ls"]), body);
            assert!(
                out.contains("foo") && out.contains("bar"),
                "{sub} dispatch must preserve rows"
            );
        }
    }

    #[test]
    fn logs_routes_to_logs_compactor() {
        // Logs are application stdout/stderr; the dedicated compactor
        // dedups repeated lines and folds noise.
        let body = "2026-06-01 starting\nrequest handled\nrequest handled\nrequest handled\n";
        let out = dispatch(&argv(&["docker", "logs", "myapp"]), body);
        assert!(out.contains("starting") && out.contains("handled"));
    }

    #[test]
    fn inspect_routes_to_inspect_compactor() {
        // `docker inspect` emits JSON arrays; the dedicated compactor
        // strips low-signal fields.
        let body = r#"[
  {
    "Id": "abc",
    "Name": "/box",
    "State": {"Status": "running"}
  }
]"#;
        let out = dispatch(&argv(&["docker", "inspect", "box"]), body);
        assert!(out.contains("box") || out.contains("running"));
    }

    #[test]
    fn system_info_version_route_to_auto() {
        // `docker system df`, `docker info`, `docker version` are
        // free-form; auto picks the best fit.
        for sub in &["system", "info", "version"] {
            let body = "Server Version: 24.0.7\nKernel: 6.5.0\nContainers: 12\n";
            let out = dispatch(&argv(&["docker", sub]), body);
            assert!(
                out.contains("Server Version") || out.contains("Kernel"),
                "{sub} dispatch must preserve content"
            );
        }
    }

    #[test]
    fn exec_run_route_to_auto() {
        // `exec` and `run` outputs are whatever the user's command
        // produced — let auto recognize the shape.
        let body = "{\"foo\":1,\"bar\":2,\"baz\":3,\"qux\":4,\"quux\":5}";
        for sub in &["exec", "run"] {
            let out = dispatch(&argv(&["docker", sub, "box", "--", "echo", "x"]), body);
            assert!(out.contains("foo"), "{sub} must let auto handle JSON");
        }
    }

    #[test]
    fn unknown_subcommand_falls_back_to_generic() {
        // `swarm`, `service`, `node`, etc. fall through to generic
        // (ANSI strip + dedup + head/tail truncate).
        let body = "swarm joined\nnode added\nnode added\n";
        let out = dispatch(&argv(&["docker", "swarm", "join", "--token", "x"]), body);
        assert!(out.contains("swarm joined"));
    }
}
