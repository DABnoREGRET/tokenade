//! Which compactors fall over when the output is not in English?
//!
//! `cargo run -p tokenade-bench --bin locale_census`
//!
//! A language test placed in a compactor's `detect()` is a GATE:
//! one missing word and the compactor does not run at all, the output goes out
//! whole. A census by READING the source does not answer the question:
//! it returned 456 `detect()` "carrying vocabulary", the vast
//! majority of which read API field names (`TopicArn`, `DBInstances`) or
//! column headers the program does not translate (`kubectl`, `pytest`).
//! That is not language, those are formats.
//!
//! The question is not read, it is executed: the SAME program, two
//! locales, and we compare what the fold KEEPS on each side. The compactor's
//! name is not enough to answer — a first version classed
//! as "held" everything that routed the same, and so counted `last` (1,514 →
//! 1,446 bytes, nothing folded) and `journalctl` (2,851 → 2,922, the output
//! GROWS) as successes. What is measured is the kept share:
//!
//! * `NOT LOCALISED` — the output is identical in both locales. The
//!   `detect()` may read its words, there is no language to lose.
//! * `HELD` — the share kept in French is that of English, within five
//!   points. That is the target.
//! * `LOST` — French keeps noticeably more bytes than English. The
//!   difference is the price of the language, and it is the worklist.
//!
//! The invocations are a CLOSED list of side-effect-free reads,
//! written by hand. Nothing is guessed from a compactor's name: a
//! census that executes what it finds is not an instrument.

/// Program, arguments. Pure reads, no writes, no network.
const READS: &[(&str, &[&str])] = &[
    ("free", &["-h"]),
    ("df", &["-h"]),
    ("lscpu", &[]),
    ("lsblk", &[]),
    ("uptime", &[]),
    ("ps", &["aux"]),
    ("last", &["-n", "20"]),
    ("who", &[]),
    ("id", &[]),
    ("uname", &["-a"]),
    ("vmstat", &[]),
    ("mount", &[]),
    ("route", &["-n"]),
    ("ip", &["addr"]),
    ("date", &[]),
    ("stat", &["/etc/hostname"]),
    ("ls", &["-la", "/etc"]),
    ("du", &["-sh", "/etc"]),
    ("timedatectl", &[]),
    ("hostnamectl", &[]),
    ("localectl", &[]),
    ("systemctl", &["--no-pager", "status", "--no-block"]),
    ("journalctl", &["--no-pager", "-n", "20"]),
    ("nproc", &[]),
    ("lsusb", &[]),
    ("lspci", &[]),
    ("git", &["status"]),
    ("git", &["log", "-5"]),
    ("git", &["log", "--oneline", "-20"]),
    ("git", &["branch", "-a"]),
    ("git", &["stash", "list"]),
    ("git", &["ls-files"]),
    ("git", &["diff", "--numstat"]),
    ("git", &["show", "--stat", "HEAD"]),
    ("git", &["reflog", "-20"]),
    (
        "grep",
        &["-rn", "pub fn", "crates/tokenade-core/src/auto.rs"],
    ),
    ("grep", &["-c", "fn", "crates/tokenade-core/src/auto.rs"]),
    ("make", &["-n"]),
    ("pacman", &["-Qi", "bash"]),
    ("tar", &["--version"]),
    ("systemd-analyze", &["blame"]),
    ("systemd-analyze", &["security", "--no-pager"]),
    ("loginctl", &["--no-pager"]),
    ("systemctl", &["--no-pager", "list-units", "--no-legend"]),
    (
        "systemctl",
        &["--no-pager", "list-unit-files", "--no-legend"],
    ),
    ("journalctl", &["--no-pager", "-p", "err", "-n", "40"]),
    ("xz", &["--version"]),
    ("free", &["-m"]),
    ("df", &["-i"]),
    ("ps", &["-ef"]),
    ("lsof", &["-p", "1"]),
    ("ss", &["-tuln"]),
    ("dmesg", &["--help"]),
];

fn capture(prog: &str, args: &[&str], locale: &str) -> Option<String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .env("LC_ALL", locale)
        .env("LANG", locale)
        .env("LANGUAGE", locale)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn main() {
    // This bench takes NO argument: silently accepting one would
    // make it answer a different question from the one asked.
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    // Without the locale installed, the census measures nothing and must SAY
    // so rather than report "all is well": both captures would be
    // identical and every program would be classed "not localised".
    let probe = capture("locale", &["-a"], "C").unwrap_or_default();
    let Some(fr) = probe
        .lines()
        .find(|l| l.starts_with("fr_FR"))
        .map(str::to_string)
    else {
        eprintln!(
            "no fr_FR locale installed: this census would class EVERYTHING as \n\
             \"not localised\" without having measured anything. Install fr_FR.UTF-8 \n\
             (`locale-gen`) then rerun."
        );
        std::process::exit(2);
    };

    let (mut absent, mut same, mut held, mut lost) = (0usize, 0usize, 0usize, 0usize);
    let mut worklist: Vec<String> = Vec::new();

    for (prog, args) in READS {
        let (Some(c), Some(loc)) = (capture(prog, args, "C"), capture(prog, args, &fr)) else {
            absent += 1;
            continue;
        };
        if c.trim().is_empty() {
            absent += 1;
            continue;
        }
        let argv: Vec<String> = std::iter::once(prog.to_string())
            .chain(args.iter().map(|a| a.to_string()))
            .collect();
        let en = tokenade_core::filter::filter_command(&argv, &c);
        if c == loc {
            same += 1;
            println!("NOT LOCALISED   {prog:-12} {:22}", en.filter);
            continue;
        }
        let after = tokenade_core::filter::filter_command(&argv, &loc);
        // The share KEPT on each side. Comparing raw sizes would say
        // nothing: the two outputs do not have the same length to begin with.
        let kept_en = en.output.len() as f64 / c.len() as f64;
        let kept_fr = after.output.len() as f64 / loc.len() as f64;
        if kept_fr <= kept_en + 0.05 {
            held += 1;
            println!(
                "HELD            {prog:-12} {:22} {:.0} % → {:.0} % kept",
                after.filter,
                100.0 * kept_en,
                100.0 * kept_fr
            );
        } else {
            lost += 1;
            worklist.push(format!(
                "{prog} {}: {} → {} ({:.0} % kept in English, {:.0} % in French, i.e. {} more bytes)",
                args.join(" "),
                en.filter,
                after.filter,
                100.0 * kept_en,
                100.0 * kept_fr,
                after.output.len() as i64
                    - (kept_en * loc.len() as f64) as i64
            ));
            println!(
                "LOST            {prog:-12} {:22} {:.0} % → {:.0} % kept",
                after.filter,
                100.0 * kept_en,
                100.0 * kept_fr
            );
        }
    }

    println!(
        "\n{same} not localised · {held} held · {lost} lost · {absent} absent from this machine"
    );
    if !worklist.is_empty() {
        println!("\nTo redo on the FORMAT rather than on the words:");
        for w in &worklist {
            println!("  {w}");
        }
    }
}
