//! **Read and write ANOTHER program's file.**
//!
//! An agent configuration is not our file: it carries
//! comments that `serde_json` rejects (JSONC), sometimes a byte order
//! mark at the start, and it may be open by the agent at the moment we
//! rewrite it. Everything that makes those three facts bearable is here — and nothing
//! here knows what the configuration CONTAINS.

use super::*;

/// Parse an agent's JSON config, tolerating a leading UTF-8 BOM.
///
/// PowerShell's `Set-Content` wrote UTF-8 WITH a BOM by default until PS 6,
/// and `Out-File -Encoding utf8` still does — so a Windows user who edited
/// their `mcp.json` from a shell hands us a file whose first three bytes make
/// strict serde reject an otherwise perfectly valid document.
///
/// The three call sites failed three different ways, and the quiet ones are
/// the worse ones: `read_config_root` reported `parse: expected value at line
/// 1 column 1`, an error naming nothing the user can act on; `purge_json_config`
/// refused; and `inspect_wrapped_entries` answered `continue`, so the config
/// simply was not there as far as every caller could tell. `json_util`
/// already stripped the BOM on the install side and said why — this side did
/// not, so the same file parsed for one half of the product and not the other.
///
/// Mined from `graphify`'s `fix(windows): stop a PowerShell BOM breaking hook
/// rebuilds`, which is the same failure one directory over.
pub(super) fn parse_json_tolerant(content: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(content.trim_start_matches('\u{feff}'))
}

/// Returns the absolute path of the currently-running tokenade binary
/// to embed in MCP wrap entries. Falls back to bare "tokenade" only
/// if the current_exe lookup fails (rare; tests pin it via the const).
///
/// Why absolute: the original install MCP entries use the absolute
/// path from env::current_exe(). When the wrapper rewrote entries
/// with the bare "tokenade", agents that don't inherit the user's
/// PATH (GUI clients, sandboxed runtimes) failed to spawn the
/// wrapper. The new value matches the install asymmetry: same
/// absolute path everywhere.
/// Does this failed-to-parse content look like JSONC rather than corruption?
///
/// VS Code's `mcp.json` — which is on [`config_paths`] — is JSONC by design:
/// comments and trailing commas are legal there and the editor never complains.
/// `serde_json` rejects all three forms (measured 2026-08-26: line comment,
/// block comment and trailing comma all fail), so a perfectly ordinary VS Code
/// config produced `parse: expected value at line 2 column 3` and nothing else.
/// A user looking at a file their editor accepts happily reads that as tokenade
/// being broken.
///
/// tokenade REFUSES such a file on purpose and that does not change: it rewrites
/// JSON structurally, so re-serialising would silently delete every comment the
/// user wrote. Naming the cause is the whole fix.
///
/// Scans OUTSIDE strings, because `"https://example.com"` carries `//` and is
/// not a comment — the reason this cannot be a `contains`.
pub(super) fn looks_like_jsonc(content: &str) -> Option<&'static str> {
    let b = content.as_bytes();
    let (mut i, mut in_str, mut esc) = (0usize, false, false);
    let mut pending_comma: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                pending_comma = None;
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => return Some("a `//` comment"),
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => return Some("a `/* */` comment"),
            b',' => pending_comma = Some(i),
            b'}' | b']' if pending_comma.is_some() => return Some("a trailing comma"),
            _ if !c.is_ascii_whitespace() => pending_comma = None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Read a config file and parse it as JSON, or say why not.
///
/// Four call sites carried these twelve lines verbatim. Only the parse-error
/// wording varied — one config appends "left untouched" because it is a file
/// we refuse to rewrite — so that suffix is the parameter and nothing else is.
pub(super) fn read_config_root(path: &std::path::Path, parse_note: &str) -> Result<Value, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    // An EMPTY file is a config with no servers, not a broken one. Agents
    // create these placeholders on first run and never write to them:
    // `~/.gemini/config/mcp_config.json` was 0 bytes and dated two months back
    // on the machine this was found on, and every `tokenade install` printed
    // `parse: EOF while parsing a value at line 1 column 0` about it.
    //
    // The cost is not the line, it is what the line teaches. A permanent error
    // nobody can act on is one the reader learns to skip, and the next one —
    // about a config that really IS corrupt, on the same surface, in the same
    // shape — gets skipped with it.
    //
    // Reading it as `{}` is also the SAFE verdict, not just the quiet one:
    // an empty object carries no server to wrap, so `needs_write` stays false
    // and the placeholder is never written to.
    if content.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    parse_json_tolerant(&content).map_err(|e| match looks_like_jsonc(&content) {
        Some(what) => format!(
            "parse: this file contains {what}, which makes it JSONC rather than \
             JSON. Editors such as VS Code accept that; tokenade will not rewrite \
             it, because it writes JSON back structurally and every comment would \
             be lost. Remove the comments and trailing commas, or wrap the servers \
             by hand{parse_note}"
        ),
        None => format!("parse: {e}{parse_note}"),
    })
}

/// TOML escape for a bare double-quoted string.
pub(super) fn toml_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            _ => o.push(c),
        }
    }
    o.push('"');
    o
}

pub(super) fn toml_array(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| toml_str(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// Section header `[mcp_servers.<name>]` → `Some(name)`. Sub-tables
/// (`[mcp_servers.x.env]`) return None: they are body, not a server.
pub(super) fn toml_server_header(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix("[mcp_servers.")?.strip_suffix(']')?;
    if inner.contains('.') || inner.is_empty() {
        return None;
    }
    Some(inner.trim_matches('"').to_string())
}

/// Wrap / unwrap `[mcp_servers.*]` entries in a TOML agent config
/// (Codex, Grok). Line-surgical for the same reason `purge_grok_toml` is:
/// the workspace `toml` crate is parse-only, and a user's comments and
/// key order are theirs to keep. Parsing is still done by the crate — we
/// read the structure, then rewrite only the lines we own.
///
/// Anything whose shape we do not recognise (multi-line arrays, a
/// `command` that is not a plain string) is reported as
/// Split a leading UTF-8 BOM off the content, keeping it to re-attach.
///
/// The JSON side got [`parse_json_tolerant`] and the TOML side did not, and the
/// TOML failure is the WORSE of the two because it is silent. `toml::from_str`
/// accepts a BOM, so the parse succeeds and `mcp_servers` is found — but both
/// TOML transformers then do a LINE-BASED rewrite, and the first line reads
/// `\u{feff}[mcp_servers.demo]`, which matches no `[` header. Measured
/// 2026-08-26 on the same config one BOM apart: `nobom_wrapped=true`,
/// `bom_wrapped=false`. `mcp-wrap install` reported nothing wrapped, wrote
/// nothing, and raised no error, so a Codex or Grok user on Windows kept
/// paying full price for every MCP result with the tool saying it had run.
///
/// The BOM is RE-ATTACHED to the output rather than dropped: the file belongs
/// to the agent, and silently changing bytes we were not asked to change is the
/// habit this whole module exists to avoid. graphify chose to rewrite the
/// marker BOM-less; it owns that file and we do not.
pub(super) fn split_bom(content: &str) -> (&'static str, &str) {
    match content.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", content),
    }
}

/// Write `content` to `path` under an exclusive file lock. Prevents
/// torn writes between `tokenade install` and a `tokenade-mcp`
/// session rescanning at the same time.
///
/// Atomicity fix. Pre-fix this opened the canonical path
/// with `truncate(true)`, then `write_all`. A process death between
/// truncate and the final flush left the file EMPTY — losing all
/// of the user's MCP entries (not just tokenade-managed ones), since
/// these configs (`~/.claude.json`, `~/.codex/config.json`, etc.)
/// are user-owned data tokenade is merely upserting into.
///
/// New shape: lock the canonical path (creates if missing) for the
/// cross-process serialization, write the new content to a sibling
/// `<path>.tokenade.tmp`, fsync it, then atomically rename tmp →
/// canonical. The rename replaces the inode the lock holder is on,
/// which is fine since we drop the lock right after. Process death
/// mid-write now leaves either the prior valid bytes OR the new
/// valid bytes, never an empty file.
pub(super) fn write_locked(path: &Path, content: &str) -> std::io::Result<()> {
    use fs2::FileExt;
    use std::fs::OpenOptions;
    use std::io::Write;
    // Resolve symlinks BEFORE choosing where to rename. A dotfiles manager
    // (chezmoi, stow) publishes `~/.gemini/settings.json` as a link into its
    // own tree; renaming a tmp file onto the link REPLACES the link, so the
    // agent starts reading a regular file outside the user's dotfiles and
    // their managed copy is silently orphaned. MEASURED 2026-08-07 with
    // symlinked Codex and Gemini configs: both came back as regular files,
    // and the Gemini wrap never reached the dotfiles source at all.
    // `atomic_io::write_json_atomic` canonicalizes for exactly this reason
    // and says so; this writer did not.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Keep the user's own bytes before touching them, and give them back when
    // this write changes nothing they can see. One rule, one place.
    crate::paths::backup_once(path);
    let restored = crate::paths::pristine_if_equivalent(path, content);
    let content = restored.as_deref().unwrap_or(content);
    // Lock-only handle on the canonical path — creates if missing so
    // first-write still gets the cross-process serialization. NO
    // truncate: we never wipe the canonical until the final rename.
    let lock_handle = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let _ = lock_handle.lock_exclusive();
    // Write new content to a sibling tmp + fsync + atomic rename.
    let tmp = path.with_extension("tokenade.tmp");
    {
        let mut tmp_f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        tmp_f.write_all(content.as_bytes())?;
        tmp_f.sync_all().ok();
    }
    // Carry the destination's mode across the rename. Same rule, same reason
    // as `paths::write_atomic`: these are agent config files that can hold an
    // API key, and widening them to the umask default is not something a save
    // is allowed to do. Best-effort — a metadata failure must not turn a
    // successful write into a lost one.
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path)?;
    let _ = FileExt::unlock(&lock_handle);
    Ok(())
}
