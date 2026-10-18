//! Which shell commands are READS of source files — and so earn the read
//! threshold and the `File(path)` footer. Split out of `disclose.rs`
//! when the pipe, sequence and loop forms crossed the 800-line guard.

use std::path::PathBuf;

/// Best-effort: if `shell_cmd` is a SINGLE plain file-reader invocation
/// (`cat` / `nl` / `head` / `tail` / … on one on-disk file, no pipe,
/// redirect, command-substitution, or multiple files), return that
/// file's path. The disclose footer uses it to redirect EXACT-match
/// lookups to grep/Read the ORIGINAL file (complete & verbatim) instead
/// of the head+tail stash preview — the `log-needle` failure mode (#1051).
///
/// Conservative by design: ANY shell metacharacter or ambiguity yields
/// `None`, so the caller keeps the generic `SourceHint::Shell` wording.
/// The candidate is resolved against the current working directory (the
/// hook runs in the agent's cwd) and accepted only when EXACTLY one
/// non-flag argument exists as a regular file — this naturally rejects
/// flag values (`head -n 50 f` → `50` isn't a file) and multi-file reads.
/// `shell_cmd` with every single- or double-quoted span removed, and the
/// first such span's content (a `sed`/`awk` program).
fn split_quoted(shell_cmd: &str) -> (String, Option<String>) {
    let mut unquoted = String::with_capacity(shell_cmd.len());
    let mut first: Option<String> = None;
    let mut chars = shell_cmd.chars();
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            let inner: String = chars.by_ref().take_while(|&d| d != c).collect();
            first.get_or_insert(inner);
            unquoted.push(' ');
        } else {
            unquoted.push(c);
        }
    }
    (unquoted, first)
}

pub fn single_file_read_target(shell_cmd: &str) -> Option<PathBuf> {
    single_file_read_target_in(shell_cmd, std::path::Path::new(""))
}

/// [`single_file_read_target`] with relative paths resolved under `base` — the
/// directory a leading `cd` of the same command line moved to.
fn single_file_read_target_in(shell_cmd: &str, base: &std::path::Path) -> Option<PathBuf> {
    // **A read through a pipe of line filters is still a read of that file.**
    // `awk 'NR>=2485 && NR<=2600' f.rs | nl -ba -v2485 | sed 's/\t/: /'`,
    // `grep -n "" f.rs | sed -n '385,520p'`, `nl -ba f.rs | sed -n …`: the
    // pipe only numbers or selects the file's own lines. Measured 2026-10-02
    // over a week of transcripts: five such reads of ~1 800–2 050 tokens were
    // stashed under context pressure — the read threshold is five times the
    // general one, and a pipe denied it — and the agent went back for the
    // middle of each. The FIRST stage must read one file; every later stage
    // must be a line filter with no file of its own.
    let mut stages = unquoted_pipe_stages(shell_cmd)?.into_iter();
    let target = single_stage_read_target(stages.next()?, base)?;
    stages.all(is_line_filter).then_some(target)
}

/// **Several source files read in one command are still a read.**
///
/// `for f in a.rs b.rs c.rs; do echo "=== $f"; sed -n 1,80p $f; done`, or
/// `cat a.ts; echo ===; sed -n 40,90p b.ts`: every statement prints a banner or
/// reads one source file. Measured 2026-10-02 over a month of transcripts:
/// 13 of 67 such `for` loops were stashed under the general threshold — a
/// head-and-tail summary that kept the first file and the last — and the next
/// call was `expand-ref`. They earn the read threshold [`single_file_read_target`]
/// gives a single file. `false` on anything else: a statement that is not a
/// banner and not a one-file read of SOURCE makes the whole command incidental.
pub fn reads_only_source_files(shell_cmd: &str) -> bool {
    let cleaned = shell_cmd.replace(" 2>/dev/null", "").replace(" 2>&1", "");
    let Some(stmts) = unquoted_statements(&cleaned) else {
        return false;
    };
    let mut expanded: Vec<String> = Vec::new();
    let mut i = 0;
    while i < stmts.len() {
        let st = stmts[i];
        if let Some(head) = st.strip_prefix("for ") {
            let Some((var, list)) = head.split_once(" in ") else {
                return false;
            };
            let (var, items) = (var.trim(), list.split_whitespace().collect::<Vec<_>>());
            let mut body = Vec::new();
            i += 1;
            while i < stmts.len() && stmts[i] != "done" {
                body.push(stmts[i].strip_prefix("do ").unwrap_or(stmts[i]));
                i += 1;
            }
            if i == stmts.len() {
                return false;
            }
            for item in &items {
                for b in &body {
                    let mut s = b.to_string();
                    for form in [
                        format!("\"${{{var}}}\""),
                        format!("\"${var}\""),
                        format!("${{{var}}}"),
                        format!("${var}"),
                    ] {
                        s = s.replace(&form, item);
                    }
                    expanded.push(s);
                }
            }
        } else {
            expanded.push(st.to_string());
        }
        i += 1;
    }
    // `cd ~/proj && sed -n 1,80p a.ts; sed -n 40,90p b.ts`: a leading `cd`
    // names where the reads are, and nothing else. Measured 2026-10-02 over two
    // weeks of transcripts: 429 chains of reads opened on a `cd`, against 561
    // that did not, and every one of them was judged statement by statement.
    let mut base = PathBuf::new();
    if let Some(dir) = expanded.first().and_then(|st| st.strip_prefix("cd ")) {
        let dir = dir.trim().trim_matches(|c| c == '\'' || c == '"');
        if dir.is_empty() || dir.contains(char::is_whitespace) {
            return false;
        }
        base = match dir.strip_prefix('~') {
            Some(rest) => match crate::platform::home_dir() {
                Some(h) => h.join(rest.trim_start_matches('/')),
                None => return false,
            },
            None => PathBuf::from(dir),
        };
        expanded.remove(0);
    }
    let mut reads = 0usize;
    for st in &expanded {
        let first = st.split_whitespace().next().unwrap_or("");
        if matches!(first, "echo" | "printf") {
            continue;
        }
        match single_file_read_target_in(st, &base) {
            Some(p) if earns_the_read_threshold(&p) => reads += 1,
            _ => return false,
        }
    }
    reads >= 2
}

/// **Which files a deliberate read of earns the read threshold.**
///
/// Source code first. Measured 2026-10-02 over a month of
/// transcripts, single-file reads stashed under the general threshold and
/// fetched back within two calls: `.md` 9 of 27, `.txt` 7 of 10, `.yml` 2 of 2,
/// `.toml` 2 of 2 — documents and configuration are read on purpose too. A
/// LOG is not: `.log` 0 of 2, `.output` 0 of 2, and it stays incidental.
pub fn earns_the_read_threshold(path: &std::path::Path) -> bool {
    let name = path.to_string_lossy();
    if crate::auto::is_source_code_extension(&name) {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "mdx"
            | "rst"
            | "adoc"
            | "txt"
            | "yml"
            | "yaml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
    )
}

/// `shell_cmd` cut at every unquoted `;`, `&&` and newline, trimmed, empties
/// dropped. `None` on a `||`, a backgrounding `&` or a subshell.
fn unquoted_statements(shell_cmd: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let (mut start, mut quote) = (0usize, None::<char>);
    let b = shell_cmd.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(' | ')' | '`') => return None,
            (None, ';' | '\n') => {
                out.push(shell_cmd[start..i].trim());
                start = i + 1;
            }
            (None, '&') => {
                if b.get(i + 1) != Some(&b'&') {
                    return None;
                }
                out.push(shell_cmd[start..i].trim());
                i += 1;
                start = i + 1;
            }
            (None, '|') if b.get(i + 1) == Some(&b'|') => return None,
            _ => {}
        }
        i += 1;
    }
    out.push(shell_cmd[start..].trim());
    Some(out.into_iter().filter(|s| !s.is_empty()).collect())
}

/// `shell_cmd` cut at every unquoted `|` — `None` on `||`, which sequences.
fn unquoted_pipe_stages(shell_cmd: &str) -> Option<Vec<&str>> {
    let mut stages = Vec::new();
    let (mut start, mut quote) = (0usize, None::<char>);
    let bytes = shell_cmd.as_bytes();
    for (i, c) in shell_cmd.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|') => {
                if bytes.get(i + 1) == Some(&b'|') || (i > 0 && bytes[i - 1] == b'|') {
                    return None;
                }
                stages.push(shell_cmd[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    stages.push(shell_cmd[start..].trim());
    Some(stages)
}

/// A pipe stage that keeps the lines it is given as lines of the same file:
/// numbering (`nl`, `cat -n`), selecting (`head`, `tail`, `sed -n '<ranges>p'`)
/// or a plain `sed 's/…/…/'` substitution. Never one naming a file of its own.
fn is_line_filter(stage: &str) -> bool {
    let (unquoted, program) = split_quoted(stage);
    if unquoted.contains(['>', '<', ';', '&', '`', '*', '?']) || unquoted.contains("$(") {
        return false;
    }
    let mut toks = unquoted.split_whitespace();
    let Some(base) = toks.next() else {
        return false;
    };
    let operands_are_numbers =
        toks.all(|t| t.starts_with('-') || t.chars().all(|c| c.is_ascii_digit()));
    match base {
        "nl" | "cat" | "head" | "tail" => operands_are_numbers,
        "sed" => {
            operands_are_numbers
                && program.as_deref().is_some_and(|p| {
                    let ranges = p.contains('p')
                        && p.chars().all(|c| c.is_ascii_digit() || ",p;$ ".contains(c));
                    ranges || (p.starts_with("s/") && !p.contains(';'))
                })
        }
        _ => false,
    }
}

fn single_stage_read_target(shell_cmd: &str, dir: &std::path::Path) -> Option<PathBuf> {
    // Only reason about a bare `<reader> [flags] <file>` form — bail on
    // any redirect / sequencing / substitution metacharacter.
    // Metacharacters count OUTSIDE quotes only: `awk 'NR>=200 && NR<=300' f`
    // carries `>`, `&` and `<` in its program, and is still one file read.
    let (unquoted, program) = split_quoted(shell_cmd);
    if unquoted.contains(['|', '>', '<', ';', '&', '`', '\n', '*', '?']) || unquoted.contains("$(")
    {
        return None;
    }
    let mut toks = unquoted.split_whitespace();
    let cmd = toks.next()?;
    // Strip a leading path on the reader binary itself (`/bin/cat`).
    let base = cmd.rsplit(['/', '\\']).next().unwrap_or(cmd);
    const READERS: &[&str] = &[
        "cat", "nl", "tac", "head", "tail", "less", "more", "bat", "strings",
    ];
    // `sed -n '120,240p' f` and `awk 'NR>=120 && NR<=240' f` print lines of
    // the file unchanged — the same reads the filter's ranged-read arm
    // already routes as reads. A program that TRANSFORMS is not a read.
    // `grep -n '' f` (or `'^'`) matches every line: the file, numbered.
    // `sed -n 1,80p f` writes its program unquoted — as often as not in the
    // transcripts (`sed -n 95,120p crates/…/nudges.rs`), and it was not a read.
    let is_range =
        |p: &str| p.contains('p') && p.chars().all(|c| c.is_ascii_digit() || ",p;$ ".contains(c));
    let program = program.or_else(|| {
        (base == "sed")
            .then(|| {
                unquoted
                    .split_whitespace()
                    .skip(1)
                    .find(|t| !t.starts_with('-') && is_range(t))
                    .map(String::from)
            })
            .flatten()
    });
    let selects_lines = match base {
        "sed" => {
            unquoted.split_whitespace().any(|t| t == "-n")
                && program.as_deref().is_some_and(|p| {
                    p.contains('p') && p.chars().all(|c| c.is_ascii_digit() || ",p;$ ".contains(c))
                })
        }
        "awk" | "gawk" | "mawk" => {
            let argv: Vec<String> = shell_cmd.split_whitespace().map(String::from).collect();
            crate::argv::awk_only_selects_lines(&argv)
        }
        "grep" => matches!(program.as_deref(), Some("" | "^")),
        _ => READERS.contains(&base),
    };
    if !selects_lines {
        return None;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for t in toks {
        if t.starts_with('-') {
            continue; // flag, e.g. `-n`, `--number`, `-5`, `-c`
        }
        let p = dir.join(t);
        if std::fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false) {
            files.push(p);
        }
    }
    // Exactly one real file → unambiguous source. Zero or many → bail.
    if files.len() == 1 {
        files.pop()
    } else {
        None
    }
}
