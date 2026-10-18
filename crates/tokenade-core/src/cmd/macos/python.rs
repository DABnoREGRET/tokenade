//! `python` — python.
//!
//! Form-aware: detects a columnar table (folds it, surfacing problem
//! rows) vs a plain listing (keeps head AND tail so the final outcome
//! line is never decapitated). Falls back to passthrough otherwise.

crate::cmd::families::table_or_head_tail_command!("python", 60, 5);
