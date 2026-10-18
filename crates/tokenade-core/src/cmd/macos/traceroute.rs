//! `traceroute` — traceroute.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.

crate::cmd::families::form_aware_command!("traceroute", 60, 5, |t: &str| t.contains(':') || t.contains('.'), "inet 192.168.1.1/24");
