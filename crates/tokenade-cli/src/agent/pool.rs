//! Free-model availability pool.
//!
//! Two goals the naive "iterate a static list" loop can't meet:
//!   1. **Sticky within a session** — keep using the SAME model across the
//!      many `tokenade agent` processes an orchestrator spawns in a session,
//!      so the provider's prompt cache stays warm (system prompt + tool
//!      preamble are identical each call). We only rotate when forced.
//!   2. **Error-weighted cooldowns** — when a model fails, mark it
//!      unavailable for a duration that depends on WHY it failed (a 429 is
//!      brief; an auth/5xx wall is long), with exponential backoff on
//!      repeated failures. This is OmniRoute's circuit-breaker idea
//!      (`src/shared/utils/circuitBreaker.ts`) applied per-model and
//!      PERSISTED, so it survives across the separate CLI invocations.
//!
//! State lives in `<tokenade-root>/agent_pool.json` (same root as the gain
//! ledger, so it honours test isolation + `TOKENADE_HOME`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a model attempt failed — drives the cooldown length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ErrKind {
    /// 429 — transient, short cooldown.
    RateLimited,
    /// 401/403 — needs credentials we don't have; long cooldown.
    Auth,
    /// 5xx — upstream trouble; medium cooldown.
    Server,
    /// Timeout / parse / spawn / unknown — medium cooldown.
    Network,
}

impl ErrKind {
    /// Classify from an `Attempt::Failed` reason string (e.g. "HTTP 401").
    pub fn from_reason(reason: &str) -> ErrKind {
        if let Some(code) = reason
            .rsplit(|c: char| !c.is_ascii_digit())
            .find(|s| !s.is_empty())
            .and_then(|s| s.parse::<u16>().ok())
        {
            return match code {
                401 | 403 => ErrKind::Auth,
                429 => ErrKind::RateLimited,
                500..=599 => ErrKind::Server,
                _ => ErrKind::Network,
            };
        }
        ErrKind::Network
    }

    fn base_secs(self) -> u64 {
        match self {
            ErrKind::RateLimited => 45,
            ErrKind::Server => 120,
            ErrKind::Network => 90,
            ErrKind::Auth => 1800,
        }
    }
}

/// Cap on any single cooldown (1h) — even a wall of auth failures shouldn't
/// permanently retire a model across a long-running session.
const MAX_COOLDOWN_SECS: u64 = 3600;

#[derive(Default, Clone)]
struct ModelState {
    cooldown_until: u64,
    fails: u32,
}

pub struct Pool {
    current: Option<String>,
    models: BTreeMap<String, ModelState>,
    path: PathBuf,
}

fn pool_path() -> PathBuf {
    // Same root as the gain ledger → honours test override + TOKENADE_HOME.
    if let Some(dir) = tokenade_core::gain::current_ledger_path().parent() {
        return dir.join("agent_pool.json");
    }
    PathBuf::from("agent_pool.json")
}

impl Pool {
    pub fn load() -> Pool {
        let path = pool_path();
        let (current, models) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| {
                let current = v["current"].as_str().map(|s| s.to_string());
                let mut models = BTreeMap::new();
                if let Some(obj) = v["models"].as_object() {
                    for (k, mv) in obj {
                        models.insert(
                            k.clone(),
                            ModelState {
                                cooldown_until: mv["cooldown_until"].as_u64().unwrap_or(0),
                                fails: mv["fails"].as_u64().unwrap_or(0) as u32,
                            },
                        );
                    }
                }
                (current, models)
            })
            .unwrap_or((None, BTreeMap::new()));
        Pool {
            current,
            models,
            path,
        }
    }

    pub fn save(&self) {
        let models: serde_json::Map<String, serde_json::Value> = self
            .models
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::json!({"cooldown_until": v.cooldown_until, "fails": v.fails}),
                )
            })
            .collect();
        let v = serde_json::json!({ "current": self.current, "models": models });
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, v.to_string());
    }

    fn available(&self, model: &str, now: u64) -> bool {
        self.models
            .get(model)
            .map(|s| s.cooldown_until <= now)
            .unwrap_or(true)
    }

    /// Ordering the free-tier loop should try this call:
    ///   1. the sticky `current` model, if still available (cache-warm);
    ///   2. the remaining AVAILABLE models, round-robin starting just after
    ///      `current` (so repeated fallbacks spread across the bench);
    ///   3. as a last resort, the cooling-down models soonest-to-recover
    ///      first (better to probe than to hard-fail when all are cooling).
    pub fn order(&self, models: &[&str]) -> Vec<String> {
        let now = tokenade_core::platform::now_unix_secs();
        let start = self
            .current
            .as_ref()
            .and_then(|c| models.iter().position(|m| m == c))
            .unwrap_or(0);
        let mut out: Vec<String> = Vec::new();

        let mut have_sticky = false;
        if let Some(cur) = &self.current {
            if models.iter().any(|m| m == cur) && self.available(cur, now) {
                out.push(cur.clone());
                have_sticky = true;
            }
        }
        // Start right AFTER the sticky model (round-robin fallback); with no
        // sticky, start from the front so the first-listed model is the
        // default (fastest one first).
        let begin = if have_sticky { start + 1 } else { start };
        for i in 0..models.len() {
            let m = models[(begin + i) % models.len()];
            if self.available(m, now) && !out.iter().any(|o| o == m) {
                out.push(m.to_string());
            }
        }
        // Last resort: everything still cooling, soonest recovery first.
        let mut cooling: Vec<&str> = models
            .iter()
            .copied()
            .filter(|m| !self.available(m, now))
            .collect();
        cooling.sort_by_key(|m| self.models.get(*m).map(|s| s.cooldown_until).unwrap_or(0));
        for m in cooling {
            if !out.iter().any(|o| o == m) {
                out.push(m.to_string());
            }
        }
        out
    }

    pub fn report_success(&mut self, model: &str) {
        self.current = Some(model.to_string());
        self.models.insert(model.to_string(), ModelState::default());
    }

    pub fn report_failure(&mut self, model: &str, kind: ErrKind) {
        let now = tokenade_core::platform::now_unix_secs();
        let st = self.models.entry(model.to_string()).or_default();
        st.fails = st.fails.saturating_add(1);
        let mult = 1u64 << (st.fails.saturating_sub(1)).min(6); // 2^(fails-1), capped
        let cooldown = (kind.base_secs().saturating_mul(mult)).min(MAX_COOLDOWN_SECS);
        st.cooldown_until = now.saturating_add(cooldown);
        // A failed sticky model must release the stickiness so the next call
        // rotates to a healthy one.
        if self.current.as_deref() == Some(model) {
            self.current = None;
        }
    }

    #[cfg(test)]
    fn with_state(current: Option<&str>, cooling: &[(&str, u64)]) -> Pool {
        let mut models = BTreeMap::new();
        for (m, until) in cooling {
            models.insert(
                (*m).to_string(),
                ModelState {
                    cooldown_until: *until,
                    fails: 1,
                },
            );
        }
        Pool {
            current: current.map(|s| s.to_string()),
            models,
            path: PathBuf::from("/dev/null"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODELS: &[&str] = &["a", "b", "c", "d"];

    #[test]
    fn sticky_current_is_tried_first() {
        let p = Pool::with_state(Some("c"), &[]);
        assert_eq!(p.order(MODELS)[0], "c");
    }

    #[test]
    fn rotates_after_current_and_skips_cooling() {
        let far = tokenade_core::platform::now_unix_secs() + 10_000;
        // current = a; b is cooling → order should be a, c, d, then b last.
        let p = Pool::with_state(Some("a"), &[("b", far)]);
        let ord = p.order(MODELS);
        assert_eq!(ord[0], "a");
        assert_eq!(&ord[1..3], &["c".to_string(), "d".to_string()]);
        assert_eq!(*ord.last().unwrap(), "b"); // cooling → last resort
    }

    #[test]
    fn all_cooling_still_returns_soonest_first() {
        let n = tokenade_core::platform::now_unix_secs();
        let p = Pool::with_state(
            None,
            &[
                ("a", n + 300),
                ("b", n + 50),
                ("c", n + 900),
                ("d", n + 120),
            ],
        );
        let ord = p.order(MODELS);
        assert_eq!(ord, vec!["b", "d", "a", "c"]); // by soonest cooldown expiry
    }

    #[test]
    fn failure_sets_cooldown_and_drops_stickiness() {
        let mut p = Pool::with_state(Some("a"), &[]);
        p.report_failure("a", ErrKind::RateLimited);
        assert!(
            p.current.is_none(),
            "failed sticky model releases stickiness"
        );
        assert!(
            !p.available("a", tokenade_core::platform::now_unix_secs()),
            "a is now on cooldown"
        );
        // next order() must not start with a
        assert_ne!(p.order(MODELS)[0], "a");
    }

    #[test]
    fn auth_cooldown_longer_than_ratelimit() {
        let mut a = Pool::with_state(None, &[]);
        a.report_failure("x", ErrKind::RateLimited);
        let rl = a.models["x"].cooldown_until;
        let mut b = Pool::with_state(None, &[]);
        b.report_failure("x", ErrKind::Auth);
        let au = b.models["x"].cooldown_until;
        assert!(au > rl, "auth cools down much longer than a 429");
    }

    #[test]
    fn backoff_is_exponential_then_capped() {
        let mut p = Pool::with_state(None, &[]);
        let mut prev = 0u64;
        for _ in 0..10 {
            p.report_failure("x", ErrKind::Server);
            let cd = p.models["x"]
                .cooldown_until
                .saturating_sub(tokenade_core::platform::now_unix_secs());
            assert!(cd >= prev || cd == MAX_COOLDOWN_SECS);
            assert!(cd <= MAX_COOLDOWN_SECS);
            prev = cd;
        }
        assert_eq!(prev, MAX_COOLDOWN_SECS, "eventually hits the cap");
    }

    #[test]
    fn success_clears_cooldown_and_sets_sticky() {
        let mut p = Pool::with_state(
            None,
            &[("a", tokenade_core::platform::now_unix_secs() + 999)],
        );
        p.report_success("a");
        assert_eq!(p.current.as_deref(), Some("a"));
        assert!(p.available("a", tokenade_core::platform::now_unix_secs()));
    }

    #[test]
    fn err_kind_classification() {
        assert_eq!(ErrKind::from_reason("HTTP 401"), ErrKind::Auth);
        assert_eq!(ErrKind::from_reason("HTTP 429"), ErrKind::RateLimited);
        assert_eq!(ErrKind::from_reason("HTTP 503"), ErrKind::Server);
        assert_eq!(ErrKind::from_reason("curl spawn: nope"), ErrKind::Network);
    }
}
