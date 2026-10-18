//! Tests for the `websearch_post` hook.

use super::*;

/// Uninstall symmetry for non-Claude agents: hook entries stripped from
/// cursor/codex/gemini configs and plugin files deleted (opencode/pi/kilo),
/// hermes enabled-list entry dropped. Orphans = missing-binary spawns after
/// package removal.
#[test]
fn uninstall_agent_integrations_removes_all_orphans() {
    // hermes_home_dir reads the process-global HERMES_HOME env, which the
    // hermes install tests set under this lock — without it, a parallel
    // schedule resolves OUR hermes dir into THEIR fixture (both fail).
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tmp("unsym");
    // cursor: one tokenade + one user hook
    fs::create_dir_all(home.join(".cursor")).unwrap();
    fs::write(
        home.join(".cursor/hooks.json"),
        r#"{"hooks":{"preToolUse":[{"command":"tokenade hookwrap-pretool","matcher":"Shell"},{"command":"my-hook"}]}}"#,
    )
    .unwrap();
    // codex: lifecycle _tag group
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(
        home.join(".codex/hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x/tokenade hookwrap-pretool","_tag":"tokenade-pretool"}]}]}}"#,
    )
    .unwrap();
    // opencode + pi + kilo plugin files
    let oc = crate::install_cli::opencode_plugin_dir(&home);
    fs::create_dir_all(&oc).unwrap();
    fs::write(oc.join("tokenade.ts"), "x").unwrap();
    let pi = home.join(".pi/agent/extensions");
    fs::create_dir_all(&pi).unwrap();
    fs::write(pi.join("tokenade.ts"), "x").unwrap();
    // hermes config with enabled entry
    let hh = crate::install_cli::hermes_home_dir(&home);
    fs::create_dir_all(hh.join("plugins/tokenade")).unwrap();
    fs::write(hh.join("plugins/tokenade/plugin.yaml"), "x").unwrap();
    fs::write(
        hh.join("config.yaml"),
        "plugins:\n  enabled:\n    - other\n    - tokenade\n",
    )
    .unwrap();

    // dry-run touches nothing
    let dry = crate::install::uninstall_agent_integrations(&home, true);
    assert!(!dry.is_empty());
    assert!(oc.join("tokenade.ts").is_file(), "dry-run must not delete");

    let lines = crate::install::uninstall_agent_integrations(&home, false);
    assert!(lines.len() >= 5, "expected >=5 removals, got {lines:?}");
    let cursor = fs::read_to_string(home.join(".cursor/hooks.json")).unwrap();
    assert!(!cursor.contains("tokenade"), "cursor entry stays: {cursor}");
    assert!(
        cursor.contains("my-hook"),
        "user hook must survive: {cursor}"
    );
    let codex = fs::read_to_string(home.join(".codex/hooks.json")).unwrap();
    assert!(!codex.contains("tokenade"), "codex entry stays: {codex}");
    assert!(!oc.join("tokenade.ts").exists());
    assert!(!pi.join("tokenade.ts").exists());
    assert!(!hh.join("plugins/tokenade").exists());
    let y = fs::read_to_string(hh.join("config.yaml")).unwrap();
    assert!(!y.contains("- tokenade"), "hermes enabled entry stays: {y}");
    assert!(y.contains("- other"), "other plugin must survive: {y}");
}
