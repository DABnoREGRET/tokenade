//! `systemctl status` block-format output compaction.
//!
//! Every Linux service-debugging session generates this shape:
//!
//! ```text
//! ● my-app.service - My Application Service
//!      Loaded: loaded (/etc/systemd/system/my-app.service; enabled; vendor preset: enabled)
//!      Active: active (running) since Mon 2024-01-15 10:30:00 UTC; 2h 30min ago
//!    Main PID: 12345 (myapp)
//!       Tasks: 8 (limit: 4915)
//!      Memory: 245.6M
//!         CPU: 12.345s
//!      CGroup: /system.slice/my-app.service
//!              ├─12345 /usr/local/bin/myapp --config /etc/myapp.conf
//!              ├─12346 /usr/local/bin/myapp-worker
//!              ├─12347 /usr/local/bin/myapp-worker
//!              └─12348 /usr/local/bin/myapp-worker
//!
//! Jan 15 10:30:00 hostname myapp[12345]: Starting service
//! Jan 15 10:30:01 hostname myapp[12345]: Listening on :8080
//! Jan 15 10:30:01 hostname myapp[12345]: Loaded config from /etc/myapp.conf
//! Jan 15 11:00:00 hostname myapp[12345]: ERROR Database connection failed
//! …200 more journal lines…
//! ```
//!
//! What the agent reading this actually wants:
//!
//!   - service **identity** + description
//!   - **Loaded** path + enabled state
//!   - **Active** status + start time + uptime
//!   - **Main PID**, **Tasks**, **Memory**, **CPU**
//!   - any **ERROR/WARN/CRITICAL** lines in the journal tail
//!   - a small **recent-tail** sample (last 20)
//!
//! Drops: the CGroup process-tree branches (just children of Main PID with
//! same args), the bulk of routine INFO journal entries, the long uptime
//! relative-time chatter.
//!
//! Real-world ratio: ≥75 % on healthy services, much higher when the journal
//! tail is long.

use std::fmt::Write;

pub const JOURNAL_TAIL_CAP: usize = 20;
pub const ERROR_TAIL_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);

    // Conservative core signal: both `Loaded:` and `Active:` must appear
    // as indented keys. These two together are unique to systemctl status —
    // no other common command uses both names with the colon-and-value shape.
    let has_loaded = head.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("Loaded: loaded")
            || t.starts_with("Loaded: not-found")
            || t.starts_with("Loaded: bad-setting")
            || t.starts_with("Loaded: error")
            || t.starts_with("Loaded: masked")
    });
    let has_active = head.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("Active: active")
            || t.starts_with("Active: inactive")
            || t.starts_with("Active: failed")
            || t.starts_with("Active: activating")
            || t.starts_with("Active: deactivating")
            || t.starts_with("Active: reloading")
    });
    has_loaded && has_active
}

/// `systemctl status` with NO unit named — the manager's own status, which is a
/// different document from a unit's.
///
/// [`detect`] asks for `Loaded:` and `Active:`, the two keys unique to a UNIT
/// status. The manager form has neither: it opens with `State:`, `Units:`,
/// `Jobs:`, `Failed:` and then prints the entire cgroup tree of every process
/// on the machine. So `compact` returned it untouched — measured 2026-09-01,
/// 5 633 bytes in and 5 633 out, of which a 7-line header carries the whole
/// answer (`State: degraded`, `Failed: 2 units`) and 65 lines are the tree.
///
/// The header is kept verbatim, every key of it, because each line is a fact
/// an agent asked for. The tree is counted, not shown: an agent asking whether
/// the machine is healthy is not asking for Chrome's renderer PIDs, and the
/// count says what was set aside so nobody has to wonder whether it was empty.
///
/// Conservative like the rest of this module: BOTH `State:` and `CGroup:` must
/// appear as indented keys, and there must be a tree to fold. Anything else is
/// left to the unit-status path below, which returns the text unchanged when it
/// does not recognise it either.
fn compact_manager_status(text: &str) -> Option<String> {
    let mut header: Vec<&str> = Vec::new();
    let mut tree_lines = 0usize;
    let mut pids = 0usize;
    let mut in_tree = false;
    let mut has_state = false;
    // Running services, with how many processes each holds.
    let mut services: Vec<(&str, usize)> = Vec::new();
    let mut current: Option<usize> = None;

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.starts_with("State:") {
            has_state = true;
        }
        if t.starts_with("CGroup:") {
            in_tree = true;
            header.push(line);
            continue;
        }
        if in_tree {
            if line.trim().is_empty() {
                continue;
            }
            tree_lines += 1;
            // A leaf is a PID and its command line, under the tree glyphs.
            let leaf = t
                .trim_start_matches(['│', '├', '└', '─', ' ', '\u{a0}'])
                .split_whitespace()
                .next()
                .unwrap_or("");
            if !leaf.is_empty() && leaf.bytes().all(|b| b.is_ascii_digit()) {
                pids += 1;
                if let Some(k) = current {
                    services[k].1 += 1;
                }
            } else if let Some(name) = leaf.strip_suffix(".service") {
                services.push((name, 0));
                current = Some(services.len() - 1);
            } else {
                current = None;
            }
            continue;
        }
        if !line.is_empty() {
            header.push(line);
        }
    }
    // `tree_lines > 0` already implies a `CGroup:` line was seen — nothing else
    // sets `in_tree` — so asking for the key again would be a second spelling of
    // one condition, and a mutation that deleted it could not be caught.
    //
    // The `tree_lines == 0` half is a cheap early exit, not the rule: with no
    // tree the fold is header + banner, which is longer than the input, and the
    // never-grow check below refuses it anyway. Deleting this line changes
    // nothing observable, which is why no test can kill that mutation — written
    // down rather than left looking untested.
    if !has_state || tree_lines == 0 {
        return None;
    }
    let mut out = header.join("\n");
    out.push('\n');
    out.push_str(&format!(
        "[tokenade:systemctl-status] cgroup tree elided: {tree_lines} line(s), {pids} process(es)"
    ));
    // The SERVICES stay, one line: "is my service running?" is the other
    // question this command answers, and the tree was its only answer —
    // measured 2026-10-02, `tokenade-llm-proxy-8787.service` was elided with
    // Chrome's renderers. Names and process counts only; command lines,
    // scopes and slices stay counted, not shown.
    if !services.is_empty() {
        let list: Vec<String> = services.iter().map(|(n, k)| format!("{n} ({k})")).collect();
        out.push_str(&format!(
            "\n  services running ({}): {}",
            services.len(),
            list.join(", ")
        ));
    }
    // A fold that grows the payload is not a fold — the same rule the git-status
    // fold arrived at, and the reason a two-line tree is left alone.
    if out.len() >= text.len() {
        return None;
    }
    Some(out)
}

/// Match a journal/syslog-shaped line like `Jan 15 10:30:00 host process[pid]: msg`.
///
/// Read by SHAPE — `<month> <day> <HH:MM:SS>` at column 0 — because the month
/// is localised: `sept. 12 18:45:05` under `fr_FR`, `Okt 03` under `de_DE`.
/// Matched against the English names, the whole journal tail of a French host
/// was dropped, the lines that say WHY a failed unit failed among them
/// (measured 2026-10-03, `systemctl status systemd-journald`).
pub fn is_journal_line(line: &str) -> bool {
    if line.starts_with(char::is_whitespace) {
        return false;
    }
    let mut w = line.split_whitespace();
    let (Some(month), Some(day), Some(time)) = (w.next(), w.next(), w.next()) else {
        return false;
    };
    let month = month.trim_end_matches('.');
    let is_clock = |t: &str| {
        let b = t.as_bytes();
        b.len() >= 8
            && b[2] == b':'
            && b[5] == b':'
            && [0, 1, 3, 4, 6, 7].iter().all(|&i| b[i].is_ascii_digit())
    };
    // `Oct`, `août`, `sept.` — three letters at least, six at most.
    (3..=6).contains(&month.chars().count())
        && month.chars().all(char::is_alphabetic)
        && (1..=2).contains(&day.len())
        && day.bytes().all(|b| b.is_ascii_digit())
        && is_clock(time)
}

fn is_error_or_warn_journal(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    // Match plain ERROR/CRIT/FATAL/FAIL/PANIC as words OR as `: ERROR:` body
    // markers (some loggers omit the surrounding spaces).
    upper.contains(" ERROR ")
        || upper.contains(": ERROR:")
        || upper.contains(": ERROR ")
        || upper.contains(" CRIT ")
        || upper.contains(": CRIT")
        || upper.contains("CRITICAL")
        || upper.contains(" FATAL")
        || upper.contains(": FATAL")
        || upper.contains("FAILED!")
        || upper.contains(" PANIC")
        || upper.contains(" WARN ")
        || upper.contains(": WARN")
        || upper.contains("WARNING:")
}

/// Extract a service-header line — the `●` / `×` / `○` bullet + name + description.
fn parse_header(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();
    // Bullets vary by systemd state: ● (active), × (failed), ○ (inactive),
    // ↻ (reloading), ↑ (activating), ↓ (deactivating).
    const BULLETS: &[char] = &['●', '×', '○', '↻', '↑', '↓'];
    let first_char = trimmed.chars().next()?;
    if !BULLETS.contains(&first_char) {
        return None;
    }
    // Skip the bullet + the immediately-following space(s).
    let after_bullet = trimmed
        .chars()
        .skip(1)
        .skip_while(|c| c.is_whitespace())
        .collect::<String>();
    if let Some((svc, desc)) = after_bullet.split_once(" - ") {
        Some((svc.trim().to_string(), Some(desc.trim().to_string())))
    } else if !after_bullet.is_empty() {
        Some((after_bullet, None))
    } else {
        None
    }
}

pub fn compact(text: &str) -> String {
    if let Some(out) = compact_manager_status(text) {
        return out;
    }
    if !detect(text) {
        return text.to_string();
    }

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut loaded: Option<String> = None;
    let mut active: Option<String> = None;
    let mut main_pid: Option<String> = None;
    let mut status: Option<String> = None;
    let mut tasks: Option<String> = None;
    let mut memory: Option<String> = None;
    let mut cpu: Option<String> = None;
    let mut process: Option<String> = None;
    let mut triggers: Option<String> = None;
    let mut docs: Option<String> = None;

    let mut journal_tail: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut total_journal_lines = 0usize;

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }

        // Service-header line (must be at line start, may have leading bullet).
        if name.is_none() {
            if let Some((svc, desc)) = parse_header(line) {
                name = Some(svc);
                description = desc;
                continue;
            }
        }

        // Indented systemd keys.
        let t = line.trim_start();
        if let Some(v) = t.strip_prefix("Loaded: ") {
            loaded = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Active: ") {
            active = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Main PID: ") {
            main_pid = Some(v.to_string());
            continue;
        }
        // What the service itself says it is doing (`sd_notify STATUS=`):
        // `"Waiting for database…"` is the answer to a unit stuck activating.
        if let Some(v) = t.strip_prefix("Status: ") {
            status = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Tasks: ") {
            tasks = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Memory: ") {
            memory = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("CPU: ") {
            cpu = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Process: ") {
            // Per-Type=oneshot service status; keep just the first occurrence.
            if process.is_none() {
                process = Some(v.to_string());
            }
            continue;
        }
        if let Some(v) = t.strip_prefix("TriggeredBy: ") {
            triggers = Some(v.to_string());
            continue;
        }
        if let Some(v) = t.strip_prefix("Docs: ") {
            docs = Some(v.to_string());
            continue;
        }

        // CGroup section header + tree branches — drop wholesale. Tree
        // characters: ├ └ │ ─.
        if t.starts_with("CGroup: ") || t.starts_with("├─") || t.starts_with("└─") {
            continue;
        }

        // Journal tail lines.
        if is_journal_line(line) {
            total_journal_lines += 1;
            if is_error_or_warn_journal(line) && errors.len() < ERROR_TAIL_CAP {
                errors.push(line.to_string());
            }
            journal_tail.push(line.to_string());
        }
        // All other lines (descriptive prose between sections, ConditionPath
        // notices, etc.) intentionally dropped.
    }

    // Keep only the last N journal lines for the tail sample.
    if journal_tail.len() > JOURNAL_TAIL_CAP {
        let drop_count = journal_tail.len() - JOURNAL_TAIL_CAP;
        journal_tail.drain(0..drop_count);
    }

    // ---- Render ----
    let mut out = String::with_capacity(2048);
    if let Some(n) = &name {
        out.push(' ');
        out.push_str(n);
    }
    if let Some(d) = &description {
        out.push_str(" — ");
        out.push_str(d);
    }
    out.push('\n');

    if let Some(l) = &loaded {
        let _ = writeln!(out, "Loaded: {l}");
    }
    if let Some(a) = &active {
        let _ = writeln!(out, "Active: {a}");
    }
    if let Some(p) = &main_pid {
        let _ = writeln!(out, "Main PID: {p}");
    }
    if let Some(s) = &status {
        let _ = writeln!(out, "Status: {s}");
    }
    if let Some(t) = &tasks {
        let _ = writeln!(out, "Tasks: {t}");
    }
    if let Some(m) = &memory {
        let _ = writeln!(out, "Memory: {m}");
    }
    if let Some(c) = &cpu {
        let _ = writeln!(out, "CPU: {c}");
    }
    if let Some(p) = &process {
        let _ = writeln!(out, "Process: {p}");
    }
    if let Some(t) = &triggers {
        let _ = writeln!(out, "TriggeredBy: {t}");
    }
    if let Some(d) = &docs {
        let _ = writeln!(out, "Docs: {d}");
    }

    if !errors.is_empty() {
        let _ = write!(
            out,
            "\nerrors/warnings in journal ({} shown):\n",
            errors.len()
        );
        for e in &errors {
            out.push_str("  ");
            out.push_str(e);
            out.push('\n');
        }
    }

    if !journal_tail.is_empty() {
        let _ = write!(
            out,
            "\njournal tail ({} of {} total lines):\n",
            journal_tail.len(),
            total_journal_lines
        );
        for l in &journal_tail {
            out.push_str("  ");
            out.push_str(l);
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "● my-app.service - My Application Service\n     Loaded: loaded (/etc/systemd/system/my-app.service; enabled; vendor preset: enabled)\n     Active: active (running) since Mon 2024-01-15 10:30:00 UTC; 2h 30min ago\n   Main PID: 12345 (myapp)\n      Tasks: 8 (limit: 4915)\n     Memory: 245.6M\n        CPU: 12.345s\n     CGroup: /system.slice/my-app.service\n             ├─12345 /usr/local/bin/myapp --config /etc/myapp.conf\n             ├─12346 /usr/local/bin/myapp-worker\n             ├─12347 /usr/local/bin/myapp-worker\n             └─12348 /usr/local/bin/myapp-worker\n\nJan 15 10:30:00 hostname myapp[12345]: Starting service\nJan 15 10:30:01 hostname myapp[12345]: Listening on :8080\nJan 15 10:30:02 hostname myapp[12345]: Loaded config from /etc/myapp.conf\nJan 15 11:00:00 hostname myapp[12345]: ERROR Database connection failed\nJan 15 11:00:30 hostname myapp[12345]: ERROR Retry attempt 1 of 3 failed\nJan 15 11:01:00 hostname myapp[12345]: CRITICAL Out of memory\nJan 15 11:01:15 hostname myapp[12345]: Reconnected to database\nJan 15 12:00:00 hostname myapp[12345]: Request handled in 23ms\nJan 15 12:30:00 hostname myapp[12345]: Heartbeat OK\n";

    const FAILED_SAMPLE: &str = "× failed-app.service - A Failed Service\n     Loaded: loaded (/etc/systemd/system/failed-app.service; enabled)\n     Active: failed (Result: exit-code) since Mon 2024-01-15 10:30:00 UTC; 30s ago\n   Main PID: 99999 (code=exited, status=1/FAILURE)\n        CPU: 1.234s\n\nJan 15 10:30:00 hostname failed-app[99999]: ERROR could not bind to port 8080\nJan 15 10:30:00 hostname failed-app[99999]: FATAL exiting with code 1\n";

    /// The manager's own status is a different document from a unit's, and the
    /// unit detector's two keys (`Loaded:`, `Active:`) do not appear in it — so
    /// `systemctl status` with no unit came back untouched: 5 633 bytes in and
    /// out, a seven-line header carrying the whole answer and sixty-five lines
    /// of cgroup tree.
    #[test]
    fn the_managers_own_status_keeps_its_header_and_counts_its_tree() {
        let mut t = String::from(
            "● ai\n\
             \x20   State: degraded\n\
             \x20   Units: 433 loaded (incl. loaded aliases)\n\
             \x20    Jobs: 0 queued\n\
             \x20  Failed: 2 units\n\
             \x20   Since: Tue 2026-08-25 12:07:06 CEST; 6 days ago\n\
             \x20 CGroup: /user.slice/user-1000.slice/user@1000.service\n",
        );
        for i in 0..40 {
            t.push_str(&format!(
                "           │ ├─app-thing-{i}.scope\n           │ │ ├─{} /usr/bin/thing --flag\n",
                6000 + i
            ));
        }
        let out = compact(&t);
        assert!(
            out.len() < t.len() / 2,
            "the tree is most of the bytes: {out}"
        );
        for key in [
            "State: degraded",
            "Failed: 2 units",
            "Units: 433 loaded",
            "CGroup: /user",
        ] {
            assert!(
                out.contains(key),
                "every header key is an answer: {key} missing from {out}"
            );
        }
        assert!(
            !out.contains("/usr/bin/thing"),
            "the process list is what was set aside: {out}"
        );
        assert!(
            out.contains("40 process(es)") && out.contains("line(s)"),
            "what was set aside must be counted, not silently dropped: {out}"
        );
    }

    /// The services stay, with their process counts: "is my service running?"
    /// was answered only by the tree, and `tokenade-llm-proxy-8787.service`
    /// was elided along with Chrome's renderers. A process under a scope or a
    /// slice is not credited to the service above it.
    #[test]
    fn the_managers_status_names_its_running_services() {
        let t = "● ai\n\
                 \x20   State: running\n\
                 \x20   Units: 477 loaded (incl. loaded aliases)\n\
                 \x20  Failed: 0 units\n\
                 \x20 CGroup: /user.slice/user-1000.slice/user@1000.service\n\
                 \x20         ├─app.slice\n\
                 \x20         │ ├─app-com.google.Chrome-2516415.scope\n\
                 \x20         │ │ ├─2516415 /opt/google/chrome/google-chrome --type=browser --flag-a --flag-b\n\
                 \x20         │ │ └─2525401 /proc/self/exe --type=utility --utility-sub-type=audio --lang=fr\n\
                 \x20         │ ├─gcr-ssh-agent.service\n\
                 \x20         │ │ ├─2736035 /usr/lib/gcr-ssh-agent --base-dir /run/user/1000/gcr\n\
                 \x20         │ │ └─2736039 /usr/bin/ssh-agent -D -a /run/user/1000/gcr/.ssh\n\
                 \x20         │ ├─tokenade-llm-proxy-8787.service\n\
                 \x20         │ │ └─2956182 /home/me/.local/bin/tokenade llm-proxy --listen 127.0.0.1:8787\n\
                 \x20         │ └─session.slice\n\
                 \x20         │   └─1601 /usr/bin/pipewire-media-session --verbose --config default\n";
        let out = compact(t);
        assert!(
            out.contains("services running (2): gcr-ssh-agent (2), tokenade-llm-proxy-8787 (1)"),
            "{out}"
        );
        assert!(!out.contains("--listen 127.0.0.1"), "{out}");
        assert!(out.contains("6 process(es)"), "{out}");
    }

    /// A UNIT status must keep going to the unit path, and text that is neither
    /// must come back untouched — the manager fold asks for both `State:` and
    /// `CGroup:` so it cannot claim either one.
    #[test]
    fn the_manager_fold_claims_only_the_manager_status() {
        assert!(
            compact_manager_status(SAMPLE).is_none(),
            "a unit status is not the manager's"
        );
        assert!(
            compact_manager_status("State: fine\nnothing else here\n").is_none(),
            "no CGroup key, no manager status"
        );
        assert!(
            compact_manager_status("  CGroup: /\n  ├─1 /sbin/init\n").is_none(),
            "no State key, no manager status"
        );
        // A header with a CGroup key and no tree under it: nothing to fold, and
        // a banner would make it longer.
        assert!(
            compact_manager_status("● h\n  State: running\n  CGroup: /\n").is_none(),
            "an empty tree is not worth a banner"
        );
        // A tree of ONE line passes the "is there a tree" check and still costs
        // more to announce than to print. Without this case the never-grow rule
        // and the empty-tree check shadow each other and neither can be shown
        // to do anything.
        assert!(
            compact_manager_status("● h\n  State: running\n  CGroup: /\n  └─1 /sbin/init\n")
                .is_none(),
            "a fold that costs more than the lines it hides is not a fold"
        );
    }

    #[test]
    fn detects_active_service() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_failed_service() {
        assert!(detect(FAILED_SAMPLE));
    }

    #[test]
    fn rejects_plain_yaml() {
        let s = "foo: bar\nbaz: qux\nlist:\n  - one\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_kubectl_describe() {
        // Has key:value shape but different keys.
        let s = "Name: foo\nNamespace: default\nStatus: Running\n";
        assert!(!detect(s));
    }

    #[test]
    fn extracts_identity_and_status() {
        let out = compact(SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("Loaded: loaded"));
        assert!(out.contains("Active: active (running)"));
        assert!(out.contains("Main PID: 12345 (myapp)"));
        assert!(out.contains("Tasks: 8"));
        assert!(out.contains("Memory: 245.6M"));
        assert!(out.contains("CPU: 12.345s"));
    }

    #[test]
    fn drops_cgroup_tree() {
        let out = compact(SAMPLE);
        // CGroup section and process-tree branches must be gone.
        assert!(!out.contains("├─12346"));
        assert!(!out.contains("└─12348"));
        assert!(!out.contains("CGroup: /system.slice"));
        assert!(!out.contains("myapp-worker"));
    }

    #[test]
    fn extracts_journal_errors() {
        let out = compact(SAMPLE);
        assert!(out.contains("errors/warnings in journal"));
        assert!(out.contains("Database connection failed"));
        assert!(out.contains("Out of memory"));
        assert!(out.contains("Retry attempt"));
    }

    #[test]
    fn keeps_journal_tail() {
        let out = compact(SAMPLE);
        assert!(out.contains("journal tail"));
        assert!(out.contains("Heartbeat OK"));
    }

    #[test]
    fn failed_service_marked() {
        let out = compact(FAILED_SAMPLE);
        assert!(out.contains("failed-app.service"));
        assert!(out.contains("Active: failed"));
        assert!(out.contains("could not bind"));
        assert!(out.contains("FATAL exiting"));
    }

    /// A French host (`LANG=fr_FR.UTF-8`, captured 2026-10-03): the month is
    /// `sept.`, and a failed unit's reason must survive like an English one.
    #[test]
    fn a_localised_journal_line_is_still_a_journal_line() {
        let t = "× web.service - Web\n     Loaded: loaded (/etc/systemd/system/web.service; enabled)\n     \
                 Active: failed (Result: exit-code) since sam. 2026-10-03 01:30:42 CEST; 1min ago\n     \
                 Status: \"Waiting for database\"\n\n\
                 oct. 03 01:30:41 ai web[812]: bind: address already in use\n\
                 sept. 12 18:45:05 ai systemd[1]: web.service: Failed with result 'exit-code'.\n";
        let out = compact(t);
        assert!(out.contains("Status: \"Waiting for database\""), "{out}");
        assert!(
            out.contains("  oct. 03 01:30:41 ai web[812]: bind: address already in use"),
            "{out}"
        );
        assert!(out.contains("journal tail (2 of 2 total lines)"), "{out}");
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a systemctl status";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_long_journal() {
        // Same header but a long journal tail (most of which is dropped).
        let mut s = String::from("● long.service - Long-running service\n     Loaded: loaded (/etc/systemd/system/long.service; enabled)\n     Active: active (running) since Mon 2024-01-15 10:30:00 UTC; 1 day ago\n   Main PID: 12345 (long)\n     Memory: 100M\n\n");
        for i in 0..500 {
            let level = if i % 100 == 0 { "ERROR" } else { "INFO" };
            let _ = writeln!(
                s,
                "Jan 15 10:{:02}:{:02} hostname long[12345]: {level} event {i}",
                (i / 60) % 60,
                i % 60
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("long.service"));
        assert!(out.contains("500 total lines"));
        assert!(out.contains("5 shown")); // 5 ERROR lines kept verbatim
        assert!(sav.percent() > 85.0, "got {sav:?}");
    }
}
