//! **Removing our block from someone else's shell profile.**
//!
//! The block is delimited by two markers; removing them is not enough, because
//! a guard left empty (`if …; then fi`) stays behind and makes the shell fail.
//! This module removes the block AND whatever its removal emptied, without ever
//! touching a line the user wrote.

use super::*;

/// Per-file outcome of [`uninstall_shell_proxy`].
pub(crate) enum ProxyStrip {
    /// Removed N tokenade proxy lines from an rc file.
    Stripped(usize),
    /// Dry-run: WOULD remove N lines.
    WouldStrip(usize),
    /// Deleted a generated `~/.tokenade/shellproxy.*` script.
    Deleted,
    /// Dry-run: WOULD delete the script.
    WouldDelete,
    Failed(String),
}

/// True for every line the shell-proxy installer of ANY version (v1 included)
/// ever wrote into an rc file. The installer itself is gone; this matcher is
/// what lets the migration still find its leftovers, so the historical shapes
/// must stay listed here. Matching is per-line on tokenade-specific markers so
/// user content around the block survives untouched.
pub(crate) fn shell_proxy_line_is_tokenade(line: &str) -> bool {
    let t = line.trim();
    t == "# tokenade shell proxy"
        || t.contains("tokenade shell-init")
        || t.contains("tokenade shell_init")
        || t.contains(".tokenade/shellproxy")
        || t.contains(".tokenade\\shellproxy")
        || (t.contains("BASH_ENV") && t.contains("tokenade"))
        // v2+ markers and the alias lines a legacy inline install wrote
        // (`alias git='__tokenade_proxy git'`). Both identifiers are
        // tokenade-unique, so this can't strike user content.
        || t.contains("__tokenade_proxy")
        || t.contains("tokenade shell proxy")
        // The v2 fragment's own scaffolding. These carry no tokenade word, so
        // the marker rule above cannot see them, and what survived
        // `tokenade uninstall` was a SHELL BEHAVIOUR CHANGE the user never
        // asked to keep: `shopt -s expand_aliases` alters how aliases expand
        // in non-interactive shells, and it stayed after the product was gone.
        //
        // Matched as EXACT lines, never `contains`: the generator emits them
        // as fixed literals with nothing interpolated (see
        // `install/shell_proxy.rs`), so a byte-identical line IS the one we
        // wrote. A substring rule here could eat a user's own `shopt`.
        || t == "if [ -n \"${BASH_VERSION:-}\" ] || [ -n \"${ZSH_VERSION:-}\" ]; then unset FUNCNEST 2>/dev/null || true; fi"
        || t == "if [ -n \"${BASH_VERSION:-}\" ]; then shopt -s expand_aliases; fi"
        // The alias loop's own variable, orphaned once the loop goes.
        || t == "unset __tc"
}

/// Drop every `if`-guard left with an EMPTY body, returning the cleaned text
/// and the number of lines removed. Comments inside a dropped guard survive
/// (de-indented to the guard's own level) — we repair, we never eat user prose.
///
/// Users wrap the block we wrote in a guard of their own — either an existence
/// test (`if command -v tokenade …; then`) or, far more common, an
/// interactivity test that never mentions tokenade at all:
///
/// ```sh
/// if [ -n "$PS1" ]; then
///     . ~/.tokenade/shellproxy.sh   # ← our per-line strip removes this
/// fi                                 # ← leaving an empty shell = SYNTAX ERROR
/// ```
///
/// An emptied `if …; then` / `fi` makes the WHOLE rc file fail to load, so every
/// login shell sourcing it breaks and a display manager sourcing `~/.profile`
/// can login-loop. Measured on a real `~/.profile` after an uninstall:
/// `sh -n` → "syntax error near unexpected token `fi'".
///
/// Why we drop ANY empty guard, not just ones we can prove we emptied: once our
/// line is gone the shell carries no evidence of tokenade, so victims of an
/// EARLIER release (≤0.9.4, which stripped without this pass) are
/// indistinguishable from user content. An empty guard is unconditionally a
/// syntax error — the file is already broken — so removing it either repairs
/// our own damage or repairs theirs. It can never break something that worked.
///
/// Runs to a FIXED POINT so nested guards collapse outward: the inner guard is
/// emptied first, which in turn empties its parent.
pub(crate) fn drop_emptied_guards(text: &str) -> (String, usize) {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut removed = 0usize;
    loop {
        let Some((start, end)) =
            (0..lines.len()).find_map(|i| empty_guard_end(&lines, i).map(|e| (i, e)))
        else {
            break;
        };
        let mut kept: Vec<&str> = lines[..start].to_vec();
        // Keep the user's comments; drop only the guard's own scaffolding.
        let comments: Vec<&str> = lines[start + 1..end]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .copied()
            .collect();
        removed += end - start + 1 - comments.len();
        kept.extend(comments);
        kept.extend_from_slice(&lines[end + 1..]);
        lines = kept;
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    (out, removed)
}

/// `Some(end)` when `lines[start]` opens a guard whose body holds no COMMAND —
/// inclusive index of its closing line. A body of blanks and/or comments counts
/// as empty: `if …; then` followed only by a comment is still a syntax error.
/// `None` otherwise, so a guard that still runs something is never touched.
pub(crate) fn empty_guard_end(lines: &[&str], start: usize) -> Option<usize> {
    let head = lines[start].trim();
    // A LOOP whose body no longer runs anything, closing on `done`. The v2
    // fragment aliases its commands from inside
    // `for __tc in <~150 names>; do … done`, and the per-line matcher removes
    // only the `alias` inside it — that line is the one carrying a
    // tokenade-unique marker. What survived `tokenade uninstall` was the loop
    // itself, iterating a hundred and fifty command names to execute a
    // comment, in the user's own rc file. The `if` branch below could not see
    // it: it knows `if … fi` and nothing else.
    //
    // Removing the `for` line alone is NOT an option and is why this belongs
    // here rather than in the line matcher: an orphaned `done` is a syntax
    // error, and a broken rc file is the outcome this whole pass exists to
    // avoid. Head and closer go together or neither goes.
    if (head.starts_with("for ") || head.starts_with("while ") || head.starts_with("until "))
        && head.ends_with("do")
    {
        return body_is_empty_until(lines, start, "done");
    }
    if !head.starts_with("if ") {
        return None;
    }
    // POSIX `if …; then` closes on `fi`; fish `if …` closes on `end`. A
    // single-line guard (`if …; then eval …; fi`) ends in its own closer and is
    // matched/removed whole by the line matcher, so it never reaches here.
    let closer = if head.ends_with("then") {
        "fi"
    } else if head.ends_with("end") || head.contains("; ") {
        return None; // self-contained one-liner
    } else {
        "end"
    };
    body_is_empty_until(lines, start, closer)
}

/// `Some(end)` when everything between `start` and the next `closer` is blank
/// or comment — i.e. the block runs no COMMAND. One spelling for `if`/`fi`
/// and `for`/`done` so the two cannot drift apart.
pub(crate) fn body_is_empty_until(lines: &[&str], start: usize, closer: &str) -> Option<usize> {
    let body_empty = |l: &str| {
        let t = l.trim();
        t.is_empty() || t.starts_with('#')
    };
    let mut j = start + 1;
    while j < lines.len() && body_empty(lines[j]) {
        j += 1;
    }
    (j < lines.len() && lines[j].trim() == closer).then_some(j)
}

/// If `lines[start]` opens a LEGACY inlined `__tokenade_proxy` function body
/// (some very old installs pasted the whole function straight into `.bashrc` /
/// `.zshrc` / `config.fish` instead of sourcing `~/.tokenade/shellproxy.*`),
/// return the inclusive index of its closing line. The per-line matcher only
/// catches the `alias`/`function` header lines and would orphan the body
/// (`command "$@"`, `__tk_active=1`, …), so the block must be removed whole.
/// Returns `None` when `start` isn't a block header or the block is
/// unterminated (in which case we leave the file untouched — never guess).
pub(crate) fn proxy_inline_block_end(lines: &[&str], start: usize) -> Option<usize> {
    let head = lines[start].trim_start();
    // POSIX shell function: `__tokenade_proxy() {` / `__tokenade_proxy () {`.
    let is_posix_fn = (head.starts_with("__tokenade_proxy()")
        || head.starts_with("__tokenade_proxy ()"))
        && lines[start].contains('{');
    if is_posix_fn {
        // Body braces are only ever inline (`${__tk_active:-0}`); the closing
        // brace of the function is a line that is exactly `}`.
        return lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, l)| l.trim() == "}")
            .map(|(j, _)| j);
    }
    // fish function: `function __tokenade_proxy`, closed by a depth-balanced
    // `end` (the body nests its own `if … end` blocks).
    if head.starts_with("function __tokenade_proxy") {
        let mut depth = 1usize;
        for (j, l) in lines.iter().enumerate().skip(start + 1) {
            match l.split_whitespace().next().unwrap_or("") {
                "function" | "if" | "for" | "while" | "switch" | "begin" => depth += 1,
                "end" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    None
}

/// Rip out the shell-alias proxy: strip every tokenade shell-proxy line from
/// each rc file ANY install version may have touched, and delete the generated
/// `~/.tokenade/shellproxy.*` scripts. Files without proxy lines are left
/// byte-identical and don't appear in the result.
///
/// Nothing installs the alias proxy any more (PO decision 2026-07), so this is
/// a pure migration — it must keep recognising every historical line shape,
/// including the pre-bootstrap INLINE `__tokenade_proxy` function, or a machine
/// that never upgrades again keeps a proxy it can't get rid of.
pub(crate) fn uninstall_shell_proxy(
    home: Option<&Path>,
    dry_run: bool,
) -> Vec<(PathBuf, ProxyStrip)> {
    let home = match home
        .map(PathBuf::from)
        .or_else(tokenade_core::platform::home_dir)
    {
        Some(h) => h,
        None => return vec![],
    };
    let rc_files: &[&str] = &[
        ".profile",
        ".bashrc",
        ".bash_profile",
        ".zshenv",
        ".zshrc",
        ".config/fish/config.fish",
        ".config/powershell/Microsoft.PowerShell_profile.ps1",
        "Documents/PowerShell/Microsoft.PowerShell_profile.ps1",
        "Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1",
        ".config/nushell/config.nu",
        ".cshrc",
        ".tcshrc",
    ];
    let mut results = Vec::new();
    for rel in rc_files {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        let original = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                results.push((path, ProxyStrip::Failed(e.to_string())));
                continue;
            }
        };
        let lines: Vec<&str> = original.lines().collect();
        let mut kept: Vec<&str> = Vec::new();
        let mut removed = 0usize;
        let mut i = 0;
        while i < lines.len() {
            // Eat a legacy inlined function block whole (header + body + close).
            if let Some(end) = proxy_inline_block_end(&lines, i) {
                removed += end - i + 1;
                if kept.last().is_some_and(|k| k.trim().is_empty()) {
                    kept.pop();
                    removed += 1;
                }
                i = end + 1;
                continue;
            }
            let l = lines[i];
            if shell_proxy_line_is_tokenade(l) {
                removed += 1;
                // The installer prefixes its block with one blank separator
                // line; eat it too so uninstall restores the file verbatim.
                if kept.last().is_some_and(|k| k.trim().is_empty()) {
                    kept.pop();
                    removed += 1;
                }
            } else {
                kept.push(l);
            }
            i += 1;
        }
        let mut updated = kept.join("\n");
        if original.ends_with('\n') && !updated.is_empty() {
            updated.push('\n');
        }
        // Repair guards our strip just emptied — AND guards an EARLIER release
        // (≤0.9.4, which had no such pass) already emptied and left as a syntax
        // error. Hence this runs before the `removed == 0` bail: a victim of the
        // old uninstall has no tokenade line left to strip, so bailing early
        // would leave their rc file permanently broken.
        let (updated, orphaned) = drop_emptied_guards(&updated);
        removed += orphaned;
        if removed == 0 {
            continue;
        }
        if dry_run {
            results.push((path, ProxyStrip::WouldStrip(removed)));
            continue;
        }
        // Atomic + backup: a truncating write interrupted here (Ctrl-C, ENOSPC,
        // OOM-kill) leaves a half-written rc file — the exact "shell no longer
        // starts" outcome this pass exists to prevent. The installer already
        // writes atomically; the uninstaller must match it.
        crate::backup_file(&path);
        match crate::write_text_atomic(&path, &updated) {
            Ok(()) => results.push((path, ProxyStrip::Stripped(removed))),
            Err(e) => results.push((path, ProxyStrip::Failed(e.to_string()))),
        }
    }
    for rel in [
        ".tokenade/shellproxy.sh",
        ".tokenade/shellproxy.nu",
        ".tokenade/shellproxy.csh",
        ".tokenade/shellproxy.cmd",
    ] {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        if dry_run {
            results.push((path, ProxyStrip::WouldDelete));
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => results.push((path, ProxyStrip::Deleted)),
            Err(e) => results.push((path, ProxyStrip::Failed(e.to_string()))),
        }
    }
    results
}

/// Run [`uninstall_shell_proxy`] against the real home dir and print one
/// line per touched file. Returns how many files were (or would be)
/// changed — 0 means the proxy wasn't installed. Shared by
/// `tokenade uninstall` step 5 and `tokenade uninstall aliases`.
pub(crate) fn report_shell_proxy_removal(dry_run: bool) -> usize {
    let home = tokenade_core::platform::home_dir();
    let mut touched = 0usize;
    for (path, outcome) in uninstall_shell_proxy(home.as_deref(), dry_run) {
        match outcome {
            ProxyStrip::Stripped(n) => {
                println!(
                    "  ✓ removed {n} shell proxy line(s) from {}",
                    path.display()
                );
                touched += 1;
            }
            ProxyStrip::WouldStrip(n) => {
                println!(
                    "  ◐ would remove {n} shell proxy line(s) from {}",
                    path.display()
                );
                touched += 1;
            }
            ProxyStrip::Deleted => {
                println!("  ✓ deleted {}", path.display());
                touched += 1;
            }
            ProxyStrip::WouldDelete => {
                println!("  ◐ would delete {}", path.display());
                touched += 1;
            }
            ProxyStrip::Failed(e) => {
                eprintln!("  ✗ shell proxy removal in {}: {e}", path.display());
            }
        }
    }
    touched
}
