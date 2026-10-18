//! **Serving** — and the question that avoids the worst misunderstanding.
//!
//! `our_proxy_health` asks a port whether it is OURS. Without it, "the port
//! answers" reads as "it is installed and it works", while any other service
//! listening there would give the same answer — and the agent would be talking
//! to it.
//!
//! A single process serves ALL the agents the install records name: one proxy
//! per agent would multiply the ports, the supervision units and the ways of
//! getting the recipient wrong.

use super::*;

/// Serve every agent the install records name, from ONE process.
///
/// The operator's requirement, stated plainly: three agents on a machine must
/// not mean three proxies. Everything needed is already in the records — each
/// agent's provider and the port it was pointed at — so this rebuilds the whole
/// set with nothing typed twice, and an agent installed later is picked up by
/// restarting one service instead of adding another.
pub(crate) fn serve_installed(remote: bool, compaction: bool) -> i32 {
    let Some(home) = home() else {
        eprintln!("tokenade llm-proxy: no home directory");
        return 1;
    };
    let records = match install::load_records(&home) {
        Ok(r) => r,
        // A records file we cannot read is NOT "nothing is installed". This
        // repo has already shipped a fallback-to-zero that made torn state read
        // as never-used; here it would serve nothing while every installed
        // agent points at a port with no listener.
        Err(e) => {
            eprintln!("tokenade llm-proxy: {e:?}");
            return 1;
        }
    };
    let proxies: Vec<server::Proxy> = records
        .iter()
        .map(|r| {
            let cfg = config::Config {
                listen: if r.listen.is_empty() {
                    DEFAULT_LISTEN.to_string()
                } else {
                    r.listen.clone()
                },
                upstream: r.upstream.clone(),
                deployment: if remote {
                    config::Deployment::Remote
                } else {
                    config::Deployment::Local
                },
                compaction,
                agent: agent_matrix::by_name(&r.agent),
                system_preamble: super::super::features::preamble_for(agent_matrix::by_name(
                    &r.agent,
                )),
                ..config::Config::default()
            };
            server::Proxy::new(
                cfg,
                server::default_folder(),
                Box::new(server::LedgerAccounting),
            )
        })
        .collect();
    match server::serve_all(proxies) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tokenade llm-proxy: {e}");
            1
        }
    }
}

pub(crate) fn home() -> Option<PathBuf> {
    tokenade_core::platform::home_dir()
}

/// Answer "is it up?" and, if not, put it up — then return.
///
/// Deliberately cheap and deliberately silent. It runs once a minute forever on
/// a Windows desktop; anything it prints goes to a log nobody reads, and
/// anything it waits on delays the next firing.
///
/// The liveness test is a TCP connect rather than the health route: a proxy
/// that accepts a connection is a proxy holding the port, which is the whole of
/// what "already running" has to mean here. Asking for HTTP would make a slow
/// or busy proxy look dead and start a SECOND one on a port it cannot bind.
/// Is OUR proxy the thing listening on `listen`?
///
/// A `connect()` answers "someone is listening", which is not the question.
/// Measured 2026-08-21: during a bench pass, a rank-4 proxy held the port, the
/// Windows scheduled task concluded "already alive" and never started the
/// supervised proxy — the "survives a kill" column went from 24/24 `Ok proxy`
/// to 24/24 `Ko proxy` without a single line of the supervision path changing.
/// In production it is the same hole: any port squatter neutralises the
/// supervisor for good.
///
/// The proxy serves its own `/__tokenade/health` route, whose body carries
/// `"ok":true`. We ask for it, with the same time limit as the former connect,
/// and a port held by someone else answers something else (or nothing) — so we
/// start.
/// The version the answering proxy ANNOUNCES, when it is ours.
///
/// `None` = nobody of ours on that port. `Some(None)` = it is our proxy, but it
/// does not state its version — so a binary older than the field, i.e. a
/// process left in memory since an update.
pub(crate) fn our_proxy_health(listen: &str) -> Option<Option<String>> {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let addr = listen.to_socket_addrs().ok().and_then(|mut it| it.next())?;
    let timeout = std::time::Duration::from_millis(700);
    let mut sock = std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let req =
        format!("GET /__tokenade/health HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    // Bounded: a health response fits in a few hundred bytes, and a chatty
    // server must not be able to hold the supervisor.
    let _ = sock.take(8192).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    if !(text.contains("200") && text.contains("\"ok\":true")) {
        return None;
    }
    Some(version_in_health_body(&text))
}

/// The `version` field of the health body, without depending on a JSON parser
/// for a response we already read as bounded text.
pub(crate) fn version_in_health_body(text: &str) -> Option<String> {
    let at = text.find("\"version\"")? + "\"version\"".len();
    let rest = &text[at..];
    let open = rest.find('"')? + 1;
    let rest = &rest[open..];
    let close = rest.find('"')?;
    let v = &rest[..close];
    (!v.is_empty()).then(|| v.to_string())
}

pub(crate) fn do_ensure_running(listen: &str, upstream: &str, agent: Option<&str>) -> i32 {
    if our_proxy_health(listen).is_some() {
        return 0;
    }
    // Same rule as when writing the unit: after the binary is replaced,
    // `/proc/self/exe` returns a path suffixed ` (deleted)` that cannot be
    // executed. `stable_exe` falls back to the installed tokenade.
    let exe = std::path::PathBuf::from(tokenade_core::platform::stable_exe());
    match tokenade_core::process_util::spawn_outliving_its_job(
        &exe,
        &[
            &["llm-proxy", "--listen", listen, "--upstream", upstream][..],
            agent
                .map(|a| ["--agent", a])
                .as_ref()
                .map(|a| &a[..])
                .unwrap_or(&[]),
        ]
        .concat(),
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tokenade llm-proxy: could not start the proxy: {e}");
            1
        }
    }
}
