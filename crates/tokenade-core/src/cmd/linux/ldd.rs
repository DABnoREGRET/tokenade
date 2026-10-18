//! `ldd <binary>` — shared library dependencies.
//!
//! ```text
//!     linux-vdso.so.1 (0x00007ffd...)
//!     libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f...)
//!     /lib64/ld-linux-x86-64.so.2 (0x00007f...)
//! ```
//!
//! Compaction: drop the `(0x...)` load addresses — they're random
//! per-invocation and add nothing for the agent.


pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(40) {
        let t = line.trim_start();
        if (t.contains(".so") || t.contains(".so.")) && t.contains("0x") {
            hits += 1;
        }
    }
    hits >= 3
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        // Drop the trailing `(0x...)` segment.
        let stripped = match line.rfind(" (0x") {
            Some(idx) => &line[..idx],
            None => line,
        };
        out.push_str(stripped);
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "    linux-vdso.so.1 (0x00007ffd1234)\n    libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f5678)\n    /lib64/ld-linux-x86-64.so.2 (0x00007f9012)\n";

    #[test]
    fn detects_ldd() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_load_addresses() {
        let out = compact(SAMPLE);
        assert!(out.contains("libc.so.6"));
        assert!(!out.contains("0x00007f5678"));
    }
}
