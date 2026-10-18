//! **A loose detector at the head of a chain must not hide a DIFFERENT
//! behaviour.**
//!
//! The dispatcher chooses between a command's variants through an ordered
//! `or_else`: `a::try_compact(text).or_else(|| b::try_compact(text))`. If
//! `a`'s detector accepts everything, `b` NEVER runs. Nothing fails — there
//! is an output — and the only trace is a word in a banner.
//!
//! Two cases measured on 2026-09-09, and they call for opposite answers:
//!
//! - **`net`**: the two compactors do DIFFERENT work (Windows removes its
//!   conclusion, macOS folds by shape) and no detector establishes the
//!   platform. The second was dead and the banner announced macOS on a
//!   Windows output. Fixed by applying both.
//! - **`ssh`**: `ssh_win` is byte for byte identical to `ssh_cmd`. The dead
//!   arm costs nothing at all.
//!
//! Hence the exact rule: the chain is at fault only if the hidden arm would
//! have returned something else. That is what this guard checks, chain by
//! chain.
//!
//! - **`whoami`**: `linux::whoami` is a PASSTHROUGH with `any_output` as its
//!   detector, placed at the head. It swallowed Windows' `whoami /all` and
//!   macOS' `whoami -all`, and returned the WHOLE table. Fixed: it now only
//!   answers a single line, as its own comment says.

use tokenade_core::cmd::*;

/// Outputs of very different shapes: a line, a key-value pair, a number, a
/// sentence, and a small table.
const PROBES: [&str; 5] = [
    "root\n",
    "a: b\n",
    "1.2\n",
    "Le service a démarré.\n",
    "NAME   VALUE\n----   -----\nfoo    bar\n",
];

/// The decision, alone and pure — and therefore TESTABLE.
///
/// Inline in the loop, it was reached by no chain once the two faults were
/// fixed: the mutation that disarms it survived for lack of a case able to
/// kill it. A synthetic fixture exercises it below.
fn hidden_branch_would_differ(
    prog: &str,
    first_detect: fn(&str) -> bool,
    first: fn(&str) -> String,
    hidden: &[(&str, fn(&str) -> String)],
) -> Option<String> {
    if !PROBES.iter().all(|t| first_detect(t)) {
        return None; // the detector discriminates: the next arm is reachable
    }
    for (name, other) in hidden {
        for t in PROBES {
            if first(t) != other(t) {
                return Some(format!(
                    "{prog}: the first detector accepts everything, so `{name}` never \
                     runs — and it would have returned something else on {t:?}"
                ));
            }
        }
    }
    None
}

fn check(
    prog: &str,
    first_detect: fn(&str) -> bool,
    first: fn(&str) -> String,
    hidden: &[(&str, fn(&str) -> String)],
    dead: &mut Vec<String>,
) {
    if let Some(note) = hidden_branch_would_differ(prog, first_detect, first, hidden) {
        dead.push(note);
    }
}

/// The decision itself, on the three cases it must tell apart.
#[test]
fn the_rule_names_only_a_hidden_branch_that_would_differ() {
    fn always(_: &str) -> bool {
        true
    }
    fn picky(t: &str) -> bool {
        t.starts_with("root")
    }
    fn identity(t: &str) -> String {
        t.to_string()
    }
    fn shouty(t: &str) -> String {
        t.to_uppercase()
    }
    // Loose detector AND different behaviour: that is the fault.
    assert!(
        hidden_branch_would_differ("x", always, identity, &[("autre", shouty)]).is_some(),
        "a hidden arm that would return something else must be named"
    );
    // Loose detector but IDENTICAL behaviours: nothing to report, that is
    // the case of `ssh`, whose Windows variant is byte for byte the Linux
    // one.
    assert!(
        hidden_branch_would_differ("x", always, identity, &[("autre", identity)]).is_none(),
        "two identical branches are not a fault"
    );
    // A detector that DISCRIMINATES: the next arm is reachable.
    assert!(
        hidden_branch_would_differ("x", picky, identity, &[("autre", shouty)]).is_none(),
        "a detector that refuses some probes lets the rest through"
    );
}

#[test]
fn no_loose_first_detector_hides_a_different_behaviour() {
    let mut dead: Vec<String> = Vec::new();
    check(
        "ssh",
        linux::ssh_cmd::detect,
        linux::ssh_cmd::compact,
        &[("windows::ssh_win", windows::ssh_win::compact)],
        &mut dead,
    );
    check(
        "whoami",
        linux::whoami::detect,
        linux::whoami::compact,
        &[
            ("macos::whoami_macos", macos::whoami_macos::compact),
            ("windows::whoami_win", windows::whoami_win::compact),
        ],
        &mut dead,
    );
    check(
        "tar",
        linux::tar::detect,
        linux::tar::compact,
        &[("windows::tar_win", windows::tar_win::compact)],
        &mut dead,
    );
    check(
        "hostname",
        linux::hostname::detect,
        linux::hostname::compact,
        &[("windows::hostname_win", windows::hostname_win::compact)],
        &mut dead,
    );
    check(
        "curl",
        linux::curl::detect,
        linux::curl::compact,
        &[("windows::curl_win", windows::curl_win::compact)],
        &mut dead,
    );
    check(
        "date",
        linux::date::detect,
        linux::date::compact,
        &[("windows::date_cmd", windows::date_cmd::compact)],
        &mut dead,
    );
    check(
        "logname",
        linux::logname::detect,
        linux::logname::compact,
        &[("macos::logname_macos", macos::logname_macos::compact)],
        &mut dead,
    );
    check(
        "type",
        linux::type_cmd::detect,
        linux::type_cmd::compact,
        &[("windows::type_win", windows::type_win::compact)],
        &mut dead,
    );
    check(
        "timeout",
        linux::timeout::detect,
        linux::timeout::compact,
        &[("windows::timeout_cmd", windows::timeout_cmd::compact)],
        &mut dead,
    );
    assert!(
        dead.is_empty(),
        "these chains have an unreachable arm that would have returned something else: {dead:?}"
    );
}
