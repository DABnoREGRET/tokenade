//! **The command-compactor families, written once each.**
//!
//! `cmd/**` holds 990 files, one per command and per platform, and
//! `build.rs` generates the `pub mod`s from the directory's CONTENT: the
//! file is mandatory, the registry leans on it. What was not
//! is the BODY — the 2026-09-13 census found **125 files
//! carrying another one's body up to their module name**, in thirteen families.
//!
//! Each family lives here, in a macro, and each file invokes it in one
//! line. `no_two_commands_share_a_body` refuses the next copy.
//!
//! **Why here and not in `archetypes`.** They were first written there,
//! and two guards refused it the same day: the fallback of several
//! families is `crate::json::compact`, so the FOLD layer started
//! naming the FORMATS layer; and `archetypes.rs` went from 1 485 to 1 757
//! shipped lines. A family of commands belongs to the commands.

/// **An AWS command that returns short JSON, or nothing — written ONCE.**
///
/// Twenty-one files of `cmd/linux/aws_*` carried this body identically,
/// tests included: detect any output, try AWS's JSON response
/// fold, and let it through otherwise. These are ACTION commands
/// (`delete-stack`, `put-item`, `attach-role-policy`): they answer `{}`,
/// a two-field acknowledgement, or nothing at all.
///
/// One file per command remains mandatory — `build.rs` generates the `pub mod`s
/// of `cmd/linux/` from the directory's CONTENT, and the registry leans on it.
/// What changes is that there are no longer twenty-one copies of a body to repair
/// one by one: that is the original complaint of this effort, « the same code
/// in fifteen places ».
#[macro_export]
macro_rules! aws_action_output {
    () => {
        pub fn detect(text: &str) -> bool {
            $crate::archetypes::any_output(text)
        }

        $crate::archetypes::detect_gated_try_compact!(super::aws_json::compact_if_json);

        $crate::archetypes::cmd_entry_points!($crate::archetypes::passthrough);

        #[cfg(test)]
        mod tests {
            use super::*;
            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
                assert!(!detect("  \n"));
            }
            #[test]
            fn compact_does_not_panic_on_empty_json() {
                let _ = compact("{}");
            }
            #[test]
            fn compact_does_not_panic_on_plain() {
                let _ = compact("ok\n");
            }
        }
    };
}
pub use crate::aws_action_output;

/// **An `aws s3` command whose output is already short — written ONCE.**
///
/// Seven files carried this body identically. `cp`, `mv`, `sync`, `rm`,
/// `mb`, `rb`, `website` return one transfer line per object: there is
/// nothing to fold, and `try_compact` SAYS so by returning `None` rather than
/// letting it be believed that a fold was attempted.
///
/// The test's example is passed as an argument: it is the only thing that
/// distinguished the seven.
#[macro_export]
macro_rules! aws_s3_transfer_output {
    ($sample:expr) => {
        pub fn detect(text: &str) -> bool {
            $crate::archetypes::any_output(text)
        }

        pub fn try_compact(_text: &str) -> Option<String> {
            None // s3 operation output is already short — passthrough is optimal
        }

        pub fn compact(text: &str) -> String {
            $crate::archetypes::passthrough(text)
        }

        pub fn compact_with_savings(text: &str) -> (String, $crate::Savings) {
            $crate::archetypes::savings_of(text, compact)
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
                assert!(!detect("  \n"));
            }
            #[test]
            fn compact_returns_unchanged() {
                let s = $sample;
                assert_eq!(compact(s), $crate::archetypes::passthrough(s));
            }
        }
    };
}
pub use crate::aws_s3_transfer_output;

/// **A command whose output is short by nature — written ONCE.**
///
/// `detect` says « there is output », `try_compact` says `Some` without removing
/// anything, and `compact` lets it through. Four files carried this body
/// identically (`depmod`, `insmod`, `kmod`, `rmmod`).
#[macro_export]
macro_rules! any_output_passthrough {
    () => {
        pub fn detect(text: &str) -> bool {
            $crate::archetypes::any_output(text)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            Some($crate::archetypes::passthrough(text))
        }

        pub fn compact(text: &str) -> String {
            $crate::archetypes::passthrough(text)
        }

        pub fn compact_with_savings(text: &str) -> (String, $crate::Savings) {
            $crate::archetypes::savings_of(text, compact)
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
                assert!(!detect("   \n"));
            }

            #[test]
            fn try_compact_some_or_none() {
                let s = "sample output line\nsecond line\n";
                let _ = try_compact(s);
                let _ = compact(s);
                let _ = compact_with_savings(s);
            }
        }
    };
}
pub use crate::any_output_passthrough;

/// **A SHAPE-SENSITIVE output: table folded, otherwise head and tail.**
///
/// Fourteen files carried this body identically, but for two things: the
/// head budget and the banner label, which is the command's name.
/// A table is folded by bringing its problem rows up; an ordinary
/// list keeps its HEAD AND TAIL, so that the final result line is
/// never decapitated.
#[macro_export]
macro_rules! table_or_head_tail_command {
    ($label:expr, $head:expr, $tail:expr) => {
        const HEAD: usize = $head;

        pub fn detect(text: &str) -> bool {
            $crate::archetypes::table_or_over_budget(text, HEAD)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            $crate::archetypes::table_or_head_tail(text, HEAD, $tail, $label, "line(s)")
        }

        $crate::archetypes::cmd_entry_points!($crate::archetypes::passthrough);

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
            }

            #[test]
            fn table_folds_and_surfaces_problem_rows() {
                let mut t = String::from("NAME   STATUS   AGE\n");
                for i in 0..(HEAD + 20) {
                    t.push_str(&format!("res-{i}   Running   {i}m\n"));
                }
                t.push_str("res-bad   Error   9m\n");
                let out = try_compact(&t).unwrap();
                assert!(out.contains("STATUS"));
                assert!(out.contains("res-bad"));
            }

            #[test]
            fn plain_listing_keeps_tail() {
                let mut t = String::new();
                for i in 0..(HEAD + 20) {
                    t.push_str(&format!("item {i}\n"));
                }
                t.push_str("FINAL OUTCOME LINE\n");
                let out = try_compact(&t).unwrap();
                assert!(out.contains("FINAL OUTCOME LINE"));
                assert!(out.contains("item 0"));
            }

            #[test]
            fn short_output_passes_through() {
                assert!(try_compact("one line\ntwo line\n").is_none());
            }
        }
    };
}
pub use crate::table_or_head_tail_command;

/// **A JSON response that is an OBJECT, recognised by a field — written ONCE.**
///
/// Eleven files carried this body identically, but for a single word: the
/// field that identifies the response (`"hardwareProfile"` for a VM,
/// `"primaryEndpoints"` for a storage account…).
#[macro_export]
macro_rules! json_object_response {
    ($needle:expr) => {
        pub fn detect(text: &str) -> bool {
            $crate::archetypes::json_object_with(text, $needle)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            super::aws_json::compact_response(text)
        }

        // `crate::json`, not `$crate::json`: the fallback is the
        // JSON compactor of the FORMATS layer, which is not in this
        // crate. A `crate::` path in a `macro_rules!` body resolves
        // at the CALLER, which is exactly what is needed here — and the
        // compiler says so unambiguously if we get it wrong:
        // « cannot find `json` in `$crate` ».
        $crate::archetypes::cmd_entry_points!($crate::json::compact);

        #[cfg(test)]
        mod tests {
            use super::*;

            /// A response with `n` entries, carrying the field that identifies it.
            fn make_resp(n: usize) -> String {
                let key = $needle.trim_matches('"');
                let items = (0..n)
                    .map(|i| format!(r#"{{"{key}":"item-{i}","Status":"active"}}"#))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{\"{key}\":[{items}]}}")
            }

            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
            }
            #[test]
            fn detector_rejects_plain_text() {
                assert!(!detect("plain\n"));
            }
            #[test]
            fn detector_rejects_wrong_key() {
                assert!(!detect("{\"Other\":[]}"));
            }
            #[test]
            fn detector_accepts_valid_json() {
                assert!(detect(&make_resp(2)));
            }
            #[test]
            fn compact_does_not_panic() {
                let _ = compact(&make_resp(3));
            }
            #[test]
            fn key_preserved_in_output() {
                let out = compact(&make_resp(25));
                assert!(out.contains($needle.trim_matches('"')));
            }
        }
    };
}
pub use crate::json_object_response;

/// **A JSON response that is an ARRAY, recognised by a field.**
///
/// The twin of the previous one for list commands: four files.
#[macro_export]
macro_rules! json_array_response {
    ($needle:expr) => {
        pub fn detect(text: &str) -> bool {
            let t = text.trim_start();
            !t.is_empty() && t.starts_with('[') && text.contains($needle)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            super::aws_json::compact_response(text)
        }

        $crate::archetypes::cmd_entry_points!($crate::json::compact);

        #[cfg(test)]
        mod tests {
            use super::*;

            /// An ARRAY of `n` entries, carrying the field that identifies it.
            fn make_resp(n: usize) -> String {
                let key = $needle.trim_matches('"');
                let items = (0..n)
                    .map(|i| format!(r#"{{"{key}":"item-{i}","Status":"active"}}"#))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("[{items}]")
            }

            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
            }
            #[test]
            fn detector_rejects_plain_text() {
                assert!(!detect("plain\n"));
            }
            #[test]
            fn detector_rejects_wrong_key() {
                assert!(!detect("[{\"Other\":1}]"));
            }
            #[test]
            fn detector_accepts_valid_json() {
                assert!(detect(&make_resp(2)));
            }
            #[test]
            fn compact_does_not_panic() {
                let _ = compact(&make_resp(3));
            }
            #[test]
            fn key_preserved_in_output() {
                let out = compact(&make_resp(25));
                assert!(out.contains($needle.trim_matches('"')));
            }
        }
    };
}
pub use crate::json_array_response;

/// **The same command on another platform: DELEGATE, do not copy.**
///
/// One file per command AND per platform is mandatory — `build.rs`
/// generates the `pub mod`s of `cmd/<os>/` from the directory's content, and the
/// registry leans on it. But when `macos/fc` does exactly what
/// `linux/fc` does, the second copy costs only one thing: a fix.
/// **Three modules drifted exactly like that and shipped to two
/// platforms the behaviour from BEFORE the fix (2026-08-04).**
///
/// The four entry points refer to the named module. Seven files of
/// `cmd/macos` wrote these twenty lines by hand.
#[macro_export]
macro_rules! delegates_to {
    ($($target:tt)*) => {
        pub fn detect(text: &str) -> bool {
            $($target)*::detect(text)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            $($target)*::try_compact(text)
        }

        pub fn compact(text: &str) -> String {
            $($target)*::compact(text)
        }

        pub fn compact_with_savings(text: &str) -> (String, $crate::Savings) {
            $($target)*::compact_with_savings(text)
        }
    };
}
pub use crate::delegates_to;

/// **A SHAPE-SENSITIVE output, with its own detector.**
///
/// The twin of [`table_or_head_tail_command!`] for commands whose
/// detector is not « a table or a budget overrun »: a substantive
/// line (`has_substantive_line`), a `key: value`, a dotted name.
/// Twenty-five files carried this copied body, in three families that only
/// differed by this predicate.
///
/// `$detect` is a callable expression — a named function or a
/// closure — so that the detector stays READABLE at the call site rather
/// than hidden behind a flag.
///
/// `$sample` is a line the detector MUST accept, and it is a
/// parameter because the first version had made it up
/// (`concat!("output line from ", $label)`). Fourteen tests went red on the
/// spot: the families whose detector looks for a `key: value` have no use
/// for a sentence without a colon. A macro that INVENTS a test's input
/// no longer carries the test, it replaces it.
#[macro_export]
macro_rules! form_aware_command {
    ($label:expr, $head:expr, $tail:expr, $detect:expr, $sample:expr) => {
        const HEAD: usize = $head;

        pub fn detect(text: &str) -> bool {
            ($detect)(text)
        }

        pub fn try_compact(text: &str) -> Option<String> {
            if !detect(text) {
                return None;
            }
            $crate::archetypes::form_aware(text, HEAD, $tail, $label, "line(s)")
        }

        $crate::archetypes::cmd_entry_points!($crate::archetypes::passthrough);

        #[cfg(test)]
        mod tests {
            use super::*;

            const SAMPLE: &str = $sample;

            #[test]
            fn detector_rejects_empty() {
                assert!(!detect(""));
            }

            #[test]
            fn detector_accepts_specific_format() {
                assert!(detect(SAMPLE));
            }

            #[test]
            fn short_output_passes_through() {
                let s: String = std::iter::repeat_n(SAMPLE, 3)
                    .map(|l| format!("{l}\n"))
                    .collect();
                assert!(try_compact(&s).is_none());
            }

            #[test]
            fn long_output_folds_non_decapitating() {
                let s: String = std::iter::repeat_n(SAMPLE, HEAD + 20)
                    .map(|l| format!("{l}\n"))
                    .collect();
                let out = try_compact(&s).unwrap();
                assert!(out.contains(concat!("[tokenade:", $label)));
            }
        }
    };
}
pub use crate::form_aware_command;

/// **The same HELPER module on another platform: delegate.**
///
/// The twin of [`delegates_to!`] for a helper module rather than a
/// compactor: `fold` and `fold_paths` refer to the named module, and
/// [`cmd_helper_module!`] declares that this file is not a command.
/// `macos/git_listing` and `windows/git_listing` wrote these thirty lines,
/// tests included, to call `linux/git_listing`.
#[macro_export]
macro_rules! delegates_listing_to {
    ($($target:tt)*) => {
        pub fn fold(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
            $($target)*::fold(text, head, label, noun)
        }

        /// Fold a PATH listing, with the tail broken down by top-level prefix.
        pub fn fold_paths(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
            $($target)*::fold_paths(text, head, label, noun)
        }

        $crate::archetypes::cmd_helper_module!();

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn fold_returns_none_when_short() {
                assert!(fold("a\nb\nc\n", 20, "git-ls", "file(s)").is_none());
            }

            #[test]
            fn fold_returns_some_when_long() {
                let text = (0..30).map(|i| format!("item{i}\n")).collect::<String>();
                let out = fold(&text, 20, "git-branch", "branch(es)").unwrap();
                assert!(out.contains("[tokenade:git-branch] 30 branch(es)"));
                assert!(out.contains("item0"));
                assert!(out.contains("item19"));
                assert!(!out.contains("item20"));
                assert!(out.contains("(+10 more)"));
            }

            #[test]
            fn fold_paths_groups_tail_by_dir() {
                let paths = (0..25)
                    .map(|i| format!("src/file{i}.rs\n"))
                    .chain((0..5).map(|i| format!("tests/test{i}.rs\n")))
                    .collect::<String>();
                let out = fold_paths(&paths, 20, "git-diff", "file(s)").unwrap();
                assert!(out.contains("[tokenade:git-diff] 30 file(s)"));
                assert!(out.contains("src/ ×"));
                assert!(out.contains("tests/ ×"));
            }
        }
    };
}
pub use crate::delegates_listing_to;
