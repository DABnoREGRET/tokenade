//! **A banner does not name a platform it cannot establish.**
//!
//! Many compactors carry a name suffixed with the platform (`-macos`, `-win`,
//! `-linux`) and the dispatcher chains them with `or_else`. When the FIRST
//! detector of the chain is loose, the next ones are dead and the banner
//! announces the wrong platform on the user's output.
//!
//! MEASURED 2026-09-09: Windows `net start` came back as
//! `[tokenade:net-macos]`, because the macOS `net` detector answers "yes" to
//! any text carrying a `:` or a `.` and so runs before a `net_win` whose
//! detector is `any_output`. Neither knows which system it is on; both now
//! apply, under a neutral label.
//!
//! This guard rereads the corpus: a capture's directory SAYS its platform,
//! and a banner naming another one is a lie.

use std::path::Path;

fn platform_named(label: &str) -> Option<&'static str> {
    let l = label.to_ascii_lowercase();
    if l.ends_with("-macos") || l.ends_with("-mac") || l.contains("-mac-") {
        Some("macos")
    } else if l.ends_with("-win") || l.contains("-win-") || l.contains("windows") {
        Some("windows")
    } else if l.contains("linux") {
        Some("linux")
    } else {
        None
    }
}

#[test]
fn no_banner_claims_a_platform_the_capture_contradicts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Under a pinned threshold: a compactor that delivers a table WHOLE when
    // it fits writes no banner, and each one that learned to (`sc query`,
    // `net start`, `ps`) took captures out of this sweep — 8, 6, 5, then 3.
    // Squeezed, every compactor folds and signs.
    let _g = tk_base::delivery::threshold_on_this_thread(50);
    let mut checked = 0usize;
    let mut lies: Vec<String> = Vec::new();
    for plat in ["linux", "macos", "windows"] {
        let dir = root.join(plat);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("txt") {
                continue;
            }
            let stem = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let key = format!("{plat}/{stem}");
            let Ok(Some(line)) = tokenade_core::corpus_key::command_of(&key) else {
                continue;
            };
            let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            // **The BANNER, not the routing name.** Written on
            // `FilterResult.filter`, this guard read another string than the
            // one the user sees: renaming the label to `net-macos` left it
            // green. That is the very fault — the sentence must come from the
            // path taken — committed in the guard that exists for it.
            let out = tokenade_core::filter::filter_command(&argv, &body).output;
            // ALL the banners of the output, not just the first line's: most
            // compactors write theirs at the top, but not all, and a guard
            // that reads only one reads only seven captures out of two hundred
            // and sixty-five.
            for line in out.lines() {
                let Some(rest) = line.trim_start().strip_prefix("[tokenade:") else {
                    continue;
                };
                let Some(label) = rest.split([']', ' ']).next() else {
                    continue;
                };
                checked += 1;
                if let Some(claimed) = platform_named(label) {
                    if claimed != plat {
                        lies.push(format!(
                            "{key}: banner `[tokenade:{label}]` on a {plat} capture"
                        ));
                    }
                }
            }
        }
    }
    // Eleven under the pinned threshold; without it only three remained.
    assert!(
        checked >= 10,
        "the sweep read only {checked} banner(s) — it does not read the corpus. \
         Eleven named banners are written at the first stage on this corpus under the pinned threshold; \
         the other banners of a delivered output come from the SECOND, and do \
         not name a compactor."
    );
    assert!(
        lies.is_empty(),
        "these banners name a platform the capture contradicts — a loose \
         detector placed at the head of an `or_else` makes the next arm dead \
         and makes the label lie: {lies:?}"
    );
}

/// **Both `net` passes apply, and the label names nobody.**
///
/// The exact case that opened this guard: Windows `net start` came back as
/// `[tokenade:net-macos]`, because the macOS detector answers "yes" to any
/// text carrying a `:` or a `.` and runs before a `net_win` whose detector is
/// `any_output`. The second arm was dead: its conclusion line was never
/// removed, and the banner announced the wrong platform.
#[test]
fn net_applies_both_passes_and_names_no_platform() {
    // Past the delivery threshold, so that the fold — and its banner — fires:
    // under it `net` hands the list over whole.
    let _g = tk_base::delivery::threshold_on_this_thread(50);
    let argv = vec!["net".to_string(), "start".to_string()];
    let mut body = String::from("These Windows services are started:\n\n");
    for i in 0..80 {
        body.push_str(&format!("   Service number {i} of this host\n"));
    }
    body.push_str("\nThe command completed successfully.\n");
    let out = tokenade_core::filter::filter_command(&argv, &body).output;
    assert!(
        !out.contains("net-macos"),
        "the banner names a platform no detector establishes: {}",
        out.lines().next().unwrap_or_default()
    );
    assert!(
        out.contains("[tokenade:net]"),
        "the `net` compactor did not fire: {}",
        out.lines().next().unwrap_or_default()
    );
    assert!(
        !out.contains("The command completed successfully."),
        "the Windows conclusion was not removed — the second arm is dead \
         again: {out}"
    );
    // And nothing of the content vanished on the sly: the first service is there.
    assert!(out.contains("Service number 0 of this host"), "{out}");
}
