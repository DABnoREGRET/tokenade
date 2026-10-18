//! A bench rejects what it does not understand — checked by RUNNING it.
//!
//! The rejection exits the process (`exit(2)`), so no in-process test can
//! observe it: the first mutations on `reject_unknown_args` all
//! survived for that reason alone, the tested half being the one that ACCEPTS.
//! Here the bench is a real subprocess, and it is the exit code and the
//! message that are exercised.

use std::process::Command;

/// FOUND by using it: `duplication -- 4` — the form without the flag's
/// name — silently ignored, measurement returned at the DEFAULT threshold, and its
/// answer was read as the answer to the question asked.
#[test]
fn an_unknown_flag_stops_the_bench_and_names_what_it_knows() {
    let out = Command::new(env!("CARGO_BIN_EXE_duplication"))
        .arg("--no-such-flag")
        .output()
        .expect("the bench must start");
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unknown argument must stop the bench, not let it measure something else"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--no-such-flag"),
        "the rejection must name the argument: {err}"
    );
    assert!(err.contains("--min-lines"), "and say what it knows: {err}");
}

/// The known flag, for its part, goes through with its value — that is the half a
/// hardening breaks, and a bench that rejects its own flags is no longer of use.
#[test]
fn a_known_flag_still_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_duplication"))
        .args(["--min-lines", "40"])
        .output()
        .expect("the bench must start");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("bodies >= 40 lines"),
        "the flag's value must have been TAKEN, not just accepted: {stdout}"
    );
}

/// The attached form `--flag=value` does NOT consume the next word, so an
/// unknown placed after it is still seen. Without this distinction, a
/// known flag would shield the first argument that follows it — and the
/// guard would let through exactly what it exists to catch.
#[test]
fn an_attached_value_does_not_swallow_the_next_argument() {
    let out = Command::new(env!("CARGO_BIN_EXE_duplication"))
        .args(["--min-lines=40", "--no-such-flag"])
        .output()
        .expect("the bench must start");
    assert_eq!(
        out.status.code(),
        Some(2),
        "the unknown after an attached value was swallowed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // …and the separate form does consume ITS value.
    let ok = Command::new(env!("CARGO_BIN_EXE_duplication"))
        .args(["--min-lines", "40"])
        .output()
        .expect("the bench must start");
    assert_eq!(ok.status.code(), Some(0));
}
