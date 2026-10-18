//! Env-var back-compat shim for the `STS_*` → `TOKENADE_*` rename.
//!
//! When the project rebranded `sts` → `tokenade` we also renamed every
//! exported env var (e.g. `STS_DEBUG_LOG` → `TOKENADE_DEBUG_LOG`).
//! Existing users who set the old names in their shell profiles or in
//! their agent's MCP config (`~/.claude.json` `env.STS_*`) shouldn't
//! suddenly lose those settings.
//!
//! Use [`var`] / [`is_truthy`] anywhere a TOKENADE_* env var is read.
//! Both return the canonical (new) value first, falling back to the
//! legacy name only if the new one is unset. This keeps the rename
//! invisible to existing setups for one release cycle — we'll drop the
//! legacy fallback in a later major version.

/// Read `new` (the canonical name); if unset, fall back to `legacy`.
/// Returns `Err(VarError::NotPresent)` only if both are unset.
pub fn var(new: &str, legacy: &str) -> Result<String, std::env::VarError> {
    std::env::var(new).or_else(|_| std::env::var(legacy))
}

/// Truthy check: `"1"` or case-insensitive `"true"`. Reads `new` first,
/// then `legacy`. Returns `false` if neither is set or the value is
/// anything else.
pub fn is_truthy(new: &str, legacy: &str) -> bool {
    var(new, legacy).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn is_truthy_one(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches_truthy(&v))
}

/// Shared truthy vocabulary for [`is_truthy`] and [`is_truthy_one`].
/// Kept private so the project has exactly ONE definition of what
/// counts as a truthy env value (matched against the truthy-half of
/// [`is_boolean_typo`]).
fn matches_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Read `name` as a `usize` knob, returning `default` if the env var
/// is unset OR set to a value that fails to parse (typos like `=5k`,
/// `=5,000`, `=-1`, `=`). The fall-through is logged to debug_log so
/// operators tuning a long-lived MCP server have a way to see WHY
/// their tuning didn't take effect — previously the silent fallback
/// produced bug reports of the form "I set TOKENADE_FOO=10000 but
/// nothing changed" that took hours to trace.
///
/// Empty / whitespace-only values are treated as "not set" without
/// logging (common shell pattern: `EXPORT TOKENADE_X=` clears it).
///
/// Mirrors the spirit of the CLI-flag rule (CLI flag typos error out)
/// for the env-var surface, where erroring would break dotfile-set
/// values across every binary invocation.
pub fn parse_usize_or(name: &str, default: usize) -> usize {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default;
    }
    match trimmed.parse::<usize>() {
        Ok(n) => n,
        Err(_) => {
            crate::debug_log::log(
                "env",
                "parse_usize_rejected",
                serde_json::json!({
                    "name": name,
                    "raw": raw,
                    "default": default,
                }),
            );
            default
        }
    }
}

/// Read `name` as a path, rejecting boolean-typo values
/// (`TOKENADE_*_DIR=1` etc. — see [`is_boolean_typo`]). Returns
/// `None` for unset / empty / boolean-typo so the caller falls
/// through to the documented default. Non-UTF-8 OS paths are
/// accepted as-is (they can't possibly be the literal string `"1"`).
pub fn path_from_env(name: &str) -> Option<std::path::PathBuf> {
    let raw = std::env::var_os(name)?;
    if let Some(s) = raw.to_str() {
        if !is_usable_value(s) {
            return None;
        }
    }
    Some(std::path::PathBuf::from(raw))
}

/// True when an env value is worth honouring; false when the caller must fall
/// through to its documented default.
///
/// Unset is the caller's business. What this decides is the three values a user
/// sets by accident: empty, whitespace-only, and boolean-typo (`…_DIR=1`).
///
/// Four sites spelled this rule out by hand — `config_dir`, an activation endpoint,
/// `update_endpoint` and `path_from_env` — and one of them disagreed: only
/// `path_from_env` omitted the `trim()`, so `TOKENADE_CONFIG_DIR="   "` was a
/// usable path there and rejected in the other three. A comment at the
/// `config_dir` site even named the shared posture ("same posture as
/// update_endpoint / debug_log") that had no shared code.
pub fn is_usable_value(v: &str) -> bool {
    !v.trim().is_empty() && !is_boolean_typo(v)
}

/// True when `v` is a value users typically set as a boolean toggle —
/// `0/1/true/false/on/off/yes/no`, case + whitespace tolerant. Use this
/// to reject `TOKENADE_*_DIR=1` / `TOKENADE_DEBUG_LOG=1` typos: every
/// boolean-shaped tokenade env var (`TOKENADE_NO_HOOK`,
/// `TOKENADE_HOOK_DISABLED`, …) takes `"1"` as on, so users naturally
/// try the same on path-typed vars and silently land artefacts in
/// `./1/` in whatever CWD the process happens to be in.
pub fn is_boolean_typo(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "1" | "true" | "false" | "on" | "off" | "yes" | "no"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env mutation across tests (process-wide state).
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn prefers_new_over_legacy() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("TOKENADE_TEST_X", "new");
        std::env::set_var("STS_TEST_X", "legacy");
        assert_eq!(var("TOKENADE_TEST_X", "STS_TEST_X").unwrap(), "new");
        std::env::remove_var("TOKENADE_TEST_X");
        std::env::remove_var("STS_TEST_X");
    }

    #[test]
    fn falls_back_to_legacy_when_new_unset() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("TOKENADE_TEST_Y");
        std::env::set_var("STS_TEST_Y", "legacy-only");
        assert_eq!(var("TOKENADE_TEST_Y", "STS_TEST_Y").unwrap(), "legacy-only");
        std::env::remove_var("STS_TEST_Y");
    }

    #[test]
    fn errors_when_both_unset() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("TOKENADE_TEST_Z");
        std::env::remove_var("STS_TEST_Z");
        assert!(var("TOKENADE_TEST_Z", "STS_TEST_Z").is_err());
    }

    #[test]
    fn is_truthy_handles_both_names_and_values() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("TOKENADE_TEST_T");
        std::env::remove_var("STS_TEST_T");
        assert!(!is_truthy("TOKENADE_TEST_T", "STS_TEST_T"));

        std::env::set_var("STS_TEST_T", "true");
        assert!(is_truthy("TOKENADE_TEST_T", "STS_TEST_T"));

        std::env::set_var("TOKENADE_TEST_T", "false");
        // New is set to false — overrides legacy.
        assert!(!is_truthy("TOKENADE_TEST_T", "STS_TEST_T"));

        std::env::set_var("TOKENADE_TEST_T", "1");
        assert!(is_truthy("TOKENADE_TEST_T", "STS_TEST_T"));

        std::env::remove_var("TOKENADE_TEST_T");
        std::env::remove_var("STS_TEST_T");
    }
}

/// **A service address read from the environment.**
///
/// Variables naming a server (such as `TOKENADE_UPDATE_ENDPOINT`) used to
/// apply the same boolean-typo guard each on its own. One of them
/// ALSO applied a refusal of plaintext, the other did not, and nothing
/// said which or why.
///
/// The answer is written here, once: the refusal depends on what the address
/// CARRIES, not on the variable. `require_https` is therefore an argument.
///
/// - An endpoint that receives a secret (a key, a machine identifier):
///   an external `http://` puts it in clear on the network: refused, falling back to
///   the documented default.
/// - The update manifest is SIGNED, and the `signing` module says so:
///   the signature is the source of truth, not TLS. A manifest served in
///   clear and correctly signed is fine; a manifest served over HTTPS and badly
///   signed is not. Imposing HTTPS there would forbid an internal mirror without
///   protecting anything more.
///
/// In both cases, a LOOPBACK `http://` stays allowed: it is the
/// test server the test suite launches itself.
pub fn service_endpoint(var: &str, default: impl Fn() -> String, require_https: bool) -> String {
    let raw = std::env::var(var)
        .ok()
        .filter(|v| is_usable_value(v))
        .unwrap_or_else(&default);
    let raw = raw.trim_end_matches('/');
    if require_https && !is_confidential_transport(raw) {
        // `Fn` and not `FnOnce` for this line: the fallback must be able to
        // call the default again after refusing the environment's value.
        return default().trim_end_matches('/').to_string();
    }
    raw.to_string()
}

/// True if the address does not put a secret in clear on the network: HTTPS, or
/// a loopback `http://` — the test server the suite launches
/// itself, which never leaves the machine.
fn is_confidential_transport(raw: &str) -> bool {
    raw.starts_with("https://")
        || raw.starts_with("http://127.")
        || raw.starts_with("http://[::1]")
        || raw.starts_with("http://localhost")
}

#[cfg(test)]
mod adresse_de_service {
    use super::*;

    fn with(var: &str, val: Option<&str>, f: impl FnOnce()) {
        // Every test here sets the SAME variable, and the harness runs them
        // in parallel: one test's value leaked into another's assertion
        // (two of them red in a full `cargo test -p tk-base`, green alone).
        let _held = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var(var).ok();
        match val {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        f();
        match saved {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }

    const VAR: &str = "TOKENADE_TEST_ENDPOINT_XYZ";
    fn def() -> String {
        "https://default.example".to_string()
    }

    /// The boolean-typo guard: `=1` is the natural typo, because
    /// all the other `TOKENADE_*` variables take "1" as a switch.
    #[test]
    fn a_boolean_typo_falls_back_instead_of_becoming_the_url() {
        for typo in ["1", "0", "true", "yes", "", "   "] {
            with(VAR, Some(typo), || {
                assert_eq!(service_endpoint(VAR, def, false), "https://default.example");
            });
        }
    }

    #[test]
    fn a_trailing_slash_never_survives() {
        with(VAR, Some("https://a.example/"), || {
            assert_eq!(service_endpoint(VAR, def, false), "https://a.example");
        });
    }

    /// Both directions of the decision: what carries a secret refuses
    /// plaintext, what carries signed content accepts it.
    #[test]
    fn plaintext_is_refused_only_where_a_secret_travels() {
        with(VAR, Some("http://mirror.internal"), || {
            assert_eq!(service_endpoint(VAR, def, true), "https://default.example");
            assert_eq!(service_endpoint(VAR, def, false), "http://mirror.internal");
        });
    }

    /// And loopback stays open on both sides: it is this suite's test
    /// server.
    #[test]
    fn loopback_plaintext_stays_usable_under_the_strict_rule() {
        for local in [
            "http://127.0.0.1:8080",
            "http://localhost:9",
            "http://[::1]:1234",
        ] {
            with(VAR, Some(local), || {
                assert_eq!(service_endpoint(VAR, def, true), local);
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// The downloads address
// ─────────────────────────────────────────────────────────────────────
//
// It lived in `updater`, a product layer, and `model_fetch` — the code
// layer, which goes there to fetch a model archive — named it by climbing up.
// A service address does not belong to the module that read it first;
// it is of the same nature as `service_endpoint` just above, which is
// in fact what applies it.

/// GitHub Releases of the open-source repo.
pub const GITHUB_RELEASES: &str = "https://github.com/pi-infected/tokenade/releases";

/// Base URL for the manifest + binary downloads: the LATEST GitHub Release
/// (`<base>/manifest.json`, `<base>/manifest.json.sig`). Override via the
/// `TOKENADE_UPDATE_ENDPOINT` env var (used by CI / staging / mirrors).
pub const DEFAULT_UPDATE_ENDPOINT: &str =
    "https://github.com/pi-infected/tokenade/releases/latest/download";

pub fn update_endpoint() -> String {
    // `require_https: false` — and that is a DECISION, not an oversight. The
    // manifest is signed, and this module's header as well as `signing`'s
    // say the same thing: the signature is the source of truth, not TLS.
    // Imposing HTTPS here would forbid a plaintext internal mirror without
    // protecting anything more, since a tampered manifest fails verification
    // whatever the transport.
    crate::env_compat::service_endpoint(
        "TOKENADE_UPDATE_ENDPOINT",
        || DEFAULT_UPDATE_ENDPOINT.to_string(),
        false,
    )
}
