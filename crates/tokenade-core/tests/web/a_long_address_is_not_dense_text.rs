//! **Link density was measured on the addresses, not on the text.**
//!
//! A short block whose links carry long URLs passed for a navigation cluster:
//! `[label](url)` counted IN FULL, address included, against the block's
//! length. MEASURED 2026-09-12 on the battery's 40 pages — the `net/http`
//! documentation on `pkg.go.dev` lost its `View Source` blocks along with the
//! Go declarations they announce (3,875 characters), an MDN reference page
//! its return-type sentences, and a product sheet its sellers' ratings.
//!
//! The label against the visible text: it is the same question, asked of
//! what the page SHOWS.

fn md(body: &str) -> String {
    let filler = "Un chapeau de plus de deux cents caracteres pour que le premier etage garde \
                  le document et juge vraiment la suite, sans quoi l'etage relache rendrait \
                  tout et le test ne prouverait rien du tout ici. ";
    tokenade_core::web::html_to_markdown(&format!(
        "<html><body><main><p>{filler}</p>{body}</main></body></html>"
    ))
}

const LONG: &str = "https://example.test/a/very/long/canonical/address/that/weighs/far/more\
                    /than/the/words/it/points/at/index.html?utm_source=x&utm_campaign=y";

/// Four words outside the links — fewer than the prose guard's threshold, so
/// it is indeed DENSITY that decides — and two short labels on two long
/// addresses. What the page shows is not made of links; what the measurement
/// counted was.
#[test]
fn a_short_block_survives_two_long_addresses() {
    let out = md(&format!(
        "<div><a href=\"{LONG}\">4.2</a> Recommandations complementaires disponibles \
         maintenant <a href=\"{LONG}\">Avis</a></div>"
    ));
    assert!(
        out.contains("Recommandations complementaires disponibles"),
        "the block was cut as navigation:\n{out}"
    );
}

/// The bound: a navigation cluster stays cut. Its labels are short AND its
/// visible text is almost only labels — which the measurement on visible
/// text still says.
#[test]
fn a_navigation_cluster_is_still_cut() {
    let out = md(
        "<div><p><a href=\"/a\">Accueil</a> <a href=\"/b\">Contact</a> \
         <a href=\"/c\">Aide</a> <a href=\"/d\">Plan</a></p></div>",
    );
    assert!(
        !out.contains("Accueil") && !out.contains("Contact"),
        "the navigation cluster stayed:\n{out}"
    );
}
