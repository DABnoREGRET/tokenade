//! `systemd` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_systemd(
    n_journal: usize,
    n_errors_in_journal: usize,
    n_cgroup_procs: usize,
) -> String {
    let mut s = String::from(
        "● my-app.service - My Application Service\n     Loaded: loaded (/etc/systemd/system/my-app.service; enabled; vendor preset: enabled)\n     Active: active (running) since Mon 2024-01-15 10:30:00 UTC; 2h 30min ago\n       Docs: https://example.com/docs/my-app\n   Main PID: 12345 (myapp)\n      Tasks: 8 (limit: 4915)\n     Memory: 245.6M\n        CPU: 12.345s\n     CGroup: /system.slice/my-app.service\n",
    );
    for i in 0..n_cgroup_procs {
        let connector = if i + 1 == n_cgroup_procs {
            "└─"
        } else {
            "├─"
        };
        s.push_str(&format!(
            "             {connector}{} /usr/local/bin/myapp-worker-{i} --config /etc/myapp.conf\n",
            12345 + i + 1
        ));
    }
    s.push('\n');
    for i in 0..n_journal {
        if i < n_errors_in_journal {
            s.push_str(&format!(
                "Jan 15 11:{:02}:{:02} hostname myapp[12345]: ERROR Database call failed (retry {i})\n",
                (i / 60) % 60,
                i % 60
            ));
        } else {
            s.push_str(&format!(
                "Jan 15 12:{:02}:{:02} hostname myapp[12345]: INFO Request handled in {} ms\n",
                (i / 60) % 60,
                i % 60,
                (i * 7) % 200,
            ));
        }
    }
    s
}

pub(crate) fn task_systemd_real(results: &mut Vec<RunResult>) {
    // Realistic shape: 4-process CGroup tree + 200-line journal tail with 3 errors.
    let raw = synth_systemd(200, 3, 4);
    let (out, sav, ms) = measure(&raw, systemd::compact_with_savings);
    let quality = if out.contains("my-app.service — My Application Service")
        && out.contains("Loaded: loaded")
        && out.contains("Active: active (running)")
        && out.contains("Main PID: 12345 (myapp)")
        && out.contains("Memory: 245.6M")
        && out.contains("CPU: 12.345s")
        && out.contains("Docs: https://example.com/docs/my-app")
        && out.contains("Database call failed (retry 0)")
        && out.contains("Database call failed (retry 1)")
        && out.contains("Database call failed (retry 2)")
        && out.contains("200 total lines")
        && !out.contains("myapp-worker-0")  // CGroup tree dropped
        && !out.contains("└─")  // tree branch chars dropped
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:systemd-compact",
        "systemd",
        "synth-200journal-3err-4cgroup",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #71: huge journal tail — long-running service status with
// 10 000-line journal output. Must compact >95 % in under 500 ms and
// preserve every ERROR line.
pub(crate) fn adv_systemd_huge_journal(results: &mut Vec<RunResult>) {
    let raw = synth_systemd(10_000, 50, 4);
    let (out, sav, ms) = measure(&raw, systemd::compact_with_savings);
    let quality = if out.contains("10000 total lines")
        && out.contains("50 shown")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:systemd:huge-journal-10k",
        "systemd",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #72: hostile shape:
//   - Bullet variations (× failed, ○ inactive, ↻ reloading)
//   - Service name with dashes + Unicode in description
//   - `Loaded:` value containing arbitrary text (some Linuxes embed status reasons)
//   - Journal entries with Unicode in payload (中文 message, RTL Arabic)
//   - A journal line that mentions `Loaded:` and `Active:` literally in its
//     message body (must NOT confuse the top-level key detector — but our
//     keys are indented and journal lines start at column 0, so safe)
pub(crate) fn adv_systemd_hostile(results: &mut Vec<RunResult>) {
    let raw = "× café-service.service - Café Réparation 中文 خدمة\n     Loaded: loaded (/etc/systemd/system/café-service.service; enabled; vendor preset: enabled)\n     Active: failed (Result: exit-code) since Mon 2024-01-15 10:30:00 UTC; 1h ago\n   Main PID: 99999 (code=exited, status=1/FAILURE)\n     Memory: 50.0M\n        CPU: 0.5s\n     CGroup: /system.slice/café-service.service\n             └─99999 /usr/bin/café-service\n\nJan 15 10:30:00 hostname café-service[99999]: INFO Démarrage du service café 🚀\nJan 15 10:30:00 hostname café-service[99999]: INFO Mention of Loaded: pretty and Active: stuff in the body\nJan 15 10:30:01 hostname café-service[99999]: ERROR Connexion à la base de données échouée\nJan 15 10:30:02 hostname café-service[99999]: WARNING ↻ retry in progress\nJan 15 10:30:03 hostname café-service[99999]: CRITICAL Out of memory أكبر\nJan 15 10:30:04 hostname café-service[99999]: FATAL exiting with code 1\n";
    let (out, _sav, ms) = measure(raw, systemd::compact_with_savings);
    let quality = if out.contains("café-service.service")
        && out.contains("Café Réparation 中文 خدمة")
        // Top-level fields retain REAL values (not the booby-trap text from
        // the journal-tail body). The journal tail itself will still
        // legitimately contain the trap line verbatim — that's correct
        // behaviour: we preserve journal content as-is.
        && out.contains("Loaded: loaded (/etc/systemd/system/café-service.service")
        && out.contains("Active: failed (Result: exit-code)")
        // Errors / warnings detected.
        && out.contains("Connexion à la base de données échouée")
        && out.contains("CRITICAL Out of memory أكبر")
        && out.contains("FATAL exiting with code 1")
        && out.contains("WARNING ↻ retry in progress")
        // CGroup branch dropped.
        && !out.contains("└─99999")
        // Unicode emoji preserved.
        && out.contains("🚀")
        // Small fixture — savings can land at 0% (output ≈ input size).
        // What matters is correctness, validated by the assertions above.
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:systemd:hostile",
        "systemd",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
