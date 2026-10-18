//! `netstat` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_netstat(n_listen: usize, n_established: usize, n_time_wait: usize) -> String {
    let mut s = String::from(
        "Active Internet connections (servers and established)\nProto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program\n"
    );
    for p in 0..n_listen {
        s.push_str(&format!(
            "tcp        0      0 0.0.0.0:{}\t\t0.0.0.0:*               LISTEN      {}/svc-{p}\n",
            8000 + p,
            1000 + p
        ));
    }
    for i in 0..n_established {
        s.push_str(&format!(
            "tcp        0      0 192.168.1.1:80          10.0.{}.{}:{}        ESTABLISHED 5678/nginx\n",
            (i / 256) % 256,
            i % 256,
            30000 + i % 30000
        ));
    }
    for i in 0..n_time_wait {
        s.push_str(&format!(
            "tcp        0      0 192.168.1.1:80          10.0.99.{}:{}         TIME_WAIT   -\n",
            i % 256,
            40000 + i,
        ));
    }
    s
}

pub(crate) fn task_netstat_real(results: &mut Vec<RunResult>) {
    // Realistic busy-server: 30 LISTEN + 500 ESTABLISHED + 50 TIME_WAIT.
    let raw = synth_netstat(30, 500, 50);
    let (out, sav, ms) = measure(&raw, netstat::compact_with_savings);
    let quality = if out.contains("[tokenade:netstat] 580 connection(s)")
        && out.contains("30 LISTEN")
        && out.contains("500 ESTABLISHED")
        && out.contains("state distribution:")
        && out.contains("listening ports (30):")
        // All LISTEN ports preserved.
        && out.contains("0.0.0.0:8000")
        && out.contains("0.0.0.0:8029")
        // Process aggregation.
        && out.contains("5678/nginx")
        && out.contains("top processes")
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:netstat-compact",
        "netstat",
        "synth-30L-500E-50TW",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #79: 100 000-connection busy-server netstat output. Must
// compact >97 % in <500 ms with all LISTEN preserved.
pub(crate) fn adv_netstat_huge_server(results: &mut Vec<RunResult>) {
    let raw = synth_netstat(50, 100_000, 5_000);
    let (out, sav, ms) = measure(&raw, netstat::compact_with_savings);
    let quality = if out.contains("105050 connection(s)")
        && out.contains("50 LISTEN")
        && out.contains("100000 ESTABLISHED")
        && sav.percent() > 97.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:netstat:huge-105k",
        "netstat",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #80: hostile mix of formats + edge cases:
//   - mixed netstat + ss + lsof output (3 different shapes in same input)
//   - IPv6 addresses (`:::22`, `2001:db8::1:443`, `fe80::1%eth0:8080`)
//   - Unicode in process name (lsof can show command names with Unicode)
//   - dash-only state column (-) for some netstat variants
//   - extra blank lines + footer banners
pub(crate) fn adv_netstat_hostile(results: &mut Vec<RunResult>) {
    let raw = "Active Internet connections (servers and established)\n\
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program\n\
tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd\n\
tcp        0      0 0.0.0.0:80              0.0.0.0:*               LISTEN      5678/nginx\n\
tcp6       0      0 :::443                  :::*                    LISTEN      5678/nginx\n\
tcp6       0      0 2001:db8::1:9090        2001:db8::ff:55555      ESTABLISHED 9999/café-app\n\
tcp6       0      0 fe80::1%eth0:8080       fe80::99%eth0:33333     ESTABLISHED 9999/café-app\n\
udp        0      0 0.0.0.0:53              0.0.0.0:*                           1234/dnsmasq\n\
\n\
State    Recv-Q Send-Q Local Address:Port    Peer Address:Port   Process\n\
LISTEN   0      128    0.0.0.0:8080          0.0.0.0:*           users:((\"中文-svc\",pid=7777,fd=3))\n\
ESTAB    0      0      192.168.1.1:8080      10.0.0.5:34567      users:((\"中文-svc\",pid=7777,fd=12))\n\
TIME-WAIT 0     0      192.168.1.1:8080      10.0.0.99:65432\n";
    let (out, _sav, ms) = measure(raw, netstat::compact_with_savings);
    let quality = if out.contains("[tokenade:netstat]")
        // LISTEN entries from BOTH netstat and ss formats present.
        && out.contains("0.0.0.0:22")
        && out.contains("0.0.0.0:80")
        && out.contains(":::443")
        && out.contains("0.0.0.0:8080")
        // IPv6 ESTABLISHED counted (but not individually listed).
        && out.contains("ESTAB")
        // Unicode in process names preserved.
        && out.contains("café-app")
        && out.contains("中文-svc")
        // TIME-WAIT or TIME_WAIT detected (normalised forms differ).
        && (out.contains("TIME_WAIT") || out.contains("TIME-WAIT"))
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:netstat:hostile",
        "netstat",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
