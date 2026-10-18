//! **A user's `deny` and `ask` rules judge the command the user typed.**
//!
//! Claude Code evaluates `permissions.deny` / `permissions.ask` against the
//! input a PreToolUse hook RETURNS (hooks reference, "decision control").
//! Rewritten to `tokenade wrap 'rm -rf build'`, a command no longer starts
//! with `rm`, so `Bash(rm:*)` in `deny` stopped matching it and the call ran —
//! a rule the user wrote, silently disarmed by the wrap. rtk closed the same
//! class in its gate (2026-09-18, three segmentation holes).
//!
//! The answer here is the cheap, safe one: when any `Bash` deny or ask rule
//! COULD cover the command or one of its segments, the hook steps aside and
//! Claude Code judges the original. Matching errs wide on purpose — a
//! false hit costs one command's compaction, a miss costs the user's rule.

use std::path::{Path, PathBuf};

/// Every settings file whose `permissions` Claude Code may apply here: the
/// user's, each `.claude/` from `cwd` up (project and local), and the managed
/// policy. A file that is absent or unreadable contributes nothing.
fn settings_files(cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = tokenade_core::platform::home_dir() {
        files.push(tokenade_core::platform::claude_config_dir_for(&home).join("settings.json"));
    }
    for dir in cwd.into_iter().flat_map(Path::ancestors) {
        files.push(dir.join(".claude").join("settings.json"));
        files.push(dir.join(".claude").join("settings.local.json"));
    }
    files.push(PathBuf::from("/etc/claude-code/managed-settings.json"));
    files.push(PathBuf::from(
        "/Library/Application Support/ClaudeCode/managed-settings.json",
    ));
    files.push(PathBuf::from(
        r"C:\Program Files\ClaudeCode\managed-settings.json",
    ));
    files
}

/// The inside of each `<tool>` / `<tool>(…)` deny or ask rule (`""` for the
/// bare tool name) — `Bash`, or `PowerShell` for the Windows shell tool.
fn guarding_patterns(files: &[PathBuf], tool: &str) -> Vec<String> {
    rules_of(files, tool).into_iter().map(|(p, _)| p).collect()
}

/// Each deny/ask rule of `tool` with the directory its settings file governs
/// (the one holding `.claude/`): a `Read(/path)` rule is relative to it.
fn rules_of(files: &[PathBuf], tool: &str) -> Vec<(String, PathBuf)> {
    let mut patterns = Vec::new();
    for file in files {
        let root = file
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        for list in ["deny", "ask"] {
            let rules = v
                .pointer(&format!("/permissions/{list}"))
                .and_then(|r| r.as_array());
            for rule in rules.into_iter().flatten().filter_map(|r| r.as_str()) {
                let rule = rule.trim();
                if rule == tool {
                    patterns.push((String::new(), root.clone()));
                } else if let Some(inner) = rule
                    .strip_prefix(tool)
                    .and_then(|r| r.strip_prefix('('))
                    .and_then(|r| r.strip_suffix(')'))
                {
                    patterns.push((inner.trim().to_string(), root.clone()));
                }
            }
        }
    }
    patterns
}

/// The simple commands of `cmd`, cut at every operator and grouping a shell
/// knows, each without its leading `VAR=value` words and `!`.
fn segments(cmd: &str) -> Vec<String> {
    cmd.split([';', '|', '&', '\n', '(', ')', '{', '}', '`'])
        .map(|s| {
            let mut words = s.split_whitespace().peekable();
            while words
                .peek()
                .is_some_and(|w| *w == "!" || (w.contains('=') && !w.starts_with('=')))
            {
                words.next();
            }
            words.collect::<Vec<_>>().join(" ")
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Could `pattern` match `segment`? Its literal head (before any `*`, without
/// the legacy `:` of `:*`) as a word-prefix of the segment, case aside
/// (PowerShell's names are case-insensitive) — wider than Claude Code's own
/// matcher, never narrower.
fn could_cover(pattern: &str, segment: &str) -> bool {
    let (pattern, segment) = (pattern.to_lowercase(), segment.to_lowercase());
    let head = pattern
        .split('*')
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .trim();
    if head.is_empty() {
        return true;
    }
    let head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    segment.strip_prefix(head.as_str()).is_some_and(|rest| {
        rest.is_empty() || rest.starts_with(' ') || !head.ends_with(|c: char| c.is_alphanumeric())
    })
}

/// Must the hook leave `cmd` to Claude Code's own judgement?
pub(crate) fn a_user_rule_may_guard(tool: &str, cmd: &str, cwd: Option<&Path>) -> bool {
    guards(&guarding_patterns(&settings_files(cwd), tool), cmd)
}

fn guards(patterns: &[String], cmd: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let whole = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    let parts = segments(cmd);
    patterns
        .iter()
        .any(|p| could_cover(p, &whole) || parts.iter().any(|s| could_cover(p, s)))
}

/// Must the Read hook leave `file` to Claude Code? Its redirect hands Claude
/// Code a converted copy under `~/.tokenade/`, and a `Read(./secrets/**)`
/// deny rule is then judged on the COPY's path — the hook has already read the
/// denied file and the copy goes through.
pub(crate) fn a_user_rule_may_guard_path(file: &str, cwd: Option<&Path>) -> bool {
    let rules = rules_of(&settings_files(cwd), "Read");
    if rules.is_empty() {
        return false;
    }
    let home = tokenade_core::platform::home_dir().unwrap_or_default();
    path_guarded(&rules, Path::new(file), cwd, &home)
}

fn path_guarded(rules: &[(String, PathBuf)], file: &Path, cwd: Option<&Path>, home: &Path) -> bool {
    let cwd = cwd.map(Path::to_path_buf).unwrap_or_default();
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        cwd.join(file)
    };
    let mut paths = vec![file.clone()];
    paths.extend(std::fs::canonicalize(&file).ok());
    rules.iter().any(|(pattern, root)| {
        let globs: Vec<String> = if pattern.is_empty() {
            vec!["*".into()]
        } else if let Some(abs) = pattern.strip_prefix("//") {
            vec![format!("/{abs}")]
        } else if let Some(rel) = pattern.strip_prefix("~/") {
            vec![home.join(rel).display().to_string()]
        } else if pattern.contains('/') {
            let rel = pattern.trim_start_matches("./").trim_start_matches('/');
            vec![
                root.join(rel).display().to_string(),
                cwd.join(rel).display().to_string(),
            ]
        } else {
            vec![format!("*/{pattern}")]
        };
        paths.iter().flat_map(|p| p.ancestors()).any(|a| {
            let a = a.display().to_string();
            globs.iter().any(|g| wildcard(g.as_bytes(), a.as_bytes()))
        })
    })
}

/// Must the WebFetch hook leave `url` to Claude Code? The hook fetches the
/// page ITSELF and hands it back in a decision's reason, so a
/// `WebFetch(domain:x)` deny rule no longer kept the page out.
pub(crate) fn a_user_rule_may_guard_url(url: &str, cwd: Option<&Path>) -> bool {
    url_guarded(&guarding_patterns(&settings_files(cwd), "WebFetch"), url)
}

fn url_guarded(patterns: &[String], url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_lowercase();
    patterns.iter().any(|p| {
        // A form other than `domain:` is not understood here: covered.
        let Some(domain) = p.strip_prefix("domain:") else {
            return true;
        };
        let domain = domain.trim().trim_start_matches("*.").to_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    })
}

/// Does any `tool` deny/ask rule exist? The Grep hook runs its own `rg` over a
/// whole tree, so ANY `Read` rule may cover a file it would quote.
pub(crate) fn any_user_rule_for(tool: &str, cwd: Option<&Path>) -> bool {
    !rules_of(&settings_files(cwd), tool).is_empty()
}

/// `*` (and so `**`) any run of characters, slashes included; `?` any one.
fn wildcard(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_sees_every_segment_behind_operators_and_prefixes() {
        let hit = |p: &str, cmd: &str| segments(cmd).iter().any(|s| could_cover(p, s));
        assert!(hit("rm:*", "cd build && rm -rf out"));
        assert!(hit("rm:*", "ls; { rm -rf / ; }"));
        assert!(hit("rm:*", "FOO=1 rm x"));
        assert!(hit("git push:*", "git status | cat && git  push --force"));
        assert!(hit("git push", "x=$(git push)"));
        assert!(hit("", "anything"));
        assert!(!hit("rm:*", "rmdir x; echo rm"));
        assert!(!hit("git push:*", "git pull"));
    }

    #[test]
    fn a_webfetch_rule_covers_its_domain_and_subdomains() {
        let ps = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let d = ps(&["domain:example.com"]);
        assert!(url_guarded(&d, "https://example.com/a"));
        assert!(url_guarded(&d, "https://docs.Example.com:8443/a?q"));
        assert!(url_guarded(&d, "http://user@api.example.com"));
        assert!(!url_guarded(&d, "https://notexample.com/"));
        assert!(!url_guarded(&d, "https://example.com.evil.net/"));
        assert!(url_guarded(&ps(&[""]), "https://any.org"));
        assert!(
            url_guarded(&ps(&["https://example.com/*"]), "https://other.org"),
            "an unknown form errs wide"
        );
    }

    #[test]
    fn a_read_rule_covers_its_path_wherever_it_is_written() {
        let rules = |ps: &[&str]| {
            ps.iter()
                .map(|p| (p.to_string(), PathBuf::from("/proj")))
                .collect::<Vec<_>>()
        };
        let (cwd, home) = (Some(Path::new("/proj/sub")), Path::new("/home/u"));
        let hit = |ps: &[&str], f: &str| path_guarded(&rules(ps), Path::new(f), cwd, home);
        assert!(hit(&["./secrets/**"], "/proj/sub/secrets/a.pdf"));
        assert!(hit(&["/secrets/**"], "/proj/secrets/deep/a.png"));
        assert!(
            hit(&["./secrets"], "secrets/a.pdf"),
            "a directory covers what it holds"
        );
        assert!(hit(&["~/.ssh/**"], "/home/u/.ssh/id.png"));
        assert!(hit(&["//etc/**"], "/etc/x.pdf"));
        assert!(hit(&["*.pdf"], "/elsewhere/report.pdf"));
        assert!(hit(&[".env"], "/proj/.env"));
        assert!(hit(&[""], "/any/thing.png"));
        assert!(!hit(&["./secrets/**"], "/proj/sub/public/a.pdf"));
        assert!(!hit(&["*.pdf"], "/proj/a.png"));
    }

    #[test]
    fn deny_and_ask_rules_are_read_from_the_project_up() {
        let root = std::env::temp_dir().join(format!("tokenade-perm-rules-{}", std::process::id()));
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(deep.join(".claude")).unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude").join("settings.json"),
            r#"{"permissions":{"deny":["Bash(rm:*)","Read(./.env)"],"allow":["Bash(curl:*)"]}}"#,
        )
        .unwrap();
        std::fs::write(
            deep.join(".claude").join("settings.local.json"),
            r#"{"permissions":{"ask":["Bash(git push:*)"]}}"#,
        )
        .unwrap();
        let files: Vec<PathBuf> = settings_files(Some(&deep))
            .into_iter()
            .filter(|f| f.starts_with(&root))
            .collect();
        let got = guarding_patterns(&files, "Bash");
        assert!(
            got.contains(&"rm:*".to_string()) && got.contains(&"git push:*".to_string()),
            "{got:?}"
        );
        assert!(
            !got.iter().any(|p| p.contains("curl") || p.contains(".env")),
            "{got:?}"
        );
        assert!(guards(&got, "cd x && rm -rf y"));
        assert!(!guards(&got, "curl -s example.com"));
        std::fs::write(
            root.join(".claude").join("settings.json"),
            r#"{"permissions":{"deny":["Bash(ls:*)","PowerShell(Remove-Item:*)"]}}"#,
        )
        .unwrap();
        std::fs::write(
            deep.join(".claude").join("settings.local.json"),
            r#"{"permissions":{"ask":["WebSearch"]}}"#,
        )
        .unwrap();
        assert_eq!(
            rules_of(&files, "WebSearch").len(),
            1,
            "a bare tool rule counts"
        );
        assert!(rules_of(&files, "WebFetch").is_empty());
        let ps = guarding_patterns(&files, "PowerShell");
        assert_eq!(ps, vec!["Remove-Item:*".to_string()]);
        assert!(guards(&ps, "Get-ChildItem; remove-item -Recurse C:\\tmp"));
        assert!(!guards(&ps, "ls"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
