//! The name of each supervisor, derived from its proxy's port.
use super::{LAUNCHD_LABEL, SERVICE_NAME, WINDOWS_TASK};

/// **One supervisor per PROXY, and there is one proxy per agent.**
///
/// `install::listen_for` deliberately assigns a distinct port to each agent
/// (8787, 8788, 8789…) because their UPSTREAMS differ — aider to OpenAI,
/// grok to x.ai, qwen-code to dashscope: a single process cannot serve them.
/// This layer, however, only knew ONE service name.
///
/// Consequence measured on a real machine: three agents configured, a single
/// unit, rewritten by each agent in turn. The last one won, the other two
/// pointed at a dead port, and the upgrade undid them one after the other
/// while announcing "nothing answered".
///
/// The port is the key: it is unique by construction, and it is what the
/// unit launches.
pub fn service_name(listen: &str) -> String {
    per_port(SERVICE_NAME, listen)
}

// A listen address we cannot read keeps the historical name: better the
// previous unit than a name made up from anything at all.
pub(super) fn per_port(base: &str, listen: &str) -> String {
    match listen.rsplit_once(':') {
        Some((_, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
            format!("{base}-{port}")
        }
        _ => base.to_string(),
    }
}

/// The matching launchd label.
pub fn launchd_label(listen: &str) -> String {
    per_port(LAUNCHD_LABEL, listen)
}

/// The matching scheduled task. Same rule: on 2026-10-06 on a Windows VM,
/// `install --agent codex` (8788) rewrote Claude Code's single
/// `tokenade\llm-proxy` task (8787); macOS already wrote one plist per port
/// but activated it under the portless name, a file that did not exist.
pub fn windows_task(listen: &str) -> String {
    per_port(WINDOWS_TASK, listen)
}

/// This task's XML file, next to the others.
pub(super) fn windows_task_file(listen: &str) -> String {
    format!("{}.xml", per_port("llm-proxy-task", listen))
}
