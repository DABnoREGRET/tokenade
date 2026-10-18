//! The web route: what a page delivers, what it declares, and what pruning is not allowed to take away.
//!
//! ONE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all the
//! dependencies of `tokenade-core`. This crate carried 87 of them. The pattern is
//! the one of `crates/tokenade-cli/tests/e2e/main.rs`.
//!
//! Adding a subject: a file here, a `mod` below. Not a file at
//! the root of `tests/`.

// The shared helpers, mounted ONCE for the binary.
#[path = "../support/ab.rs"]
mod ab;

mod a11y_corpus;
mod a_card_keeps_its_address;
mod a_carousel_is_not_a_navigation_bar;
mod a_cms_body_reaches_the_page;
mod a_code_block_keeps_its_language;
mod a_component_named_promo_is_not_an_advert;
mod a_declared_dialog_is_chrome;
mod a_declared_heading_is_a_heading;
mod a_diagram_is_not_an_icon;
mod a_disclosure_label_names_a_section;
mod a_figure_marker_means_an_image;
mod a_formula_is_delivered_once;
mod a_frameset_names_its_document;
mod a_headline_is_served_once;
mod a_list_item_is_not_always_a_child;
mod a_long_address_is_not_dense_text;
mod a_page_declares_what_a_region_is;
mod a_phrasing_element_stays_in_the_line;
mod a_product_keeps_its_safety_notice;
mod a_section_heading_is_not_empty;
mod a_shopping_guide_is_not_a_teaser;
mod a_table_says_what_it_is;
mod an_image_marker_owns_its_line;
mod html_nesting_corpus;
mod relative_reference_origin;
mod serp_corpus;
mod summary_body_injection;
mod web_concurrency;
mod web_corpus_sweep;
mod web_cost;
mod web_fidelity;
mod web_proxy;
mod web_reach;
