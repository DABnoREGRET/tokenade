//! Tests for `install_cli`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn install_hermes_plugin_writes_bridge_and_enables_config_idempotently() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let saved = std::env::var_os("HERMES_HOME");
    let tmp = std::env::temp_dir().join(format!("tok-hermes-inst-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let hh = tmp.join(".hermes");
    std::fs::create_dir_all(&hh).unwrap();
    std::fs::write(hh.join("config.yaml"), "model: x\n").unwrap();
    std::env::set_var("HERMES_HOME", &hh);

    // dry-run must not touch disk.
    install_hermes_plugin(&tmp, true).unwrap();
    assert!(
        !hh.join("plugins/tokenade/__init__.py").exists(),
        "dry-run wrote files"
    );

    install_hermes_plugin(&tmp, false).expect("install ok");
    let init = std::fs::read_to_string(hh.join("plugins/tokenade/__init__.py")).unwrap();
    assert!(
        init.contains("transform_tool_result"),
        "bridge body written"
    );
    assert!(
        !init.contains("@@TOKENADE_BIN@@"),
        "bin path injected (no placeholder left)"
    );
    assert!(hh.join("plugins/tokenade/plugin.yaml").exists());
    let cfg = std::fs::read_to_string(hh.join("config.yaml")).unwrap();
    assert!(cfg.contains("model: x"), "preserves pre-existing config");
    assert!(cfg.contains("- tokenade"), "enables the plugin");

    // idempotent: a second install must not double-enable.
    install_hermes_plugin(&tmp, false).unwrap();
    let cfg2 = std::fs::read_to_string(hh.join("config.yaml")).unwrap();
    assert_eq!(cfg2.matches("- tokenade").count(), 1);

    match saved {
        Some(v) => std::env::set_var("HERMES_HOME", v),
        None => std::env::remove_var("HERMES_HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn install_opencode_plugin_drops_bridge() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let saved = std::env::var_os("XDG_CONFIG_HOME");
    let tmp = std::env::temp_dir().join(format!("tok-oc-inst-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", tmp.join(".config"));

    install_opencode_plugin(&tmp, true).unwrap();
    let f = tmp.join(".config/opencode/plugin/tokenade.ts");
    assert!(!f.exists(), "dry-run wrote a file");

    install_opencode_plugin(&tmp, false).expect("install ok");
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("tool.execute.after"), "registers the hook");
    assert!(!body.contains("@@TOKENADE_BIN@@"), "bin path injected");
    assert!(
        body.contains("TOKENADE_AGENT"),
        "attributes savings to opencode"
    );

    match saved {
        Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn escape_shim_literal_handles_windows_paths_and_quotes() {
    // Windows binary paths are full of backslashes; they must be escaped so
    // the injected JS/Python string literal stays valid (multi-OS).
    assert_eq!(
        escape_for_embedded_literal(r"C:\Program Files\tokenade.exe"),
        r"C:\\Program Files\\tokenade.exe"
    );
    assert_eq!(escape_for_embedded_literal("a\"b"), "a\\\"b");
    assert_eq!(
        escape_for_embedded_literal("/usr/bin/tokenade"),
        "/usr/bin/tokenade"
    );
}

#[test]
fn inject_shim_bin_replaces_placeholder() {
    let out = inject_tokenade_bin("const b = \"@@TOKENADE_BIN@@\";");
    assert!(
        !out.contains("@@TOKENADE_BIN@@"),
        "placeholder must be replaced"
    );
    assert!(out.starts_with("const b = \"") && out.ends_with("\";"));
}

/// `openclaw` on PATH, or nothing to assert about the register step.
fn which_openclaw() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("openclaw"))
            .find(|c| c.is_file())
    })
}

#[test]
fn install_openclaw_plugin_writes_package() {
    // No env coupling (resolves home/.tokenade). The `openclaw plugins
    // install` step is best-effort and a no-op without the binary; we test
    // that the package files land regardless.
    //
    // "A no-op without the binary" was true and beside the point on a machine
    // where openclaw IS installed: the registration ran, inherited the ambient
    // HOME, and wrote the temp path into the developer's real
    // `~/.openclaw/openclaw.json` — which then refused to start once the temp
    // directory was gone. The installer now pins HOME and OPENCLAW_STATE_DIR to
    // the home it was handed, and the assertion below is what keeps it there.
    let tmp = std::env::temp_dir().join(format!("tok-ocl-inst-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    install_openclaw_plugin(&tmp, true).unwrap();
    let dir = tmp.join(".tokenade/openclaw-plugin");
    assert!(!dir.join("index.js").exists(), "dry-run wrote files");

    install_openclaw_plugin(&tmp, false).expect("install ok");
    let idx = std::fs::read_to_string(dir.join("index.js")).unwrap();
    // Assert the contract the HOST actually has. This line used to require
    // `registerAgentToolResultMiddleware`, a name that appears nowhere in
    // openclaw's `dist/` — so the test was green for as long as the bridge was
    // dead, and green BECAUSE it agreed with the bridge instead of with the
    // host. `e2e_the_openclaw_bridge_is_loaded_by_a_real_openclaw` is the one
    // that can tell the difference; this only keeps the asset honest.
    assert!(
        idx.contains("tool_result_persist"),
        "must hook `tool_result_persist` — the only hook in openclaw that can \
         REPLACE a tool result"
    );
    assert!(
        idx.contains("api.on("),
        "must attach through `api.on`, the real plugin API"
    );
    // The IMPORT, not the word: the asset's header explains the old breakage
    // and therefore names `plugin-sdk` in prose. A substring test cannot tell a
    // call from a comment — this file has made that mistake before.
    assert!(
        !idx.contains(r#"from "openclaw/"#),
        "must not import from the host package: that subpath is what made the \
         previous bridge fail to load"
    );
    assert!(!idx.contains("@@TOKENADE_BIN@@"), "bin path injected");
    assert!(
        idx.contains("TOKENADE_AGENT"),
        "attributes savings to openclaw"
    );
    // Nothing outside the home we were handed was touched. Reading the real
    // config would couple this test to the developer's machine, so ask the
    // narrower question that still catches the bug: the registration the
    // installer performed landed under `tmp`, which is only possible if the
    // child was given that home.
    if which_openclaw().is_some() {
        let registered = tmp.join(".openclaw").join("openclaw.json");
        assert!(
            registered.exists(),
            "openclaw is installed here, so the register step ran — and it must \
             have written into the home it was GIVEN ({}), not the ambient one",
            tmp.display()
        );
    }

    let manifest = std::fs::read_to_string(dir.join("openclaw.plugin.json")).unwrap();
    // The manifest used to DECLARE `contracts.agentToolResultMiddleware`, a
    // name that exists nowhere in openclaw. Declaring a contract the host does
    // not know is not neutral — it is a claim, and this test asserted it.
    assert!(
        !manifest.contains("agentToolResultMiddleware"),
        "the manifest declares a contract the host does not have"
    );
    let pkg = std::fs::read_to_string(dir.join("package.json")).unwrap();
    // openclaw keys the config entry by the PACKAGE name and then looks the
    // plugin up by its `id`. When the two differ, it registers the plugin and
    // then refuses its own config: `plugin not found: <package-name>`. Measured
    // against the real CLI on 2026-08-10 — no amount of reading found this.
    for f in [&manifest, &pkg] {
        assert!(
            f.contains("\"tokenade\""),
            "package name, manifest id and exported id must be the SAME string"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn install_pi_extension_drops_bridge() {
    // No env coupling (resolves home/.pi/agent) so no lock needed.
    let tmp = std::env::temp_dir().join(format!("tok-pi-inst-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    install_pi_extension(&tmp, true).unwrap();
    let ext = tmp.join(".pi/agent/extensions/tokenade.ts");
    assert!(!ext.exists(), "dry-run wrote a file");

    install_pi_extension(&tmp, false).expect("install ok");
    let body = std::fs::read_to_string(&ext).unwrap();
    assert!(body.contains("tool_result"), "subscribes to tool_result");
    assert!(!body.contains("@@TOKENADE_BIN@@"), "bin path injected");
    assert!(body.contains("TOKENADE_AGENT"), "attributes savings to pi");
    let _ = std::fs::remove_dir_all(&tmp);
}

// #644 — install flag validation locks. The whitelist + value-flag
// skip means `tokenade install --only claude-code` doesn't flag
// `claude-code` as unknown, while a real typo does.
#[test]
fn validate_install_flags_accepts_known_bool_flags() {
    let args = vec!["--dry-run".to_string(), "--with-claude-md".to_string()];
    assert!(super::validate_install_flags(&args).is_ok());
}

#[test]
fn validate_install_flags_accepts_value_flag_with_value() {
    // The next positional after --only is consumed as its value.
    let args = vec!["--only".to_string(), "claude-code".to_string()];
    assert!(super::validate_install_flags(&args).is_ok());
    let args = vec!["--profile".to_string(), "coding".to_string()];
    assert!(super::validate_install_flags(&args).is_ok());
}

#[test]
fn validate_install_flags_rejects_unknown_flag() {
    let args = vec!["--bogus-flag".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
}

// #645 — value-flag missing its value: the next positional must
// be a real value, not another --flag. Was silently stealing.
#[test]
fn validate_install_flags_rejects_value_flag_at_end() {
    // `--debug-log` is the last token — no value to consume.
    let args = vec!["--debug-log".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
}

#[test]
fn validate_install_flags_rejects_value_flag_followed_by_flag() {
    // `--debug-log --no-cleanup` used to consume --no-cleanup as path.
    let args = vec!["--debug-log".to_string(), "--no-cleanup".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
    let args = vec!["--only".to_string(), "--dry-run".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
    let args = vec!["--profile".to_string(), "--yes".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
}

#[test]
fn validate_install_flags_rejects_typo_after_valid() {
    // Mixed: a valid flag followed by an unknown typo.
    let args = vec!["--dry-run".to_string(), "--whith-claude-md".to_string()];
    assert_eq!(super::validate_install_flags(&args), Err(2));
}

// #655 — generic bool-only validator powering cmd_uninstall +
// cmd_unwrap_mcps. Before this, `uninstall --dryrun` (typo) ran a
// real uninstall; the strip-pass backup happened to save the user
// but the UX is hostile and one of the future destructive steps
// wouldn't necessarily take a backup.
#[test]
fn validate_simple_bool_flags_accepts_known() {
    let args: Vec<String> = vec![];
    assert!(super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS).is_ok());
    let args = vec!["--dry-run".to_string()];
    assert!(super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS).is_ok());
    // Full-uninstall flags added for data/binary removal.
    for f in ["--headless", "--keep-data"] {
        let args = vec![f.to_string()];
        assert!(
            super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS).is_ok(),
            "{f} should be accepted"
        );
    }
}

#[test]
fn two_shape_flags_are_refused_not_silently_merged() {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // The case measured on 1.1.9: `--history` dropped without a word.
    assert_eq!(
        super::reject_conflicting_shape_flags(
            &v(&["--history", "--json"]),
            "gain",
            "--history",
            "--json",
            "note"
        ),
        Err(2)
    );
    // Order changes nothing: these are two flags, not a sequence.
    assert_eq!(
        super::reject_conflicting_shape_flags(
            &v(&["--json", "--history"]),
            "gain",
            "--history",
            "--json",
            "note"
        ),
        Err(2)
    );
    // Anti-vacuity: each form ALONE must pass, otherwise the rule would break
    // the two uses it is meant to protect.
    for one in [vec!["--history"], vec!["--json"], vec![], vec!["--recent"]] {
        assert_eq!(
            super::reject_conflicting_shape_flags(&v(&one), "gain", "--history", "--json", "note"),
            Ok(()),
            "{one:?} does not carry BOTH forms"
        );
    }
}

#[test]
fn the_refusal_names_both_flags_and_what_each_gives() {
    // A refusal that does not say which one to pick costs one more turn.
    let v = vec!["--history".to_string(), "--json".to_string()];
    assert_eq!(
        super::reject_conflicting_shape_flags(&v, "gain", "--history", "--json", "note"),
        Err(2)
    );
}

#[test]
fn validate_simple_bool_flags_rejects_typo() {
    // The exact bug found in #655: --dryrun (no dash) silently fell
    // through and performed a real uninstall.
    let args = vec!["--dryrun".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS),
        Err(2)
    );
}

/// #779 — `tokenade upgrade --offline` (typo for `--off`) used to
/// fall through to the default branch and run `check_and_swap_now`,
/// performing a real binary swap. Lock in the validator rejects it.
#[test]
fn validate_upgrade_flags_rejects_offline_typo() {
    let args = vec!["--offline".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "upgrade", super::UPGRADE_FLAGS),
        Err(2)
    );
    // All real upgrade flags still pass.
    for ok in ["--check", "--on", "--off", "--help", "-h"] {
        let args = vec![ok.to_string()];
        assert!(
            super::validate_simple_bool_flags(&args, "upgrade", super::UPGRADE_FLAGS).is_ok(),
            "{ok} should be accepted by UPGRADE_FLAGS"
        );
    }
}

#[test]
fn validate_simple_bool_flags_rejects_cross_command_flag() {
    // --no-cleanup is an install flag — running it on uninstall
    // would have silently been a no-op (user thinks they're
    // suppressing something they aren't).
    let args = vec!["--no-cleanup".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS),
        Err(2)
    );
}

#[test]
fn validate_simple_bool_flags_rejects_positional() {
    // Neither uninstall nor unwrap-mcps takes positional args.
    let args = vec!["claude-code".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "uninstall", super::UNINSTALL_FLAGS),
        Err(2)
    );
}

#[test]
fn validate_simple_bool_flags_unwrap_mcps_flag_set() {
    // The unwrap-mcps validator must accept --dry-run too (parity
    // with uninstall) — the flag sets are intentionally identical.
    let args = vec!["--dry-run".to_string()];
    assert!(
        super::validate_simple_bool_flags(&args, "unwrap-mcps", super::UNWRAP_MCPS_FLAGS).is_ok()
    );
}

// #656 — evict-stale has NO real flags (TTL is hard-coded). Until
// this iteration, `cmd_evict_stale()` discarded the rest entirely,
// so `evict-stale --dryrun` ran the destructive sweep + VACUUM.
#[test]
fn validate_simple_bool_flags_evict_stale_rejects_dryrun() {
    let args = vec!["--dryrun".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "evict-stale", super::EVICT_STALE_FLAGS),
        Err(2)
    );
}

#[test]
fn validate_simple_bool_flags_evict_stale_accepts_empty() {
    let args: Vec<String> = vec![];
    assert!(
        super::validate_simple_bool_flags(&args, "evict-stale", super::EVICT_STALE_FLAGS).is_ok()
    );
}

// #657 — scrub-secret takes exactly ONE positional + flags from a
// whitelist. Validator must reject the typo class (--dryrun) AND
// the extra-positional class (forgot to quote a space-bearing
// secret).
#[test]
fn validate_scrub_secret_flags_accepts_needle_only() {
    let args = vec!["mysecret".to_string()];
    assert!(super::validate_scrub_secret_flags(&args).is_ok());
}

#[test]
fn validate_scrub_secret_flags_accepts_needle_with_dry_run() {
    let args = vec!["mysecret".to_string(), "--dry-run".to_string()];
    assert!(super::validate_scrub_secret_flags(&args).is_ok());
    // Order shouldn't matter.
    let args = vec!["--dry-run".to_string(), "mysecret".to_string()];
    assert!(super::validate_scrub_secret_flags(&args).is_ok());
}

#[test]
fn validate_scrub_secret_flags_rejects_dryrun_typo() {
    // The exact bug found in #657: --dryrun silently fell through.
    let args = vec!["mysecret".to_string(), "--dryrun".to_string()];
    assert_eq!(super::validate_scrub_secret_flags(&args), Err(2));
}

#[test]
fn validate_scrub_secret_flags_rejects_extra_positional() {
    // User forgot to quote a space-bearing secret — used to scrub
    // only "foo" and silently drop "bar".
    let args = vec!["foo".to_string(), "bar".to_string()];
    assert_eq!(super::validate_scrub_secret_flags(&args), Err(2));
}

#[test]
fn validate_simple_bool_flags_scrub_cache_rejects_dryrun_typo() {
    let args = vec!["--dryrun".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "scrub-cache", super::SCRUB_CACHE_FLAGS),
        Err(2)
    );
}

#[test]
fn validate_simple_bool_flags_scrub_cache_rejects_positional() {
    // scrub-cache takes no positional args; differs from
    // scrub-secret on this dimension.
    let args = vec!["foo".to_string()];
    assert_eq!(
        super::validate_simple_bool_flags(&args, "scrub-cache", super::SCRUB_CACHE_FLAGS),
        Err(2)
    );
}

// An upgrade refreshed Claude Code's hooks and Gemini's and NOTHING else:
// `PLUGIN_INSTALLS` had one caller, the interactive `tokenade install`. So a
// plugin-agent user kept the bridge from the day they installed it, and every
// bridge fix — including redaction correctness — stopped at the release. Two
// halves to pin, because the second is what makes the first safe.
#[test]
fn upgrade_refreshes_installed_plugin_bridges_and_only_those() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!(
        "tok-plugin-refresh-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);

    // One bridge installed, carrying a bygone version of the file.
    let opencode = opencode_plugin_dir(&tmp);
    std::fs::create_dir_all(&opencode).unwrap();
    std::fs::write(
        opencode.join("tokenade.ts"),
        "// tokenade v0, no redaction signal\n",
    )
    .unwrap();

    refresh_installed_plugin_bridges(&tmp);

    let refreshed = std::fs::read_to_string(opencode.join("tokenade.ts")).unwrap();
    assert!(
        refreshed.contains(crate::compactors::REDACTION_SIGNAL),
        "a stale bridge must be brought up to the shipped asset"
    );
    assert!(
        !refreshed.contains("@@TOKENADE_BIN@@"),
        "the refreshed bridge must still get its binary path injected"
    );

    // And the half that keeps this from being an installer: an agent that
    // never had a bridge must not acquire one from an upgrade.
    assert!(
        !tmp.join(".pi").exists(),
        "refresh created a bridge the user never installed"
    );
    assert!(
        !tmp.join(".tokenade").join("cline-plugin").exists(),
        "refresh created a bridge the user never installed"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// One asset, two agents: Kilo Code mirrors OpenCode's plugin system, so it gets
// the SAME bridge file. The agent label inside it was hardcoded `"opencode"`,
// so every saving a Kilo user made landed in the ledger under opencode — and
// the "has this bridge ever fired?" check answered no for kilo-code, for ever.
// A false alarm is worse than silence, so this pins the substitution.
#[test]
fn the_kilo_bridge_reports_itself_as_kilo_not_opencode() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!(
        "tok-kilo-label-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);

    install_kilo_plugin(&tmp, false).unwrap();
    install_opencode_plugin(&tmp, false).unwrap();

    let kilo = std::fs::read_to_string(kilo_plugin_dir(&tmp).join("tokenade.ts")).unwrap();
    let opencode = std::fs::read_to_string(opencode_plugin_dir(&tmp).join("tokenade.ts")).unwrap();
    assert!(
        kilo.contains("const AGENT_INJECTED = \"kilo-code\""),
        "the kilo bridge must credit kilo-code"
    );
    assert!(
        opencode.contains("const AGENT_INJECTED = \"opencode\""),
        "the opencode bridge must still credit opencode"
    );
    // And no placeholder survives either write.
    for body in [&kilo, &opencode] {
        assert!(
            !body.contains("@@TOKENADE_AGENT@@"),
            "agent label not injected"
        );
        assert!(
            !body.contains("@@TOKENADE_BIN@@"),
            "binary path not injected"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
