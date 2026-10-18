//! The light-agent's tool surface + execution, every call gated by the
//! [`Sandbox`]. Modelled on opencode's tool registry
//! (`packages/opencode/src/tool/registry.ts`) but deliberately *without*
//! a `task`/subagent tool — a light-agent must never recurse
//! (opencode denies `task` for subagents at
//! `agent/subagent-permissions.ts`; here we simply never register it).
//!
//! Tools route through the `tokenade` binary wherever a compacting
//! equivalent exists (`map`/`query`/`skeleton`/`semantic`/`web`), so the
//! agent's own tool output is folded by the same engine the orchestrator
//! benefits from — the delegate pays fewer of its (free) tokens too.

use super::sandbox::{Capability, Sandbox};
use std::path::Path;
use std::process::Command;

/// A parsed tool invocation from the model.
#[derive(Debug)]
pub enum Tool {
    /// Read a file (optionally a line range "start:end").
    Read {
        path: String,
        range: Option<(usize, usize)>,
    },
    /// Run a shell one-liner (sandboxed: read-only FS, writable /tmp only).
    Bash { cmd: String },
    /// Run a script in an interpreter, sandboxed. `code` is the remainder of
    /// the model's message after the `EXEC <lang>` line.
    Exec { lang: String, code: String },
    /// Run a `tokenade` explore/compact subcommand (map/query/skeleton/
    /// impact/semantic/auto/web …). Args are passed through verbatim.
    Tokenade { args: Vec<String> },
    /// Fetch a URL (shared HTTP client) and fold it (`… | tokenade web`).
    Web { url: String },
    /// Web search for a query (DuckDuckGo HTML → folded results).
    Search { query: String },
    /// Create or overwrite a file the agent owns. `content` is the whole
    /// remainder of the model's message after the `WRITE <path>` line.
    Write { path: String, content: String },
    /// Delete a file the agent created this run.
    Delete { path: String },
}

/// The self-describing tool catalogue injected into the agent's system
/// prompt (NOT the orchestrator scaffold). Kept compact — every byte is
/// paid on each of the agent's turns.
pub fn catalogue(sb: &Sandbox) -> String {
    let mut lines = vec![
        "TOOLS — emit exactly ONE action per turn. All but WRITE are a single line:".to_string(),
        "  READ <path> [start:end]        read a file (within the jail)".to_string(),
        "  TOKENADE <args…>               explore code: map | query <sym> | skeleton <f…> | impact <f> | semantic \"<q>\"".to_string(),
    ];
    if sb.has(Capability::Net) {
        lines.push("  SEARCH <query>                 web search (folded results)".to_string());
        lines.push("  WEB <url>                      fetch + fold a web page".to_string());
    }
    if sb.has(Capability::Shell) {
        lines.push("  BASH <cmd>                     run a shell one-liner (SANDBOXED: read-only FS + writable /tmp)".to_string());
        lines.push("  EXEC <lang>                    then code on the FOLLOWING lines; runs sandboxed (bash|sh|python|node|ruby|awk|jq|perl)".to_string());
    }
    if sb.has(Capability::Write) {
        lines.push("  WRITE <path>                   then the file body on the FOLLOWING lines (create/overwrite a file you own)".to_string());
    }
    if sb.has(Capability::Delete) {
        lines.push(
            "  RM <path>                      delete a file you created this run".to_string(),
        );
    }
    lines.push(
        "  FINAL <answer>                 stop and return this answer to the orchestrator"
            .to_string(),
    );
    lines.join("\n")
}

/// Parse one action line emitted by the model. Returns `Ok(None)` when
/// the line is a FINAL (handled by the runner), `Err` on malformed input.
pub fn parse(line: &str) -> Result<Tool, String> {
    let line = line.trim();
    let (verb, rest) = match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v.to_ascii_uppercase(), r.trim()),
        None => (line.to_ascii_uppercase(), ""),
    };
    match verb.as_str() {
        "READ" => {
            let mut parts = rest.rsplitn(2, char::is_whitespace);
            let last = parts.next().unwrap_or("");
            if let Some((a, b)) = last.split_once(':') {
                if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                    let path = parts.next().unwrap_or("").trim().to_string();
                    if !path.is_empty() {
                        return Ok(Tool::Read {
                            path,
                            range: Some((a, b)),
                        });
                    }
                }
            }
            Ok(Tool::Read {
                path: rest.to_string(),
                range: None,
            })
        }
        "BASH" | "SH" | "SHELL" => Ok(Tool::Bash {
            cmd: rest.to_string(),
        }),
        // Code is filled by the runner from the lines after this one.
        "EXEC" | "RUN" => Ok(Tool::Exec {
            lang: rest.trim().to_string(),
            code: String::new(),
        }),
        "TOKENADE" | "TK" => Ok(Tool::Tokenade {
            args: shell_split(rest),
        }),
        "WEB" | "FETCH" => Ok(Tool::Web {
            url: rest.to_string(),
        }),
        "SEARCH" | "GOOGLE" | "DDG" => Ok(Tool::Search {
            query: rest.to_string(),
        }),
        // Content is filled by the runner from the lines after this one.
        "WRITE" | "CREATE" => Ok(Tool::Write {
            path: rest.to_string(),
            content: String::new(),
        }),
        "RM" | "DELETE" | "DEL" => Ok(Tool::Delete {
            path: rest.to_string(),
        }),
        other => Err(format!(
            "unknown action `{other}` (expected READ/TOKENADE/SEARCH/WEB/BASH/WRITE/RM/FINAL)"
        )),
    }
}

/// Execute a gated tool, returning a text result (already trimmed to a
/// sane size). Denials are returned as `Ok(text)` so the agent sees the
/// refusal reason and can adapt, rather than crashing the loop.
pub fn execute(
    tool: &Tool,
    sb: &Sandbox,
    exe: &str,
    owned: &mut std::collections::HashSet<std::path::PathBuf>,
) -> String {
    match tool {
        Tool::Read { path, range } => {
            let p = Path::new(path);
            if let Err(d) = sb.gate_read(p) {
                return d.to_string();
            }
            read_file(p, *range)
        }
        Tool::Bash { cmd } => {
            if !sb.has(Capability::Shell) {
                return super::sandbox::Denial::Capability(Capability::Shell).to_string();
            }
            run_sandboxed(exe, "bash", cmd, sb)
        }
        Tool::Exec { lang, code } => {
            if !sb.has(Capability::Shell) {
                return super::sandbox::Denial::Capability(Capability::Shell).to_string();
            }
            let lang = if lang.trim().is_empty() {
                "bash"
            } else {
                lang.trim()
            };
            run_sandboxed(exe, lang, code, sb)
        }
        Tool::Tokenade { args } => {
            // SECURITY: this runs `tokenade <args>` OUTSIDE the bwrap sandbox,
            // so it MUST be an allow-list, not a deny-list. `tokenade wrap` /
            // `exec` / `raw` / `filter` / `proxy` all execute arbitrary
            // commands or scripts unconfined — a deny-list that misses any one
            // of them is a full sandbox escape (RCE). Only the read-only
            // code-intel explorers are permitted.
            match args.first().map(|s| s.as_str()) {
                Some(cmd) if TOKENADE_EXPLORE.contains(&cmd) => run(exe, args),
                Some(cmd) => format!(
                    "DENIED: `tokenade {cmd}` is not available to a light-agent — only \
                     {} (read-only code exploration). To run code use EXEC/BASH (sandboxed).",
                    TOKENADE_EXPLORE.join("/")
                ),
                None => "DENIED: empty tokenade command".to_string(),
            }
        }
        Tool::Web { url } => {
            if let Err(d) = sb.gate_net() {
                return d.to_string();
            }
            // SECURITY: block SSRF to loopback / private / link-local hosts and
            // cloud metadata endpoints (literal IPs + known names).
            if let Some(reason) = internal_host_block(url) {
                return format!("DENIED: {reason}");
            }
            // The shared HTTP client only speaks http/https, so a model-supplied
            // URL can't become `file:///etc/passwd`, `gopher://` SSRF, etc. on
            // the initial request OR a redirect (was curl's --proto/--proto-redir).
            let fetched = web_fetch(url, None);
            fold_web(exe, &fetched)
        }
        Tool::Search { query } => {
            if let Err(d) = sb.gate_net() {
                return d.to_string();
            }
            let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));
            let html = web_fetch(&url, Some("Mozilla/5.0"));
            // Fold the SERP HTML into title/url/snippet triples.
            fold_pipe(exe, &["search".into(), "-".into()], &html)
        }
        Tool::Write { path, content } => {
            // The sandbox resolves the airtight canonical target (jail +
            // symlink-safe) and enforces provenance; we write to exactly that.
            let target = match sb.gate_write(Path::new(path), owned) {
                Ok(t) => t,
                Err(d) => return d.to_string(),
            };
            match std::fs::write(&target, content) {
                Ok(()) => {
                    owned.insert(target.clone());
                    format!("WROTE {} ({} bytes)", target.display(), content.len())
                }
                Err(e) => format!("ERROR writing {}: {e}", target.display()),
            }
        }
        Tool::Delete { path } => {
            let target = match sb.gate_delete(Path::new(path), owned) {
                Ok(t) => t,
                Err(d) => return d.to_string(),
            };
            match std::fs::remove_file(&target) {
                Ok(()) => {
                    owned.remove(&target);
                    format!("DELETED {}", target.display())
                }
                Err(e) => format!("ERROR deleting {}: {e}", target.display()),
            }
        }
    }
}

/// Minimal percent-encoding for a search query.
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Read-only code-intel commands the light-agent may run via the TOKENADE
/// tool. Everything else — especially any command that executes code (`wrap`,
/// `exec`, `execute`, `raw`, `bypass`, `filter`, `proxy`, `run-script`) — is
/// refused, because this path runs OUTSIDE the sandbox. Code execution must
/// go through EXEC/BASH (which are bwrap-confined).
const TOKENADE_EXPLORE: &[&str] = &[
    "map", "query", "find", "impact", "skeleton", "semantic", "ask", "sem",
];

fn read_file(p: &Path, range: Option<(usize, usize)>) -> String {
    match std::fs::read_to_string(p) {
        Ok(s) => match range {
            Some((a, b)) => s
                .lines()
                .enumerate()
                .filter(|(i, _)| *i + 1 >= a && *i + 1 <= b)
                .map(|(i, l)| format!("{}\t{}", i + 1, l))
                .collect::<Vec<_>>()
                .join("\n"),
            None => clamp(&s, 24_000),
        },
        Err(e) => format!("ERROR reading {}: {e}", p.display()),
    }
}

/// Fetch a URL through the shared HTTP client (reqwest+rustls, corporate-proxy
/// aware) for the WEB/SEARCH tools. The client follows redirects and times out
/// at 30 s, and — crucially — only speaks http/https, so a model-supplied URL
/// can't reach `file://`, `gopher://`, `dict://`, etc. on either the initial
/// request or a redirect (was curl's `--proto`/`--proto-redir` guard). Returns
/// the body (clamped) or an error line, mirroring the old `run("curl", …)`.
fn web_fetch(url: &str, ua: Option<&str>) -> String {
    let headers: Vec<(&str, &str)> = ua.map(|u| vec![("User-Agent", u)]).unwrap_or_default();
    match tokenade_core::net::http_get(url, &headers, 30) {
        Some(r) => clamp(&String::from_utf8_lossy(&r.body), 24_000),
        None => "ERROR fetching url".to_string(),
    }
}

/// Reject URLs whose host is loopback / private / link-local, or a known
/// cloud-metadata endpoint — the classic SSRF targets. Heuristic on literal
/// hosts (does not resolve DNS, so a name that resolves to a private IP still
/// gets through — DNS-rebinding is out of scope for this layer).
fn internal_host_block(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    // host = up to the first '/', '?' or '#', minus any userinfo, minus port.
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    let host = if let Some(end) = hostport.strip_prefix('[').and_then(|h| h.split_once(']')) {
        end.0.to_ascii_lowercase() // IPv6 literal inside [...]
    } else {
        hostport
            .split(':')
            .next()
            .unwrap_or(hostport)
            .to_ascii_lowercase()
    };
    if host.is_empty() {
        return Some("empty host".into());
    }
    let named_block = ["localhost", "metadata.google.internal", "metadata.goog"];
    if named_block.contains(&host.as_str())
        || host.ends_with(".localhost")
        || host.starts_with("::1")
        || host.starts_with("fe80")
        || host.starts_with("fc")
        || host.starts_with("fd")
    {
        return Some(format!(
            "host `{host}` is internal/link-local (SSRF blocked)"
        ));
    }
    // IPv4 literal ranges.
    let oct: Vec<u8> = host
        .split('.')
        .filter_map(|s| s.parse::<u8>().ok())
        .collect();
    if oct.len() == 4 {
        let blocked = oct[0] == 127
            || oct[0] == 10
            || oct[0] == 0
            || (oct[0] == 192 && oct[1] == 168)
            || (oct[0] == 169 && oct[1] == 254)
            || (oct[0] == 172 && (16..=31).contains(&oct[1]))
            || (oct[0] == 100 && (64..=127).contains(&oct[1]));
        if blocked {
            return Some(format!("IP {host} is private/link-local (SSRF blocked)"));
        }
    }
    None
}

fn run(program: &str, args: &[String]) -> String {
    let mut probe = tokenade_core::process_util::internal_command(program);
    probe.args(args);
    tokenade_core::process_util::configure_no_window(&mut probe);
    match probe.output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            if s.trim().is_empty() {
                s = String::from_utf8_lossy(&o.stderr).to_string();
            }
            clamp(&s, 24_000)
        }
        Err(e) => format!("ERROR running {program}: {e}"),
    }
}

fn fold_web(exe: &str, html: &str) -> String {
    fold_pipe(exe, &["web".to_string(), "-".to_string()], html)
}

/// Run `code` in `lang` through `tokenade execute --sandbox` — a controlled
/// interpreter (timeout, stdout cap, secret redaction, output compaction)
/// confined by bubblewrap (read-only FS, writable /tmp only). This is the
/// ONE execution path for a sub-agent: it cannot write/delete real files, so
/// the provenance guarantee of WRITE/RM holds even for arbitrary scripts.
fn run_sandboxed(exe: &str, lang: &str, code: &str, sb: &Sandbox) -> String {
    // SECURITY: scripts run with NO network, deliberately. The Net capability
    // gates the runner-controlled SEARCH/WEB tools — it does NOT grant
    // arbitrary model-authored scripts internet access. Coupling them would
    // let a script read the (ro-bound) filesystem and exfiltrate it, which the
    // write-only sandbox otherwise prevents.
    let args: Vec<String> = vec![
        "execute".into(),
        "--lang".into(),
        lang.to_string(),
        "--script".into(),
        "-".into(),
        "--sandbox".into(),
        "--cwd".into(),
        sb.primary_root(),
    ];
    // `tokenade execute` already caps + compacts its own stdout; relay it.
    fold_pipe(exe, &args, code)
}

/// Pipe `input` into `tokenade <args…>` and return its folded stdout.
fn fold_pipe(exe: &str, args: &[String], input: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut fold = Command::new(exe);
    fold.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    tokenade_core::process_util::configure_no_window(&mut fold);
    let mut child = match fold.spawn() {
        Ok(c) => c,
        Err(_) => return clamp(input, 12_000),
    };
    if let Some(si) = child.stdin.as_mut() {
        let _ = si.write_all(input.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => clamp(&String::from_utf8_lossy(&o.stdout), 24_000),
        Err(_) => clamp(input, 12_000),
    }
}

/// Keep a tool result from blowing the free agent's own context.
fn clamp(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // The cut must land on a char boundary — and so must the slice that FINDS
    // it. `&s[..max]` panics on its own when `max` falls inside a multibyte
    // character, before any boundary logic below it could run.
    let head = tokenade_core::caps::truncate_at_char_boundary(s, max);
    format!("{}\n…[truncated {} bytes]", head, s.len() - head.len())
}

/// Minimal shell-ish arg splitter honouring single/double quotes.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_read_range() {
        match parse("READ src/main.rs 10:40").unwrap() {
            Tool::Read { path, range } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(range, Some((10, 40)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_tokenade_args() {
        match parse("TOKENADE query foo bar").unwrap() {
            Tool::Tokenade { args } => assert_eq!(args, vec!["query", "foo", "bar"]),
            _ => panic!(),
        }
    }

    #[test]
    fn shell_split_quotes() {
        assert_eq!(
            shell_split("semantic \"a b\" c"),
            vec!["semantic", "a b", "c"]
        );
    }

    #[test]
    fn parses_bash_web_verbs() {
        assert!(matches!(parse("BASH ls -la").unwrap(), Tool::Bash { .. }));
        assert!(matches!(
            parse("WEB https://x.dev").unwrap(),
            Tool::Web { .. }
        ));
        assert!(parse("BOGUS whatever").is_err());
    }

    #[test]
    fn tokenade_mutators_denied() {
        use super::super::sandbox::Sandbox;
        use std::path::Path;
        let sb = Sandbox::read_only(Path::new("."));
        let mut owned = std::collections::HashSet::new();
        // The allow-list must refuse every code-executing subcommand — a miss
        // here is a full sandbox escape (`tokenade wrap <cmd>` runs unconfined).
        for verb in [
            "wrap",
            "exec",
            "execute",
            "run-script",
            "raw",
            "bypass",
            "noproxy",
            "filter",
            "proxy",
            "install",
            "hooks",
            "login",
            "uninstall",
            "agent",
            "shellwrap",
        ] {
            let out = execute(
                &Tool::Tokenade {
                    args: vec![verb.into()],
                },
                &sb,
                "tokenade",
                &mut owned,
            );
            assert!(
                out.starts_with("DENIED"),
                "expected {verb} denied, got: {out}"
            );
        }
    }

    #[test]
    fn ssrf_internal_hosts_blocked() {
        for u in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:8080/",
            "https://localhost/x",
            "http://10.0.0.5/",
            "http://172.16.4.9/",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://user@127.0.0.1/",
        ] {
            assert!(internal_host_block(u).is_some(), "must block {u}");
        }
        for u in [
            "https://crates.io/api",
            "http://example.com/",
            "https://8.8.8.8/",
        ] {
            assert!(internal_host_block(u).is_none(), "must allow {u}");
        }
    }

    #[test]
    fn parses_new_verbs() {
        assert!(matches!(
            parse("SEARCH rust tokio").unwrap(),
            Tool::Search { .. }
        ));
        assert!(matches!(parse("WRITE a.txt").unwrap(), Tool::Write { .. }));
        assert!(matches!(parse("RM a.txt").unwrap(), Tool::Delete { .. }));
    }

    #[test]
    fn write_then_delete_roundtrip_tracks_provenance() {
        use super::super::sandbox::Sandbox;
        let dir = std::env::temp_dir().join(format!("tk_agent_prov_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sb = Sandbox::default_box(&dir);
        let mut owned = std::collections::HashSet::new();
        let f = dir.join("s.py");
        let fp = f.display().to_string();
        // create
        let out = execute(
            &Tool::Write {
                path: fp.clone(),
                content: "print(1)".into(),
            },
            &sb,
            "tokenade",
            &mut owned,
        );
        assert!(out.starts_with("WROTE"), "{out}");
        assert!(f.exists());
        // delete the owned file
        let out = execute(
            &Tool::Delete { path: fp.clone() },
            &sb,
            "tokenade",
            &mut owned,
        );
        assert!(out.starts_with("DELETED"), "{out}");
        assert!(!f.exists());
        // deleting a file the agent did NOT create is denied
        let other = dir.join("pre.txt");
        std::fs::write(&other, "x").unwrap();
        let out = execute(
            &Tool::Delete {
                path: other.display().to_string(),
            },
            &sb,
            "tokenade",
            &mut owned,
        );
        assert!(out.starts_with("DENIED"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clamp_respects_utf8_boundary() {
        // 400 × 'é' (2 bytes each) = 800 bytes; clamp at 500 must not split a
        // codepoint (multi-language safety).
        let s = "é".repeat(400);
        let out = clamp(&s, 500);
        assert!(out.is_char_boundary(out.len())); // trivially true, but also:
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        assert!(out.contains("truncated"));
        // Non-truncating case returns input unchanged.
        assert_eq!(clamp("short", 500), "short");
    }
}
