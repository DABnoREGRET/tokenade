//! `cargo run` — `Compiling … Running` then the binary stdout.
//!
//! Strategy: keep cargo prefix compact via the bigger build
//! compactor; the binary's stdout is unconstrained so we route it
//! through generic.

use crate::compress::generic;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    if let Some(running_idx) = s.find("\n    Running ") {
        let cargo_prefix = &s[..running_idx];
        let bin_output_start = s[running_idx..]
            .find('\n')
            .map(|n| running_idx + n + 1)
            .unwrap_or(s.len());
        let bin_output = &s[bin_output_start..];
        let cargo_compact = crate::cmd::linux::cargo::build::compact(cargo_prefix);
        let bin_compact = generic(bin_output);
        return format!("{cargo_compact}\n    Running ...\n{bin_compact}");
    }
    crate::cmd::linux::cargo::build::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cargo_prefix_from_binary_stdout() {
        let s = "   Compiling foo v0.1.0\n    Finished dev [unoptimized + debuginfo] target(s)\n    Running `target/debug/foo`\nHello, world!\n";
        let out = compact(s);
        assert!(out.contains("Hello, world!"));
        assert!(out.contains("Running ..."));
    }

    #[test]
    fn no_running_line_falls_through_to_build() {
        let s = "Compiling foo\nFinished\n";
        let out = compact(s);
        // Should not panic; output non-empty.
        assert!(!out.is_empty());
    }
}
