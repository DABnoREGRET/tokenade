//! **What we have already done, and what we propose to do.**
//!
//! The plan reads the REAL state of the files, not a journal: a user may have
//! edited their configuration by hand between two calls. A refusal, though, is
//! remembered — offering an already-declined install at every start is the
//! best way to have it declined for ever.

use super::*;

/// Read a site's file, tolerating absence where the recipe allows it.
///
/// Returns `(text, created)`. The parent directory must exist even then: it is
/// what proves the agent is installed, and without that check an install would
/// scatter config for agents this machine does not have.
pub(super) fn read_site(site: Site, path: &Path) -> Result<(String, bool), InstallError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok((t, false)),
        // A MISSING shell profile is the normal case, not proof that the
        // agent is absent: the person simply never needed to write one. The
        // directory is not proof either — on Windows,
        // `Documents\WindowsPowerShell\` only exists if someone has already
        // customised PowerShell. Measured 2026-08-21: with the common rule,
        // cursor went from `Partial proxy` to `Ko proxy` on Windows over a
        // missing folder, while the same install succeeded on Linux. The proof
        // that the agent is there comes from elsewhere — we install it because
        // it was asked for by name.
        Err(_) if matches!(site, Site::ShellProfile { .. }) => Ok((String::new(), true)),
        Err(_) if may_create(site) && path.parent().is_some_and(Path::is_dir) => {
            Ok((String::new(), true))
        }
        Err(_) => Err(InstallError::NoConfig(path.to_path_buf())),
    }
}

/// Read an agent's config and work out what installing would do. Writes nothing.
pub fn plan(agent: Agent, home: &Path, listen_url: &str) -> Result<Plan, InstallError> {
    let site = recipe(agent).ok_or(InstallError::NoRecipeMeasured(agent.as_str()))?;
    let path = site.path(home);
    if url_has_path(listen_url) {
        return Err(InstallError::ListenUrlHasPath(listen_url.to_string()));
    }
    let (text, created_file) = read_site(site, &path)?;
    let (mut path, mut created_file) = (path, created_file);
    let (upstream, previous_selector) = match site {
        Site::CodexToml => read_codex(&text, &path)?,
        Site::AiderYaml => (read_aider(&text), None),
        Site::OpenCodeJson { .. } => read_opencode(&text, &path)?,
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        } => read_qwen(&text, &path)?,
        Site::SettingsEnvJson { flavour } => (read_env_key(&text, &path, flavour.key())?, None),
        // `previous_selector` carries what the variable was worth before us,
        // and `upstream` what the proxy must relay to. With no previous value,
        // the upstream is the one cursor uses by default.
        Site::ShellProfile { var } => {
            let prev = read_shell_profile(&text, var);
            (
                Some(
                    prev.clone()
                        .unwrap_or_else(|| CURSOR_DEFAULT_UPSTREAM.to_string()),
                ),
                prev,
            )
        }
        Site::GrokToml => (read_grok(&text, &path)?, None),
        Site::ReasonixToml => read_reasonix(&text, &path)?,
        Site::HermesYaml => (read_hermes(&text), None),
        Site::ClineProvidersJson => read_cline(&text, &path)?,
        Site::GeminiEnvFile => (read_dotenv(&text, GEMINI_ENDPOINT_KEY), None),
        Site::FactoryConfigJson => read_factory(&text, &path)?,
        Site::PiModelsStore => read_pi(&text, &path)?,
        Site::VsCodeSettingsJson => (read_flat_key(&text, &path, DEVIN_INFERENCE_KEY)?, None),
        Site::VsCodeCopilotSettings => (read_flat_key(&text, &path, COPILOT_CAPI_KEY)?, None),
    };
    // Qwen: a base URL in `~/.qwen/.env` outranks `settings.env`.
    let (upstream, previous_selector) = match (site, &previous_selector) {
        (
            Site::SettingsEnvJson {
                flavour: SettingsFlavour::Qwen,
            },
            None,
        ) => {
            let env_path = path.with_file_name(".env");
            match std::fs::read_to_string(&env_path)
                .ok()
                .and_then(|t| read_dotenv(&t, "OPENAI_BASE_URL"))
            {
                Some(u) => {
                    path = env_path;
                    created_file = false;
                    (Some(u), Some(QWEN_DOTENV_SELECTOR.to_string()))
                }
                None => (upstream, previous_selector),
            }
        }
        _ => (upstream, previous_selector),
    };
    let had_explicit_upstream = upstream.is_some();
    if upstream.is_none()
        && matches!(
            site,
            Site::ClineProvidersJson | Site::FactoryConfigJson | Site::PiModelsStore
        )
    {
        return Err(InstallError::NoUpstreamMeasured {
            agent: agent.as_str(),
            provider: previous_selector.unwrap_or_else(|| "?".to_string()),
        });
    }
    // Qwen loads `~/.qwen/.env` into its environment: a base URL set there is
    // where it really goes when the settings say nothing — the upstream, but
    // not a key of ours to restore on revert.
    let upstream = upstream.unwrap_or_else(|| match site {
        Site::CodexToml if codex_uses_chatgpt_account(home) => CODEX_CHATGPT_UPSTREAM.to_string(),
        _ => site.vendor_default().to_string(),
    });
    if would_loop(&upstream, home, listen_url) {
        return Err(InstallError::WouldLoop(upstream));
    }
    Ok(Plan {
        agent,
        path,
        had_explicit_upstream,
        upstream,
        previous_selector,
        listen_url: listen_url.to_string(),
        created_file,
    })
}

/// The `host:port` to point the next agent at.
///
/// Each agent gets its OWN port, and that is what makes one process able to
/// serve them all: the port carries the agent's identity, so the proxy knows
/// which upstream to forward to and whether it may fold. Sharing one port
/// between two agents would silently send one agent's traffic to the other's
/// provider — a config that looks right and fails on every turn.
///
/// Re-installing an agent keeps the port it already had, so an agent whose
/// config still names it is not left pointing at a port nobody listens on.
/// Does the agent, as configured NOW, go around the proxy it was installed
/// for? Qwen: a selected model whose provider entry carries its own `baseUrl`
/// (not ours) — the env key an older install wrote is never read for it.
pub fn bypassed(agent: Agent, home: &Path, listen_url: &str) -> bool {
    // A re-plan that finds a higher layer NOT pointing at us (it would refuse
    // with `WouldLoop` if it were ours).
    matches!(
        recipe(agent),
        Some(Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen
        })
    ) && plan(agent, home, listen_url).is_ok_and(|p| p.previous_selector.is_some())
}

pub fn listen_for(home: &Path, agent: Agent, base: &str, requested: Option<&str>) -> String {
    if let Some(r) = requested {
        return r.to_string();
    }
    let records = load_records(home).unwrap_or_default();
    if let Some(mine) = records
        .iter()
        .find(|r| r.agent == agent.as_str() && !r.listen.is_empty())
    {
        return mine.listen.clone();
    }
    let (host, first) = match base.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(8787)),
        None => (base.to_string(), 8787),
    };
    let taken: std::collections::HashSet<&str> =
        records.iter().map(|r| r.listen.as_str()).collect();
    for n in 0..64u16 {
        let candidate = format!("{host}:{}", first.saturating_add(n));
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    base.to_string()
}

/// The agents the user said no to.
///
/// The proxy now installs itself wherever it turns on a switched-off box, and
/// a thing that installs itself MUST know how to keep quiet. Without this
/// list, an `llm-proxy uninstall` would be undone by the next automatic repair
/// — the product would fight its user, in a loop and without saying so.
///
/// A separate file rather than a field of the records: those are a serialised
/// `Vec<Record>`, and adding a sibling would change the format of a file that
/// older versions read back.
pub fn declined_path(home: &Path) -> PathBuf {
    home.join(".tokenade").join("llm-proxy-declined")
}

/// Was this agent explicitly declined?
pub fn is_declined(home: &Path, agent: Agent) -> bool {
    let Ok(text) = std::fs::read_to_string(declined_path(home)) else {
        return false;
    };
    text.lines().any(|l| l.trim() == agent.as_str())
}

/// Remember a refusal. Idempotent.
pub fn mark_declined(home: &Path, agent: Agent) {
    if is_declined(home, agent) {
        return;
    }
    let path = declined_path(home);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(agent.as_str());
    text.push('\n');
    let _ = tokenade_core::paths::write_atomic(&path, &text);
}

/// Forget a refusal — that is what a hand-requested install does.
pub fn clear_declined(home: &Path, agent: Agent) {
    let path = declined_path(home);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| l.trim() != agent.as_str() && !l.trim().is_empty())
        .collect();
    if kept.len() == text.lines().filter(|l| !l.trim().is_empty()).count() {
        return;
    }
    if kept.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let _ = tokenade_core::paths::write_atomic(&path, &format!("{}\n", kept.join("\n")));
}

/// Where the install records live, under `home`.
pub fn records_path(home: &Path) -> PathBuf {
    home.join(".tokenade").join("llm-proxy-install.json")
}

/// Why the records could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum RecordsError {
    /// The file is there and does not parse. Emphatically NOT the same as "no
    /// installs": this project has already shipped a fallback-to-zero that made
    /// torn state files read as "never used", and here that mistake would tell a
    /// user nothing is installed while their agent points at a dead port.
    Unreadable(PathBuf),
}

/// Read the install records. `Ok(vec![])` means nothing is installed.
pub fn load_records(home: &Path) -> Result<Vec<Record>, RecordsError> {
    let path = records_path(home);
    let Ok(text) = std::fs::read_to_string(&path) else {
        // Absent is a fact, and it is the fact "nothing installed".
        return Ok(Vec::new());
    };
    if text.trim().is_empty() {
        // A zero-length file is what a torn write leaves behind. Reporting it as
        // "no installs" is the failure mode above, so it is an error here.
        return Err(RecordsError::Unreadable(path));
    }
    serde_json::from_str(&text).map_err(|_| RecordsError::Unreadable(path))
}

/// Write the records atomically.
///
/// `write_atomic` rather than `fs::write`, which truncates first: a crash
/// between truncate and write leaves an empty file, and this repo has measured
/// 69 % empty reads from exactly that pattern.
pub(super) fn save_records(home: &Path, records: &[Record]) -> Result<(), InstallError> {
    let path = records_path(home);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| InstallError::Io(e.to_string()))?;
    }
    let body =
        serde_json::to_string_pretty(records).map_err(|e| InstallError::Io(e.to_string()))?;
    tokenade_core::paths::write_atomic(&path, &body).map_err(|e| InstallError::Io(e.to_string()))
}
