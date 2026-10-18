//! Rows booked on behalf of another agent (the LLM proxy serves several from
//! one process).

thread_local! {
    /// The agent (and model) a row is booked for, when the writing process is
    /// not that agent — see [`as_agent`].
    pub(super) static ROW_AGENT: std::cell::RefCell<Option<(String, Option<String>)>> = const { std::cell::RefCell::new(None) };
}

/// Book every row written inside `f` for `agent` (and `model`, when known)
/// rather than for whatever this process's environment says. The LLM proxy
/// serves several agents from ONE process: [`crate::detect::from_env`] would
/// name none of them, and the session-derived model is absent.
pub fn as_agent<R>(agent: &str, model: Option<&str>, f: impl FnOnce() -> R) -> R {
    ROW_AGENT.with(|c| *c.borrow_mut() = Some((agent.to_string(), model.map(str::to_string))));
    let r = f();
    ROW_AGENT.with(|c| *c.borrow_mut() = None);
    r
}

static PROCESS_AGENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Book every row this PROCESS writes for `agent`, on every thread. For a
/// wrapper that serves exactly one app whose environment names none — the MCP
/// proxy a desktop app spawns (Claude Desktop, the Codex app) books from its
/// relay thread, and `detect::from_env` sees nothing there. First call wins.
pub fn set_process_agent(agent: &str) {
    let _ = PROCESS_AGENT.set(agent.to_string());
}

/// The agent (and model) rows are being booked for, if [`as_agent`] is active
/// or the process named one.
pub(super) fn row_agent() -> Option<(String, Option<String>)> {
    ROW_AGENT
        .with(|c| c.borrow().clone())
        .or_else(|| PROCESS_AGENT.get().map(|a| (a.clone(), None)))
}
