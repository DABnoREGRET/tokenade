//! Interactive web dashboard renderer.
//!
//! `tokenade dashboard` opens this page in a browser (in addition to the
//! terminal report — see [`crate::dashboard`] for that renderer). Design
//! constraints, in order:
//!
//! - **100 % local.** The page is a single self-contained HTML file:
//!   inline CSS, inline vanilla JS, inline JSON payload, zero external
//!   requests. This is a product promise (the dashboard exists locally
//!   precisely because shipping usage data to tokenade.net was rejected
//!   on privacy grounds), not just an offline nicety.
//! - **Pure rendering.** Callers aggregate the gain ledger into a JSON
//!   payload and pass it in; this module only does template injection.
//!   No I/O, no env, no clock — same testability rule as `dashboard.rs`.
//! - **Drillable time series.** Beta-tester ask (2026-06-10): summary by
//!   day/week/month with click-through detail per period (functions
//!   called, agents involved, projects), plus a with/without-tokenade
//!   comparison. All slicing happens client-side over per-day buckets,
//!   so one payload powers every view.

use std::sync::OnceLock;

/// Full `YYYY-MM-DDTHH:MM:SSZ` form of [`utc_date_from_ts`], for the
/// "generated at" stamp.
pub fn utc_datetime_from_ts(ts: u64) -> String {
    let s = ts % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate::platform::utc_date_from_ts(ts),
        s / 3_600,
        (s % 3_600) / 60,
        s % 60
    )
}

/// Inject the aggregated payload into the page template.
///
/// `payload_json` must be a serialized JSON object (the CLI builds it
/// from `~/.tokenade/gain.jsonl`). `</` is escaped to `<\/` before
/// embedding — a ledger label containing `</script>` must not be able
/// to terminate the inline script block early.
pub fn render_html(payload_json: &str, version: &str) -> String {
    let safe = payload_json.replace("</", "<\\/");
    template()
        .replace("__PAYLOAD__", &safe)
        .replace("__VERSION__", version)
}

/// Page shell with `__STYLES__` / `__SCRIPT__` slots. CSS and JS live beside it
/// as real `.css` / `.js` files instead of a 1164-line raw string in this module:
/// the browser assets are edited, highlighted and linted as browser assets, and
/// this file stays Rust.
const SHELL: &str = include_str!("webdash/dashboard.html");
const STYLES: &str = include_str!("webdash/dashboard.css");
const SCRIPT: &str = include_str!("webdash/dashboard.js");

/// The assembled page, built once per process. Assets are substituted *before*
/// the payload, so a ledger label containing `__SCRIPT__` stays inert text.
fn template() -> &'static str {
    static TEMPLATE: OnceLock<String> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let slot = |s: &str| s.strip_suffix('\n').unwrap_or(s).to_string();
        SHELL
            .replace("__STYLES__", &slot(STYLES))
            .replace("__SCRIPT__", &slot(SCRIPT))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_known_values() {
        assert_eq!(crate::platform::utc_date_from_ts(0), "1970-01-01");
        assert_eq!(crate::platform::utc_date_from_ts(86_399), "1970-01-01");
        assert_eq!(crate::platform::utc_date_from_ts(86_400), "1970-01-02");
        // 2026-06-10 00:00:00 UTC
        assert_eq!(
            crate::platform::utc_date_from_ts(1_781_049_600),
            "2026-06-10"
        );
        // leap day
        assert_eq!(
            crate::platform::utc_date_from_ts(1_709_164_800),
            "2024-02-29"
        );
        assert_eq!(
            utc_datetime_from_ts(1_781_049_600 + 3_661),
            "2026-06-10T01:01:01Z"
        );
    }

    #[test]
    fn render_embeds_payload_and_version() {
        let html = render_html(r#"{"days":[],"x":1}"#, "9.9.9");
        assert!(html.contains(r#"{"days":[],"x":1}"#));
        assert!(html.contains("v9.9.9"));
        assert!(html.contains("100% local"));
        // Full lists, lazily rendered in scrollable panels (no slice caps).
        assert!(html.contains("function lazyTable"));
        assert!(html.contains(".scrolly{"));
        assert!(html.contains(">Tokenade Dashboard<"));
        // Savings card rates against total agent usage, not intercepted-only.
        assert!(html.contains("function consumedTotal"));
        assert!(!html.contains("__PAYLOAD__"));
        assert!(!html.contains("__VERSION__"));
        // self-contained: no external fetches
        assert!(!html.contains("http-equiv=\"refresh\""));
        assert!(!html.contains("src=\"http"));
        assert!(!html.contains("href=\"http://"));
    }

    #[test]
    fn every_asset_slot_is_filled() {
        // Rename a slot on one side only and the page ships the literal
        // marker instead of the stylesheet — visible, but only in a browser.
        let html = render_html("{}", "1");
        assert!(!html.contains("__STYLES__"));
        assert!(!html.contains("__SCRIPT__"));
        assert!(html.contains("<style>\n/* tokenade.net design system"));
        assert!(html.contains("<script>\n\"use strict\";"));
        assert!(html.contains(":root{"));
    }

    /// The range selector must be the only thing that decides what is on
    /// screen.
    ///
    /// It was not: each panel derived its own cutoff from `generated_ts`, and
    /// two of them (per-model consumption, "outside tokenade") sliced their
    /// per-day series against the MEASURED ledger's day list — so a day with
    /// agent activity and no tokenade op did not exist for them at all.
    /// Switching 7d/30d/90d moved some figures and left others frozen, with
    /// nothing on the page saying which. One window, one predicate, and every
    /// panel prints the window it used.
    #[test]
    fn every_panel_windows_through_one_predicate() {
        assert_eq!(
            SCRIPT.matches("function windowRange(").count(),
            1,
            "the window is computed in exactly one place"
        );
        assert_eq!(
            SCRIPT.matches("function inWindow(").count(),
            1,
            "the window is applied in exactly one place"
        );
        assert_eq!(
            SCRIPT.matches("const isLifetime =").count(),
            1,
            "undated rows have exactly one escape hatch"
        );
        assert!(
            !SCRIPT.contains("generated_ts - "),
            "a panel is deriving its own cutoff from the generation stamp again"
        );
        // Every windowed series goes through `inWindow`, so a new series that
        // forgets is visible as a series nobody filters.
        for series in [
            "est_by_day",
            "redact_by_day",
            "models_by_day",
            "bypass_by_day",
            "compounding_by_day",
            "cache_creation_by_day",
        ] {
            assert!(
                SCRIPT.contains(series),
                "payload series {series} is not read at all"
            );
        }
        // And the panels that CANNOT be windowed say so on their own head
        // rather than ignoring the control in silence.
        assert!(
            template().contains(r#"<span class="scope life""#),
            "the lifetime-only panel lost its scope chip"
        );
    }

    /// The terminal report is no longer printed beside the page, so the page
    /// carries the controls it used to lean on the terminal for.
    #[test]
    fn page_states_its_window_and_its_provenance() {
        let html = render_html("{}", "1");
        assert!(html.contains(r#"id="windowline""#));
        assert!(html.contains(r#"id="chart-scope""#));
        assert!(html.contains(r#"id="tab-scope""#));
        assert!(html.contains("100% local"));
    }

    #[test]
    fn render_escapes_script_breakout() {
        // A hostile (or just unlucky) ledger label must not be able to
        // close the inline <script> block.
        let html = render_html(r#"{"label":"</script><script>alert(1)"}"#, "1");
        assert!(!html.contains("</script><script>alert(1)"));
        assert!(html.contains(r#"<\/script><script>alert(1)"#));
    }
}
