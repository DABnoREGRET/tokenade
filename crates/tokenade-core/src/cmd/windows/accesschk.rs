//! `accesschk` — accesschk.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.

crate::cmd::families::form_aware_command!("accesschk", 50, 5, crate::archetypes::has_substantive_line, "output line from accesschk");
