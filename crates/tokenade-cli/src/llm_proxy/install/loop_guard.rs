//! **Never designate yourself as the upstream.**
//!
//! A proxy pointing at itself loops until the system is exhausted, and the
//! mistake is easy: the listen address and the upstream address are not
//! written the same way (`localhost` versus `127.0.0.1`, implicit port versus
//! explicit port). Every rule here compares what is COMPARABLE rather than two
//! strings.

use super::*;

/// The sentinel [`Site::ClineProvidersJson`] returns instead of a vendor URL.
///
/// A value that could never be a working upstream, so a future caller that
/// forgets the refusal in `plan` fails loudly rather than sending traffic
/// somewhere plausible.
pub(super) const UPSTREAM_MUST_BE_EXPLICIT: &str = "";

/// The `host:port` inside a base URL we wrote.
///
/// The record keeps the authority rather than the URL because that is what
/// `serve` binds; `base_url_for` is the one place that turns it back into a URL,
/// so the two forms cannot drift.
pub(super) fn listen_of(url: &str) -> String {
    url.split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string()
}

/// True when `url` carries anything after its authority.
///
/// `http://127.0.0.1:8787` and `http://127.0.0.1:8787/` are both path-free; the
/// trailing slash is the empty path and joins cleanly.
pub fn url_has_path(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find(['/', '?']) {
        None => false,
        Some(i) => !rest[i..].is_empty() && rest[i..] != *"/",
    }
}

/// `(host, port)` of a URL, lowercased host, IPv6 brackets removed. `None`
/// when there is no port to compare — a loopback URL with no port is `:80`,
/// and saying so explicitly beats guessing.
pub(super) fn host_port(url: &str) -> Option<(String, u16)> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or("");
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        // `[::1]` has colons of its own: a port is only what follows the
        // closing bracket.
        Some((h, p))
            if !p.contains(']') && p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() =>
        {
            (h, p.parse::<u16>().ok()?)
        }
        _ => (
            authority,
            if scheme.eq_ignore_ascii_case("https") {
                443
            } else {
                80
            },
        ),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_ascii_lowercase(), port))
}

/// True when `url`'s host is on this machine.
pub(super) fn is_loopback_host(url: &str) -> bool {
    let Some((host, _)) = host_port(url) else {
        return false;
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// True when installing would make the proxy its own upstream.
///
/// Loopback ALONE used to answer this, and that refused the install for every
/// user running a local model — Ollama on 11434, LM Studio on 1234, llama.cpp,
/// vLLM on 8000 — with "the agent already points at this proxy", about an
/// endpoint that is not this proxy and never was. Chaining in front of a local
/// server is not a loop, it is the ordinary case, and it is the setup our own
/// documentation tells people to run.
///
/// The port is what tells us apart, and the records file says which ports are
/// ours: every install writes its `listen` there, so a proxy moved to another
/// port is still recognised (the case loopback-alone was reaching for) without
/// swallowing every other local service with it. A loopback endpoint on a port
/// we have no record of is asked directly — our health route answers only from
/// us, so a live proxy nobody recorded is still caught, and any other server
/// answers 404 or nothing and is chained normally.
pub(super) fn would_loop(url: &str, home: &Path, listen_url: &str) -> bool {
    match loop_by_port(url, home, listen_url) {
        // Known to be one of our ports — no need to ask anyone.
        true => true,
        // Unknown port on this machine: ask it. Only our proxy answers this
        // route, so a live proxy nobody recorded is still caught and every
        // other local server is chained normally.
        false => is_loopback_host(url) && answers_our_health_route(url),
    }
}

/// The half of [`would_loop`] that reads only local state: is this a loopback
/// URL on a port we KNOW is ours — the one this install is about to take, or
/// one a previous install wrote into the records file?
///
/// Separated so the decision can be asserted without a socket. The probe below
/// is the other half, and it is exercised against a listener a test stands up
/// itself rather than against whatever happens to be running on the machine.
pub(super) fn loop_by_port(url: &str, home: &Path, listen_url: &str) -> bool {
    if !is_loopback_host(url) {
        return false;
    }
    let Some(target) = host_port(url) else {
        return false;
    };
    if host_port(listen_url).is_some_and(|l| l.1 == target.1) {
        return true;
    }
    load_records(home)
        .unwrap_or_default()
        .iter()
        .any(|r| host_port(&r.listen).is_some_and(|l| l.1 == target.1))
}

/// Ask a loopback endpoint whether it is a tokenade proxy.
///
/// The supervisor asks the SAME question of the SAME route to decide whether
/// to start a proxy (`cli::our_proxy_health`), so this calls it rather than
/// carrying a second copy of the request, the timeout and the body test. Two
/// copies of one fact is the class this repository keeps paying for, and here
/// they would drift into disagreeing about what "is that us?" means.
///
/// A server that does not answer in time is not ours as far as this decision
/// goes, and being wrong in that direction costs a refused install — which is
/// the failure the caller exists to prevent.
pub(super) fn answers_our_health_route(url: &str) -> bool {
    let Some((host, port)) = host_port(url) else {
        return false;
    };
    crate::llm_proxy::cli::our_proxy_health(&format!("{host}:{port}")).is_some()
}

/// A site whose file this recipe may create when it is not there yet.
///
/// Only the dotenv one. Every other site carries the user's provider choice,
/// and a file we invented would not carry it — the install would succeed and
/// point the agent at a proxy forwarding to a vendor default nobody chose.
/// A dotenv file holds ONE key and nothing else, so creating it adds no
/// assumption.
pub(super) fn may_create(site: Site) -> bool {
    matches!(site, Site::GeminiEnvFile | Site::ShellProfile { .. })
}
