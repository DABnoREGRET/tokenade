//! `metrics` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Realistic `/metrics` scrape: 100 metrics × 30 label combos each (3 000 lines).
pub(crate) fn task_metrics_real(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    for m in 0..100 {
        t.push_str(&format!(
            "# HELP metric_{m} Description of metric {m} for benchmark.\n# TYPE metric_{m} counter\n"
        ));
        for i in 0..30 {
            t.push_str(&format!(
                "metric_{m}{{instance=\"host{i}\",job=\"app\",region=\"eu-west-1\"}} {i}\n"
            ));
        }
    }
    let (out, sav, ms) = measure(&t, metrics::compact_with_savings);
    // Quality: all 100 metric names visible, label-combo cap marker present,
    // sample row from metric_0 still rendered.
    let names_kept = out.contains("metric_0 (counter)") && out.contains("metric_99 (counter)");
    let cap_marker = out.contains("more label-combo(s)");
    let sample_row = out.contains("metric_0{instance=\"host0\"");
    let quality = if names_kept && cap_marker && sample_row {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:metrics-compact",
        "metrics",
        "prom-100x30",
        &t,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #39: histogram metric with 100 buckets — must be summarised
// (first 3 + last 3 + `…N more histogram sample(s)…`), not all 100 emitted.
pub(crate) fn adv_metrics_histogram_explosion(results: &mut Vec<RunResult>) {
    let mut t = String::from("# HELP h Time spent on requests.\n# TYPE h histogram\n");
    let buckets: Vec<f64> = (0..100).map(|i| 0.001 * 2_f64.powi(i)).collect();
    for b in &buckets {
        t.push_str(&format!("h_bucket{{le=\"{b}\"}} 42\n"));
    }
    t.push_str("h_count 4200\nh_sum 1234.56\n");
    let (out, sav, ms) = measure(&t, metrics::compact_with_savings);
    let quality = if out.contains("# h (histogram)")
        && out.contains("more histogram sample(s)")
        && out.contains("h_count 4200")
        && out.contains("h_sum 1234.56")
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:metrics:histogram-explosion",
        "metrics",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #40: a single metric with 10 000 label combinations (cardinality
// blow-up — real prod incident shape). Must cap, complete <500ms, keep name.
pub(crate) fn adv_metrics_cardinality_blowup(results: &mut Vec<RunResult>) {
    let mut t =
        String::from("# HELP big_metric A high-cardinality counter.\n# TYPE big_metric counter\n");
    for i in 0..10_000 {
        t.push_str(&format!(
            "big_metric{{user_id=\"{i:06}\",action=\"click\"}} 1\n"
        ));
    }
    let (out, sav, ms) = measure(&t, metrics::compact_with_savings);
    let quality = if out.contains("big_metric (counter)")
        && out.contains("more label-combo(s)")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:metrics:cardinality-blowup",
        "metrics",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}
