//! `cargo bench` — `test bench_name ... bench: N ns/iter (+/- M)`.
//! Already concise; keep all benchmark results + summary + diagnostics.

use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut keep = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if (t.starts_with("test ") && t.contains("bench:"))
            || t.starts_with("test result:")
            || t.contains("error")
            || t.contains("warning")
            || t.starts_with("Finished")
            || t.starts_with("running")
        {
            keep.push(line);
        }
    }
    if keep.is_empty() {
        return crate::cmd::linux::cargo::build::compact(output);
    }
    truncate_middle(&keep.join("\n"), 100, 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_bench_results_and_summary() {
        let s = "Compiling foo v0.1.0\n   Finished release [optimized] target(s)\nrunning 3 tests\ntest hash_sha256 ... bench:     12345 ns/iter (+/- 678)\ntest hash_sha512 ... bench:     23456 ns/iter (+/- 789)\ntest hash_keccak ... bench:     34567 ns/iter (+/- 890)\ntest result: ok. 0 passed; 0 failed; 0 ignored; 3 measured\n";
        let out = compact(s);
        assert!(out.contains("hash_sha256"));
        assert!(out.contains("12345 ns/iter"));
        assert!(out.contains("test result: ok"));
    }

    #[test]
    fn empty_falls_back_to_build_compactor() {
        let out = compact("just compiling chatter\nCompiling foo\nFinished\n");
        assert!(out.contains("Compiling") || out.contains("Finished"));
    }
}
