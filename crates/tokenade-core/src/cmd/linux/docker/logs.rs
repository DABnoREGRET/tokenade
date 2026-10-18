//! `docker logs <container>` — application stdout/stderr.
//!
//! Strip ANSI, dedup runs, truncate. Falls back to generic when
//! auto's specific compactors don't apply.

use crate::compress::generic;

pub fn compact(output: &str) -> String {
    let (_, c, _) = crate::auto::compact(output);
    if c.len() < output.len() {
        c
    } else {
        generic(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_repeated_log_lines() {
        let mut s = String::new();
        for _ in 0..50 {
            s.push_str("2024-01-01T00:00:00Z INFO ready\n");
        }
        let out = compact(&s);
        assert!(out.len() < s.len());
    }
}
