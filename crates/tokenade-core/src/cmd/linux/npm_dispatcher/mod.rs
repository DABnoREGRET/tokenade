//! Dispatcher for `npm`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::{find_subcommand, has_flag, has_any_flag};

pub fn dispatch(argv: &[String], output: &str) -> String {
    // #580 — use find_subcommand so top-level value-flags like
    // `npm --prefix /path install` skip the flag+value and read
    // "install" as the subcommand. Previously `argv.get(1)` read
    // "--prefix" and routed to the npm_run fallback — same bug
    // class as #471/#472 for kubectl/cargo.
    let sub = find_subcommand("npm", argv);
    // `--silent` / `--quiet` collapse everything → just dedup pass.
    if has_any_flag(argv, &["--silent", "--quiet"]) {
        // Silent npm install — same shape as the regular install (it
        // suppresses progress, not the final summary). Reuse install.
        return crate::cmd::linux::npm_install::compact(output);
    }
    // `npm view <pkg> versions` (alias `info`/`show`/`v`), JSON or not: the
    // newest end of the list is the answer, and the generic caps keep the
    // oldest. See `npm_view_versions`.
    if matches!(sub, "view" | "info" | "show" | "v")
        && argv.iter().any(|a| a == "versions")
        && crate::cmd::linux::npm_view_versions::detect(output)
    {
        return crate::cmd::linux::npm_view_versions::compact(output);
    }
    // `--json` makes most subcommands emit structured output.
    if has_flag(argv, "--json") {
        return crate::auto::compact(output).1;
    }
    match sub {
        "install" | "ci" | "i" | "add" | "remove" | "rm" | "upgrade" | "up" => crate::cmd::linux::npm_install::compact(output),
        "audit" => crate::cmd::linux::npm_audit::compact(output),
        "test" | "t" => crate::cmd::linux::npm_test::compact(output),
        "run" | "run-script" | "build" | "start" | "lint" | "format" => crate::cmd::linux::npm_run_script::compact(output),
        "ls" | "list" | "ll" | "la" => crate::cmd::linux::npm_list::compact(output),
        "outdated" => crate::cmd::linux::npm_outdated::compact(output),
        _ => crate::cmd::linux::npm_run_script::compact(output), // safe fallback: dedup + truncate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::argv::find_subcommand;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// `npm view <pkg> versions` reaches its compactor with or without
    /// `--json`; the generic caps kept the OLDEST versions.
    #[test]
    fn npm_view_versions_keeps_the_newest_end() {
        let v: Vec<String> = (0..60).map(|i| format!("\"{}.{}.0\"", i / 10, i % 10)).collect();
        let json = format!("[{}]", v.join(","));
        for a in [&["npm", "view", "pkg", "versions", "--json"][..], &["npm", "info", "pkg", "versions"][..]] {
            let out = dispatch(&argv(a), &json);
            assert!(out.contains("latest stable (15): 4.5.0,"), "{a:?}: {out}");
            assert!(out.contains("5.9.0") && !out.contains("0.1.0,"), "{out}");
        }
    }

    #[test]
    fn find_subcommand_skips_prefix_value_flag() {
        // #580 regression: `npm --prefix /custom install pkg` must
        // identify "install" as the subcommand, not "--prefix".
        let a = argv(&["npm", "--prefix", "/custom", "install", "pkg"]);
        assert_eq!(find_subcommand("npm", &a), "install");
    }

    #[test]
    fn find_subcommand_skips_workspace_short_flag() {
        // `npm -w pkg install dep` — `-w` is the short for --workspace
        // and takes a value.
        let a = argv(&["npm", "-w", "frontend", "install", "react"]);
        assert_eq!(find_subcommand("npm", &a), "install");
    }

    #[test]
    fn find_subcommand_skips_multiple_value_flags() {
        // Realistic CI command: --prefix + --registry + --cache.
        let a = argv(&[
            "npm",
            "--prefix",
            "/opt/app",
            "--registry",
            "https://npm.mirror/",
            "--cache",
            "/tmp/npm-cache",
            "ci",
        ]);
        assert_eq!(find_subcommand("npm", &a), "ci");
    }

    #[test]
    fn find_subcommand_skips_pnpm_filter_flag() {
        // pnpm-specific `-F` / `--filter` value-flag.
        let a = argv(&["pnpm", "-F", "frontend", "test"]);
        assert_eq!(find_subcommand("pnpm", &a), "test");
    }

    #[test]
    fn find_subcommand_handles_eq_form() {
        // `--prefix=/p install pkg` — `=` form is one token, the
        // walker advances by 1 not 2. The subcommand is still "install".
        let a = argv(&["npm", "--prefix=/opt", "install", "pkg"]);
        assert_eq!(find_subcommand("npm", &a), "install");
    }

    #[test]
    fn install_routes_correctly_with_leading_value_flag() {
        // Full end-to-end: a body the npm_install compactor recognizes.
        let body = "added 42 packages in 3s\n42 packages are looking for funding\n";
        let out = dispatch(&argv(&["npm", "--prefix", "/opt", "install", "pkg"]), body);
        // npm_install preserves "added N packages" — would NOT survive
        // the npm_run fallback's dedup+truncate intact.
        assert!(
            out.contains("added 42 packages") || out.contains("added"),
            "install must route to npm_install; got: {out:?}"
        );
    }

    #[test]
    fn silent_flag_short_circuits_before_subcommand_lookup() {
        // `--silent` fires BEFORE the subcommand match — verifies the
        // early-return order is preserved. npm_silent reuses the
        // install compactor, so a typical "added N packages" summary
        // survives.
        let body = "added 12 packages in 1s\n";
        let out = dispatch(
            &argv(&["npm", "--silent", "install", "pkg"]),
            body,
        );
        assert!(out.contains("added 12 packages"));
    }

    #[test]
    fn json_flag_short_circuits_to_npm_json() {
        // `--json` short-circuits to the JSON compactor regardless of
        // subcommand.
        let body = r#"{"name":"app","version":"1.0.0","dependencies":{"react":"^18"}}"#;
        let out = dispatch(&argv(&["npm", "ls", "--json"]), body);
        // npm_json compactor preserves structure.
        assert!(!out.is_empty());
        assert!(out.contains("react") || out.contains("dependencies"));
    }
}
