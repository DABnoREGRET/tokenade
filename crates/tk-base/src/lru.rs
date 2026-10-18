//! Tiny generic LRU cache.
//!
//! Ported from codegraph's `resolution/lru-cache.ts` pattern. Used by
//! per-resolver state caches in their indexer; we use it for embedding
//! cache + query-result cache so repeated `semantic_search` calls with
//! the same query don't re-embed and don't re-rank.
//!
//! ## Why a custom one
//!
//! We were already pulling in `std::collections::HashMap` and a small
//! intrusive linked-list would be overkill for the cache sizes we run
//! (typically ≤ 512 entries). The `VecDeque` order-tracker below is
//! O(N) on access but `N` is small and the constant factor beats any
//! `unsafe` linked-list. If profiling ever shows this is hot, swap for
//! `lru` crate — the public API matches.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// Fixed-capacity least-recently-used cache.
///
/// Construction: `LruCache::new(capacity)`. Capacity ≥ 1; `0` is
/// silently clamped to `1` so callers can't accidentally make a cache
/// that drops every insertion.
pub struct LruCache<K, V> {
    cap: usize,
    map: HashMap<K, V>,
    /// MRU at the back, LRU at the front. `VecDeque` is contiguous and
    /// cache-friendly, which matters at our typical sub-512 sizes more
    /// than the O(N) lookup on access does.
    order: VecDeque<K>,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            cap,
            map: HashMap::with_capacity(cap),
            order: VecDeque::with_capacity(cap),
        }
    }

    /// Number of live entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up `key`. On hit, the entry moves to MRU position.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            self.touch(key);
            self.map.get(key)
        } else {
            None
        }
    }

    /// Insert / replace. Evicts the LRU entry when full.
    pub fn put(&mut self, key: K, value: V) -> Option<V> {
        if let Some(old) = self.map.insert(key.clone(), value) {
            // Existing entry — bump it to MRU.
            self.touch(&key);
            return Some(old);
        }
        if self.map.len() > self.cap {
            if let Some(evict) = self.order.pop_front() {
                self.map.remove(&evict);
            }
        }
        self.order.push_back(key);
        None
    }

    fn touch(&mut self, key: &K) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos).expect("position came from iter");
            self.order.push_back(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_basic() {
        let mut c: LruCache<&str, i32> = LruCache::new(2);
        c.put("a", 1);
        c.put("b", 2);
        assert_eq!(c.get(&"a"), Some(&1));
        assert_eq!(c.get(&"b"), Some(&2));
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut c: LruCache<&str, i32> = LruCache::new(2);
        c.put("a", 1);
        c.put("b", 2);
        let _ = c.get(&"a"); // a becomes MRU
        c.put("c", 3); // evicts b
        assert_eq!(c.get(&"b"), None);
        assert_eq!(c.get(&"a"), Some(&1));
        assert_eq!(c.get(&"c"), Some(&3));
    }

    #[test]
    fn capacity_zero_clamps_to_one() {
        let mut c: LruCache<&str, i32> = LruCache::new(0);
        c.put("a", 1);
        assert_eq!(c.get(&"a"), Some(&1));
    }

    #[test]
    fn replace_updates_and_promotes() {
        let mut c: LruCache<&str, i32> = LruCache::new(2);
        c.put("a", 1);
        c.put("b", 2);
        let old = c.put("a", 10);
        assert_eq!(old, Some(1));
        // a is now MRU, b is LRU — adding c evicts b.
        c.put("c", 3);
        assert_eq!(c.get(&"b"), None);
        assert_eq!(c.get(&"a"), Some(&10));
    }
}
