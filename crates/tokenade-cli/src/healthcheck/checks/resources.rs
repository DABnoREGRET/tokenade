//! **What tokenade OCCUPIES** on this machine.
//!
//! Embeddings cache, MCP processes left alive, user filters, disk footprint.
//! Nothing here says an installation is broken — it says what it costs, and
//! that is another question.
//!
//! An invalid filter is the exception: it costs nothing and it does not
//! apply, so the user believes they are filtering and is not.

use super::*;

/// 7. Embedder + reranker model cache.
pub(crate) fn check_embedder_cache() -> Vec<HealthCheckResult> {
    // C13-class fix (owner report 2026-07-04): this check still probed the
    // RETIRED HuggingFace stack (~/.cache/huggingface/hub + bge-reranker),
    // reporting "NOT cached" on machines where semantic search had worked
    // for weeks. Both the embedder AND the reranker are potion-code-16M
    // via model2vec now, cached under <cache_dir>/model/.
    let mut out = Vec::new();
    let Some(c) = tokenade_core::platform::cache_dir() else {
        return out;
    };
    let model_root = c.join("tokenade").join("model");
    let cached = std::fs::read_dir(&model_root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("potion-code") && e.path().is_dir());
    out.push(hc(
        "semantic_model",
        if cached {
            CheckStatus::Ok
        } else {
            CheckStatus::Info
        },
        if cached {
            "semantic model cached (potion-code, embed + rerank)".to_string()
        } else {
            "semantic model NOT cached (fetched once, ~30 MB, on first `semantic` use)".to_string()
        },
        vec![],
    ));
    out
}

/// 8. Stale MCP processes (#466 / #467 / #503).
#[cfg(target_os = "linux")]
pub(crate) fn check_stale_mcp_processes() -> HealthCheckResult {
    let stale_pids = detect_stale_mcp_pids();
    if stale_pids.is_empty() {
        hc(
            "stale_mcp_procs",
            CheckStatus::Info,
            "no stale tokenade MCP processes detected".into(),
            vec![],
        )
    } else {
        let mut detail: Vec<String> = stale_pids
            .iter()
            .take(3)
            .map(|(pid, path)| format!("- pid {pid} → {path}"))
            .collect();
        if stale_pids.len() > 3 {
            detail.push(format!("... {} more", stale_pids.len() - 3));
        }
        hc(
            "stale_mcp_procs",
            CheckStatus::Fail,
            format!(
                "{} tokenade MCP process(es) running an OLD binary — restart your AI agent to pick up the current build",
                stale_pids.len()
            ),
            detail,
        )
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn check_stale_mcp_processes() -> HealthCheckResult {
    hc(
        "stale_mcp_procs",
        CheckStatus::Info,
        "stale-MCP-process check skipped (Linux only)".into(),
        vec![],
    )
}

/// Filter files that do not parse. FAIL and not INFO: unlike an untrusted
/// project filter (declining to trust one is a legitimate steady state), a
/// broken file has no steady state where keeping it is right, and nothing
/// anywhere used to say so —
/// `load_all` dropped it without a word and the user saw a compactor that
/// simply never fired.
pub(crate) fn check_invalid_filters() -> HealthCheckResult {
    let broken = tokenade_core::user_filters::invalid_filters();
    if broken.is_empty() {
        return hc(
            "filter-syntax",
            CheckStatus::Ok,
            "every compactor file parses".to_string(),
            Vec::new(),
        );
    }
    let mut detail: Vec<String> = broken
        .iter()
        .take(5)
        .map(|(path, reason)| {
            // The reason is multi-line for a regex error; the first line is
            // the one that names what is wrong.
            let first = reason.lines().next().unwrap_or(reason).trim();
            format!("{}: {first}", path.display())
        })
        .collect();
    if broken.len() > 5 {
        detail.push(format!("… {} more", broken.len() - 5));
    }
    detail.push(
        "fix the file, or delete it — an unparseable compactor applies to nothing".to_string(),
    );
    hc(
        "filter-syntax",
        CheckStatus::Fail,
        format!(
            "{} compactor file(s) do not parse, so they never fire",
            broken.len()
        ),
        detail,
    )
}

/// Pending project-local filters, surfaced as INFO (they are not a failure —
/// declining to trust one is a legitimate steady state).
pub(crate) fn check_untrusted_project_filters() -> HealthCheckResult {
    let pending = tokenade_core::user_filters::untrusted_project_filters();
    if pending.is_empty() {
        return hc(
            "project-filters",
            CheckStatus::Ok,
            "no project-local compactors awaiting approval".to_string(),
            Vec::new(),
        );
    }
    let mut detail: Vec<String> = pending
        .iter()
        .take(5)
        .map(|(path, name)| format!("{name} ({})", path.display()))
        .collect();
    if pending.len() > 5 {
        detail.push(format!("… {} more", pending.len() - 5));
    }
    detail.push("approve with `tokenade trust-filters` (or inspect first with --list)".to_string());
    hc(
        "project-filters",
        CheckStatus::Info,
        format!(
            "{} project-local compactor(s) in this repository are not approved, so they do nothing",
            pending.len()
        ),
        detail,
    )
}

/// What tokenade is holding on disk, and where.
///
/// Nothing anywhere reported this. On the development machine `~/.tokenade` had
/// grown to **21 GB** — 20 of them local-model weights downloaded once in July
/// — and the only way to find out was `du`. A tool that stores caches, a stash,
/// several indexes and optionally multi-gigabyte model weights owes the user a
/// line saying so before their disk says it for us.
///
/// `Info`, never `Fail`: a large cache is not a fault, it is a fact the owner
/// of the disk should be able to see. It turns into a warning only past a size
/// nobody chooses by accident.
pub(crate) fn check_disk_footprint(home: &std::path::Path) -> HealthCheckResult {
    use tokenade_core::platform::{dir_bytes, human_bytes as human};
    let root = home.join(".tokenade");
    let cache = tokenade_core::platform::cache_dir().map(|c| c.join("tokenade"));
    let total = dir_bytes(&root) + cache.as_deref().map(dir_bytes).unwrap_or(0);
    // Name the biggest contributors rather than a single opaque number: the
    // useful question is not "how much" but "what, and can I drop it".
    let mut parts: Vec<(String, u64)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let size = match e.file_type() {
                // Files count too: the dedup and symbol indexes are single
                // files at the root, and listing only directories would rank a
                // 113 MB directory above a 168 MB file sitting next to it.
                Ok(t) if t.is_dir() => dir_bytes(&e.path()),
                _ => e.metadata().map(|m| m.len()).unwrap_or(0),
            };
            (e.file_name().to_string_lossy().to_string(), size)
        })
        .collect();
    if let Some(c) = &cache {
        parts.push(("(cache)".to_string(), dir_bytes(c)));
    }
    parts.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    let mut detail: Vec<String> = parts
        .iter()
        .take(3)
        .filter(|(_, b)| *b > 64 * 1_048_576)
        .map(|(name, b)| format!("{name}: {}", human(*b)))
        .collect();
    // A gigabyte of caches is worth an actionable line; a hundred megabytes is
    // not, and saying so every time would train the reader to skip the block.
    const LOUD_AT: u64 = 5 * 1_073_741_824;
    if total >= LOUD_AT {
        detail.push(
            "reclaim what has aged out with `tokenade evict-stale`; add `--models` \
             to that command to drop local-agent model weights too"
                .to_string(),
        );
    }
    hc(
        "disk-footprint",
        if total >= LOUD_AT {
            CheckStatus::Info
        } else {
            CheckStatus::Ok
        },
        format!("tokenade is holding {} on disk", human(total)),
        detail,
    )
}
