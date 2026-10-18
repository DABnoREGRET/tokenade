//! `rg` — ripgrep shared compactor.
//!
//! ripgrep (rg) produces the same `path:line:match` format as grep.
//! Delegates to `super::grep::compact` to reuse path-prefix stripping
//! and searchout grouping.
//!
//! All generic `rg_*` subcommand files delegate to this module.

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    super::grep::compact(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegates_to_grep() {
        let s = "src/main.rs:42:    let x = foo();\nsrc/lib.rs:10:fn foo() {}\n";
        let out = compact(s);
        assert!(!out.is_empty(), "must produce output");
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("  \n"));
    }
}
