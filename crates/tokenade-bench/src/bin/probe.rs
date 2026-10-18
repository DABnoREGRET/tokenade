//! Measure ONE CLI invocation, honestly.
//!
//! `cargo run -p tokenade-bench --bin probe -- read <file>`
//! `cargo run -p tokenade-bench --bin probe -- --raw wrap "grep -rn x ."`
//!
//! Without this, the measurement is done at the shell — and an agent's `Bash` goes through the hook,
//! which compacts the output before it reaches me. MEASURED while writing this
//! file: `tokenade read` on a 45,355-byte fixture reported
//! **156 bytes delivered**, i.e. −99.7 %, where the ledger gives −32 % for this
//! path. The gap was not in the product, it was in the instrument:
//! my own output had been folded a second time.
//!
//! Here it is `Command::output()` from a Rust process: bytes, not a
//! terminal. The printed summary is short on purpose — IT is what goes through the
//! hook, not the measurement.

use tokenade_bench::Bench;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--raw` also prints the measured output. Only use it on small things:
    // beyond that, the summary is what counts, and the body would go back through the hook.
    // Removed wherever they are: testing them by POSITION made
    // `probe --here --raw index` incomprehensible — the second flag went
    // to the CLI, which refused a `--raw` it does not know.
    let show = args.iter().any(|a| a == "--raw");
    // `--here` runs from the REAL current directory, the state staying isolated.
    // `map`, `query`, `impact`, `skeleton` and `semantic` answer from an
    // index built on the cwd's code: measured from the bench's empty sandbox
    // they return "0 files indexed", which reads as a perfect
    // compaction when it is a refusal.
    let here = args.iter().any(|a| a == "--here");
    // The common seam: it refuses an instrument older than the bench.
    // The measured CLI's flags follow, so we only give it ours.
    let ours: Vec<String> = args
        .iter()
        .filter(|a| *a == "--raw" || *a == "--here")
        .cloned()
        .collect();
    tokenade_bench::reject_unknown_args(&["--raw", "--here"], &ours);
    args.retain(|a| a != "--raw" && a != "--here");

    if args.is_empty() {
        eprintln!("usage: probe [--raw] <tokenade CLI args…>");
        std::process::exit(2);
    }

    let bench = match Bench::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("probe: {e}");
            std::process::exit(2);
        }
    };
    // A binary older than the tree measures a version that no longer exists.
    // MEASURED 2026-09-08: 273,366 then 284,024 bytes delivered on the SAME base.
    if let Some(src) = bench.source_newer_than_exe() {
        eprintln!("{}", tokenade_bench::stale_exe_note(bench.exe(), &src));
        std::process::exit(2);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    // The input size is only known if the argument IS a file. Say so
    // rather than display a ratio computed on nothing.
    let input = refs
        .iter()
        .find_map(|a| std::fs::metadata(a).ok().map(|m| m.len()))
        .unwrap_or(0);

    let cwd = here.then(std::env::current_dir).transpose().ok().flatten();
    let run = match bench.run_in(cwd.as_deref(), &refs, None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("probe: {e}");
            std::process::exit(2);
        }
    };

    let out = run.bytes() as u64;
    println!(
        "exe      {}  ({:?})",
        bench.exe().display(),
        bench.profile()
    );
    println!(
        "code     {:?}{}",
        run.code,
        if run.survived() {
            ""
        } else {
            "   ← killed by a signal: this is NOT an output"
        }
    );
    match input {
        0 => println!("delivered {out} bytes  (input not measurable from the arguments)"),
        n => println!(
            "input    {n} bytes\ndelivered {out} bytes   ({:+.1}%)",
            -100.0 * (n as f64 - out as f64) / n as f64
        ),
    }
    match run.ms_comparable() {
        Some(ms) => println!("duration {ms} ms"),
        None => println!(
            "duration {} ms  ← DEBUG build: good for a RATIO, not in absolute terms",
            run.ms_ratio_only()
        ),
    }
    // An instrument that returns zero bytes MUST say why, otherwise a
    // refusal reads as a perfect compaction. It happened on the first try: the
    // probe displayed "delivered 0 bytes (-100.0 %)" on a run that had failed.
    if !run.survived() || run.code != Some(0) || out == 0 {
        let err = String::from_utf8_lossy(&run.stderr);
        let err = err.trim();
        if err.is_empty() {
            println!("stderr   (empty) — the command explained nothing");
        } else {
            println!(
                "stderr   {}",
                err.lines().take(3).collect::<Vec<_>>().join(" / ")
            );
        }
    }
    if show {
        println!("---");
        print!("{}", run.text());
    }
}
