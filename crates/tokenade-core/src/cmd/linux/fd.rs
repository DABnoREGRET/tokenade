//! `fd` — shared compactor for fd (find alternative) subcommand output.
pub fn detect(text: &str) -> bool { !text.trim().is_empty() }
pub fn try_compact(text: &str) -> Option<String> { if !detect(text) { return None; } Some(compact(text)) }
pub fn compact(text: &str) -> String { super::find::compact(text) }
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) { crate::archetypes::savings_of(text, compact) }
