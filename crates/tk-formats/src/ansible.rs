//! Ansible playbook output compaction.
//!
//! `ansible-playbook` emits a strictly-shaped, deeply-verbose output:
//!
//! ```text
//! PLAY [Deploy webservers] ****
//!
//! TASK [Gathering Facts] ****
//! ok: [web-1]
//! ok: [web-2]
//!
//! TASK [Install nginx] ****
//! changed: [web-1] => {"changed": true, "rc": 0, "stdout_lines": […], …}
//! changed: [web-2] => {"changed": true, "rc": 0, "stdout_lines": […], …}
//!
//! TASK [Apply database migration] ****
//! fatal: [db-1]: FAILED! => {"changed": false, "msg": "Database is not reachable"}
//!
//! PLAY RECAP ****
//! db-1   : ok=2  changed=0  unreachable=0  failed=1  skipped=0  rescued=0  ignored=0
//! web-1  : ok=4  changed=3  unreachable=0  failed=0  skipped=0  rescued=0  ignored=0
//! web-2  : ok=4  changed=4  unreachable=0  failed=0  skipped=0  rescued=0  ignored=0
//! ```
//!
//! What the agent reading this actually wants:
//!   - the **outline**: play names + task names + per-task host outcome counts
//!   - every **failure** verbatim (the `fatal:` line is the diagnostic gold)
//!   - the **PLAY RECAP** host-level stats table
//!
//! What we drop:
//!   - the verbose `ok: [host] => {"facts": {…}}` per-host JSON dumps (with
//!     `-v`/`-vv`/`-vvv` flags these can be MB-sized per host)
//!   - the row of `*` separators that Ansible prints after every header
//!   - retry-file hints and other interstitial chatter
//!
//! Real-world ratio: ≥90 % on `-v`+ runs, ≥70 % on default verbosity.

use std::fmt::Write;

pub const FAILURES_CAP: usize = 100;
pub const PLAYS_CAP: usize = 50;
pub const TASKS_CAP_PER_PLAY: usize = 100;
pub const HOSTS_CAP: usize = 500;
/// Failure line truncation cap (Ansible failure messages can include
/// stdout_lines/stderr_lines arrays with kilobytes of payload).
pub const FAILURE_LINE_CAP: usize = 400;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);

    // Strong signal: the PLAY RECAP banner.
    if head.contains("PLAY RECAP") {
        return true;
    }

    let has_play = head.contains("\nPLAY [") || head.starts_with("PLAY [");
    let has_task = head.contains("\nTASK [") || head.starts_with("TASK [");
    if has_play && has_task {
        return true;
    }

    // Recap-row pattern: `host : ok=N changed=N failed=N …`. Real PLAY RECAP
    // pads the host name with spaces before the colon.
    let recap_rows = head
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.contains("ok=") && t.contains("changed=") && t.contains("failed=")
        })
        .count();
    recap_rows >= 2
}

#[derive(Default)]
struct TaskSummary {
    name: String,
    ok: usize,
    changed: usize,
    skipping: usize,
    failed: usize,
    unreachable: usize,
}

impl TaskSummary {
    fn is_silent(&self) -> bool {
        self.ok == 0
            && self.changed == 0
            && self.skipping == 0
            && self.failed == 0
            && self.unreachable == 0
    }
    fn fmt_counts(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ok > 0 {
            parts.push(format!("ok={}", self.ok));
        }
        if self.changed > 0 {
            parts.push(format!("changed={}", self.changed));
        }
        if self.skipping > 0 {
            parts.push(format!("skipping={}", self.skipping));
        }
        if self.failed > 0 {
            parts.push(format!("FAILED={}", self.failed));
        }
        if self.unreachable > 0 {
            parts.push(format!("UNREACHABLE={}", self.unreachable));
        }
        if parts.is_empty() {
            "(no host outcomes)".to_string()
        } else {
            parts.join(" ")
        }
    }
}

#[derive(Default)]
struct PlaySummary {
    name: String,
    tasks: Vec<TaskSummary>,
}

#[derive(Default, Clone)]
struct HostRecap {
    host: String,
    line: String, // raw "ok=N changed=N …" segment
}

#[derive(Default)]
struct Failure {
    task: String,
    host: String,
    line: String,
}

/// Extract content between `[` and `]` on a line that starts with `prefix`.
/// `PLAY [Deploy webservers] ****` → `Some("Deploy webservers")`.
fn extract_bracketed(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let open = rest.find('[')?;
    let close = rest[open + 1..].rfind(']')?;
    Some(rest[open + 1..open + 1 + close].to_string())
}

/// Extract the host name from lines like `ok: [web-1]`, `changed: [web-1]`,
/// `fatal: [db-1]: FAILED!`, etc. Returns the host part without brackets.
fn extract_host_after_status(line: &str, status: &str) -> Option<String> {
    // Status prefixes are always at column 0 in normal Ansible output.
    let after = line.strip_prefix(status)?;
    let open = after.find('[')?;
    let close = after[open + 1..].find(']')?;
    Some(after[open + 1..open + 1 + close].to_string())
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // L42 — strip ANSI. ansible-playbook outputs are heavily
    // colourised (green=ok, yellow=changed, red=failed). Stripping
    // upfront keeps the task-line parser working on real-world
    // captures from CI.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    let mut plays: Vec<PlaySummary> = Vec::new();
    let mut current_play: Option<PlaySummary> = None;
    let mut current_task: Option<TaskSummary> = None;
    let mut failures: Vec<Failure> = Vec::new();
    let mut recap_rows: Vec<HostRecap> = Vec::new();
    let mut in_recap = false;
    // Flush helpers.
    let flush_task = |current_task: &mut Option<TaskSummary>,
                      current_play: &mut Option<PlaySummary>| {
        if let Some(t) = current_task.take() {
            if let Some(p) = current_play.as_mut() {
                if p.tasks.len() < TASKS_CAP_PER_PLAY {
                    p.tasks.push(t);
                }
            }
        }
    };

    for raw_line in text.lines() {
        let line = raw_line.trim_end();

        if in_recap {
            // Recap rows: `host : ok=N changed=N …`.
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Some((host_raw, stats_raw)) = t.split_once(" :") {
                let host = host_raw.trim().to_string();
                let stats = stats_raw.trim().to_string();
                if !host.is_empty() && stats.contains("ok=") {
                    recap_rows.push(HostRecap { host, line: stats });
                }
            }
            continue;
        }

        // PLAY RECAP banner — switch into recap mode.
        if line.starts_with("PLAY RECAP") {
            flush_task(&mut current_task, &mut current_play);
            if let Some(p) = current_play.take() {
                plays.push(p);
            }
            in_recap = true;
            continue;
        }

        // PLAY [name] banner.
        if line.starts_with("PLAY [") {
            flush_task(&mut current_task, &mut current_play);
            if let Some(p) = current_play.take() {
                plays.push(p);
            }
            if let Some(name) = extract_bracketed(line, "PLAY ") {
                current_play = Some(PlaySummary {
                    name,
                    tasks: Vec::new(),
                });
            }
            continue;
        }

        // TASK [name] banner. Some Ansible versions emit `RUNNING HANDLER [name]`
        // for handler executions — treat them as tasks for our purposes.
        if line.starts_with("TASK [") || line.starts_with("RUNNING HANDLER [") {
            flush_task(&mut current_task, &mut current_play);
            let name = extract_bracketed(line, "TASK ")
                .or_else(|| extract_bracketed(line, "RUNNING HANDLER "))
                .unwrap_or_default();
            // Ensure there is a current play (some plays-less invocations exist).
            if current_play.is_none() {
                current_play = Some(PlaySummary {
                    name: "(no play)".to_string(),
                    tasks: Vec::new(),
                });
            }
            current_task = Some(TaskSummary {
                name,
                ..TaskSummary::default()
            });
            continue;
        }

        // Per-host outcome lines — these are very common and dominate byte
        // count with -v/-vv/-vvv.
        let outcome = if line.starts_with("ok: [") {
            Some("ok")
        } else if line.starts_with("changed: [") {
            Some("changed")
        } else if line.starts_with("skipping: [") {
            Some("skipping")
        } else if line.starts_with("fatal: [") {
            Some("fatal")
        } else if line.starts_with("unreachable: [") {
            Some("unreachable")
        } else if line.starts_with("failed: [") {
            // Less common spelling; treat as fatal.
            Some("fatal")
        } else {
            None
        };

        if let Some(outcome) = outcome {
            let status_prefix = match outcome {
                "ok" => "ok: ",
                "changed" => "changed: ",
                "skipping" => "skipping: ",
                "fatal" => {
                    if line.starts_with("failed: [") {
                        "failed: "
                    } else {
                        "fatal: "
                    }
                }
                "unreachable" => "unreachable: ",
                _ => unreachable!(),
            };
            let host = extract_host_after_status(line, status_prefix).unwrap_or_default();

            if let Some(task) = current_task.as_mut() {
                match outcome {
                    "ok" => task.ok += 1,
                    "changed" => task.changed += 1,
                    "skipping" => task.skipping += 1,
                    "fatal" => task.failed += 1,
                    "unreachable" => task.unreachable += 1,
                    _ => {}
                }
            }

            if outcome == "fatal" || outcome == "unreachable" {
                let task_name = current_task
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                let mut snipped = line.to_string();
                if snipped.len() > FAILURE_LINE_CAP {
                    // Truncate at a char boundary so we never split a UTF-8
                    // code point (Ansible facts JSON can contain Unicode
                    // hostnames, error messages in non-ASCII locales, etc.).
                    let cap = (0..=FAILURE_LINE_CAP)
                        .rev()
                        .find(|i| snipped.is_char_boundary(*i))
                        .unwrap_or(0);
                    snipped.truncate(cap);
                    snipped.push('…');
                }
                if failures.len() < FAILURES_CAP {
                    failures.push(Failure {
                        task: task_name,
                        host,
                        line: snipped,
                    });
                }
            }
            continue;
        }

        // All other lines (`*` separator banners, `=> {…}` continuations,
        // retry-file hints, blank lines) are intentionally ignored.
    }

    // Final flush.
    flush_task(&mut current_task, &mut current_play);
    if let Some(p) = current_play.take() {
        plays.push(p);
    }

    // ---- Aggregate counters for the header ----
    let total_plays = plays.len();
    let total_tasks: usize = plays.iter().map(|p| p.tasks.len()).sum();
    let total_failed: usize = recap_rows
        .iter()
        .map(|r| parse_stat_field(&r.line, "failed=").unwrap_or(0))
        .sum::<usize>()
        + recap_rows
            .iter()
            .map(|r| parse_stat_field(&r.line, "unreachable=").unwrap_or(0))
            .sum::<usize>();
    let total_changed: usize = recap_rows
        .iter()
        .map(|r| parse_stat_field(&r.line, "changed=").unwrap_or(0))
        .sum();
    let total_hosts = recap_rows.len();

    // ---- Render ----
    // A rebuild that parsed NOTHING has not compacted, it has invented. This
    // compactor writes its counters unconditionally, so a file that merely
    // MENTIONS `PLAY RECAP` came back as
    //
    //     0 play(s), 0 task(s), 0 host(s), 0 failed, 0 changed
    //
    // — 52 bytes of a run that never happened, in place of the file. Measured
    // 2026-08-26: `token-saver-ppgranger/tests/failure_fixtures.py` (17 428
    // bytes, the string sits in a Python list) and `tokenslim/…/
    // ansible_plugin/methods.rs` (9 525 bytes, it is that plugin's own
    // `starts_with("PLAY RECAP")`) were both delivered as that one line.
    //
    // Worse than the empty string the emptiness floor catches: an empty output is
    // visibly nothing, this one reads like a fact.
    if plays.is_empty() && recap_rows.is_empty() && failures.is_empty() {
        crate::debug_log::log(
            "ansible",
            "nothing_parsed",
            serde_json::json!({ "bytes_in": text.len() }),
        );
        return text.to_string();
    }
    let mut out = String::with_capacity(2048);
    // Top summary — restored after the L122 banner-strip pass left the
    // totals computed but unused. Format mirrors other compactors
    // (cargobench, journal, accesslog): plain bullet list, no
    // `[tokenade:xxx]` prefix.
    let _ = writeln!(
        out,
        "{total_plays} play(s), {total_tasks} task(s), {total_hosts} host(s), {total_failed} failed, {total_changed} changed"
    );

    if !plays.is_empty() {
        out.push_str("\nplays:\n");
        for play in plays.iter().take(PLAYS_CAP) {
            let _ = writeln!(out, "  PLAY [{}] ({} tasks)", play.name, play.tasks.len());
            for t in &play.tasks {
                if t.is_silent() {
                    let _ = writeln!(out, "    {}", t.name);
                } else {
                    let _ = writeln!(out, "    {:<40} {}", t.name, t.fmt_counts());
                }
            }
        }
        if plays.len() > PLAYS_CAP {
            let _ = writeln!(out, "  … {} more play(s) elided", plays.len() - PLAYS_CAP);
        }
    }

    if !failures.is_empty() {
        let _ = write!(out, "\nfailures ({}):\n", failures.len());
        for f in &failures {
            let _ = write!(out, "  TASK [{}] on {}\n    {}\n", f.task, f.host, f.line);
        }
    }

    if !recap_rows.is_empty() {
        let _ = write!(out, "\nPLAY RECAP ({} hosts):\n", recap_rows.len());
        // Stable order: as they appeared in the input.
        // The cap is on CLEAN hosts: one that failed or was unreachable past
        // the 500th row is the line the recap exists for.
        let troubled = |r: &&HostRecap| {
            ["failed=", "unreachable="]
                .iter()
                .any(|k| parse_stat_field(&r.line, k).unwrap_or(0) > 0)
        };
        let mut clean_shown = 0usize;
        let mut elided = 0usize;
        for r in &recap_rows {
            if troubled(&r) || clean_shown < HOSTS_CAP {
                clean_shown += usize::from(!troubled(&r));
                let _ = writeln!(out, "  {:<30} {}", r.host, r.line);
            } else {
                elided += 1;
            }
        }
        if elided > 0 {
            let _ = writeln!(
                out,
                "  … {elided} more host(s) elided, all with failed=0 unreachable=0"
            );
        }
    }

    out.trim_end().to_string()
}

/// Parse `failed=12` style fragment out of a longer string. Returns `None`
/// if the field isn't present or its value isn't an unsigned integer.
fn parse_stat_field(s: &str, key: &str) -> Option<usize> {
    let idx = s.find(key)?;
    let after = &s[idx + key.len()..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<usize>().ok()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
PLAY [Deploy webservers] ****

TASK [Gathering Facts] ****
ok: [web-1]
ok: [web-2]

TASK [Install nginx] ****
changed: [web-1] => {"changed": true, "rc": 0, "stdout_lines": ["Reading package lists...", "Building dependency tree...", "...50 more lines..."]}
changed: [web-2] => {"changed": true, "rc": 0, "stdout_lines": ["Reading package lists...", "..."]}

TASK [Copy nginx config] ****
ok: [web-1] => (item=default)
changed: [web-2] => (item=default)

TASK [Restart nginx service] ****
changed: [web-1]
changed: [web-2]

PLAY [Configure databases] ****

TASK [Gathering Facts] ****
ok: [db-1]

TASK [Apply database migration] ****
fatal: [db-1]: FAILED! => {"changed": false, "msg": "Database is not reachable", "rc": 1}
...to retry, use: --limit @/tmp/playbook.retry

PLAY RECAP ****
db-1                       : ok=2    changed=0    unreachable=0    failed=1    skipped=0    rescued=0    ignored=0
web-1                      : ok=4    changed=3    unreachable=0    failed=0    skipped=0    rescued=0    ignored=0
web-2                      : ok=4    changed=4    unreachable=0    failed=0    skipped=0    rescued=0    ignored=0
"#;

    #[test]
    fn a_failed_host_past_the_cap_stays_in_the_recap() {
        let mut s =
            String::from("PLAY [all] ****\n\nTASK [ping] ****\nok: [h0]\n\nPLAY RECAP *********\n");
        for i in 0..(HOSTS_CAP + 30) {
            let (f, u) = match i {
                510 => (1, 0),
                520 => (0, 1),
                _ => (0, 0),
            };
            let _ = writeln!(s, "web{i:04} : ok=3 changed=0 unreachable={u} failed={f} skipped=0 rescued=0 ignored=0");
        }
        let out = compact(&s);
        assert!(out.contains("web0510") && out.contains("web0520"), "{out}");
        assert!(
            out.contains("web0499") && !out.contains("web0500 "),
            "{out}"
        );
        assert!(out.contains("… 28 more host(s) elided"), "{out}");
        // The headline counts every host, not the first 500 parsed.
        assert!(out.contains("530 host(s), 2 failed"), "{out}");
    }
    #[test]
    fn detects_full_playbook() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_recap_only() {
        let s = "PLAY RECAP\nhost-a : ok=5 changed=2 failed=0\nhost-b : ok=5 changed=0 failed=1\n";
        assert!(detect(s));
    }

    #[test]
    fn rejects_rust_code() {
        assert!(!detect("fn play() { let task = 1; }\n"));
    }

    #[test]
    fn rejects_generic_logs() {
        let s = "[2024-01-15 10:30:00] INFO server started\n[2024-01-15 10:30:01] INFO listening on 8080\n";
        assert!(!detect(s));
    }

    #[test]
    fn extracts_plays_and_tasks() {
        let out = compact(SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("PLAY [Deploy webservers]"));
        assert!(out.contains("PLAY [Configure databases]"));
        assert!(out.contains("Gathering Facts"));
        assert!(out.contains("Install nginx"));
        // Outcome counts.
        assert!(out.contains("ok=2"), "install nginx outcomes: {out}");
    }

    #[test]
    fn extracts_failure_verbatim() {
        let out = compact(SAMPLE);
        assert!(out.contains("failures (1):"));
        assert!(out.contains("TASK [Apply database migration] on db-1"));
        assert!(out.contains("Database is not reachable"));
        // The verbose `changed: => {...stdout_lines...}` JSON dumps must NOT leak.
        assert!(!out.contains("stdout_lines"));
        assert!(!out.contains("Reading package lists"));
    }

    #[test]
    fn extracts_play_recap() {
        let out = compact(SAMPLE);
        assert!(out.contains("PLAY RECAP (3 hosts):"));
        assert!(out.contains("db-1"));
        assert!(out.contains("web-1"));
        assert!(out.contains("web-2"));
        // The retry hint must NOT leak.
        assert!(!out.contains("--limit @/tmp/playbook.retry"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is unrelated";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_verbose_run() {
        // Realistic -v output: each `ok:` line has a 2 KB facts JSON dump.
        let facts_dump = format!("{{\"facts\": \"{}\"}}", "x".repeat(2000));
        let mut s = String::from("PLAY [Deploy] ****\n\nTASK [Gathering Facts] ****\n");
        for i in 0..20 {
            let _ = writeln!(s, "ok: [host-{i}] => {facts_dump}");
        }
        s.push_str("\nPLAY RECAP ****\n");
        for i in 0..20 {
            let _ = writeln!(
                s,
                "host-{i:<5} : ok=1 changed=0 unreachable=0 failed=0 skipped=0 rescued=0 ignored=0"
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("20 hosts"));
        assert!(sav.percent() > 88.0, "got {sav:?}");
    }

    #[test]
    fn parse_stat_field_basic() {
        assert_eq!(
            parse_stat_field("ok=2 changed=3 failed=1", "changed="),
            Some(3)
        );
        assert_eq!(
            parse_stat_field("ok=5    changed=0    failed=0", "failed="),
            Some(0)
        );
        assert_eq!(parse_stat_field("nothing here", "ok="), None);
    }
}

#[cfg(test)]
mod a_run_that_never_happened {
    //! This compactor writes its counters unconditionally, so a file that
    //! merely MENTIONS `PLAY RECAP` came back as a five-figure summary of a
    //! run that never happened — 52 bytes in place of the file.

    const PY_FIXTURE: &str = "import pytest\n\nCASES = [\n    [\"PLAY RECAP *********************************************************\"],\n    [\"host | FAILED\"],\n]\n\ndef test_cases():\n    assert CASES\n";

    #[test]
    fn a_source_file_quoting_the_banner_is_left_alone() {
        assert_eq!(super::compact(PY_FIXTURE), PY_FIXTURE);
    }

    #[test]
    fn the_invented_summary_is_gone() {
        assert!(
            !super::compact(PY_FIXTURE).contains("0 play(s)"),
            "a run that never happened must not be reported"
        );
    }

    #[test]
    fn a_real_playbook_run_still_compacts() {
        let run = "PLAY [webservers] **************************************************\n\nTASK [Gathering Facts] *********************************************\nok: [web1]\n\nTASK [install nginx] ***********************************************\nchanged: [web1]\n\nPLAY RECAP *********************************************************\nweb1                       : ok=2    changed=1    unreachable=0    failed=0\n";
        let out = super::compact(run);
        assert!(out.len() < run.len(), "real output must still fold");
        assert!(out.contains("1 play(s)"), "{out}");
    }
}
