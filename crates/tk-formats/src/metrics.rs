//! Prometheus text-exposition-format compaction.
//!
//! A typical `/metrics` endpoint emits hundreds of metrics, each with one
//! `# HELP`, one `# TYPE`, and then N label-combination rows. Histogram
//! metrics explode: a single `http_request_duration_seconds_bucket` can
//! emit a `_bucket{le="..."}` line per quantile (12+ rows) **per label
//! combination**. A real `/metrics` scrape can be 50 000 lines.
//!
//! For an agent the signal is:
//!   * **what metrics exist** (names),
//!   * **what types they are** (counter/gauge/histogram/summary),
//!   * **summary stats per metric** (count of label combos, sample row),
//!   * **the actual values for cardinality ≤ small**.
//!
//! [`compact`] groups lines by metric base name, keeps one HELP+TYPE pair,
//! caps label-combination rows at [`PER_METRIC_CAP`], and reports the
//! eliminated row count.

use std::collections::BTreeMap;
use std::fmt::Write;

/// Beyond this many label combinations for a single metric, fold the rest.
pub const PER_METRIC_CAP: usize = 8;

pub fn compact(text: &str) -> String {
    if !looks_like_prometheus(text) {
        return text.to_string();
    }
    // Group lines by metric base name (the prefix before `{` or whitespace).
    // BTreeMap to keep the output deterministic.
    let mut metrics: BTreeMap<String, MetricRows> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# HELP ") {
            if let Some((name, msg)) = rest.split_once(' ') {
                metrics
                    .entry(name.to_string())
                    .or_insert_with(|| {
                        order.push(name.to_string());
                        MetricRows::default()
                    })
                    .help = Some(msg.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            if let Some((name, ty)) = rest.split_once(' ') {
                metrics
                    .entry(name.to_string())
                    .or_insert_with(|| {
                        order.push(name.to_string());
                        MetricRows::default()
                    })
                    .type_ = Some(ty.to_string());
            }
            continue;
        }
        if line.starts_with('#') {
            // Unknown comment — drop.
            continue;
        }
        // Sample row: `name{labels} value [timestamp]` OR `name value`.
        let name = metric_base_name(line);
        let entry = metrics.entry(name.clone()).or_insert_with(|| {
            order.push(name.clone());
            MetricRows::default()
        });
        entry.rows.push(line.to_string());
    }

    // Render.
    let mut out = String::with_capacity(text.len() / 4);
    let mut total_metrics = 0usize;
    let mut total_rows = 0usize;
    let mut elided = 0usize;
    for name in &order {
        let Some(m) = metrics.get(name) else { continue };
        total_metrics += 1;
        total_rows += m.rows.len();
        // HELP / TYPE one-liner.
        let help = m.help.as_deref().unwrap_or("");
        let ty = m.type_.as_deref().unwrap_or("?");
        if !help.is_empty() {
            let _ = writeln!(out, "# {name} ({ty}): {help}");
        } else {
            let _ = writeln!(out, "# {name} ({ty})");
        }
        // Histograms / summaries: aggregate buckets.
        if (ty == "histogram" || ty == "summary") && m.rows.len() > PER_METRIC_CAP {
            // Summarise: count + first 3 buckets + last 3 buckets.
            for r in m.rows.iter().take(3) {
                out.push_str(r);
                out.push('\n');
            }
            let elide_n = m.rows.len() - 6;
            if elide_n > 0 {
                let _ = writeln!(out, "# … {elide_n} more {ty} sample(s) …");
                elided += elide_n;
            }
            for r in m.rows.iter().rev().take(3).rev() {
                out.push_str(r);
                out.push('\n');
            }
            continue;
        }
        // Regular metric: cap at PER_METRIC_CAP.
        for r in m.rows.iter().take(PER_METRIC_CAP) {
            out.push_str(r);
            out.push('\n');
        }
        if m.rows.len() > PER_METRIC_CAP {
            let elide_n = m.rows.len() - PER_METRIC_CAP;
            let _ = writeln!(out, "# … {elide_n} more label-combo(s) …");
            elided += elide_n;
        }
    }
    let header = format!(
        "[tokenade:metrics] {total_metrics} metric(s), {total_rows} row(s), {elided} elided\n"
    );
    format!("{header}{}", out.trim_end())
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[derive(Default)]
struct MetricRows {
    help: Option<String>,
    type_: Option<String>,
    rows: Vec<String>,
}

/// Heuristic: at least two `# TYPE …` lines OR multiple `metric_name{...}` rows
/// in the first 40 lines.
fn looks_like_prometheus(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);
    head.lines().any(|l| l.starts_with("# TYPE "))
        || head
            .lines()
            .take(30)
            .filter(|l| {
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    return false;
                }
                let last = t.split_whitespace().last().unwrap_or("");
                last.parse::<f64>().is_ok() || matches!(last, "NaN" | "+Inf" | "-Inf")
            })
            .nth(2)
            .is_some()
}

/// Strip labels suffix from a sample row to get the metric base name.
/// `http_requests_total{method="post"} 1` → `http_requests_total`
/// Also strip the histogram/summary suffix (`_bucket`/`_count`/`_sum`) so
/// related rows group together — they're emitted under the same `# TYPE`.
fn metric_base_name(line: &str) -> String {
    let t = line.trim_start();
    let end = t
        .find(|c: char| c == '{' || c.is_whitespace())
        .unwrap_or(t.len());
    let name = &t[..end];
    for suffix in ["_bucket", "_count", "_sum"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_prometheus() {
        let t = "# HELP foo bar\n# TYPE foo counter\nfoo 42\n";
        assert!(looks_like_prometheus(t));
        assert!(!looks_like_prometheus(
            "not prometheus\njust prose\nwith no metrics\n"
        ));
    }

    #[test]
    fn keeps_one_help_type_per_metric() {
        let t = "# HELP http_requests_total Total HTTP requests.\n\
                 # TYPE http_requests_total counter\n\
                 http_requests_total{method=\"get\",code=\"200\"} 1027\n\
                 http_requests_total{method=\"post\",code=\"200\"} 500\n";
        let out = compact(t);
        assert!(out.contains("http_requests_total (counter): Total HTTP requests."));
        assert!(out.contains("http_requests_total{method=\"get\",code=\"200\"} 1027"));
        assert!(out.contains("http_requests_total{method=\"post\",code=\"200\"} 500"));
    }

    #[test]
    fn caps_high_cardinality_metric() {
        let mut t = String::from("# HELP m help\n# TYPE m counter\n");
        for i in 0..40 {
            let _ = writeln!(t, "m{{label=\"x{i}\"}} {i}");
        }
        let out = compact(&t);
        assert!(out.contains("# m (counter)"));
        assert!(out.contains("m{label=\"x0\"} 0"));
        // Beyond cap, the rest is folded.
        assert!(out.contains("more label-combo(s)"));
        assert!(!out.contains("m{label=\"x39\"}"));
    }

    #[test]
    fn histogram_summarised() {
        let mut t = String::from("# HELP h help\n# TYPE h histogram\n");
        for le in &[
            "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10", "+Inf",
        ] {
            let _ = writeln!(t, "h_bucket{{le=\"{le}\"}} 100");
        }
        t.push_str("h_count 1200\n");
        t.push_str("h_sum 600.5\n");
        let out = compact(&t);
        assert!(out.contains("# h (histogram)"));
        // First 3 buckets + last 3 lines kept; middle folded.
        assert!(out.contains("h_bucket{le=\"0.005\"}"));
        assert!(out.contains("more histogram sample(s)"));
        assert!(out.contains("h_sum 600.5"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "totally not prometheus output\njust prose\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        // 50 metrics × 20 label combos each.
        let mut t = String::new();
        for m in 0..50 {
            let _ = write!(
                t,
                "# HELP metric_{m} Description of metric {m}.\n# TYPE metric_{m} counter\n"
            );
            for i in 0..20 {
                let _ = writeln!(
                    t,
                    "metric_{m}{{instance=\"host{i}\",region=\"eu-west-1\"}} {i}"
                );
            }
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("50 metric(s)"));
        assert!(sav.percent() > 50.0, "got {:?}", sav);
    }
}
