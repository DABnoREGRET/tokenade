//! **What a page tokenade serves is worth, for the agent in front.**
//!
//! One dispatch for the whole-page and the excerpt paths: every agent whose
//! counterfactual is not read page by page is credited by the measured web
//! economy (×3) on both; an agent that would have read the raw HTML keeps
//! that measured page.

use crate::coefficients::{self, WebCreditModel};
use crate::Savings;

fn agent_model() -> WebCreditModel {
    coefficients::agent_web_binary_model(crate::detect::from_env().as_str()).0
}

/// A whole page served: `raw` = its HTML tokens, `after` = what is delivered.
pub fn web_page_credit(raw: usize, after: usize) -> Savings {
    coefficients::web_credit_savings(agent_model(), raw, after)
}

/// An excerpt of a page served: `page` = the distilled page, `extract` = the
/// excerpt delivered, `floor` = the scoped path's one-turn floor.
pub fn web_scoped_credit(page: usize, extract: usize, floor: usize) -> Savings {
    match agent_model() {
        WebCreditModel::NativeSummary
        | WebCreditModel::ReaderClean
        | WebCreditModel::Unmeasured => coefficients::web_economy_credit_savings(extract),
        _ => coefficients::web_fetch_scoped_credit_savings(page, extract, floor),
    }
}

/// What a web delivery's credit is computed from, so it can be computed on
/// what is FINALLY delivered — after the stash, the excerpt, the replay
/// pointer. Booked on the distilled page instead, a page put behind a
/// pointer was credited at three times a page the agent never received.
#[derive(Debug, Clone, Copy)]
pub enum WebBasis {
    /// A whole page; `raw` = its HTML tokens.
    Page { raw: usize },
    /// An excerpt of a distilled page of `page` tokens.
    Excerpt { page: usize, floor: usize },
    /// A credit fixed by the caller (a reader's flat reliability credit, a
    /// document's turn tax): its amount is kept, its `after` follows the
    /// delivery.
    Flat(Savings),
}

/// The credit of `basis` when `delivered` tokens reach the agent.
pub fn credit_on_delivery(basis: WebBasis, delivered: usize) -> Savings {
    match basis {
        WebBasis::Page { raw } => web_page_credit(raw, delivered),
        WebBasis::Excerpt { page, floor } => web_scoped_credit(page, delivered, floor),
        WebBasis::Flat(s) => {
            let mut out = s;
            out.tokens_before = s.tokens_before.saturating_sub(s.tokens_after) + delivered;
            out.tokens_after = delivered;
            out
        }
    }
}
