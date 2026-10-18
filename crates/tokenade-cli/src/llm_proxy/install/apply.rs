//! **Executing the plan, and knowing how to undo it.**
//!
//! Install and uninstall are written side by side on purpose: every write from
//! here must have its inverse, and reading them together is the only way to
//! see that one is missing.

use super::*;

/// Install, and remember it so an uninstall does not have to guess.
///
/// The ORDER is the point, and it is the reason this function exists rather
/// than leaving the caller to compose the two halves. The record goes down
/// first: if the process dies between the two steps, the worst case is a record
/// for an install that did not happen — and [`uninstall`] treats a config that
/// no longer mentions us as a no-op. The opposite order would leave a rewritten
/// config with nothing that remembers what it used to say, which is a user
/// whose agent points at a dead port and no way back but a backup file.
pub fn install(home: &Path, plan: &Plan) -> Result<Record, InstallError> {
    let mut records = load_records(home).unwrap_or_default();
    let record = Record {
        agent: plan.agent.as_str().to_string(),
        path: plan.path.clone(),
        upstream: plan.upstream.clone(),
        previous_selector: plan.previous_selector.clone(),
        had_explicit_upstream: plan.had_explicit_upstream,
        created_file: plan.created_file,
        listen: listen_of(&plan.listen_url),
    };
    records.retain(|r| r.agent != record.agent);
    records.push(record.clone());
    save_records(home, &records)?;

    if let Err(e) = apply(plan) {
        // Put the books back: a record for an install that failed would make
        // `uninstall` rewrite a config we never touched.
        let mut back = load_records(home).unwrap_or_default();
        back.retain(|r| r.agent != record.agent);
        let _ = save_records(home, &back);
        return Err(e);
    }
    Ok(record)
}

/// Undo the install for one agent, using the remembered record.
///
/// `Ok(None)` when there is nothing recorded for that agent — an uninstall of
/// something that was never installed is a no-op, not a failure.
pub fn uninstall(home: &Path, agent: Agent) -> Result<Option<Record>, InstallError> {
    uninstall_slug(home, agent.as_str())
}

/// [`uninstall`] by the recorded name — the only one left to an agent removed
/// from the product (see [`legacy_site`]).
pub fn uninstall_slug(home: &Path, slug: &str) -> Result<Option<Record>, InstallError> {
    let mut records = match load_records(home) {
        Ok(r) => r,
        // We cannot know what to put back. Refusing beats guessing: reverting
        // to a vendor default we invented would silently move a user off the
        // provider they had configured.
        Err(RecordsError::Unreadable(p)) => {
            return Err(InstallError::Io(format!(
                "{} could not be read, so the previous configuration is unknown — \
                 restore it from the .tokenade-backup-* file next to the agent's config",
                p.display()
            )))
        }
    };
    let Some(i) = records.iter().position(|r| r.agent == slug) else {
        return Ok(None);
    };
    let record = records.remove(i);
    revert(&record)?;
    save_records(home, &records)?;
    Ok(Some(record))
}

/// Apply a plan: back the file up, write the new config atomically.
///
/// Left separate from [`install`] so a test can assert the file content without
/// a second side effect, and so the ordering rationale lives in exactly one
/// place.
pub fn apply(plan: &Plan) -> Result<Record, InstallError> {
    let site = recipe(plan.agent).ok_or(InstallError::NoRecipeMeasured(plan.agent.as_str()))?;
    let (text, _) = read_site(site, &plan.path)?;
    let next = match site {
        Site::CodexToml => write_codex(
            &text,
            &plan.listen_url,
            &plan.path,
            plan.upstream == CODEX_CHATGPT_UPSTREAM,
        )?,
        Site::AiderYaml => write_aider(&text, &plan.listen_url),
        Site::HermesYaml => write_hermes(&text, &plan.listen_url),
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        } if plan.previous_selector.as_deref() == Some(QWEN_DOTENV_SELECTOR) => {
            write_dotenv(&text, "OPENAI_BASE_URL", &plan.listen_url)
        }
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        } if plan
            .previous_selector
            .as_deref()
            .is_some_and(|s| s.starts_with(QWEN_MODEL_SELECTOR)) =>
        {
            let id =
                &plan.previous_selector.as_deref().unwrap_or_default()[QWEN_MODEL_SELECTOR.len()..];
            write_qwen_model(&text, &plan.listen_url, id, &plan.path)?
        }
        Site::SettingsEnvJson { flavour } => {
            write_env_key(&text, &plan.listen_url, flavour.key(), &plan.path)?
        }
        Site::ShellProfile { var } => write_shell_profile(&text, &plan.path, var, &plan.listen_url),
        Site::OpenCodeJson { .. } => write_opencode(
            &text,
            &plan.listen_url,
            plan.previous_selector
                .as_deref()
                .unwrap_or(OPENCODE_DEFAULT_PROVIDER),
            &plan.path,
        )?,
        Site::GrokToml => write_grok(&text, &plan.listen_url, &plan.path)?,
        Site::ReasonixToml => write_reasonix(
            &text,
            &plan.listen_url,
            plan.previous_selector.as_deref().unwrap_or_default(),
            &plan.path,
        )?,
        Site::GeminiEnvFile => write_dotenv(&text, GEMINI_ENDPOINT_KEY, &plan.listen_url),
        Site::FactoryConfigJson => write_factory(
            &text,
            &plan.listen_url,
            plan.previous_selector.as_deref().unwrap_or_default(),
            &plan.path,
        )?,
        Site::VsCodeSettingsJson => {
            write_flat_key(&text, &plan.path, DEVIN_INFERENCE_KEY, &plan.listen_url)?
        }
        Site::VsCodeCopilotSettings => {
            write_flat_key(&text, &plan.path, COPILOT_CAPI_KEY, &plan.listen_url)?
        }
        Site::PiModelsStore => write_pi(
            &text,
            &plan.listen_url,
            plan.previous_selector.as_deref().unwrap_or_default(),
            &plan.path,
        )?,
        Site::ClineProvidersJson => write_cline(
            &text,
            &plan.listen_url,
            plan.previous_selector
                .as_deref()
                .ok_or_else(|| InstallError::Unparseable(plan.path.clone()))?,
            &plan.path,
        )?,
    };
    crate::diagnostics_cli::backup_file(&plan.path);
    // The profile directory may not exist — see `read_site`.
    if let Some(dir) = plan.path.parent() {
        if !dir.is_dir() {
            std::fs::create_dir_all(dir).map_err(|e| InstallError::Io(e.to_string()))?;
        }
    }
    tokenade_core::paths::write_atomic(&plan.path, &next)
        .map_err(|e| InstallError::Io(e.to_string()))?;
    Ok(Record {
        agent: plan.agent.as_str().to_string(),
        had_explicit_upstream: plan.had_explicit_upstream,
        path: plan.path.clone(),
        upstream: plan.upstream.clone(),
        previous_selector: plan.previous_selector.clone(),
        created_file: plan.created_file,
        listen: listen_of(&plan.listen_url),
    })
}

/// Point Codex at the proxy, leaving every other key exactly as it was.
///
/// `toml_edit` rather than `toml`: this file carries the user's own comments,
/// their project trust levels and their hook trust hashes. A round trip through
/// a value-only parser would rewrite all of it, and a config rewrite that loses
/// a `trusted_hash` silently disables the hooks the user approved.
pub(super) fn write_codex(
    text: &str,
    listen_url: &str,
    path: &Path,
    chatgpt_account: bool,
) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    doc["model_provider"] = toml_edit::value(PROVIDER_ID);
    let providers = doc
        .entry("model_providers")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(t) = providers.as_table_mut() {
        // Implicit: a parent that exists only to hold `[model_providers.tokenade]`
        // must not print an empty `[model_providers]` header of its own. This is
        // the user's own config file and they read it; noise we added is noise
        // they have to reason about.
        t.set_implicit(true);
        let mut entry = toml_edit::Table::new();
        entry["name"] = toml_edit::value("tokenade llm proxy");
        entry["base_url"] = toml_edit::value(listen_url);
        // Codex's only route is the Responses API, measured in its binary.
        entry["wire_api"] = toml_edit::value("responses");
        // The agent keeps handling its own credential; we never read or store
        // it. A ChatGPT account (no API key) is sent by Codex itself when the
        // provider asks for it; an API key is named by `env_key`, where CODEX
        // looks — not a variable we set.
        if chatgpt_account {
            entry["requires_openai_auth"] = toml_edit::value(true);
        } else {
            entry["env_key"] = toml_edit::value("OPENAI_API_KEY");
        }
        t[PROVIDER_ID] = toml_edit::Item::Table(entry);
    }
    Ok(doc.to_string())
}

/// Point aider at the proxy.
///
/// Line-oriented on purpose: `.aider.conf.yml` is flat, and a real YAML round
/// trip would need a writer this tree does not carry. Replacing one line, or
/// appending it, touches nothing else in the file.
pub(super) fn write_aider(text: &str, listen_url: &str) -> String {
    let line = format!("openai-api-base: {listen_url}");
    let mut replaced = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("openai-api-base:") && !l.trim_start().starts_with('#') {
                replaced = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !replaced {
        out.push(line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Put the agent back where it was.
///
/// Uses the record rather than the backup file: a backup is a snapshot of the
/// whole file, so restoring it would also revert anything the user changed in
/// the meantime. Only the keys this module wrote are put back.
pub fn revert(record: &Record) -> Result<(), InstallError> {
    let text = std::fs::read_to_string(&record.path)
        .map_err(|_| InstallError::NoConfig(record.path.clone()))?;
    // Dispatch on the RECORDED AGENT, not on the file name. The first draft
    // branched on `path.ends_with("config.toml")` and treated everything else
    // as aider's flat YAML — which was true while there were two recipes and
    // silently wrong the moment a third arrived, because an opencode JSON file
    // would have been "reverted" by a line-oriented YAML rewriter.
    let site = crate::llm_proxy::agent_matrix::by_name(&record.agent)
        .and_then(recipe)
        .or_else(|| legacy_site(&record.agent))
        .ok_or(InstallError::NoRecipeMeasured("recorded agent"))?;
    let next = match site {
        Site::CodexToml => revert_codex(&text, record)?,
        Site::AiderYaml => revert_aider(&text, record),
        Site::HermesYaml => revert_hermes(&text, record),
        Site::OpenCodeJson { .. } => revert_opencode(&text, record)?,
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        } if record.previous_selector.as_deref() == Some(QWEN_DOTENV_SELECTOR) => {
            revert_dotenv(&text, record, "OPENAI_BASE_URL")
        }
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Qwen,
        } if record
            .previous_selector
            .as_deref()
            .is_some_and(|s| s.starts_with(QWEN_MODEL_SELECTOR)) =>
        {
            let id = &record.previous_selector.as_deref().unwrap_or_default()
                [QWEN_MODEL_SELECTOR.len()..];
            revert_qwen_model(&text, record, id)?
        }
        Site::SettingsEnvJson { flavour } => revert_env_key(&text, record, flavour.key())?,
        Site::ShellProfile { var } => revert_shell_profile(&text, record, &record.path, var),
        Site::GrokToml => revert_grok(&text, record)?,
        Site::ReasonixToml => revert_reasonix(&text, record)?,
        Site::ClineProvidersJson => revert_cline(&text, record)?,
        Site::GeminiEnvFile => revert_dotenv(&text, record, GEMINI_ENDPOINT_KEY),
        Site::FactoryConfigJson => revert_factory(&text, record)?,
        Site::PiModelsStore => revert_pi(&text, record)?,
        Site::VsCodeSettingsJson => revert_flat_key(&text, record, DEVIN_INFERENCE_KEY)?,
        Site::VsCodeCopilotSettings => revert_flat_key(&text, record, COPILOT_CAPI_KEY)?,
    };
    // A file we created, now empty again, is ours to take away. Leaving an
    // empty `.env` behind is not a byte-for-byte uninstall, and the bench
    // measures exactly that.
    if record.created_file && next.trim().is_empty() {
        crate::diagnostics_cli::backup_file(&record.path);
        return std::fs::remove_file(&record.path).map_err(|e| InstallError::Io(e.to_string()));
    }
    crate::diagnostics_cli::backup_file(&record.path);
    tokenade_core::paths::write_atomic(&record.path, &next)
        .map_err(|e| InstallError::Io(e.to_string()))
}

pub(super) fn revert_codex(text: &str, record: &Record) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    match &record.previous_selector {
        Some(prev) => doc["model_provider"] = toml_edit::value(prev.as_str()),
        // There was no selector before us, so leaving one behind would be a
        // change we made and did not undo.
        None => {
            doc.remove("model_provider");
        }
    }
    if let Some(t) = doc
        .get_mut("model_providers")
        .and_then(|p| p.as_table_mut())
    {
        t.remove(PROVIDER_ID);
        // An empty table we created is ours to remove too.
        if t.is_empty() {
            doc.remove("model_providers");
        }
    }
    Ok(doc.to_string())
}

pub(super) fn revert_aider(text: &str, record: &Record) -> String {
    // Same rule as the opencode twin: restore only what the agent HAD. Our
    // write APPENDS the key when the file carried none, so putting the vendor
    // default back would leave an assumption behind under the user's name.
    if !record.had_explicit_upstream {
        let mut s = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("openai-api-base:"))
            .collect::<Vec<_>>()
            .join("\n");
        s.push('\n');
        return s;
    }
    let keep = record.upstream.clone();
    text.lines()
        .map(|l| {
            if l.trim_start().starts_with("openai-api-base:") {
                format!("openai-api-base: {keep}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Put an opencode-schema config back.
///
/// The base URL is restored only if the agent HAD one; if it did not, the key
/// is removed rather than pinned to the vendor default we assumed. Writing our
/// assumption back as if it were the user's setting is how an install quietly
/// becomes a configuration change.
pub(super) fn revert_opencode(text: &str, record: &Record) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    let provider = record
        .previous_selector
        .clone()
        .unwrap_or_else(|| OPENCODE_DEFAULT_PROVIDER.to_string());
    let had_one = record.had_explicit_upstream;
    let Some(opts) = v
        .get_mut("provider")
        .and_then(|p| p.get_mut(&provider))
        .and_then(|p| p.get_mut("options"))
        .and_then(|o| o.as_object_mut())
    else {
        return serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()));
    };
    if had_one {
        opts.insert(
            "baseURL".to_string(),
            serde_json::Value::String(record.upstream.clone()),
        );
    } else {
        opts.remove("baseURL");
    }
    // Clean up the scaffolding we created and nothing else.
    let empty_opts = opts.is_empty();
    if empty_opts {
        if let Some(p) = v.get_mut("provider").and_then(|p| p.get_mut(&provider)) {
            if let Some(o) = p.as_object_mut() {
                o.remove("options");
                if o.is_empty() {
                    if let Some(all) = v.get_mut("provider").and_then(|p| p.as_object_mut()) {
                        all.remove(&provider);
                        if all.is_empty() {
                            if let Some(root) = v.as_object_mut() {
                                root.remove("provider");
                            }
                        }
                    }
                }
            }
        }
    }
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}
