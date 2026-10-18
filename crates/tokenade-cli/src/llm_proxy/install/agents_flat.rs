//! **The agents whose configuration is a LINE.**
//!
//! One key per line, a `.env`, a flat entry. The form is simpler than a
//! document's and goes wrong differently: there is no tree to get lost in,
//! but nothing distinguishes our line from the user's either unless we mark
//! it.

use super::*;

/// Read one TOP-LEVEL key out of a JSON settings file.
///
/// Top-level and literal, dots included: that is how VS Code stores
/// `"codeium.inferenceApiServerUrl"`, and walking the dots would look for a
/// `codeium` object that is not there.
pub(super) fn read_flat_key(
    text: &str,
    path: &Path,
    key: &str,
) -> Result<Option<String>, InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    Ok(v.get(key)
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .filter(|u| !u.is_empty()))
}

pub(super) fn write_flat_key(
    text: &str,
    path: &Path,
    key: &str,
    listen_url: &str,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    v[key] = serde_json::Value::String(listen_url.to_string());
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

pub(super) fn revert_flat_key(
    text: &str,
    record: &Record,
    key: &str,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| InstallError::Unparseable(record.path.to_path_buf()))?;
    if record.had_explicit_upstream {
        v[key] = serde_json::Value::String(record.upstream.clone());
    } else if let Some(o) = v.as_object_mut() {
        // The key was ours. Removing it beats writing the vendor default back
        // under the user's name — the same rule the opencode recipe states.
        o.remove(key);
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

/// The one key gemini dials its service through.
pub(super) const GEMINI_ENDPOINT_KEY: &str = "CODE_ASSIST_ENDPOINT";

/// Read `KEY=value` out of a dotenv file.
///
/// Deliberately tiny: a dotenv file this product WRITES holds one key, and the
/// dialects that differ (`export `, multi-line quoting, interpolation) differ
/// in ways a base URL never uses. Surrounding quotes are stripped because both
/// forms are common in a file a human edited.
pub(super) fn read_dotenv(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("export ").unwrap_or(l).split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn write_dotenv(text: &str, key: &str, listen_url: &str) -> String {
    let line = format!("{key}={listen_url}");
    let mut replaced = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let bare = l.trim_start();
            let bare = bare.strip_prefix("export ").unwrap_or(bare);
            if !bare.starts_with('#') && bare.split_once('=').is_some_and(|(k, _)| k.trim() == key)
            {
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
    out.retain(|l| !l.is_empty());
    let mut s = out.join("\n");
    s.push('\n');
    s
}

pub(super) fn revert_dotenv(text: &str, record: &Record, key: &str) -> String {
    if record.had_explicit_upstream {
        return write_dotenv(text, key, &record.upstream);
    }
    let mut out: Vec<String> = text
        .lines()
        .filter(|l| {
            let bare = l.trim_start();
            let bare = bare.strip_prefix("export ").unwrap_or(bare);
            bare.starts_with('#') || !bare.split_once('=').is_some_and(|(k, _)| k.trim() == key)
        })
        .map(str::to_string)
        .collect();
    if out.iter().all(|l| l.trim().is_empty()) {
        return String::new();
    }
    out.retain(|l| !l.is_empty());
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Current `(base_url, model_display_name)` from droid's config.
///
/// The FIRST custom model carrying a `base_url` is the one redirected, and its
/// display name is the selector so uninstall puts the URL back on the same
/// entry even if the list has been reordered since.
pub(super) fn read_factory(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    let entry = v
        .get("custom_models")
        .and_then(|m| m.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("base_url").is_some_and(|b| b.is_string()))
        });
    let base = entry
        .and_then(|e| e.get("base_url"))
        .and_then(|b| b.as_str())
        .map(str::to_string);
    let name = entry
        .and_then(|e| e.get("model_display_name").or_else(|| e.get("model")))
        .and_then(|n| n.as_str())
        .map(str::to_string);
    Ok((base, name))
}

pub(super) fn factory_entry_name(e: &serde_json::Value) -> Option<&str> {
    e.get("model_display_name")
        .or_else(|| e.get("model"))
        .and_then(|n| n.as_str())
}

pub(super) fn write_factory(
    text: &str,
    listen_url: &str,
    selector: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    let Some(models) = v.get_mut("custom_models").and_then(|m| m.as_array_mut()) else {
        return Err(InstallError::Unparseable(path.to_path_buf()));
    };
    for e in models.iter_mut() {
        if factory_entry_name(e) == Some(selector) {
            e["base_url"] = serde_json::Value::String(listen_url.to_string());
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

pub(super) fn revert_factory(text: &str, record: &Record) -> Result<String, InstallError> {
    let selector = record.previous_selector.clone().unwrap_or_default();
    let mut v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| InstallError::Unparseable(record.path.to_path_buf()))?;
    if let Some(models) = v.get_mut("custom_models").and_then(|m| m.as_array_mut()) {
        for e in models.iter_mut() {
            if factory_entry_name(e) == Some(selector.as_str()) {
                e["base_url"] = serde_json::Value::String(record.upstream.clone());
            }
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

/// Current `(base_url, provider)` from pi's model store.
///
/// The provider is read from `settings.json` NEXT TO the store, because that
/// is where pi keeps the one it dials by default. Falling back to the store's
/// first key would pick a provider the user does not use, and the install
/// would succeed while changing nothing they can see.
pub(super) fn read_pi(
    text: &str,
    path: &Path,
) -> Result<(Option<String>, Option<String>), InstallError> {
    let v: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?
    };
    let provider = path
        .parent()
        .map(|d| d.join("settings.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|s| {
            s.get("defaultProvider")
                .and_then(|p| p.as_str())
                .map(str::to_string)
        });
    let base = provider.as_deref().and_then(|id| {
        v.get(id)?
            .get("models")?
            .as_array()?
            .iter()
            .find_map(|m| m.get("baseUrl")?.as_str().map(str::to_string))
    });
    Ok((base, provider))
}

pub(super) fn write_pi(
    text: &str,
    listen_url: &str,
    provider: &str,
    path: &Path,
) -> Result<String, InstallError> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| InstallError::Unparseable(path.to_path_buf()))?;
    set_pi_base(&mut v, provider, listen_url);
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

pub(super) fn revert_pi(text: &str, record: &Record) -> Result<String, InstallError> {
    let provider = record.previous_selector.clone().unwrap_or_default();
    let mut v: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| InstallError::Unparseable(record.path.to_path_buf()))?;
    set_pi_base(&mut v, &provider, &record.upstream);
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))
}

/// Every model under one provider, because pi cycles models within a provider.
pub(super) fn set_pi_base(v: &mut serde_json::Value, provider: &str, url: &str) {
    let Some(models) = v
        .get_mut(provider)
        .and_then(|p| p.get_mut("models"))
        .and_then(|m| m.as_array_mut())
    else {
        return;
    };
    for m in models.iter_mut() {
        if m.get("baseUrl").is_some() {
            m["baseUrl"] = serde_json::Value::String(url.to_string());
        }
    }
}
