//! `uv_install` — uv pip install / uv sync / uv add compactor.
//!
//! Delegates to `super::uv::compact` which detects the summary+delta shape
//! and declines for unrelated subcommands like `uv venv`.

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::uv::try_compact(text)
}

pub fn compact(text: &str) -> String {
    super::uv::compact(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn try_compact_some_or_none() {
        let _ = try_compact("sample\n");
        let _ = compact("sample\n");
        let _ = compact_with_savings("sample\n");
    }
}
