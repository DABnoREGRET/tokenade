//! **The agents whose configuration is a DOCUMENT.**
//!
//! JSON, TOML, YAML: a tree that has to be read, modified in a single place,
//! and rewritten without destroying what the user had put there. One agent per
//! `read`/`write`/`revert` trio, because each one files the upstream under a
//! different key.

use super::*;

/// Current `(base_url, selector)` from a Codex config.
pub(super) fn read_codex(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    let selector = doc
        .get("model_provider")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let base = selector.as_deref().and_then(|s| {
        doc.get("model_providers")?
            .get(s)?
            .get("base_url")?
            .as_str()
            .map(str::to_string)
    });
    Ok((base, selector))
}

/// Codex's ChatGPT-account backend: where a user signed in with ChatGPT (no
/// API key — the desktop app's default, and most CLI users) really goes.
pub(super) const CODEX_CHATGPT_UPSTREAM: &str = "https://chatgpt.com/backend-api/codex";

/// Is this Codex signed in with a ChatGPT account rather than an API key?
/// Read off `~/.codex/auth.json` — the mode and the presence of tokens only,
/// never their values. With such an account a provider that names
/// `env_key = "OPENAI_API_KEY"` has no key to send: the install broke Codex.
pub(super) fn codex_uses_chatgpt_account(home: &Path) -> bool {
    let Ok(t) = std::fs::read_to_string(home.join(".codex").join("auth.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
        return false;
    };
    let key = v
        .get("OPENAI_API_KEY")
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.is_empty());
    v.get("auth_mode").and_then(|m| m.as_str()) == Some("chatgpt")
        || (!key && v.get("tokens").is_some_and(|t| !t.is_null()))
}

/// Current `[endpoints] models_base_url` from a grok config.
pub(super) fn read_grok(text: &str, path: &Path) -> Result<Option<String>, InstallError> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    Ok(doc
        .get("endpoints")
        .and_then(|e| e.get("models_base_url"))
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Point grok at the proxy, leaving every other key exactly as it was.
pub(super) fn write_grok(
    text: &str,
    listen_url: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    // `doc["endpoints"]["x"] = …` on an absent key creates an INLINE table
    // (`endpoints = { models_base_url = "…" }`). Both parse the same, but the
    // revert reaches the block through `as_table_mut`, which does not see an
    // inline one — so the table would be created and never removed. Caught by
    // `grok_gains_an_endpoints_table_and_loses_it_again`, which is why it
    // asserts the removal rather than only the write.
    let endpoints = doc
        .entry("endpoints")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    match endpoints {
        toml_edit::Item::Table(t) => {
            t["models_base_url"] = toml_edit::value(listen_url);
        }
        // A user who wrote their own inline table keeps it inline: rewriting
        // their formatting is not this tool's business.
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => {
            t.insert("models_base_url", listen_url.into());
        }
        _ => return Err(InstallError::Unparseable(path.to_path_buf())),
    }
    Ok(doc.to_string())
}

/// Put a grok config back.
pub(super) fn revert_grok(text: &str, record: &Record) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    if record.had_explicit_upstream {
        doc["endpoints"]["models_base_url"] = toml_edit::value(record.upstream.as_str());
        return Ok(doc.to_string());
    }
    // Both table shapes, for the same reason `write_grok` handles both: a user
    // may have written `endpoints = { … }` themselves, and reaching only for
    // `as_table_mut` would silently leave our key in place.
    let emptied = match doc.get_mut("endpoints") {
        Some(toml_edit::Item::Table(t)) => {
            t.remove("models_base_url");
            t.is_empty()
        }
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => {
            t.remove("models_base_url");
            t.is_empty()
        }
        _ => false,
    };
    if emptied {
        doc.remove("endpoints");
    }
    Ok(doc.to_string())
}

/// The reasonix provider currently selected, and its base URL.
///
/// `default_model` names a provider by its `name` field; the entries live in a
/// `[[providers]]` array, so the selector cannot be a table key the way it is
/// for cline. Falls back to the first entry when nothing selects, because a
/// config with one provider and no `default_model` is what `reasonix setup`
/// leaves behind and refusing it would be refusing the common case.
pub(super) fn read_reasonix(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    let wanted = doc.get("default_model").and_then(|v| v.as_str());
    // A config.toml WITHOUT `[[providers]]` reads perfectly well: it simply
    // has no provider yet, which happens before the first `reasonix setup`.
    // Making it `Unparseable` accused the file of being unreadable and sent the
    // operator looking for a syntax error that does not exist.
    let Some(providers) = doc.get("providers").and_then(|p| p.as_array_of_tables()) else {
        return Err(InstallError::NoConfig(path.to_path_buf()));
    };
    let chosen = providers
        .iter()
        .find(|t| match (wanted, t.get("name").and_then(|v| v.as_str())) {
            (Some(w), Some(n)) => w == n,
            _ => false,
        })
        .or_else(|| providers.iter().next());
    let Some(t) = chosen else {
        return Ok((None, None));
    };
    Ok((
        t.get("base_url")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        t.get("name").and_then(|v| v.as_str()).map(str::to_string),
    ))
}

/// Point one reasonix provider at the proxy, leaving the others alone.
pub(super) fn write_reasonix(
    text: &str,
    listen_url: &str,
    provider: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    let providers = doc
        .get_mut("providers")
        .and_then(|p| p.as_array_of_tables_mut())
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?;
    let t = providers
        .iter_mut()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(provider))
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?;
    t["base_url"] = toml_edit::value(listen_url);
    Ok(doc.to_string())
}

/// Put a reasonix config back.
pub(super) fn revert_reasonix(text: &str, record: &Record) -> Result<String, InstallError> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    let Some(provider) = record.previous_selector.clone() else {
        return Ok(doc.to_string());
    };
    let Some(providers) = doc
        .get_mut("providers")
        .and_then(|p| p.as_array_of_tables_mut())
    else {
        return Ok(doc.to_string());
    };
    if let Some(t) = providers
        .iter_mut()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(provider.as_str()))
    {
        if record.had_explicit_upstream {
            t["base_url"] = toml_edit::value(record.upstream.as_str());
        } else {
            t.remove("base_url");
        }
    }
    Ok(doc.to_string())
}

/// The provider a cline config is currently using, and its base URL.
pub(super) fn read_cline(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    let id = v
        .get("lastUsedProvider")
        .and_then(|p| p.as_str())
        .map(str::to_string);
    let base = id.as_deref().and_then(|id| {
        v.get("providers")?
            .get(id)?
            .get("settings")?
            .get("baseUrl")?
            .as_str()
            .map(str::to_string)
    });
    Ok((base, id))
}

/// Point cline at the proxy, under the provider it is actually using.
pub(super) fn write_cline(
    text: &str,
    listen_url: &str,
    provider: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    v.get_mut("providers")
        .and_then(|p| p.get_mut(provider))
        .and_then(|p| p.get_mut("settings"))
        .and_then(|s| s.as_object_mut())
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .insert(
            "baseUrl".to_string(),
            serde_json::Value::String(listen_url.to_string()),
        );
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}

/// Put a cline config back.
///
/// Only ever restores; never removes. `plan` refuses an install when the
/// provider had no base URL, so a record for cline always carries one the user
/// wrote themselves.
pub(super) fn revert_cline(text: &str, record: &Record) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    let Some(provider) = record.previous_selector.clone() else {
        return serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()));
    };
    if let Some(s) = v
        .get_mut("providers")
        .and_then(|p| p.get_mut(&provider))
        .and_then(|p| p.get_mut("settings"))
        .and_then(|s| s.as_object_mut())
    {
        if record.had_explicit_upstream {
            s.insert(
                "baseUrl".to_string(),
                serde_json::Value::String(record.upstream.clone()),
            );
        } else {
            s.remove("baseUrl");
        }
    }
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}

/// Read one key out of a settings file's `env` block.
pub(super) fn read_env_key(
    text: &str,
    path: &Path,
    key: &str,
) -> Result<Option<String>, InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    Ok(v.get("env")
        .and_then(|e| e.get(key))
        .and_then(|b| b.as_str())
        .map(str::to_string))
}

/// Set one key in a settings file's `env` block, leaving the rest alone.
pub(super) fn write_env_key(
    text: &str,
    listen_url: &str,
    key: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    v.as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .insert(
            key.to_string(),
            serde_json::Value::String(listen_url.to_string()),
        );
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}

/// Qwen's selected model, when a `modelProviders` entry gives it its own
/// `baseUrl`: (model id, that base URL). Qwen resolves the base URL in layers
/// and this one comes FIRST — before `OPENAI_BASE_URL`, read in its own
/// `modelConfigResolver.ts` (`modelProvider.baseUrl`, then `--openaiBaseUrl`,
/// then the env, then `security.auth.baseUrl`). An install that wrote only the
/// env key was never on the path for such a model, and recorded the vendor
/// default as upstream — DashScope, for a user on OpenRouter.
///
/// Only the current shape, `modelProviders.<auth>: [ { id, baseUrl, … } ]`.
/// The legacy `{ protocol, models }` object is SKIPPED by Qwen 0.25 ("is not
/// an array; skipping", its own log on this machine) — reading it would put
/// the proxy where Qwen never looks.
pub(super) fn qwen_model_route(v: &serde_json::Value) -> Option<(String, String)> {
    let id = v.pointer("/model/name")?.as_str()?;
    v.get("modelProviders")?
        .as_object()?
        .values()
        .find_map(|p| {
            p.as_array()?.iter().find_map(|m| {
                let base = m.get("baseUrl")?.as_str()?;
                (m.get("id")?.as_str()? == id).then(|| (id.to_string(), base.to_string()))
            })
        })
}

/// Qwen loads `~/.qwen/.env` BEFORE `settings.env`, and `settings.env` only
/// fills keys still unset (its `loadEnvironment`): a base URL in `.env` makes
/// the settings key dead. The install then goes into `.env`.
pub(super) const QWEN_DOTENV_SELECTOR: &str = "dotenv";

pub(super) const QWEN_MODEL_SELECTOR: &str = "model:";

/// Where Qwen's base URL lives for this config, and what it is now:
/// (upstream, selector). `Some("model:<id>")` when the selected model routes
/// through its own provider entry.
pub(super) fn read_qwen(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    if let Some((id, base)) = qwen_model_route(&v) {
        return Ok((Some(base), Some(format!("{QWEN_MODEL_SELECTOR}{id}"))));
    }
    Ok((read_env_key(text, path, "OPENAI_BASE_URL")?, None))
}

/// Point the selected model's provider entry at the proxy.
pub(super) fn write_qwen_model(
    text: &str,
    listen_url: &str,
    id: &str,
    path: &Path,
) -> Result<String, InstallError> {
    set_qwen_model_base(text, id, listen_url)
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))
}

/// Put the model entry's `baseUrl` back.
pub(super) fn revert_qwen_model(
    text: &str,
    record: &Record,
    id: &str,
) -> Result<String, InstallError> {
    set_qwen_model_base(text, id, &record.upstream)
        .ok_or_else(|| InstallError::Unparseable(record.path.clone()))
}

fn set_qwen_model_base(text: &str, id: &str, base: &str) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let models = v
        .get_mut("modelProviders")?
        .as_object_mut()?
        .values_mut()
        .find_map(|p| {
            let ms = p.as_array_mut()?;
            ms.iter()
                .any(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))
                .then_some(ms)
        })?;
    let m = models
        .iter_mut()
        .find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))?;
    m.as_object_mut()?.insert(
        "baseUrl".into(),
        serde_json::Value::String(base.to_string()),
    );
    serde_json::to_string_pretty(&v).ok()
}

/// Put a settings file's `env` block back.
pub(super) fn revert_env_key(
    text: &str,
    record: &Record,
    key: &str,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(record.path.clone()))?;
    let Some(env) = v.get_mut("env").and_then(|e| e.as_object_mut()) else {
        return serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()));
    };
    if record.had_explicit_upstream {
        env.insert(
            key.to_string(),
            serde_json::Value::String(record.upstream.clone()),
        );
    } else {
        env.remove(key);
    }
    // An `env` block we created and then emptied is ours to remove; one the
    // user already had, with other keys in it, is not.
    if env.is_empty() {
        if let Some(root) = v.as_object_mut() {
            root.remove("env");
        }
    }
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}

/// The provider an opencode config falls back to when its `model` says nothing.
pub(super) const OPENCODE_DEFAULT_PROVIDER: &str = "openai";

/// Current `(base_url, provider id)` from an opencode-schema config.
///
/// The provider id is the half of `model` before the slash. It matters because
/// the base URL lives UNDER it: writing to the wrong provider entry leaves the
/// agent talking to its vendor while a `tokenade` entry sits unused nearby, and
/// nothing would look wrong.
pub(super) fn read_opencode(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    let id = v
        .get("model")
        .and_then(|m| m.as_str())
        .and_then(|m| m.split_once('/'))
        .map(|(p, _)| p.to_string())
        .unwrap_or_else(|| OPENCODE_DEFAULT_PROVIDER.to_string());
    let base = v
        .get("provider")
        .and_then(|p| p.get(&id))
        .and_then(|p| p.get("options"))
        .and_then(|o| o.get("baseURL"))
        .and_then(|b| b.as_str())
        .map(str::to_string);
    Ok((base, Some(id)))
}

/// Point an opencode-schema config at the proxy, under the provider it uses.
pub(super) fn write_opencode(
    text: &str,
    listen_url: &str,
    provider: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    let root = v
        .as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?;
    root.entry("provider")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .entry(provider)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .entry("options")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| InstallError::Unparseable(path.to_path_buf()))?
        .insert(
            "baseURL".to_string(),
            serde_json::Value::String(listen_url.to_string()),
        );
    serde_json::to_string_pretty(&v).map_err(|e| InstallError::Io(e.to_string()))
}

/// Current `openai-api-base` from a flat aider config.
pub(super) fn read_aider(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix("openai-api-base:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

/// `model.base_url` in hermes' YAML, scoped to the `model:` block.
///
/// "Scoped" is the whole difference with the aider twin: `base_url` is a
/// plausible key name in more than one section, and a file-wide search would
/// happily rewrite somebody else's. The block ends at the next line that
/// starts in column zero.
pub(super) fn hermes_model_block(text: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim_end() == "model:")?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| !l.is_empty() && !l.starts_with([' ', '\t']))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    Some((start, end))
}

pub(super) fn read_hermes(text: &str) -> Option<String> {
    let (start, end) = hermes_model_block(text)?;
    text.lines()
        .skip(start + 1)
        .take(end - start - 1)
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix("base_url:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn write_hermes(text: &str, listen_url: &str) -> String {
    let Some((start, end)) = hermes_model_block(text) else {
        // No `model:` block at all. Create one rather than return the text
        // unchanged: measured 2026-08-19, the tokenade plugin install writes
        // a config.yaml carrying only a `plugins:` block, so on a machine
        // where the bridge landed first this branch is the COMMON one, and
        // returning unchanged made the install report success while writing
        // nothing.
        let mut s = text.trim_end().to_string();
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(&format!("model:\n  base_url: {listen_url}\n"));
        return s;
    };
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    let mut replaced = false;
    for l in out.iter_mut().take(end).skip(start + 1) {
        if l.trim_start().starts_with("base_url:") && !l.trim_start().starts_with('#') {
            *l = format!("  base_url: {listen_url}");
            replaced = true;
        }
    }
    if !replaced {
        out.insert(start + 1, format!("  base_url: {listen_url}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

pub(super) fn revert_hermes(text: &str, record: &Record) -> String {
    if record.had_explicit_upstream {
        return write_hermes(text, &record.upstream);
    }
    // Nothing was there before, so take the line back out rather than leave
    // the vendor default written down as if the user had chosen it.
    let Some((start, end)) = hermes_model_block(text) else {
        return text.to_string();
    };
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    let cut = out
        .iter()
        .enumerate()
        .take(end)
        .skip(start + 1)
        .find(|(_, l)| l.trim_start().starts_with("base_url:"))
        .map(|(i, _)| i);
    if let Some(i) = cut {
        out.remove(i);
        // A `model:` block we created ourselves, now emptied, is ours to take
        // back too. Leaving a bare `model:` behind hands hermes a null where
        // it had no key at all. Same rule the codex recipe applies to an
        // emptied `[model_providers]` table.
        let block_empty = out
            .iter()
            .skip(start + 1)
            .take_while(|l| l.is_empty() || l.starts_with([' ', '\t']))
            .all(|l| l.trim().is_empty());
        if block_empty {
            out.remove(start);
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// The VS Code setting Devin Desktop's language server is launched with.
pub(super) const DEVIN_INFERENCE_KEY: &str = "codeium.inferenceApiServerUrl";

/// The one key Copilot's VS Code half dials its model API through.
///
/// ONE key, not the two the extensions between them read, because a `Record`
/// carries one previous value and one flag — two keys would need two records
/// and the uninstall could only put one back. So the choice is which single
/// key routes the most, and it is this one: the built-in `copilot-chat` 0.62.0
/// in VS Code 1.134 names it `DebugOverrideCapiUrl` and names
/// `advanced.debug.overrideCapiUrl` `DebugOverrideCapiUrlLegacy` beside it.
/// The legacy name is what the marketplace `GitHub.copilot` 1.388.0 declares
/// for the COMPLETIONS half, which stays on its vendor route until somebody
/// needs it enough to carry a second record.
///
/// It appears in no `package.json`: it is read and undeclared, found by
/// reading the shipped bundle rather than the settings schema.
pub(super) const COPILOT_CAPI_KEY: &str = "github.copilot.internal.capiUrl";
