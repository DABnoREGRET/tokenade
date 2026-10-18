//! The `npm run build` compactor, measured on a REAL build log.
//!
//! Its unit test builds its own log out of N identical lines and gets a large
//! number back. Production gave 10%. The difference is what a real build log
//! actually repeats: not identical lines, but the SAME sentence with one slot
//! changed — one warning per route, per file, per module. Byte-equality never
//! matches two of them.
//!
//! Fixture: `pnpm build` on a real Next.js application, captured verbatim
//! (paths generalised, nothing else touched).

fn log() -> String {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/build/nextjs-pnpm-build.log");
    std::fs::read_to_string(p).expect("build-log fixture")
}

#[test]
fn a_real_build_log_compacts() {
    let raw = log();
    let (out, sav) = tokenade_core::cmd::linux::npm_run_script::compact_with_savings(&raw);
    println!(
        "nextjs-pnpm-build: {} -> {} tokens ({:.1}%)",
        sav.tokens_before,
        sav.tokens_after,
        sav.percent()
    );
    assert!(
        sav.percent() >= 30.0,
        "real build log fell to {:.1}% (floor 30%)\n{}",
        sav.percent(),
        &out[..out.len().min(1500)]
    );
}

/// A build log is read for what went wrong. Every error, warning and summary
/// line must still be reachable — folding may group them, never erase the
/// distinct values that tell an engineer WHICH route or file is affected.
#[test]
fn a_build_log_keeps_what_broke() {
    let raw = log();
    let out = tokenade_core::cmd::linux::npm_run_script::compact(&raw);
    for needle in [
        "/api/cron/refresh-link-status/route",
        "/api/indexnow-key/route",
        "Compiled with warnings",
        "Generating static pages (323/323)",
    ] {
        assert!(
            raw.contains(needle),
            "fixture drifted: {needle:?} no longer present"
        );
        assert!(out.contains(needle), "compaction lost {needle:?}");
    }
}
