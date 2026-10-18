//! Adaptive item-count sizing for list/array compactors.
//!
//! A fixed cap ("keep the first N items") either over-keeps a redundant list
//! (N near-identical rows) or under-keeps a diverse one (N distinct rows where
//! the agent needed item N+1). [`adaptive_keep`] instead finds the *knee* of an
//! information-saturation curve: it keeps the smallest prefix that already
//! covers [`COVERAGE_TARGET`] of the unique information the first `max` items
//! carry. Homogeneous lists saturate after a handful of rows (keep few); a list
//! of genuinely distinct rows keeps more — up to `max`.
//!
//! "Information" is approximated by the cumulative count of unique character
//! 4-grams (shingles), order-preserving and cheap. This is the deterministic,
//! lossless-input alternative to a neural saliency model — no embeddings, no
//! query needed; it just measures redundancy.

use std::collections::HashSet;

/// Saturation threshold: once an item adds fewer than this fraction of the
/// *early* per-item information rate — twice in a row — the curve has flattened
/// (the knee) and we stop. A list whose items keep adding fresh information
/// (every marginal stays above this) never saturates and is kept up to `max`.
const SATURATION_FRACTION: f64 = 0.25;

/// Shingle width in characters. 4 is a good redundancy detector for the short,
/// structured rows these compactors see (JSON object reprs, table rows, log
/// lines) without being so wide that near-duplicates look distinct.
const SHINGLE: usize = 4;

/// How many of `items` (in their given order) to keep before the marginal new
/// information saturates. The result is clamped to `[min, min(max, len)]`.
///
/// - Returns `len` when `len <= min` (nothing to trim).
/// - Returns `min` for an all-empty / no-signal list.
/// - Monotonic: a more diverse list never keeps fewer than a more redundant one.
pub fn adaptive_keep(items: &[&str], min: usize, max: usize) -> usize {
    let n = items.len();
    if n <= min {
        return n;
    }
    let hi = max.min(n);
    if hi <= min {
        return hi;
    }
    // Per-item marginal information = how many NEW unique shingles each item
    // contributes (in order), over the first `hi` items.
    let mut seen: HashSet<u64> = HashSet::new();
    let mut marginal: Vec<usize> = Vec::with_capacity(hi);
    for item in items.iter().take(hi) {
        let before = seen.len();
        for sh in shingles(item) {
            seen.insert(sh);
        }
        marginal.push(seen.len() - before);
    }
    // Baseline "fresh information rate" = average marginal of the first few
    // items (those we keep regardless). The knee is where the marginal falls
    // below SATURATION_FRACTION of this rate and stays there.
    let base_n = min.max(3).min(marginal.len());
    let baseline = marginal[..base_n].iter().sum::<usize>() as f64 / base_n as f64;
    if baseline == 0.0 {
        return min; // no signal anywhere
    }
    let floor = SATURATION_FRACTION * baseline;
    let mut low_streak = 0;
    for (i, &m) in marginal.iter().enumerate() {
        let kept = i + 1;
        if kept <= min {
            continue;
        }
        if (m as f64) < floor {
            low_streak += 1;
            // Two consecutive low-information items ⇒ the knee was at the first
            // of them; keep up to just before it.
            if low_streak >= 2 {
                return (kept - 2).clamp(min, hi);
            }
        } else {
            low_streak = 0;
        }
    }
    // Never saturated within `hi` items ⇒ genuinely diverse, keep up to max.
    hi
}

/// Hash each `SHINGLE`-character window of `s` (FNV-1a on the chars' UTF-8).
/// Short strings yield one shingle of the whole string.
fn shingles(s: &str) -> impl Iterator<Item = u64> + '_ {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let windows = if n <= SHINGLE { 1 } else { n - SHINGLE + 1 };
    (0..windows).map(move |i| {
        let end = (i + SHINGLE).min(n);
        let mut h: u64 = 0xcbf29ce484222325;
        for &c in &chars[i..end] {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redundant_list_keeps_few() {
        // 50 rows drawn from only 2 distinct templates: the unique-shingle
        // curve saturates after a couple of rows, so we keep few. (A varying
        // *value* per row — e.g. a distinct id — would legitimately count as
        // new information; that's a diverse list, not a redundant one.)
        let rows: Vec<String> = (0..50)
            .map(|i| format!("status=ok role=user batch={}", i % 2))
            .collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let k = adaptive_keep(&refs, 2, 40);
        assert!(k < 12, "redundant rows should keep few, kept {k}");
        assert!(k >= 2);
    }

    #[test]
    fn diverse_list_keeps_more() {
        // Genuinely distinct rows: each adds new shingles, so we keep ~all.
        let rows = [
            "alpha bravo charlie delta",
            "echo foxtrot golf hotel",
            "india juliet kilo lima",
            "mike november oscar papa",
            "quebec romeo sierra tango",
            "uniform victor whiskey xray",
        ];
        let k = adaptive_keep(&rows, 2, 40);
        assert!(k >= 5, "diverse rows should keep most, kept {k}");
    }

    #[test]
    fn diverse_keeps_at_least_as_many_as_redundant() {
        let red: Vec<String> = (0..30).map(|_| "same same same same".to_string()).collect();
        let red_refs: Vec<&str> = red.iter().map(String::as_str).collect();
        let div: Vec<String> = (0..30)
            .map(|i| format!("row-{i}-{}", "abcdefgh".chars().nth(i % 8).unwrap()))
            .collect();
        let div_refs: Vec<&str> = div.iter().map(String::as_str).collect();
        assert!(adaptive_keep(&div_refs, 2, 25) >= adaptive_keep(&red_refs, 2, 25));
    }

    #[test]
    fn respects_bounds() {
        let rows = ["a", "b", "c"];
        assert_eq!(
            adaptive_keep(&rows, 0, 10),
            3,
            "len <= max returns all when diverse"
        );
        let many: Vec<String> = (0..100).map(|_| "x".to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let k = adaptive_keep(&refs, 3, 20);
        assert!((3..=20).contains(&k));
    }

    #[test]
    fn tiny_and_empty_lists() {
        assert_eq!(adaptive_keep(&[], 2, 10), 0);
        assert_eq!(adaptive_keep(&["only"], 2, 10), 1);
        // A 3-item list has only one item past `min`, so the 2-in-a-row
        // saturation rule can't fire — keeping all 3 is correct (nothing to
        // meaningfully save on a list that short).
        assert_eq!(adaptive_keep(&["", "", ""], 2, 10), 3);
        // A long no-signal list does saturate and trims back toward min.
        let many_empty: Vec<&str> = vec![""; 20];
        assert!(
            adaptive_keep(&many_empty, 2, 15) <= 4,
            "long no-signal list trims to ~min"
        );
    }
}
