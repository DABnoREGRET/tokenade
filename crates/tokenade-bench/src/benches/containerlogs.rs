//! `containerlogs` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_containerlogs(
    n_streams: usize,
    lines_per_stream: usize,
    error_every: usize,
) -> String {
    let mut s = String::with_capacity(n_streams * lines_per_stream * 80);
    for line_idx in 0..(n_streams * lines_per_stream) {
        let stream_idx = line_idx % n_streams;
        let level = if error_every > 0 && line_idx % error_every == 0 {
            "ERROR"
        } else {
            "INFO"
        };
        s.push_str(&format!(
            "[pod/svc-{stream_idx}/main] 2024-01-15T10:{:02}:{:02}.{:03} {level} event {line_idx}\n",
            (line_idx / 60) % 60,
            line_idx % 60,
            line_idx % 1000,
        ));
    }
    s
}

pub(crate) fn task_containerlogs_real(results: &mut Vec<RunResult>) {
    // Realistic: 8 streams × 500 lines + 1 error every 200 lines = 4 000 lines, 20 errors.
    let raw = synth_containerlogs(8, 500, 200);
    let (out, sav, ms) = measure(&raw, containerlogs::compact_with_savings);
    let quality = if out.contains("4000 prefixed line(s) across 8 stream(s)")
        && out.contains("20 error(s)")
        && out.contains("pod/svc-0/main")
        && out.contains("pod/svc-7/main")
        && out.contains("streams (by line count):")
        && out.contains("errors")
        && out.contains("recent tail")
        && sav.percent() > 85.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:containerlogs-compact",
        "containerlogs",
        "synth-8streams-500lines",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #83: 100 000-line multi-stream log across 20 streams + 50
// errors interspersed. Must compact >97% in <500ms.
pub(crate) fn adv_containerlogs_huge(results: &mut Vec<RunResult>) {
    let raw = synth_containerlogs(20, 5_000, 2_000);
    let (out, sav, ms) = measure(&raw, containerlogs::compact_with_savings);
    let quality = if out.contains("100000 prefixed line(s) across 20 stream(s)")
        && out.contains("50 error(s)")
        && sav.percent() > 97.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:containerlogs:huge-100k",
        "containerlogs",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #84: hostile content:
//   - Unicode in pod/container names (café-app, 中文-pod, 🚀-rocket)
//   - mixed `[pod/...]` and `[ns/pod/container]` prefix forms
//   - log lines that LOOK LIKE a prefix in the message body (must not
//     create false streams)
//   - level keywords inside quoted message body (must still be detected
//     by the prefix-line whole-line scan)
pub(crate) fn adv_containerlogs_hostile(results: &mut Vec<RunResult>) {
    let raw = "[pod/café-app/main] 2024-01-15T10:30:00 INFO Démarrage\n\
[pod/café-app/main] 2024-01-15T10:30:01 ERROR Connexion à la base de données échouée\n\
[default/中文-pod/sidecar] 2024-01-15T10:30:00 INFO 启动\n\
[default/中文-pod/sidecar] 2024-01-15T10:30:01 WARN 重试\n\
[default/中文-pod/sidecar] 2024-01-15T10:30:02 INFO 完成\n\
[default/🚀-rocket/launcher] 2024-01-15T10:30:00 INFO Launch sequence started\n\
[default/🚀-rocket/launcher] 2024-01-15T10:30:01 INFO Stage 1 ignition\n\
[default/🚀-rocket/launcher] 2024-01-15T10:30:02 INFO Stage 2 separation\n\
[default/🚀-rocket/launcher] 2024-01-15T10:30:03 ERROR Müller-Lüdenscheid valve stuck\n\
[default/🚀-rocket/launcher] 2024-01-15T10:30:04 INFO Recovered via fallback\n\
[pod/café-app/main] 2024-01-15T10:30:05 INFO Body contains literal \"[pod/fake/log]\" but is just a message\n\
";
    let (out, _sav, ms) = measure(raw, containerlogs::compact_with_savings);
    // Note: the line containing literal `[pod/fake/log]` in its MESSAGE
    // BODY (not as a prefix) is preserved verbatim in the recent-tail
    // block — that's the right behaviour. We verify the body-mentioned
    // fake prefix didn't create a NEW stream by checking the stream count
    // is exactly 3.
    let quality = if out.contains("11 prefixed line(s) across 3 stream(s)")
        // Unicode stream names preserved.
        && out.contains("café-app")
        && out.contains("中文-pod")
        && out.contains("🚀-rocket")
        // Errors + warning detected.
        && out.contains("2 error(s)")
        && out.contains("1 warning(s)")
        // Specific error messages preserved.
        && out.contains("Connexion à la base de données échouée")
        && out.contains("Müller-Lüdenscheid valve stuck")
        // Warning preserved.
        && out.contains("重试")
        // No 4th stream created from the body-mentioned `[pod/fake/log]`.
        && !out.contains("4 stream(s)")
        // Streams summary doesn't include the fake one (the streams block
        // shows `pod/fake/log   N line(s)` if a 4th stream existed).
        && !out.contains("pod/fake/log   ")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:containerlogs:hostile",
        "containerlogs",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
