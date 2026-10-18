//! User-supplied command-rewrite presets (mcp-rtk-inspired).
//!
//! The built-in [`crate::command_rewrite`] rule table targets the long
//! tail of CLIs most agents touch — git, cargo, pytest, kubectl, docker,
//! npm. Users with niche stacks (helm, ansible-playbook, terraform,
//! their own internal CLI) want to add rules **without recompiling**.
//!
//! At startup we read `~/.config/tokenade/presets/*.toml`. Each file may
//! contribute one or more rules of the form:
//!
//! ```toml
//! # ~/.config/tokenade/presets/helm.toml
//! [[rule]]
//! match = "helm list"
//! rewrite = "helm list --output=json"
//! slug = "helm-list-json"
//!
//! [[rule]]
//! match_prefix = "helm install"
//! append = "--quiet"
//! slug = "helm-install-quiet"
//! # do NOT fire if any of these flags are already present
//! skip_if_contains = ["--debug", "--dry-run"]
//! ```
//!
//! Matching is line-prefix or exact, never regex — the goal is "any user
//! can write a rule in 30 seconds" not "rule writers need to know
//! lookahead assertions."
//!
//! Presets are cached for the process lifetime; restart `tokenade` to pick up
//! new files. This is intentional: hot-reload would require watching
//! every user's home dir, which is a far worse cost than the workflow
//! penalty of `pkill tokenade`.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct PresetRule {
    /// Exact whole-command match (preferred when the user's input is
    /// fully predictable).
    #[serde(default)]
    pub r#match: Option<String>,
    /// Prefix match on the command. e.g. `match_prefix = "helm install"`
    /// fires for `helm install foo`, `helm install foo --bar`, etc.
    #[serde(default)]
    pub match_prefix: Option<String>,
    /// Full rewrite — replaces the whole command. Used with `match` or
    /// `match_prefix` standalone.
    #[serde(default)]
    pub rewrite: Option<String>,
    /// Append these tokens at the end of the command (after the user's
    /// existing args). Only one of `rewrite`/`append`/`prepend` may be set.
    #[serde(default)]
    pub append: Option<String>,
    /// Insert these tokens between the program name and the user's args.
    #[serde(default)]
    pub prepend: Option<String>,
    /// Skip this rule if any of these tokens appear in the command —
    /// lets the user opt out without commenting the rule.
    #[serde(default)]
    pub skip_if_contains: Vec<String>,
    /// Human label that ends up in `gain.jsonl`. Required.
    pub slug: String,
}

#[derive(Debug, Default, Deserialize)]
struct PresetFile {
    #[serde(default)]
    rule: Vec<PresetRule>,
}

/// In-memory cache. `None` once unloaded; `Some(rules)` after the first
/// successful load. We never error on a bad TOML — log to stderr and
/// keep going with whatever loaded.
static CACHE: OnceLock<Vec<PresetRule>> = OnceLock::new();

/// Standard XDG path: `~/.config/tokenade/presets/`. Windows: the same
/// portable resolver as the rest of the binary.
pub fn presets_dir() -> PathBuf {
    crate::platform::config_dir()
        .join("tokenade")
        .join("presets")
}

/// Load every `*.toml` in `dir` and return their merged rule list.
/// Files are loaded in lexicographic order so the rule precedence is
/// deterministic across platforms (`readdir` is unordered on Linux).
/// Used by [`load`] and tests.
pub fn load_from(dir: &Path) -> Vec<PresetRule> {
    let mut rules = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return rules,
    };
    let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    for p in paths {
        let s = match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade: skipping preset {}: {e}", p.display());
                continue;
            }
        };
        let f: PresetFile = match toml::from_str(&s) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("tokenade: malformed preset {}: {e}", p.display());
                continue;
            }
        };
        for r in f.rule {
            if validate(&r) {
                rules.push(r);
            } else {
                eprintln!(
                    "tokenade: rule '{}' in {} is malformed (set exactly one of rewrite/append/prepend, with match or match_prefix)",
                    r.slug,
                    p.display(),
                );
            }
        }
    }
    rules
}

/// Get all user presets — cached for the process lifetime.
pub fn load() -> &'static [PresetRule] {
    CACHE.get_or_init(|| load_from(&presets_dir())).as_slice()
}

/// Apply user presets to `cmd`. Returns `Some((rewritten, slug))` if any
/// rule fired, else `None`. Rules are tried in load order — earlier
/// files win.
pub fn apply(cmd: &str) -> Option<(String, &'static str)> {
    for rule in load() {
        if let Some(out) = try_rule(rule, cmd) {
            return Some((out, leak(&rule.slug)));
        }
    }
    None
}

fn try_rule(rule: &PresetRule, cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();

    // Bail on opt-out patterns.
    for needle in &rule.skip_if_contains {
        if trimmed.contains(needle) {
            return None;
        }
    }

    let matched = match (&rule.r#match, &rule.match_prefix) {
        (Some(m), _) if trimmed == m.trim() => true,
        (_, Some(p)) if trimmed.starts_with(p.trim()) => true,
        _ => false,
    };
    if !matched {
        return None;
    }

    if let Some(rw) = &rule.rewrite {
        return Some(rw.clone());
    }
    if let Some(suffix) = &rule.append {
        return Some(format!("{trimmed} {}", suffix.trim()));
    }
    if let Some(insert) = &rule.prepend {
        // Insert after the program name, before any args.
        let (prog, rest) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        if rest.is_empty() {
            return Some(format!("{prog} {}", insert.trim()));
        }
        return Some(format!("{prog} {} {rest}", insert.trim()));
    }
    None
}

fn validate(r: &PresetRule) -> bool {
    if r.slug.is_empty() {
        return false;
    }
    if r.r#match.is_none() && r.match_prefix.is_none() {
        return false;
    }
    let action_count = [r.rewrite.is_some(), r.append.is_some(), r.prepend.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    action_count == 1
}

/// Leak a slug to `&'static str` so we can store it in `Rewrite`.
/// Bounded by the number of user-defined rules — fine.
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp() -> Tmp {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tokenade-presets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn write_preset(dir: &Path, name: &str, body: &str) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn empty_dir_yields_no_rules() {
        let t = tmp();
        assert!(load_from(&t.0).is_empty());
    }

    #[test]
    fn full_rewrite_rule() {
        let t = tmp();
        write_preset(
            &t.0,
            "helm.toml",
            r#"
            [[rule]]
            match = "helm list"
            rewrite = "helm list --output=json"
            slug = "helm-list-json"
            "#,
        );
        let rules = load_from(&t.0);
        assert_eq!(rules.len(), 1);

        let r = try_rule(&rules[0], "helm list").unwrap();
        assert_eq!(r, "helm list --output=json");
        assert!(try_rule(&rules[0], "helm install foo").is_none());
    }

    #[test]
    fn prefix_with_append() {
        let t = tmp();
        write_preset(
            &t.0,
            "helm.toml",
            r#"
            [[rule]]
            match_prefix = "helm install"
            append = "--quiet"
            slug = "helm-install-quiet"
            "#,
        );
        let rules = load_from(&t.0);
        let r = try_rule(&rules[0], "helm install nginx --namespace=web").unwrap();
        assert!(r.ends_with(" --quiet"));
        assert!(r.contains("nginx"));
    }

    #[test]
    fn skip_if_contains() {
        let t = tmp();
        write_preset(
            &t.0,
            "helm.toml",
            r#"
            [[rule]]
            match_prefix = "helm install"
            append = "--quiet"
            skip_if_contains = ["--debug", "--dry-run"]
            slug = "helm-install-quiet"
            "#,
        );
        let rules = load_from(&t.0);
        assert!(try_rule(&rules[0], "helm install foo --debug").is_none());
        assert!(try_rule(&rules[0], "helm install foo --dry-run").is_none());
        assert!(try_rule(&rules[0], "helm install foo").is_some());
    }

    #[test]
    fn prepend_inserts_after_program_name() {
        let t = tmp();
        write_preset(
            &t.0,
            "my.toml",
            r#"
            [[rule]]
            match_prefix = "mybin"
            prepend = "--json"
            slug = "mybin-json"
            "#,
        );
        let rules = load_from(&t.0);
        let r = try_rule(&rules[0], "mybin status verbose").unwrap();
        assert_eq!(r, "mybin --json status verbose");

        let r2 = try_rule(&rules[0], "mybin").unwrap();
        assert_eq!(r2, "mybin --json");
    }

    #[test]
    fn malformed_rule_skipped_not_panicked() {
        let t = tmp();
        // Missing slug + no action.
        write_preset(
            &t.0,
            "bad.toml",
            r#"
            [[rule]]
            match = "foo"
            "#,
        );
        // And a valid one so we know we kept loading.
        write_preset(
            &t.0,
            "good.toml",
            r#"
            [[rule]]
            match = "bar"
            rewrite = "baz"
            slug = "bar-to-baz"
            "#,
        );
        let rules = load_from(&t.0);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].slug, "bar-to-baz");
    }

    #[test]
    fn invalid_toml_skipped_not_panicked() {
        let t = tmp();
        write_preset(&t.0, "broken.toml", "this is = = not valid toml [[");
        write_preset(
            &t.0,
            "ok.toml",
            r#"
            [[rule]]
            match = "x"
            rewrite = "y"
            slug = "x-to-y"
            "#,
        );
        let rules = load_from(&t.0);
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn multiple_actions_rejected() {
        // A rule with both rewrite AND append is ambiguous — refuse it.
        let t = tmp();
        write_preset(
            &t.0,
            "x.toml",
            r#"
            [[rule]]
            match = "foo"
            rewrite = "bar"
            append = "--baz"
            slug = "ambiguous"
            "#,
        );
        let rules = load_from(&t.0);
        assert!(rules.is_empty());
    }

    #[test]
    fn rules_from_multiple_files_merge_in_order() {
        let t = tmp();
        write_preset(
            &t.0,
            "a-first.toml",
            r#"
            [[rule]]
            match = "x"
            rewrite = "from-a"
            slug = "a"
            "#,
        );
        write_preset(
            &t.0,
            "b-second.toml",
            r#"
            [[rule]]
            match = "x"
            rewrite = "from-b"
            slug = "b"
            "#,
        );
        let rules = load_from(&t.0);
        assert_eq!(rules.len(), 2);
        // Both rules load; `apply` picks the first match — order
        // depends on filesystem readdir, which is alphabetical on
        // every platform STS targets.
        let first_rewrite = try_rule(&rules[0], "x").unwrap();
        assert_eq!(first_rewrite, "from-a");
    }

    #[test]
    fn empty_slug_rejected() {
        let t = tmp();
        write_preset(
            &t.0,
            "x.toml",
            r#"
            [[rule]]
            match = "foo"
            rewrite = "bar"
            slug = ""
            "#,
        );
        let rules = load_from(&t.0);
        assert!(rules.is_empty());
    }
}
