//! **The workload table** — the reduction NEXT TO the accuracy.
//!
//! A reduction figure alone proves nothing: one reduces perfectly by
//! returning nothing. The table puts the two columns side by side, one
//! row per task family, so that a reader sees the trade-off at a
//! glance.
//!
//! It is the publishable form of the "no quality loss" promise, and it
//! is publishable only because it carries the counter-example with it.

use super::*;

// ── workload table (headroom-style aggregation) ───────────────────────────────
//
// headroom's signature artifact is a *workload table*: one row per task family,
// putting token-reduction next to task accuracy so a reader sees the trade-off
// at a glance. We group the per-task rows by their `domain:` prefix and emit the
// same cross-tab — the publishable "no quality loss" evidence for the product.

/// The workload domain of a task id (`domain:slug` → `domain`).
pub(crate) fn domain_of(id: &str) -> &str {
    id.split_once(':').map(|(d, _)| d).unwrap_or(id)
}

/// One offline workload row: a domain with task count, median savings, and
/// answer-retention among the tasks that were *meaningfully* compacted.
#[derive(Serialize)]
pub(crate) struct WorkloadRow {
    pub(crate) domain: String,
    pub(crate) tasks: usize,
    pub(crate) median_savings_pct: f64,
    pub(crate) exercised: usize,
    pub(crate) survived: usize,
    pub(crate) retention_pct: f64,
}

pub(crate) fn workload_rows(rows: &[OfflineRow]) -> Vec<WorkloadRow> {
    let mut by: BTreeMap<&str, Vec<&OfflineRow>> = BTreeMap::new();
    for r in rows {
        by.entry(domain_of(&r.id)).or_default().push(r);
    }
    by.into_iter()
        .map(|(domain, rs)| {
            let exercised: Vec<&&OfflineRow> =
                rs.iter().filter(|r| r.meaningfully_compacted).collect();
            let survived = exercised.iter().filter(|r| r.answer_survived).count();
            WorkloadRow {
                domain: domain.to_string(),
                tasks: rs.len(),
                median_savings_pct: median(rs.iter().map(|r| r.savings_pct).collect()),
                exercised: exercised.len(),
                survived,
                retention_pct: 100.0 * survived as f64 / exercised.len().max(1) as f64,
            }
        })
        .collect()
}

/// Render the offline workload table as GitHub-flavoured markdown.
pub(crate) fn render_workload_md(rows: &[OfflineRow]) -> String {
    let wl = workload_rows(rows);
    let mut s = String::from("# tokenade accuracy workload (offline answer-survival)\n\n");
    s.push_str("| domain | tasks | median savings | exercised | answer retention |\n");
    s.push_str("|--------|------:|---------------:|----------:|-----------------:|\n");
    for r in &wl {
        s.push_str(&format!(
            "| {} | {} | {:.1}% | {}/{} | {}/{} ({:.0}%) |\n",
            r.domain,
            r.tasks,
            r.median_savings_pct,
            r.exercised,
            r.tasks,
            r.survived,
            r.exercised,
            r.retention_pct,
        ));
    }
    let total = rows.len();
    let exercised: Vec<&OfflineRow> = rows.iter().filter(|r| r.meaningfully_compacted).collect();
    let survived = exercised.iter().filter(|r| r.answer_survived).count();
    let tb: usize = rows.iter().map(|r| r.tokens_before).sum();
    let ta: usize = rows.iter().map(|r| r.tokens_after).sum();
    let overall = if tb == 0 {
        0.0
    } else {
        100.0 * (1.0 - ta as f64 / tb as f64)
    };
    s.push_str(&format!(
        "| **all** | **{}** | **{:.1}%** | **{}/{}** | **{}/{} ({:.0}%)** |\n",
        total,
        overall,
        exercised.len(),
        total,
        survived,
        exercised.len(),
        100.0 * survived as f64 / exercised.len().max(1) as f64,
    ));
    s
}
