//! `journal` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Realistic 5 000-line journalctl across 6 services with heavy repetition
/// + a sprinkling of ERROR/WARN entries.
pub(crate) fn task_journal_real(results: &mut Vec<RunResult>) {
    let services = [
        "systemd[1]",
        "sshd[1234]",
        "nginx[5678]",
        "dockerd[999]",
        "kernel",
        "kubelet[4321]",
    ];
    let mut t = String::new();
    for i in 0..5000 {
        let svc = services[i % services.len()];
        t.push_str(&format!(
            "Jan 01 {:02}:{:02}:{:02} prod-srv-01 {svc}: handling request id={i}\n",
            (i / 3600) % 24,
            (i / 60) % 60,
            i % 60
        ));
    }
    // Mixed errors + warnings.
    for i in 0..20 {
        t.push_str(&format!(
            "Jan 01 23:59:{:02} prod-srv-01 kernel: ERROR: oops at 0x{i:08x}\n",
            i % 60
        ));
    }
    for _ in 0..50 {
        t.push_str("Jan 01 23:59:59 prod-srv-01 sshd[1234]: WARN: dropped excessive connection\n");
    }
    let (out, sav, ms) = measure(&t, journal::compact_with_savings);
    let quality = if out.contains("5070 entry(ies)")
        && out.contains("** ERROR-level entries")
        && out.contains("oops at 0x")
        && out.contains("[systemd[1]]")
        && out.contains("[kernel]")
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:journal-compact",
        "journal",
        "journalctl-5070",
        &t,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #43: dmesg with 100 000 entries (kernel ring buffer dump shape)
// must compact > 95 % in < 1.5 s.
pub(crate) fn adv_journal_dmesg_huge(results: &mut Vec<RunResult>) {
    let mut t = String::with_capacity(100_000 * 60);
    for i in 0..100_000 {
        t.push_str(&format!("[{:>5}.{i:06}] usb 1-1: status 0\n", i / 1000));
    }
    t.push_str("[99999.999999] CRITICAL: unique event you must keep\n");
    let (out, sav, ms) = measure(&t, journal::compact_with_savings);
    let quality = if out.contains("CRITICAL: unique event") && sav.percent() > 95.0 && ms < 1500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:journal:dmesg-100k",
        "journal",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #44: mixed format input — half journal lines, half garbage.
// Parser must extract just the journal lines, not crash, not corrupt them.
pub(crate) fn adv_journal_mixed_garbage(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    for i in 0..50 {
        t.push_str(&format!(
            "Jan 01 12:34:{:02} host nginx[789]: access {i}\n",
            i % 60
        ));
    }
    t.push_str(
        "\x00random binary blob\nnot a journal line\n[bad timestamp\nJan 99 99:99:99 garbage\n",
    );
    let (out, _sav, ms) = measure(&t, journal::compact_with_savings);
    let quality = if out.contains("50 entry(ies)")
        && out.contains("parse-skips")
        && out.contains("[nginx[789]]")
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:journal:mixed-garbage",
        "journal",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}
