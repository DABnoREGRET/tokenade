//! `tabular` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// 500 kubectl-get-pods rows — typical for a mid-size cluster snapshot.
pub(crate) fn task_tabular_kubectl(results: &mut Vec<RunResult>) {
    let mut t = String::from("NAME             READY   STATUS    RESTARTS   AGE\n");
    for i in 0..500 {
        t.push_str(&format!(
            "web-7d4b8c-x{i:04}   1/1     Running   {}          {}d\n",
            i % 7,
            (i % 365) + 1
        ));
    }
    let (out, sav, ms) = measure(&t, tabular::compact_with_savings);
    // Quality: names + statuses preserved; RESTARTS/AGE columns dropped.
    let signal = out.contains("web-7d4b8c-x0000")
        && out.contains("web-7d4b8c-x0499")
        && out.contains("Running");
    let noise_dropped = !out.contains("RESTARTS") && !out.contains("AGE");
    let quality = if signal && noise_dropped { 1.0 } else { 0.0 };
    record(
        results,
        "output:tabular-compact",
        "tabular",
        "kubectl-500-pods",
        &t,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #27: docker ps with very long IMAGE names + long ports column.
// Slice boundaries must not corrupt the IMAGE or NAMES values.
pub(crate) fn adv_tabular_docker_ps_wide(results: &mut Vec<RunResult>) {
    let t = "CONTAINER ID   IMAGE                                                COMMAND        CREATED         STATUS                  PORTS                                          NAMES\n\
abc12345def0   registry.example.com/myorg/very-long-image-name:v1.2.3  \"./run.sh\"     3 days ago      Up 10 minutes (healthy)  0.0.0.0:8080->8080/tcp, 0.0.0.0:8443->8443/tcp  app-server-primary\n\
fed98765aaa1   redis:7.2-alpine                                      \"redis-srv\"    1 week ago      Up 5 minutes             6379/tcp                                        cache-primary\n";
    let (out, _sav, ms) = measure(t, tabular::compact_with_savings);
    // Each piece of signal landed in the right output column.
    let image_kept = out.contains("registry.example.com/myorg/very-long-image-name:v1.2.3")
        && out.contains("redis:7.2-alpine");
    let names_kept = out.contains("app-server-primary") && out.contains("cache-primary");
    let ports_kept = out.contains("0.0.0.0:8080->8080/tcp") && out.contains("6379/tcp");
    let chrome_dropped = !out.contains("abc12345def0") && !out.contains("3 days ago");
    let quality = if image_kept && names_kept && ports_kept && chrome_dropped {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:tabular:docker-ps-wide-columns",
        "tabular",
        "adversarial",
        t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #28: an unknown header must pass through unchanged (no
// false-positive damage to non-tabular text).
pub(crate) fn adv_tabular_unknown_passthrough(results: &mut Vec<RunResult>) {
    let t = "This is an article.\nIt contains: PID, USER, COMMAND as words but isn't tabular.\nThere are no real columns.\n";
    let (out, sav, ms) = measure(t, tabular::compact_with_savings);
    // Identity → savings_pct should be 0.0 (modulo trailing newline trim).
    let identity = out.contains("This is an article") && out.contains("isn't tabular");
    let no_damage = sav.percent() <= 5.0;
    let quality = if identity && no_damage { 1.0 } else { 0.0 };
    record(
        results,
        "adv:tabular:unknown-header-passthrough",
        "tabular",
        "adversarial",
        t,
        &out,
        ms,
        quality,
    );
}
