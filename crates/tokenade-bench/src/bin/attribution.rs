//! **Which program does a ledger line name — and would it still name it today?**
//!
//! ```text
//! attribution            # this machine's ledger (~/.tokenade/gain.jsonl)
//! attribution <file>     # another ledger
//! attribution --transcripts  # the REAL Bash commands from the transcripts
//!   [--examples N] [--full]   # N examples per name, whole with --full
//! ```
//!
//! `--transcripts` answers another question: of what the agents really
//! typed, how many commands does the resolver name by a shell
//! launcher (`bash`, `sh`, `[`, `test`…) rather than by the command doing the
//! work? Those names ARE programs, so the ledger replay does not
//! see them — and the fleet counted 2.3 M tokens of them over 30 days
//! (2026-10-05).
//!
//! A line's `cmd` field is frozen at write time. It feeds the coverage
//! shown on tokenade.net, the ranking attribution and the recall of
//! commands without a compactor; a name that is not a program (`done;`, `&&`,
//! `""`, `-m1`) skews all three. This instrument replays the CURRENT resolver
//! (`meaningful_segment` then `program_basename`, the `tokenade wrap` path)
//! on the label of every line whose recorded `cmd` is not a program
//! name, and says what remains.
//!
//! Born on 2026-10-02: two iterations had measured the same thing with a throwaway
//! test, which the project's rules forbid.

use std::collections::BTreeMap;

/// What a replay returns: recorded name → count, replayed name → count,
/// and for each replayed name still invalid, up to three labels.
#[derive(Default)]
struct Replay {
    recorded: BTreeMap<String, usize>,
    replayed: BTreeMap<String, usize>,
    still_bad: BTreeMap<String, Vec<String>>,
    /// Lines whose label IS the chosen segment (`N=/x`), not the command:
    /// `wrap` labels by the segment it routes. The replay can say nothing
    /// about them — the segment that followed was not written.
    segment_only: usize,
}

fn resolve(label: &str) -> String {
    tokenade_core::command_rewrite::program_name(label)
}

fn replay(text: &str) -> Replay {
    let mut r = Replay::default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(cmd), Some(label)) = (v["cmd"].as_str(), v["label"].as_str()) else {
            continue;
        };
        if tokenade_core::command_rewrite::is_plausible_program_name(cmd) {
            continue;
        }
        *r.recorded.entry(cmd.to_string()).or_default() += 1;
        if tokenade_core::command_rewrite::first_program_token(label).is_empty() {
            r.segment_only += 1;
            continue;
        }
        let now = resolve(label);
        *r.replayed.entry(now.clone()).or_default() += 1;
        if !tokenade_core::command_rewrite::is_plausible_program_name(&now) {
            let ex = r.still_bad.entry(now).or_default();
            if ex.len() < 3 {
                ex.push(label.chars().take(120).collect());
            }
        }
    }
    r
}

/// The names a resolver returns when it stops at the launcher.
const LAUNCHERS: &[&str] = &[
    "bash", "sh", "zsh", "dash", "[", "[[", "test", "time", "{", "(",
];

/// Per launcher name (or invalid name): number of commands, and three examples.
fn launchers_in(
    cmds: impl Iterator<Item = String>,
    examples: usize,
    full: bool,
) -> (usize, BTreeMap<String, (usize, Vec<String>)>) {
    let mut out: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut total = 0;
    for cmd in cmds {
        total += 1;
        let name = resolve(&cmd);
        if LAUNCHERS.contains(&name.as_str())
            || !tokenade_core::command_rewrite::is_plausible_program_name(&name)
        {
            let e = out.entry(name).or_default();
            e.0 += 1;
            if e.1.len() < examples {
                // `--full`: the whole command — a resolver bug is often past the first 110 chars.
                e.1.push(
                    cmd.chars()
                        .take(if full { usize::MAX } else { 110 })
                        .collect::<String>()
                        .replace('\n', "⏎"),
                );
            }
        }
    }
    (total, out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--transcripts", "--examples", "--full"], &args);
    let full = args.iter().any(|a| a == "--full");
    let examples = args
        .iter()
        .position(|a| a == "--examples")
        .and_then(|i| args.get(i + 1)?.parse().ok())
        .unwrap_or(3);
    if args.iter().any(|a| a == "--transcripts") {
        let mut cmds = Vec::new();
        tokenade_bench::transcripts::for_each_block(
            &tokenade_bench::transcripts::transcript_root(),
            |_, _, _, b| {
                if let tokenade_bench::transcripts::Block::Bash { cmd, .. } = b {
                    // A command a cold resume masked (whole or cut short) is
                    // not what the agent typed: 158 of them were counted as a
                    // program named `[tokenade:elided` (2026-10-05).
                    if !(cmd.contains("[tokenade:elided") && cmd.contains("on cold resume")) {
                        cmds.push(cmd.to_string());
                    }
                }
            },
            || {},
        );
        let (total, found) = launchers_in(cmds.into_iter(), examples, full);
        let named: usize = found.values().map(|v| v.0).sum();
        println!("{total} real Bash command(s); {named} named by a launcher or an invalid name");
        let mut rows: Vec<_> = found.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1 .0));
        for (k, (n, ex)) in rows.iter().take(25) {
            println!("  {n:>6}  {k:?}");
            for e in ex {
                println!("            {e}");
            }
        }
        return;
    }
    let path = match args.first() {
        Some(p) => std::path::PathBuf::from(p),
        None => tokenade_core::platform::home_dir()
            .expect("HOME")
            .join(".tokenade")
            .join("gain.jsonl"),
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let r = replay(&text);
    let total: usize = r.recorded.values().sum();
    let bad: usize = r
        .replayed
        .iter()
        .filter(|(k, _)| !tokenade_core::command_rewrite::is_plausible_program_name(k))
        .map(|(_, n)| n)
        .sum();
    println!(
        "{total} line(s) whose recorded `cmd` is not a program; {bad} stay so on replay; \
         {} not replayable (the label is only the assignments segment)",
        r.segment_only
    );
    println!("\nrecorded:");
    for (k, n) in &r.recorded {
        println!("  {n:>5}  {k:?}");
    }
    println!("\nreplayed:");
    for (k, n) in &r.replayed {
        println!("  {n:>5}  {k:?}");
    }
    println!("\nstill invalid:");
    for (k, ex) in &r.still_bad {
        println!("  {k:?}");
        for e in ex {
            println!("      {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_only_invalid_names_and_says_what_remains() {
        let ledger = [
            r#"{"cmd":"git","label":"git status"}"#,
            r#"{"cmd":"done;","label":"until x; do sleep 1; done; tail -5 a.log"}"#,
            r#"{"cmd":"","label":"S=/tmp/y && cp a b"}"#,
            r#"{"cmd":"-m1","label":"PG=$(grep -m1 x .env) && python3 run.py"}"#,
        ]
        .join("\n");
        let r = replay(&ledger);
        assert_eq!(
            r.recorded.values().sum::<usize>(),
            3,
            "git is a program: not replayed"
        );
        assert_eq!(r.replayed.get("tail"), Some(&1));
        assert_eq!(r.replayed.get("cp"), Some(&1));
        assert_eq!(r.replayed.get("python3"), Some(&1));
        let seg = replay(r#"{"cmd":"","label":"N=/home/x"}"#);
        assert_eq!((seg.segment_only, seg.replayed.len()), (1, 0));
    }
}
