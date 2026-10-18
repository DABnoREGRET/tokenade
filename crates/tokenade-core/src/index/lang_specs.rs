//! Language extraction specs: file extension -> declaration/import regexes.
//!
//! Split out of `index.rs` and turned from a 700-line `match` into a table:
//! adding a language is a data edit, and the specs are `&'static` so a lookup
//! allocates nothing (the old arms rebuilt a `Vec` on every indexed file).
//!
//! Unicode-aware identifier classes per UAX #31. Rust and Python both
//! follow this spec; before switching to `\p{XID_*}` our ASCII-only regex
//! silently missed `fn 計算`, `def データ`, `class Müller`, etc. — surfaced
//! by `adv:index:unicode-identifiers`. Constants are inlined into the
//! pattern literals below (so the strings stay `&'static`):
//!   identifier:    `[\p{XID_Start}_]\p{XID_Continue}*`
//!   js identifier: `[\p{XID_Start}_$][\p{XID_Continue}$]*`  (JS allows `$`)
//!
//! #755 — case-insensitive ext dispatch. Filesystem extensions are
//! case-insensitive by convention on Linux (.R vs .r), Windows
//! (.BAT vs .bat), and macOS (case-preserving but case-insensitive
//! by default). The R LangSpec test caught this — `.R` files were
//! silently unindexed because the match arm `"r"` doesn't match
//! `"R"`. Lowercasing at the entry point is the local fix that
//! makes EVERY language case-insensitive (PS1/PSM1/BAT/CMD/VB
//! all benefit). All existing match arms are already lowercase.

mod specs;
pub(crate) use specs::*;

use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) struct LangSpec {
    /// (regex, symbol kind) pairs; group 1 of each regex is the symbol name.
    pub(crate) patterns: &'static [(&'static str, &'static str)],
    /// Regex whose group 1 is an imported module path, when the language has one.
    pub(crate) import: Option<&'static str>,
}

/// Extension -> spec, built once. The table stays a flat list because it reads
/// well; lookups go through this map because indexing calls `lang_for` once per
/// file.
fn by_ext() -> &'static HashMap<&'static str, &'static LangSpec> {
    static MAP: OnceLock<HashMap<&'static str, &'static LangSpec>> = OnceLock::new();
    MAP.get_or_init(|| {
        SPECS
            .iter()
            .flat_map(|(exts, spec)| exts.iter().map(move |e| (*e, spec)))
            .collect()
    })
}

pub(crate) fn lang_for(ext: &str) -> Option<&'static LangSpec> {
    by_ext().get(ext.to_ascii_lowercase().as_str()).copied()
}
