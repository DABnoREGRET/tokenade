//! Symbol-graph centrality (out-degree-normalized PageRank) used to RANK
//! code-intelligence results (`query`, `impact`), which were previously
//! unordered within a match tier.
//!
//! Ported concept from Atelier's `call_graph_centrality`: a plain random walk
//! over call edges ranks ubiquitous helpers (`log`, `new`, `IS_ERR`) as the
//! most "central" symbols, which is useless noise. Standard PageRank fixes it
//! by distributing each node's score across its OUT-neighbours (so a node that
//! calls many things spreads thin influence, while a node CALLED by many
//! accumulates it). We run it over the index call graph (`caller -> callee`),
//! so heavily-depended-on symbols float to the top of a result list.

use std::collections::HashMap;

use crate::index::Call;

/// PageRank damping — the canonical 0.85 (probability the walk follows an
/// edge vs. teleports). Matches Atelier's parameters.
pub const DAMPING: f64 = 0.85;
const MAX_ITER: usize = 50;
const TOL: f64 = 1e-6;

/// PageRank score per symbol NAME over the `caller -> callee` call graph,
/// normalized so the top symbol scores `1.0` (a graph-size-independent boost
/// in `[0, 1]`). Empty / edgeless graph → empty map (callers treat a missing
/// name as centrality 0, i.e. no change to the existing ordering).
pub fn symbol_centrality(calls: &[Call]) -> HashMap<String, f64> {
    if calls.is_empty() {
        return HashMap::new();
    }
    // Intern node names → dense ids (borrow from `calls`, which outlives us).
    let mut id: HashMap<&str, usize> = HashMap::new();
    for c in calls {
        let k = id.len();
        id.entry(c.caller_name.as_str()).or_insert(k);
        let k = id.len();
        id.entry(c.callee_name.as_str()).or_insert(k);
    }
    let n = id.len();
    // Weighted out-adjacency (parallel call edges collapse to a count weight).
    let mut out: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
    let mut out_weight: Vec<f64> = vec![0.0; n];
    for c in calls {
        let u = id[c.caller_name.as_str()];
        let v = id[c.callee_name.as_str()];
        if u == v {
            continue; // self-recursion adds no ranking signal
        }
        *out[u].entry(v).or_insert(0.0) += 1.0;
        out_weight[u] += 1.0;
    }
    // Power iteration with dangling-mass redistribution.
    let mut rank = vec![1.0 / n as f64; n];
    let base = (1.0 - DAMPING) / n as f64;
    for _ in 0..MAX_ITER {
        let mut next = vec![base; n];
        // Sink nodes (no out-edges) would leak rank; spread theirs uniformly.
        let dangling: f64 = (0..n)
            .filter(|&u| out_weight[u] == 0.0)
            .map(|u| rank[u])
            .sum();
        let dangle_share = DAMPING * dangling / n as f64;
        for u in 0..n {
            if out_weight[u] == 0.0 {
                continue;
            }
            let share = DAMPING * rank[u];
            for (&v, &w) in &out[u] {
                next[v] += share * (w / out_weight[u]);
            }
        }
        let mut delta = 0.0;
        for (i, x) in next.iter_mut().enumerate() {
            *x += dangle_share;
            delta += (*x - rank[i]).abs();
        }
        rank = next;
        if delta < TOL {
            break;
        }
    }
    // Normalize to max = 1 and map back to names.
    let max = rank.iter().copied().fold(0.0_f64, f64::max);
    let mut result = HashMap::with_capacity(n);
    if max > 0.0 {
        for (name, &i) in &id {
            result.insert(name.to_string(), rank[i] / max);
        }
    }
    result
}

/// Aggregate symbol centrality to the FILE level (max over a file's symbols),
/// so `impact` can order dependent FILES by how central their code is — a
/// dependent that is itself widely used means a larger real blast radius.
/// `symbols` is `(symbol_name, file)` pairs from the index.
pub fn file_centrality<'a>(
    symbol_scores: &HashMap<String, f64>,
    symbols: impl Iterator<Item = (&'a str, &'a str)>,
) -> HashMap<String, f64> {
    let mut files: HashMap<String, f64> = HashMap::new();
    for (name, file) in symbols {
        if let Some(&s) = symbol_scores.get(name) {
            let e = files.entry(file.to_string()).or_insert(0.0);
            if s > *e {
                *e = s;
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(caller: &str, callee: &str) -> Call {
        Call {
            caller_name: caller.to_string(),
            callee_name: callee.to_string(),
            file: "f.rs".to_string(),
            line: 1,
            qualifier: String::new(),
        }
    }

    #[test]
    fn empty_graph_is_empty() {
        assert!(symbol_centrality(&[]).is_empty());
    }

    #[test]
    fn heavily_called_symbol_ranks_top() {
        // a, b, c all call `core`; `core` calls nothing → core accumulates rank.
        let calls = vec![call("a", "core"), call("b", "core"), call("c", "core")];
        let cen = symbol_centrality(&calls);
        let core = cen["core"];
        assert!(
            (core - 1.0).abs() < 1e-9,
            "most-called symbol normalizes to 1.0"
        );
        assert!(cen["a"] < core && cen["b"] < core && cen["c"] < core);
    }

    #[test]
    fn out_degree_normalization_downweights_a_spreader() {
        // `hub` calls everything (spreads thin); `target` is called by two
        // distinct callers → target must out-rank a leaf that hub also calls.
        let calls = vec![
            call("hub", "target"),
            call("other", "target"),
            call("hub", "leaf"),
            call("hub", "misc"),
        ];
        let cen = symbol_centrality(&calls);
        assert!(
            cen["target"] > cen["leaf"],
            "twice-called beats once-called"
        );
    }

    #[test]
    fn scores_are_normalized_to_unit_max() {
        let calls = vec![call("a", "b"), call("b", "c"), call("a", "c")];
        let cen = symbol_centrality(&calls);
        let max = cen.values().copied().fold(0.0_f64, f64::max);
        assert!((max - 1.0).abs() < 1e-9);
    }
}
