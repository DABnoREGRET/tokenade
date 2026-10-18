//! **An address is a string, not a request.**
//!
//! Read a URL's `scheme://host`, recognise an internal address,
//! walk up to the registrable domain, derive a `domain/section` from it: none
//! of these five functions opens a connection. They lived in `net`, the
//! HTTP client, and that is what forced the rendering of an RSS feed to name the
//! web layer — the only word it needed was `scheme_host`.
//!
//! Deliberately minimal, without the `url` crate: the inputs are our own
//! well-formed https endpoints.

/// `(scheme, host)` of `url` — host lowercased, userinfo and port stripped.
/// `None` when the string isn't `scheme://host…`. Deliberately minimal (no
/// `url` crate dep): the only inputs are our own well-formed https endpoints.
pub fn scheme_host(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    // Authority ends at the first '/', '?' or '#'.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Drop userinfo (everything up to and including the last '@').
    let hostport = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    // Drop the port (first ':'; our targets are never bracketed IPv6 literals).
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// The host of `url` with any port, userinfo and IPv6 brackets removed.
/// Unlike [`scheme_host`] this correctly unwraps a `[::1]:8080` literal, which
/// matters for the SSRF guard below.
fn host_of_url(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next().unwrap_or(after);
    let hostport = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        // `[::1]:8080` → `::1`
        rest.split(']').next().unwrap_or(rest)
    } else {
        // `host:port` → `host`
        hostport.split(':').next().unwrap_or(hostport)
    };
    (!host.is_empty()).then(|| host.trim_end_matches('.').to_ascii_lowercase())
}

/// Would a fetch of this host reach the machine's OWN network — loopback, a
/// private LAN, link-local (incl. the `169.254.169.254` cloud-metadata address),
/// or a `localhost`/`.internal`/`.local` name?
///
/// The web tools run on the USER's machine, so an agent (or a prompt-injected
/// page) that asks to fetch `http://169.254.169.254/…` or `http://localhost:…`
/// would otherwise turn a public web-fetch tool into a reach into the user's
/// own infrastructure — a scope the server-side native tool structurally does
/// NOT have. Treated as internal → the caller declines and leaves it to the
/// native tool, which never does worse and applies its own policy.
pub fn is_internal_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty()
        || h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".internal")
        || h.ends_with(".local")
    {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => is_internal_v4(v4),
        Ok(std::net::IpAddr::V6(v6)) => {
            // Unwrap IPv4-mapped / -compatible forms (`::ffff:127.0.0.1`) so the
            // v4 rules apply, else check the v6 loopback / ULA / link-local ranges.
            if let Some(v4) = v6.to_ipv4() {
                return is_internal_v4(v4);
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
        Err(_) => false,
    }
}

fn is_internal_v4(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local() // 169.254.0.0/16 — includes the cloud-metadata IP
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.octets()[0] == 0 // 0.0.0.0/8
        || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1])) // 100.64/10 CGNAT
}

/// [`is_internal_host`] applied to a full URL. `true` also when the host cannot
/// be parsed at all (fail SAFE — an unfetchable target is left to native).
pub fn is_internal_url(url: &str) -> bool {
    match host_of_url(url) {
        Some(h) => is_internal_host(&h),
        None => true,
    }
}

/// Second-level public suffixes common enough to be worth handling without
/// dragging in the Public Suffix List.
///
/// The list is deliberately incomplete, and the direction of that incompleteness
/// is what makes it safe: a MISSING suffix makes `registrable_domain` keep two
/// labels instead of three, so it returns LESS than the true domain
/// (`www.bbc.co.uk` → `co.uk`, not `bbc.co.uk`). Over-reduction costs a
/// dashboard bucket; it can never surface a subdomain that was not already
/// going to be surfaced. The old wording here said "a slightly-too-long domain"
/// while its own example showed a shorter one — the invariant is the opposite
/// of what it read as, and it is the invariant that keeps a workspace slug off
/// the wire. Pinned by `a_missing_suffix_can_only_ever_under_report`.
const MULTI_PART_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "net.uk", "sch.uk", "com.au", "net.au",
    "org.au", "edu.au", "gov.au", "co.jp", "or.jp", "ne.jp", "ac.jp", "go.jp", "com.br", "com.cn",
    "net.cn", "org.cn", "gov.cn", "co.nz", "org.nz", "net.nz", "co.za", "org.za", "com.mx",
    "com.ar", "com.tr", "com.sg", "com.hk", "com.tw", "com.my", "com.pl", "com.ua", "co.in",
    "net.in", "org.in", "co.kr", "or.kr", "com.es", "com.ru", "org.ru", "net.ru", "co.il",
    "com.vn", "com.ph", "co.th", "com.sa", "com.eg", "com.ng", "co.ke",
];

/// Reduce a host to its registrable domain: `myteam.atlassian.net` →
/// `atlassian.net`, `www.bbc.co.uk` → `bbc.co.uk`.
///
/// Used before any hostname leaves the machine. The subdomain is the part
/// that identifies WHO — a workspace slug names the user's employer — while
/// the domain alone names WHICH SITE, which is all our telemetry needs.
/// Dropping it is deliberate, not an approximation to tighten later.
///
/// IP literals and single-label hosts are returned unchanged.
pub fn registrable_domain(host: &str) -> String {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() <= 2 {
        return h;
    }
    // An IPv4 literal has no domain structure to reduce.
    if labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok()) {
        return h;
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let keep = if MULTI_PART_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    if labels.len() <= keep {
        return h;
    }
    labels[labels.len() - keep..].join(".")
}

/// The site plus the SECTION of it — `stackoverflow.com/questions`, never
/// `stackoverflow.com/questions/1234567/how-do-i`.
///
/// Telemetry that keeps only the domain cannot be acted on: it says a site
/// compacts badly, never which page SHAPE does, and every reader bug so far
/// has turned on exactly that distinction (`/questions/` vs `/tags/` on one
/// host). One segment separates them; the rest is the document itself, which
/// is the user's business and never ours (PO ruling, 2026-08-02).
///
/// A first segment that is only an identifier — digits, a hash, a UUID — names
/// a document rather than a section, so it is dropped: it carries no shape and
/// would be the part most able to single someone out.
pub fn domain_and_section(url: &str) -> String {
    let Some((_, host)) = scheme_host(url) else {
        return "unknown".into();
    };
    let domain = registrable_domain(&host);
    let after_host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or("");
    let seg = after_host
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    let identifier_only = seg.is_empty()
        || seg.len() > 24
        || seg.chars().all(|c| c.is_ascii_digit())
        || (seg.len() >= 8 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    if identifier_only {
        domain
    } else {
        format!("{domain}/{}", seg.to_ascii_lowercase())
    }
}
#[cfg(test)]
include!("url_tests.rs");
