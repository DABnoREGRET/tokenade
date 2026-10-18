//! `git status` — all variants.
//!
//! - default: human-readable section headers + indented file lists.
//! - `--porcelain` (v1) / `-s` / `--short` / `-sb` / `-bs`: `XY <path>`
//!   compact rows with optional `## <branch>` header.
//! - `--porcelain=v2`: machine-format records with branch.head /
//!   branch.upstream metadata.
//! - `-z`: NUL-separated records (decoded back to newlines).
//!
//! In-progress states (rebase, cherry-pick, merge conflict, etc.)
//! are surfaced as a one-line `[…]` header so the agent knows the
//! repo isn't in a vanilla state. Detached HEAD gets its own line.

use crate::compress::generic;
use crate::argv::{flag_value, has_any_flag, has_flag};
use crate::ansi::strip_ansi;
use std::fmt::Write;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if flag_value(argv, "--porcelain").is_some_and(|v| v == "v2" || v == "2") {
        return porcelain_v2_compact(output);
    }
    let is_porcelain = has_any_flag(argv, &["--porcelain", "-s", "--short", "-sb", "-bs"])
        || (has_flag(argv, "-s") && has_any_flag(argv, &["-b", "--branch"]));
    if is_porcelain {
        if output.contains('\0') {
            return porcelain_compact(&output.replace('\0', "\n"));
        }
        return porcelain_compact(output);
    }
    default(output)
}

/// `git status --porcelain` (v1) — `XY <path>` per row.
pub fn porcelain_compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut branch_line: Option<String> = None;
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    let mut ignored = 0;
    // (code, path) rather than a rendered line: the flat and the grouped
    // rendering both need the two halves, and re-splitting a rendered line was
    // getting `" M"` wrong in a way that only LOOKED right.
    let mut entries: Vec<(String, String)> = Vec::new();
    for raw in s.lines() {
        if let Some(rest) = raw.strip_prefix("## ") {
            branch_line = Some(format!("[{}]", rest.trim()));
            continue;
        }
        if raw.len() < 3 {
            continue;
        }
        let bytes = raw.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let path = &raw[3..];
        if x == '?' && y == '?' {
            untracked += 1;
            entries.push(("?".to_string(), path.to_string()));
        } else if x == '!' && y == '!' {
            ignored += 1;
            entries.push(("!".to_string(), path.to_string()));
        } else {
            if x != ' ' && x != '?' {
                staged += 1;
            }
            if y != ' ' && y != '?' {
                unstaged += 1;
            }
            entries.push((format!("{x}{y}"), path.to_string()));
        }
    }
    if entries.is_empty() && branch_line.is_none() {
        return "clean working tree".to_string();
    }
    let mut header = String::new();
    if let Some(b) = branch_line {
        header.push_str(&b);
        header.push('\n');
    }
    let _ = writeln!(
        header,
        "{staged} staged, {unstaged} unstaged, {untracked} untracked{}",
        if ignored > 0 {
            format!(", {ignored} ignored")
        } else {
            String::new()
        }
    );
    let mut flat = header.clone();
    for (code, path) in &entries {
        let _ = writeln!(flat, "  {code} {path}");
    }
    // The SAME entries with their repeated directory factored out. Lossless:
    // every filename is still spelled, which is what separates this from the
    // file-list compactors — on a `git status` the names ARE the answer, they
    // are the files the agent is about to edit.
    //
    // Measured 2026-08-28 on this repository: 210 entries, 9 411 bytes raw and
    // 8 936 delivered (5 %), because the old rendering re-printed every line
    // with two extra spaces in front and paid for the header. `command_rewrite`
    // turns a bare `git status` INTO this format, so the rewrite was producing
    // the one shape nothing folded.
    let grouped = group_entries(&header, &entries);
    if grouped.len() < flat.len() {
        grouped.trim_end().to_string()
    } else {
        flat.trim_end().to_string()
    }
}

/// Render one section's file entries, grouped when that is shorter.
///
/// The localised fold kept every entry verbatim and elided only hint lines:
/// measured 2026-08-28 on this repository's own `git status`, **213 files,
/// 13 677 bytes in and 12 044 out — 11 %**, the worst real number in the
/// command benchmark that is not a deliberate design choice, on the command
/// an agent runs most. The porcelain form has factored the repeated directory
/// for a while and gets 55 %; the human form did not, so the rewrite that
/// exists to reach the shorter shape was the only path that folded.
fn flush_run(run: &mut Vec<(String, String)>, out: &mut String) {
    if run.is_empty() {
        return;
    }
    let mut flat = String::new();
    for (marker, path) in run.iter() {
        if marker.is_empty() {
            let _ = writeln!(flat, "  {path}");
        } else {
            let _ = writeln!(flat, "  {marker} {path}");
        }
    }
    let grouped = group_entries("", run);
    out.push_str(if grouped.len() < flat.len() { &grouped } else { &flat });
    run.clear();
}

/// `  XY dir/a.rs`, `  XY dir/b.rs` → `  XY dir/: a.rs b.rs`.
///
/// Grouped on (code, parent directory) in first-seen order: a modified file
/// and an untracked one in the same directory are different answers and never
/// share a line. A rename carries two paths and is left whole — half-folding
/// it would leave the reader guessing which half moved.
fn group_entries(header: &str, entries: &[(String, String)]) -> String {
    let mut groups: Vec<((&str, &str), Vec<&str>)> = Vec::new();
    let mut whole: Vec<(&str, &str)> = Vec::new();
    for (code, path) in entries {
        if path.contains(" -> ") || path.is_empty() {
            whole.push((code, path));
            continue;
        }
        let (parent, name) = match path.rfind('/') {
            Some(i) => (&path[..=i], &path[i + 1..]),
            None => ("./", path.as_str()),
        };
        let key = (code.as_str(), parent);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, names)) => names.push(name),
            None => groups.push((key, vec![name])),
        }
    }
    let mut out = header.to_string();
    // An empty code is not padding: the localised `git status` lists an
    // untracked file with NO marker at all, and `  {code} {parent}` would put
    // two spaces where the porcelain form puts one and a letter.
    let lead = |code: &str| if code.is_empty() { String::new() } else { format!("{code} ") };
    for ((code, parent), names) in &groups {
        // A group of ONE is not a group: `src/: a.rs` costs two bytes more
        // than `src/a.rs` and reads worse. Rendered flat, in place, so the
        // order of the section is exactly the order git printed it.
        if let [only] = names.as_slice() {
            let path = if *parent == "./" { (*only).to_string() } else { format!("{parent}{only}") };
            let _ = writeln!(out, "  {}{path}", lead(code));
            continue;
        }
        let _ = writeln!(out, "  {}{parent}: {}", lead(code), names.join(" "));
    }
    for (code, path) in &whole {
        let _ = writeln!(out, "  {}{path}", lead(code));
    }
    out
}

/// `git status --porcelain=v2` — extended machine format.
pub fn porcelain_v2_compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut branch_info: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch_info.push(format!("[{}]", rest.trim()));
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            branch_info.push(format!("(↑{})", rest.trim()));
        } else if let Some(rest) = line.strip_prefix("1 ") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let (Some(xy), Some(path)) = (toks.first(), toks.get(7)) {
                entries.push(format!("  {xy} {path}"));
            }
        } else if let Some(rest) = line.strip_prefix("2 ") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(xy) = toks.first() {
                if let Some(last) = toks.last() {
                    entries.push(format!("  {xy} {last}"));
                }
            }
        } else if let Some(rest) = line.strip_prefix("? ") {
            entries.push(format!("  ? {rest}"));
        } else if let Some(rest) = line.strip_prefix("! ") {
            entries.push(format!("  ! {rest}"));
        }
    }
    if entries.is_empty() && branch_info.is_empty() {
        return "clean working tree".to_string();
    }
    let mut out = String::new();
    if !branch_info.is_empty() {
        out.push_str(&branch_info.join(" "));
        out.push('\n');
    }
    for e in &entries {
        out.push_str(e);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Default `git status` — human-readable, with section headers and
/// state markers.
pub fn default(output: &str) -> String {
    let s = strip_ansi(output);
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    let mut files: Vec<String> = Vec::new();
    let mut section = "";
    let mut state_markers: Vec<&'static str> = Vec::new();
    let head_lower = s
        .get(..s.len().min(4096))
        .unwrap_or(&s)
        .to_ascii_lowercase();
    let probes: &[(&str, &str)] = &[
        ("rebasing", "rebase-in-progress"),
        ("you are in the middle of an am session", "am-in-progress"),
        ("you are currently cherry-picking", "cherry-pick-in-progress"),
        ("you are currently bisecting", "bisect-in-progress"),
        ("all conflicts fixed but you are still merging", "merge-in-progress"),
        ("you have unmerged paths", "merge-conflicts"),
        ("revert is in progress", "revert-in-progress"),
    ];
    for (needle, label) in probes {
        if head_lower.contains(needle) {
            state_markers.push(label);
        }
    }
    let mut detached_line: Option<String> = None;
    for line in s.lines().take(5) {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("HEAD detached ") {
            detached_line = Some(format!("HEAD detached {}", rest.trim()));
            break;
        }
    }
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with("Changes to be committed") {
            section = "staged";
        } else if t.starts_with("Changes not staged") {
            section = "unstaged";
        } else if t.starts_with("Untracked files") {
            section = "untracked";
        } else if let Some(f) = t
            .strip_prefix("modified:")
            .or_else(|| t.strip_prefix("new file:"))
            .or_else(|| t.strip_prefix("deleted:"))
        {
            files.push(format!(
                "  {} {}",
                section.chars().next().unwrap_or('?'),
                f.trim()
            ));
            match section {
                "staged" => staged += 1,
                "unstaged" => unstaged += 1,
                _ => untracked += 1,
            }
        } else if section == "untracked" && !t.is_empty() && !t.starts_with('(') {
            files.push(format!("  ? {t}"));
            untracked += 1;
        }
    }
    let mut header_lines: Vec<String> = Vec::new();
    if !state_markers.is_empty() {
        header_lines.push(format!("[{}]", state_markers.join(", ")));
    }
    if let Some(d) = detached_line {
        header_lines.push(d);
    }
    let header = if header_lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", header_lines.join("\n"))
    };
    if files.is_empty() {
        let any_substantive = s.lines().any(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with("On branch")
                && !t.starts_with("Your branch ")
                && !t.starts_with("nothing to commit")
                && !t.starts_with("(use ")
                && !t.starts_with("HEAD detached")
        });
        if any_substantive {
            return default_structural(&s);
        }
        return if header.is_empty() {
            "clean working tree".to_string()
        } else {
            format!("{header}clean working tree")
        };
    }
    format!(
        "{header}{} staged, {} unstaged, {} untracked\n{}",
        staged,
        unstaged,
        untracked,
        files.join("\n")
    )
}

/// Locale-independent structural fold for the default `git status`
/// output. The English keyword parser in [`default`] only fires under
/// an English locale; git localises its human-readable output, so under
/// any other `LANG` the section headers (`Changes to be committed`) and
/// file markers (`modified:`) are translated and never match — sending
/// the whole thing to the generic compactor, which understands none of
/// the structure. Rather than lose the fold entirely off-English, we
/// fold by the *layout*, which git keeps stable across every locale:
///
/// - file entries are TAB-indented (`\t modifié :  path`),
/// - hint lines are 2-space-indented and parenthesised (`  (utilisez …)`),
/// - branch/section header lines are not indented.
///
/// Headers, section titles and every file entry are kept verbatim (the
/// actionable content); only the hint lines and blank lines are elided
/// (pure noise for an agent that already knows git). No file is ever
/// dropped, so this is safe in any language.
fn default_structural(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut files = 0usize;
    let mut elided = 0usize;
    // Entries of the CURRENT section, flushed when a header ends the run.
    // Per-section and not per-output: a modified file and an untracked one
    // are different answers, and git already separates them by a header.
    let mut run: Vec<(String, String)> = Vec::new();
    for line in s.lines() {
        if let Some(entry) = line.strip_prefix('\t') {
            // File entry: collapse the locale-dependent padding between
            // the (translated) marker and the path to a single space.
            let mut compact = String::with_capacity(entry.len());
            let mut spaces = 0usize;
            for c in entry.trim().chars() {
                if c == ' ' {
                    spaces += 1;
                } else {
                    if spaces > 0 {
                        compact.push(' ');
                        spaces = 0;
                    }
                    compact.push(c);
                }
            }
            // `modifié : path` → ("modifié :", "path"); an untracked entry
            // carries no marker at all, so the whole line is the path.
            // FIRST `": "` and not the last: a colon is a legal byte in a
            // filename, and the marker is always the leading word.
            let (marker, path) = match compact.find(": ") {
                Some(i) => (compact[..=i].to_string(), compact[i + 2..].to_string()),
                None => (String::new(), compact),
            };
            run.push((marker, path));
            files += 1;
            continue;
        }
        let t = line.trim();
        if t.is_empty() || t.starts_with('(') {
            elided += 1;
            continue;
        }
        // Branch / section header line — keep verbatim, after the run it ends.
        flush_run(&mut run, &mut out);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    flush_run(&mut run, &mut out);
    if files == 0 {
        // No TAB-indented entry anywhere. In git's layout that is not an
        // ambiguity, it is the answer: staged, unstaged, untracked and
        // conflicted files are ALL tab-indented, and a section with no entries
        // is not printed at all. So zero entries means nothing to commit — in
        // every language, without reading a word of the message.
        //
        // The English parser above answers this case with `clean working tree`
        // and reaches it by matching `nothing to commit`. Off English it never
        // matched, `any_substantive` was true for every French or German line,
        // and the whole thing landed in `generic`, which returns it unchanged.
        // Measured 2026-09-01 on this machine: `git status` on a clean tree is
        // 155 -> 19 bytes under `LC_ALL=C` and 185 -> 185 under
        // `fr_FR.UTF-8`. Zero saving on the command an agent runs more than any
        // other, for everyone whose machine is not in English.
        //
        // What is kept is the unindented lines — the branch, its ahead/behind
        // count, any in-progress state, and the closing sentence — because
        // those are the actionable half and this path cannot tell which of them
        // is which without the language. That keeps MORE than the English fold
        // does, and it never drops a file, so it is safe in any locale.
        //
        // `out` here holds exactly those lines: the loop above has already
        // dropped blanks and parenthesised hints. An input with nothing kept
        // and nothing elided is not a status layout at all — that one still
        // belongs to the generic path.
        if out.is_empty() || elided == 0 {
            return generic(s);
        }
        // With no entries, the unindented lines AFTER the last blank line are
        // git's closing summary — the `nothing to commit, working tree clean`
        // sentence, translated. Everything before the blank is the branch, its
        // ahead/behind count and any in-progress state, which is the half worth
        // keeping. So the summary is replaced with the phrase the English path
        // already emits, and the answer reads the same in every language.
        //
        // This keeps MORE than the English fold does: that one answers `clean
        // working tree` and drops the branch and its ahead/behind count
        // entirely.
        let kept: Vec<&str> = out.lines().collect();
        let before_blank = match s.rfind("\n\n") {
            Some(_) => {
                let tail_start = s
                    .rmatch_indices("\n\n")
                    .next()
                    .map(|(i, _)| i + 2)
                    .unwrap_or(0);
                let tail: Vec<&str> = s[tail_start..]
                    .lines()
                    .map(str::trim_end)
                    .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('('))
                    .collect();
                kept.into_iter()
                    .filter(|l| !tail.contains(&l.trim_end()))
                    .collect::<Vec<_>>()
            }
            None => kept,
        };
        out = before_blank.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = write!(out, "clean working tree");
        // A fold that GROWS the payload is not a fold. On a short clean status
        // — no upstream, no hints — there is one blank line to elide and the
        // banner costs more than it saves, so the honest answer is to leave the
        // text alone. Measured rather than assumed: the same rule the wire
        // rewriter applies, arrived at here the same way, by comparing.
        if out.len() >= s.len() {
            return generic(s);
        }
        return out;
    }
    let _ = write!(out, "[tokenade:git-status] {files} file(s)");
    if elided > 0 {
        let _ = write!(out, ", {elided} hint/blank line(s) elided");
    }
    out
}
#[cfg(test)]
include!("status_tests.rs");
