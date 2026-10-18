//! CDN / origin error pages that arrive dressed as content.
//!
//! A bot wall ([`crate::stealth`]) says "prove you are human". This module
//! covers the other way a fetch returns something that is not the page: the
//! EDGE answered instead of the origin — Cloudflare 502, a Vercel deployment
//! that no longer exists, Fastly's unknown-domain page, an S3 `NoSuchBucket`.
//!
//! Why it needs its own detector rather than the HTTP status: **these are very
//! often served with `200 OK`**. Cloudflare's 5xx interstitials do carry their
//! status, but Vercel's `DEPLOYMENT_NOT_FOUND`, S3's XML errors and most
//! single-page-app error shells come back 200, so the status-note path
//! (`[tokenade: HTTP …]`) has nothing to report and the lead marker says
//! "✓ FETCH OK". The agent then reads a gateway error as the article.
//!
//! Remediation differs from a bot wall too, which is why this is not a
//! `ChallengeKind`: a wall wants the browser profile, an origin error wants a
//! later retry or a corrected URL. Nothing here is retryable by changing how we
//! ask.
//!
//! ## False positives are the whole design problem
//!
//! A page ABOUT Cloudflare errors legitimately contains "Error 502". So the
//! generic markers only count on a page with little visible text — an error
//! page is short by nature, an article about errors is not. Vendor-unique
//! tokens (`cf-error-details`, `DEPLOYMENT_NOT_FOUND`, `Fastly error: unknown
//! domain`) are unambiguous enough to fire at any length.

/// Which edge produced the error page, for the note shown to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeError {
    CloudflareOrigin,
    Vercel,
    Fastly,
    Netlify,
    Heroku,
    AwsS3,
    /// A bare reverse-proxy gateway error (nginx/Apache/HAProxy 502/503/504).
    Gateway,
}

impl EdgeError {
    pub fn label(self) -> &'static str {
        match self {
            EdgeError::CloudflareOrigin => "Cloudflare could not reach the origin server",
            EdgeError::Vercel => "Vercel returned a deployment/function error",
            EdgeError::Fastly => "Fastly returned an edge error",
            EdgeError::Netlify => "Netlify returned a site error",
            EdgeError::Heroku => "Heroku returned an application error",
            EdgeError::AwsS3 => "AWS S3 returned a bucket/key error",
            EdgeError::Gateway => "an upstream gateway error (502/503/504)",
        }
    }

    /// What the agent should do about it. None of these are fixed by asking
    /// differently, which is the point worth stating.
    pub fn advice(self) -> &'static str {
        match self {
            EdgeError::Vercel | EdgeError::AwsS3 | EdgeError::Netlify => {
                "The resource is gone or misrouted — check the URL; retrying it unchanged will \
                 not help."
            }
            _ => "The origin was unreachable, not the URL wrong — a later retry may succeed.",
        }
    }
}

/// Visible (non-markup, non-whitespace) character count, capped so a long
/// document exits early instead of being scanned whole.
fn visible_len(text: &str, cap: usize) -> usize {
    let mut n = 0;
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag && !c.is_whitespace() => {
                n += 1;
                if n >= cap {
                    return n;
                }
            }
            _ => {}
        }
    }
    n
}

/// Markers distinctive enough that no ordinary page carries them by accident.
const UNAMBIGUOUS: &[(&str, EdgeError)] = &[
    ("cf-error-details", EdgeError::CloudflareOrigin),
    ("cf-error-title", EdgeError::CloudflareOrigin),
    ("cferrorspan", EdgeError::CloudflareOrigin),
    ("deployment_not_found", EdgeError::Vercel),
    ("deployment_disabled", EdgeError::Vercel),
    ("deployment_paused", EdgeError::Vercel),
    ("function_invocation_timeout", EdgeError::Vercel),
    ("function_invocation_failed", EdgeError::Vercel),
    ("no_deployment_for_alias", EdgeError::Vercel),
    ("this deployment cannot be found", EdgeError::Vercel),
    ("fastly error: unknown domain", EdgeError::Fastly),
    ("<nosuchbucket", EdgeError::AwsS3),
    ("<code>nosuchbucket", EdgeError::AwsS3),
    ("<code>nosuchkey", EdgeError::AwsS3),
    ("page not found - netlify", EdgeError::Netlify),
    ("application error</h1>", EdgeError::Heroku),
    ("heroku-error", EdgeError::Heroku),
];

/// Markers that also occur in prose ABOUT errors, so they only count on a page
/// with too little text to be an article.
const NEEDS_SHORT_PAGE: &[(&str, EdgeError)] = &[
    ("web server is down", EdgeError::CloudflareOrigin),
    ("origin is unreachable", EdgeError::CloudflareOrigin),
    ("connection timed out", EdgeError::CloudflareOrigin),
    ("ssl handshake failed", EdgeError::CloudflareOrigin),
    ("error 502", EdgeError::Gateway),
    ("error 503", EdgeError::Gateway),
    ("error 504", EdgeError::Gateway),
    ("502 bad gateway", EdgeError::Gateway),
    ("503 service unavailable", EdgeError::Gateway),
    ("503 service temporarily unavailable", EdgeError::Gateway),
    ("504 gateway time-out", EdgeError::Gateway),
    ("504 gateway timeout", EdgeError::Gateway),
];

/// An error page is short. Above this many visible characters, the ambiguous
/// markers are assumed to be prose discussing an error, not an error.
const MAX_VISIBLE_FOR_AMBIGUOUS: usize = 800;

/// Detect an edge/origin error page. `None` means "nothing says this is one",
/// which is the fail-open direction: a missed error page reads as content (the
/// status quo), a false positive would brand a real article as broken.
pub fn detect(text: &str) -> Option<EdgeError> {
    // An error page is small. This also bounds the scan on a big document.
    if text.len() > 64 * 1024 {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    for (needle, kind) in UNAMBIGUOUS {
        if lower.contains(needle) {
            return Some(*kind);
        }
    }
    if visible_len(text, MAX_VISIBLE_FOR_AMBIGUOUS + 1) > MAX_VISIBLE_FOR_AMBIGUOUS {
        return None;
    }
    for (needle, kind) in NEEDS_SHORT_PAGE {
        if lower.contains(needle) {
            return Some(*kind);
        }
    }
    None
}

/// One-line note for the disclosure prefix. Deliberately shaped like the HTTP
/// status note so the same lead-marker logic recognises it.
pub fn note(kind: EdgeError) -> String {
    format!(
        "[tokenade: EDGE ERROR — {}. The HTTP status may still be 200: this body is the \
         edge's error page, not the page you asked for. {}]\n\n",
        kind.label(),
        kind.advice()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vercel_deployment_error_is_caught_at_any_length() {
        // Served 200, and long enough that a length gate alone would miss it.
        let mut page =
            String::from("<!doctype html><html><body><h1>404</h1><p>DEPLOYMENT_NOT_FOUND</p>");
        for i in 0..200 {
            page.push_str(&format!(
                "<p>filler paragraph number {i} padding the body.</p>"
            ));
        }
        page.push_str("</body></html>");
        assert_eq!(detect(&page), Some(EdgeError::Vercel));
    }

    #[test]
    fn a_cloudflare_origin_error_is_caught() {
        let page = "<!doctype html><html><head><title>502 Bad Gateway</title></head>\
                    <body><div class=\"cf-error-details\">Web server is down</div></body></html>";
        assert_eq!(detect(page), Some(EdgeError::CloudflareOrigin));
    }

    #[test]
    fn a_bare_nginx_gateway_page_is_caught() {
        let page = "<html><head><title>502 Bad Gateway</title></head><body>\
                    <center><h1>502 Bad Gateway</h1></center><hr><center>nginx</center>\
                    </body></html>";
        assert_eq!(detect(page), Some(EdgeError::Gateway));
    }

    #[test]
    fn an_s3_xml_error_is_caught() {
        let page = "<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code>\
                    <Message>The specified bucket does not exist</Message></Error>";
        assert_eq!(detect(page), Some(EdgeError::AwsS3));
    }

    #[test]
    fn an_article_about_gateway_errors_is_not_an_error_page() {
        // THE false positive this module is designed around: a real page whose
        // subject is 502s. Ambiguous markers must not fire once there is enough
        // visible text to be an article.
        let mut page =
            String::from("<!doctype html><html><body><h1>Debugging 502 Bad Gateway</h1>");
        for i in 0..80 {
            page.push_str(&format!(
                "<p>When nginx reports 502 Bad Gateway the upstream refused the connection, \
                 which in case {i} usually means the process died or the socket moved.</p>"
            ));
        }
        page.push_str("</body></html>");
        assert_eq!(detect(&page), None, "an article about 502s is not a 502");
    }

    #[test]
    fn ordinary_content_is_never_flagged() {
        assert_eq!(
            detect("<html><body><p>Hello world.</p></body></html>"),
            None
        );
        assert_eq!(detect("{\"ok\":true,\"items\":[1,2,3]}"), None);
    }

    #[test]
    fn the_note_states_that_the_status_may_still_be_200() {
        let n = note(EdgeError::Vercel);
        assert!(n.contains("EDGE ERROR"));
        assert!(n.contains("200"));
        assert!(n.ends_with("\n\n"));
    }
}
