//! Shared regex plumbing: the two ways this crate reaches for a compiled
//! pattern, each in one place.
//!
//! They are NOT interchangeable and the difference is a hot-path one:
//! [`lazy_regex!`] is an atomic load per call, [`re`] is a mutex plus a hash
//! lookup. Use the macro for a pattern known at compile time (nearly always);
//! use [`re`] only when the pattern itself is computed at run time.

use regex::Regex;
use std::sync::OnceLock;

/// Define a lazily-compiled `&'static Regex` accessor.
///
/// Thirteen functions across five modules carried the same eight lines of
/// ceremony around a single pattern. What matters at each site is the pattern
/// and the name; the ceremony is not a decision, so it lives here.
#[macro_export]
macro_rules! lazy_regex {
    ($(#[$meta:meta])* $vis:vis fn $name:ident() = $pat:expr) => {
        $(#[$meta])*
        $vis fn $name() -> &'static regex::Regex {
            static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            RE.get_or_init(|| {
                regex::Regex::new($pat).expect("internal regex pattern must be valid")
            })
        }
    };
}
// `pub` and not `pub(crate)`: the base layer has been a crate since
// 2026-09-13, and `tokenade-core` spells `crate::regexes::lazy_regex!` in
// some thirty modules. A macro re-exported from a module only
// crosses the crate boundary if it is public.
pub use crate::lazy_regex;

/// A compiled regex for a pattern only known at RUN time, memoised forever.
///
/// `site_readers.rs` and `web.rs` carried this cache byte for byte. One cache
/// rather than two changes nothing observable — it is a memo keyed by the
/// pattern — and leaks on purpose: these patterns are finite and live for the
/// process.
///
/// Prefer [`lazy_regex!`] whenever the pattern is a literal: this one takes a
/// lock on every call.
pub fn re(pattern: &str) -> &'static Regex {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(Default::default()));
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = g.get(pattern) {
        return r;
    }
    let leaked: &'static Regex = Box::leak(Box::new(
        Regex::new(pattern).expect("internal regex pattern must be valid"),
    ));
    g.insert(pattern.to_string(), leaked);
    leaked
}

// A hexadecimal literal (`0x1f`). It lived in `value_shapes`, the formats
// layer, and `template_fold` — a folder, hence a primitive — named it
// by climbing up. A pattern is of the same nature as this module.
/// A hexadecimal literal: `0x1f`, `0XDEADBEEF`.
///
/// Both word boundaries matter. Without the left one, `foo0x1f` carries
/// a value; without the right one the pattern is merely useless, but
/// the symmetry is what makes the rule readable.
pub const HEX_LITERAL: &str = r"\b0[xX][0-9a-fA-F]+\b";

#[cfg(test)]
mod tests {
    /// A single spelling in the whole product.
    ///
    /// The repo's rule: what is decided in two places ends up being
    /// decided differently. Here the two spellings had ALREADY diverged
    /// before being merged.
    #[test]
    fn only_this_file_spells_a_hexadecimal_literal() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .parent()
            .expect("repo root");
        let mut offenders = Vec::new();
        let mut checked = 0usize;
        for entry in walk(root) {
            if entry.ends_with("regexes.rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&entry) else {
                continue;
            };
            checked += 1;
            for (n, line) in src.lines().enumerate() {
                // A comment that QUOTES the old spelling to say
                // why it went away is not a second
                // spelling. A guard that bites its own explanation
                // teaches people to stop explaining.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.contains("0-9a-fA-F") && line.contains("0x") || line.contains("0[xX]") {
                    offenders.push(format!("{}:{}", entry.display(), n + 1));
                }
            }
        }
        assert!(checked > 100, "the sweep only read {checked} file(s)");
        assert!(
            offenders.is_empty(),
            "a hexadecimal literal is rewritten elsewhere: {offenders:?}"
        );
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if p.is_dir() {
                if name == "target" || name == "competitors" || name.starts_with('.') {
                    continue;
                }
                out.extend(walk(&p));
            } else if name.ends_with(".rs") {
                out.push(p);
            }
        }
        out
    }
}
