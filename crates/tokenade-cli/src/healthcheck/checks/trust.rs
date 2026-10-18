//! Folder trust that silently switches hooks off — reported, never granted.
//!
//! Claude Code skips EVERY hook, user-level ones included, in an interactive
//! session whose workspace trust was not accepted ("Skipping … hook execution
//! - workspace trust not accepted", read in its bundle 2026-10-06). Writing
//! the key would grant that consent in the user's name: tokenade only says it.

use super::*;

/// Is `dir` (or a parent) trusted in `~/.claude.json`?
fn claude_trusts(claude_json: &serde_json::Value, dir: &std::path::Path) -> bool {
    let Some(projects) = claude_json.get("projects").and_then(|p| p.as_object()) else {
        return false;
    };
    dir.ancestors().any(|d| {
        projects
            .get(&d.display().to_string())
            .and_then(|p| p.get("hasTrustDialogAccepted"))
            .and_then(|b| b.as_bool())
            == Some(true)
    })
}

pub(crate) fn check_claude_workspace_trust(
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<HealthCheckResult> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).ok()?).ok()?;
    if claude_trusts(&v, cwd) {
        return None;
    }
    Some(hc(
        "claude-workspace-trust",
        CheckStatus::Info,
        format!("Claude Code has not trusted {} — in an interactive session here, it runs none of its hooks (tokenade's included)", cwd.display()),
        vec!["accept the trust prompt when Claude Code opens this folder; `claude -p` runs are not affected".into()],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trusted_parent_covers_the_folder_and_nothing_else_does() {
        let v = serde_json::json!({"projects": {"/work/repo": {"hasTrustDialogAccepted": true}, "/other": {"hasTrustDialogAccepted": false}}});
        assert!(claude_trusts(
            &v,
            std::path::Path::new("/work/repo/crates/x")
        ));
        assert!(!claude_trusts(&v, std::path::Path::new("/other/sub")));
        assert!(!claude_trusts(&v, std::path::Path::new("/elsewhere")));
    }
}
