//! `ansible` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_ansible(
    n_hosts: usize,
    n_tasks: usize,
    n_failures: usize,
    facts_bytes: usize,
) -> String {
    let facts = "x".repeat(facts_bytes);
    let mut s = String::from("\nPLAY [Deploy infrastructure] ****\n\n");
    for t in 0..n_tasks {
        s.push_str(&format!("\nTASK [task-{t}] ****\n"));
        // Most tasks pass on all hosts with verbose facts JSON dump.
        for h in 0..n_hosts {
            if t < n_failures && h == 0 {
                // First failure-eligible task fails on host-0.
                s.push_str(&format!(
                    "fatal: [host-{h}]: FAILED! => {{\"changed\": false, \"msg\": \"task-{t} failed on host-{h}\", \"rc\": 1}}\n"
                ));
            } else {
                s.push_str(&format!(
                    "ok: [host-{h}] => {{\"facts\": \"{facts}\", \"task\": \"task-{t}\"}}\n"
                ));
            }
        }
    }
    s.push_str("\nPLAY RECAP ****\n");
    for h in 0..n_hosts {
        let failed = if h == 0 { n_failures } else { 0 };
        let ok = n_tasks - failed;
        s.push_str(&format!(
            "host-{h:<5} : ok={ok} changed=0 unreachable=0 failed={failed} skipped=0 rescued=0 ignored=0\n"
        ));
    }
    s
}

pub(crate) fn task_ansible_real(results: &mut Vec<RunResult>) {
    // Realistic -v run: 5 hosts × 10 tasks × 1.5 KB facts JSON per host-task.
    let raw = synth_ansible(5, 10, 1, 1_500);
    let (out, sav, ms) = measure(&raw, ansible::compact_with_savings);
    let quality = if out.contains("[tokenade:ansible] 1 plays, 10 tasks, 1 failed")
        && out.contains("PLAY [Deploy infrastructure] (10 tasks)")
        && out.contains("task-0")
        && out.contains("task-9")
        && out.contains("FAILED=1")
        && out.contains("failures (1):")
        && out.contains("task-0 failed on host-0")
        && out.contains("PLAY RECAP (5 hosts):")
        && out.contains("host-0")
        && out.contains("host-4")
        && !out.contains("\"facts\":")
        && !out.contains("xxxxxxxx")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:ansible-compact",
        "ansible",
        "synth-5h-10t-1f",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #57: 200-host PLAY RECAP fleet roll-out — must extract every
// host and count failures correctly, ≥98% savings, <1s.
pub(crate) fn adv_ansible_huge_recap(results: &mut Vec<RunResult>) {
    let raw = synth_ansible(200, 20, 5, 2_000);
    let (out, sav, ms) = measure(&raw, ansible::compact_with_savings);
    let quality = if out.contains("PLAY RECAP (200 hosts):")
        && out.contains("host-0")
        && out.contains("host-199")
        && out.contains("failures (5):")
        && sav.percent() > 98.0
        && ms < 1000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ansible:huge-recap-200",
        "ansible",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #58: hostile content — the `ok: => {…}` JSON output dumps
// contain text mimicking `PLAY [`, `TASK [`, `PLAY RECAP` markers. These
// must NOT be parsed as section headers (they're inside `=> {…}` continuations
// or quoted strings on the SAME line, which we ignore).
//
// Also tests:
//   - failed: prefix (older Ansible spelling for fatal:)
//   - failure message exceeds FAILURE_LINE_CAP and gets truncated
//   - RUNNING HANDLER [name] is treated as a task
pub(crate) fn adv_ansible_hostile(results: &mut Vec<RunResult>) {
    let huge_msg = format!("{}…end-of-msg-which-MUST-be-truncated", "y".repeat(800));
    let raw = format!(
        r#"
PLAY [Real play] ****

TASK [Real task 1] ****
ok: [host-A] => {{"output": "PLAY [fake play] TASK [fake task] PLAY RECAP should be ignored"}}

RUNNING HANDLER [restart service] ****
changed: [host-A]

TASK [Real task 2 — old-style failure] ****
failed: [host-A]: FAILED! => {{"msg": "old-style failure spelling"}}

TASK [Real task 3 — huge failure message] ****
fatal: [host-A]: FAILED! => {{"msg": "{huge_msg}"}}

PLAY RECAP ****
host-A : ok=1 changed=1 unreachable=0 failed=2 skipped=0 rescued=0 ignored=0
"#
    );
    let (out, sav, ms) = measure(&raw, ansible::compact_with_savings);
    let quality = if out.contains("PLAY [Real play]")
        && out.contains("Real task 1")
        && out.contains("Real task 2")
        && out.contains("Real task 3")
        && out.contains("restart service")  // RUNNING HANDLER treated as task
        && out.contains("failures (2):")    // both fatal + failed counted
        && out.contains("old-style failure spelling")
        // Embedded fake markers must NOT leak as separate plays/tasks.
        && !out.contains("PLAY [fake play]")
        && !out.contains("TASK [fake task]")
        // Huge failure message truncated.
        && !out.contains("end-of-msg-which-MUST-be-truncated")
        && out.contains("…")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:ansible:hostile",
        "ansible",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}
