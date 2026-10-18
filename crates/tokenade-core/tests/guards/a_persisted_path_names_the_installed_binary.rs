//! **A path written into a file that outlives us does not come from
//! `/proc/self/exe`.**
//!
//! Linux suffixes the target of `/proc/self/exe` with ` (deleted)` as soon as
//! the file has been replaced — which `tokenade upgrade` does right before
//! installing the services. OBSERVED on a real machine after 1.1.14:
//!
//! ```text
//! ExecStart=/home/<user>/.local/bin/tokenade (deleted) llm-proxy --listen 127.0.0.1:8788 …
//! ```
//!
//! The unit can no longer start. The old process survives on the OLD port,
//! `systemctl is-active` answers "active", and each following update probes
//! the new port, gets nothing, and undoes the agent's configuration
//! announcing "not installed" — three lines below "it is serving".
//!
//! `platform::stable_exe` is the declarer: "the binary a PERSISTED
//! configuration must name". It already handles this case (an absolute path
//! that does not exist ⇒ the installed tokenade), and its comment tells of
//! the same fault committed by `mcp_wrap` in 2026-08. This guard is there so
//! there is no fourth time.

use tk_base::source_guard::{crates_dir, rust_sources};

/// The modules whose output is WRITTEN to disk and read back later by a third
/// party: systemd unit, launchd plist, scheduled task, agent config.
const PERSISTS_A_PATH: &[&str] = &["llm_proxy", "mcp_wrap", "install_cli", "project_scaffold"];

/// What LAUNCHES a process right away writes nothing that outlives it: the
/// path dies with the call. Named one by one, with the reason.
const EXEMPT: &[(&str, &str)] = &[(
    "project_scaffold/housekeeping.rs",
    "relaunches `evict-stale` immediately, persists no path",
)];

#[test]
fn no_module_that_persists_a_path_calls_current_exe_directly() {
    let crates = crates_dir();
    let mut roots = tk_base::source_guard::crate_srcs();
    roots.push(crates.join("tokenade-cli").join("src"));

    let mut offenders: Vec<String> = Vec::new();
    let mut reached = 0usize;
    for root in &roots {
        for (path, code) in rust_sources(root) {
            let p = path.display().to_string();
            if !PERSISTS_A_PATH.iter().any(|m| p.contains(m)) {
                continue;
            }
            reached += 1;
            // On CODE only: my own explanatory comment names
            // `env::current_exe()`, and the first version of this guard
            // accused the file I had just fixed.
            let code = tk_base::srcsweep::code_only(&code);
            if !code.contains("env::current_exe()") {
                continue;
            }
            // The declarer itself must read `/proc/self/exe` once: a file
            // passing the result to `stable_exe_from` IS the sanitising
            // layer, not the persisting one.
            if code.contains("stable_exe_from") {
                continue;
            }
            if EXEMPT.iter().any(|(f, _)| p.ends_with(f)) {
                continue;
            }
            offenders.push(p);
        }
    }
    // Reach floor: a guard reading no file is green, and that is the worst
    // of verdicts.
    assert!(
        reached >= 10,
        "only {reached} file(s) read — the guard does not look where these modules live"
    );
    assert!(
        offenders.is_empty(),
        "these modules write a path that outlives them and take it from \
         `/proc/self/exe`, suffixed ` (deleted)` after the binary is \
         replaced — go through `platform::stable_exe`: {offenders:?}"
    );
}
