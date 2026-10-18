//! `mcp_stats` — rollup statistics over MCP server manifests, plus a
//! token-aware **skills index** (the `tool-search-mcp` idea from a
//! competitor analysis).
//!
//! Two orthogonal things an agent host wastes context on:
//!   1. MCP server manifests — how many servers are configured, how many are
//!      already routed through tokenade's wrapper, across all known configs.
//!   2. Skills — every `SKILL.md` frontmatter (name + one-line description)
//!      that a host eagerly loads into the system prompt. Indexing them lets a
//!      host load the cheap *index* and disclose the full skill body on demand.
//!
//! This module is the **pure** core: the rollup math and the frontmatter
//! parse. All IO (enumerating configs via `mcp_wrap`, walking skill dirs)
//! lives in the CLI wrapper `cmd_mcp_stats`, so the logic here is
//! deterministic and unit-testable without touching the filesystem.

use crate::tokenizer;

/// Aggregate counts across every MCP config tokenade knows about.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ManifestRollup {
    /// Number of distinct config files that contained at least one server.
    pub configs: usize,
    /// Total MCP servers declared across all configs.
    pub servers_total: usize,
    /// Of those, how many are already routed through the tokenade wrapper.
    pub servers_wrapped: usize,
}

impl ManifestRollup {
    /// Fraction of servers already wrapped, in `[0.0, 1.0]`. 0.0 when there
    /// are no servers (avoids divide-by-zero).
    pub fn wrapped_fraction(&self) -> f64 {
        if self.servers_total == 0 {
            return 0.0;
        }
        self.servers_wrapped as f64 / self.servers_total as f64
    }

    /// Servers not yet wrapped — the actionable "left on the table" number.
    pub fn servers_unwrapped(&self) -> usize {
        self.servers_total.saturating_sub(self.servers_wrapped)
    }
}

/// Build a rollup from per-config `(total_servers, wrapped_servers)` pairs.
/// Configs with zero servers are not counted (an empty `.mcp.json` is noise).
pub fn rollup(per_config: &[(usize, usize)]) -> ManifestRollup {
    let mut r = ManifestRollup::default();
    for &(total, wrapped) in per_config {
        if total == 0 {
            continue;
        }
        r.configs += 1;
        r.servers_total += total;
        // A config can never report more wrapped than it has servers; clamp
        // defensively so a bad caller can't make the fraction exceed 1.0.
        r.servers_wrapped += wrapped.min(total);
    }
    r
}

/// One indexed skill: its name, one-line description, and the token cost of
/// the *full* frontmatter blob it would otherwise contribute to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    /// Tokens the full `name + description` pair costs when loaded eagerly.
    pub tokens: usize,
}

/// A token-aware index over all discovered skills.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SkillIndex {
    pub entries: Vec<SkillEntry>,
}

impl SkillIndex {
    /// Tokens the host pays today by eager-loading every skill's name +
    /// description into the system prompt.
    pub fn eager_tokens(&self) -> usize {
        self.entries.iter().map(|e| e.tokens).sum()
    }

    /// Tokens a *searchable index* costs instead: just the names (the
    /// descriptions are disclosed on demand once a name matches). This is the
    /// `tool-search-mcp` lever — pay for the menu, not every dish.
    pub fn index_tokens(&self) -> usize {
        self.entries
            .iter()
            .map(|e| tokenizer::estimate(&e.name))
            .sum()
    }

    /// Tokens saved by switching from eager-load to index-then-disclose.
    pub fn savings(&self) -> usize {
        self.eager_tokens().saturating_sub(self.index_tokens())
    }
}

/// Parse a `SKILL.md` body, returning `(name, description)` from its YAML
/// frontmatter. Returns `None` when there is no frontmatter or no `name:`.
///
/// Frontmatter is the leading `---`-fenced block. We only need two scalar
/// keys, so this is a deliberately tiny line scanner rather than a full YAML
/// parse — it tolerates quotes and ignores everything else in the block.
pub fn parse_skill_frontmatter(md: &str) -> Option<(String, String)> {
    let body = md.strip_prefix("---")?;
    // The frontmatter ends at the next line that is exactly `---`.
    let end = body.find("\n---")?;
    let front = &body[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    for line in front.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(unquote(v.trim()));
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(unquote(v.trim()));
        }
    }
    let name = name?;
    if name.is_empty() {
        return None;
    }
    Some((name, description.unwrap_or_default()))
}

/// Build a `SkillEntry` from a parsed name/description, measuring its eager
/// token cost.
pub fn skill_entry(name: String, description: String) -> SkillEntry {
    let tokens = tokenizer::estimate(&name) + tokenizer::estimate(&description);
    SkillEntry {
        name,
        description,
        tokens,
    }
}

/// Strip one layer of matching single/double quotes from a scalar value.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_sums_and_skips_empty_configs() {
        let r = rollup(&[(3, 1), (0, 0), (2, 2)]);
        assert_eq!(r.configs, 2);
        assert_eq!(r.servers_total, 5);
        assert_eq!(r.servers_wrapped, 3);
        assert_eq!(r.servers_unwrapped(), 2);
    }

    #[test]
    fn rollup_clamps_overreported_wrapped() {
        // A buggy caller claiming 9 wrapped out of 2 servers can't exceed 2.
        let r = rollup(&[(2, 9)]);
        assert_eq!(r.servers_wrapped, 2);
        assert!((r.wrapped_fraction() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn wrapped_fraction_safe_on_empty() {
        assert_eq!(ManifestRollup::default().wrapped_fraction(), 0.0);
    }

    #[test]
    fn parse_frontmatter_basic() {
        let md = "---\nname: deploy\ndescription: ship the build to prod\n---\n# Deploy\nbody";
        let (n, d) = parse_skill_frontmatter(md).unwrap();
        assert_eq!(n, "deploy");
        assert_eq!(d, "ship the build to prod");
    }

    #[test]
    fn parse_frontmatter_unquotes() {
        let md = "---\nname: \"my-skill\"\ndescription: 'does a thing'\n---\nx";
        let (n, d) = parse_skill_frontmatter(md).unwrap();
        assert_eq!(n, "my-skill");
        assert_eq!(d, "does a thing");
    }

    #[test]
    fn parse_frontmatter_requires_name() {
        assert!(parse_skill_frontmatter("---\ndescription: x\n---\nbody").is_none());
        assert!(parse_skill_frontmatter("no frontmatter here").is_none());
    }

    #[test]
    fn skill_index_savings_is_index_vs_eager() {
        let idx = SkillIndex {
            entries: vec![
                skill_entry("a".into(), "a fairly long description here".into()),
                skill_entry("b".into(), "another long description string".into()),
            ],
        };
        // Index (names only) must be strictly cheaper than eager (names+desc).
        assert!(idx.index_tokens() < idx.eager_tokens());
        assert_eq!(idx.savings(), idx.eager_tokens() - idx.index_tokens());
    }
}
