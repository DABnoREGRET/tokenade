//! The invariants the repo holds about ITSELF: the map is true, advertised flags exist, subprocesses declare themselves, routing does not lie.
//!
//! ONE SINGLE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate carried 87 of them. The pattern
//! is the one of `crates/tokenade-cli/tests/e2e/main.rs`.
//!
//! Adding a subject: one file here, one `mod` below. Not a file at the root
//! of `tests/`.

mod a_measurement_declares_its_pressure;
mod a_persisted_path_names_the_installed_binary;
mod a_subject_that_claims_its_own_process_has_one;
mod advertised_env_vars_exist;
mod advertised_flags_exist;
mod architecture_doc_is_true;
mod dispatch_chains;
mod every_agent_slug_is_classified;
mod every_crate_is_on_the_map;
mod every_library_layer_is_optimised;
mod every_nudge_kind_is_classified;
mod every_test_sibling_is_mounted;
mod every_testless_bin_says_so;
mod internal_subprocesses_are_marked;
mod label_platform;
mod layers_only_point_downwards;
mod no_detector_claims_a_shape_it_does_not_own;
mod no_file_ships_more_than_it_should;
mod no_guard_reads_a_trunk_that_has_branches;
mod no_hand_written_source_path_rots;
mod no_phone_home;
mod no_sweep_truncates_at_a_test_block;
mod no_table_ships_without_a_test;
mod no_test_is_ignored_in_silence;
mod no_two_commands_share_a_body;
mod no_two_names_differ_only_by_case;
mod one_test_binary_per_subject;
mod one_variable_one_reader;
mod panic_paths_are_counted;
mod platform_twin_drift;
mod routing_guards;
mod test_modules_are_gated;
