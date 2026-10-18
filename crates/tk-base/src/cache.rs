//! Content-addressed on-disk cache under `<root>/.tokenade/cache/`.
//!
//! Used to memoise expensive results (an index build, a filtered output) keyed
//! by a hash of their inputs. Dependency-free FNV-1a hashing keeps it tiny.
//!
//! ## Observability
//!
//! Every hit increments an in-process counter accessible via [`Cache::stats`],
//! and (when a label is provided) emits a `cache_hit` record into the shared
//! `gain.jsonl` ledger — so `tokenade gain` can show how much recomputation the
//! cache spared on top of the per-compactor savings. Persistent counters live
//! in `<root>/.tokenade/cache/_stats.json` so they survive across runs.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// FNV-1a 64-bit hash — small, fast, good enough for cache keys.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Snapshot of the running cache counters.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    /// Total bytes served from cache (i.e. recomputation avoided).
    pub bytes_served: u64,
}

impl Stats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

pub struct Cache {
    dir: PathBuf,
    hits: AtomicU64,
    misses: AtomicU64,
    bytes_served: AtomicU64,
}

impl Cache {
    pub fn open(root: &Path) -> Cache {
        let dir = root.join(".tokenade").join("cache");
        let _ = fs::create_dir_all(&dir);
        let prior = Cache::load_stats(&dir);
        Cache {
            dir,
            hits: AtomicU64::new(prior.hits),
            misses: AtomicU64::new(prior.misses),
            bytes_served: AtomicU64::new(prior.bytes_served),
        }
    }

    fn stats_path(dir: &Path) -> PathBuf {
        dir.join("_stats.json")
    }

    fn load_stats(dir: &Path) -> Stats {
        fs::read_to_string(Cache::stats_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Snapshot the live counters.
    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            bytes_served: self.bytes_served.load(Ordering::Relaxed),
        }
    }

    /// Persist counters to disk. Cheap to call (small JSON), best-effort.
    pub fn flush_stats(&self) {
        let _ = fs::write(
            Cache::stats_path(&self.dir),
            serde_json::to_string(&self.stats()).unwrap_or_else(|_| "{}".into()),
        );
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:016x}", hash(key.as_bytes())))
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let v = fs::read_to_string(self.path_for(key)).ok();
        if let Some(ref s) = v {
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.bytes_served
                .fetch_add(s.len() as u64, Ordering::Relaxed);
        }
        v
    }

    pub fn put(&self, key: &str, value: &str) -> std::io::Result<()> {
        self.misses.fetch_add(1, Ordering::Relaxed);
        fs::write(self.path_for(key), value)
    }

    /// Get the cached value or compute, store, and return it.
    pub fn get_or<F: FnOnce() -> String>(&self, key: &str, compute: F) -> String {
        if let Some(v) = self.get(key) {
            return v;
        }
        let v = compute();
        let _ = self.put(key, &v);
        v
    }

    // #736 — `get_or_labeled` and its `gain::record("cache", "cache_hit", …)`
    // telemetry path were removed: dashboard audit confirmed ZERO callers
    // anywhere in the workspace. No row with `source="cache"` has ever
    // appeared in production `~/.tokenade/gain.jsonl`. Keeping the
    // function shipped the misleading impression that a "cache_hit"
    // attribution target exists, tempting future refactors to wire it
    // into `attribute_agent_tool`. If cache-hit telemetry is wanted
    // later, design it from scratch — the previous implementation
    // double-counted (the cached value's bytes were already in the
    // ledger from the original write).
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.flush_stats();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tokenade-cache-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash(b"abc"), hash(b"abc"));
        assert_ne!(hash(b"abc"), hash(b"abd"));
    }

    #[test]
    fn get_or_computes_once() {
        let dir = tmp("once");
        let c = Cache::open(&dir);
        let mut calls = 0;
        let v1 = c.get_or("k", || {
            calls += 1;
            "v".to_string()
        });
        let v2 = c.get_or("k", || {
            calls += 1;
            "other".to_string()
        });
        assert_eq!(v1, "v");
        assert_eq!(v2, "v");
        assert_eq!(calls, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_track_hits_and_misses() {
        let dir = tmp("stats");
        let c = Cache::open(&dir);
        c.get_or("a", || "AAA".repeat(100));
        c.get_or("a", || "should-not-fire".into());
        c.get_or("a", || "should-not-fire".into());
        let s = c.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert_eq!(s.bytes_served, 600);
        assert!(s.hit_rate() > 0.5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_persist_across_reopen() {
        let dir = tmp("persist");
        {
            let c = Cache::open(&dir);
            c.get_or("a", || "v".into());
            c.get_or("a", || "v".into()); // hit
            c.flush_stats();
        }
        let c2 = Cache::open(&dir);
        let s = c2.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
