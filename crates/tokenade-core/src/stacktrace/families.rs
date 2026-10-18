//! **Grouping by what FAILED, not by what is written.**
//!
//! Twenty different traces can describe a single failure. The family is the
//! right reporting unit: it answers "how many problems?" where the trace
//! count answers "how many times?".
//!
//! The order is total and declared: a report whose order depends on a hash
//! table changes on every run, and a diff that moves for no reason ends up
//! ignored.

use super::*;

/// Failure-family taxonomy (Tokenless-inspired).
///
/// Tests typically fail in highly repetitive ways — 50 cases that all hit
/// the same `Cannot find module 'X'` produce 50 nearly-identical stack
/// traces. [`group_by_failure_family`] classifies each block into one of
/// the known families and emits one line per family: kind, count, up to
/// three representative examples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureFamily {
    /// `Cannot find module …`, `ModuleNotFoundError`, `Could not resolve`.
    MissingModule,
    /// `TimeoutError`, `await timed out`, `exceeded 5000 ms`.
    AsyncTimeout,
    /// `Snapshot does not match`, `received does not equal expected snapshot`.
    SnapshotMismatch,
    /// `Cannot read property … of undefined`, `NoneType has no attribute …`.
    UndefinedAccess,
    /// `AssertionError`, `expect(…).toBe(…)`, `assert_eq!`.
    Assertion,
    /// `EACCES`, `PermissionError`, `Permission denied`.
    Permission,
    /// `ECONNREFUSED`, `Network is unreachable`, `getaddrinfo failed`.
    Network,
    /// `SyntaxError`, `unexpected token`.
    Syntax,
    /// Anything that doesn't match a known pattern.
    Other,
}

impl FailureFamily {
    /// Human-readable label used in the compacted output.
    pub fn label(&self) -> &'static str {
        match self {
            FailureFamily::MissingModule => "Missing module",
            FailureFamily::AsyncTimeout => "Async timeout",
            FailureFamily::SnapshotMismatch => "Snapshot mismatch",
            FailureFamily::UndefinedAccess => "Undefined property access",
            FailureFamily::Assertion => "Assertion failure",
            FailureFamily::Permission => "Permission denied",
            FailureFamily::Network => "Network failure",
            FailureFamily::Syntax => "Syntax error",
            FailureFamily::Other => "Other failure",
        }
    }

    /// Classify a single failure block.
    pub fn classify(block: &str) -> FailureFamily {
        let lower = block.to_lowercase();
        if lower.contains("cannot find module")
            || lower.contains("modulenotfounderror")
            || lower.contains("could not resolve")
            || lower.contains("no module named")
        {
            FailureFamily::MissingModule
        } else if lower.contains("timed out")
            || lower.contains("timeouterror")
            || lower.contains("exceeded ") && (lower.contains(" ms") || lower.contains(" sec"))
        {
            FailureFamily::AsyncTimeout
        } else if lower.contains("snapshot")
            && (lower.contains("does not match") || lower.contains("mismatch"))
        {
            FailureFamily::SnapshotMismatch
        } else if lower.contains("cannot read prop")
            || lower.contains("of undefined")
            || lower.contains("of null")
            || lower.contains("attributeerror")
            || lower.contains("has no attribute")
        {
            FailureFamily::UndefinedAccess
        } else if lower.contains("assertionerror")
            || lower.contains("assertion failed")
            || lower.contains("expect(")
            || lower.contains("assert_eq!")
            || lower.contains(".tobe(")
        {
            FailureFamily::Assertion
        } else if lower.contains("eacces")
            || lower.contains("permissionerror")
            || lower.contains("permission denied")
        {
            FailureFamily::Permission
        } else if lower.contains("econnrefused")
            || lower.contains("network is unreachable")
            || lower.contains("getaddrinfo failed")
            || lower.contains("dns error")
        {
            FailureFamily::Network
        } else if lower.contains("syntaxerror") || lower.contains("unexpected token") {
            FailureFamily::Syntax
        } else {
            FailureFamily::Other
        }
    }
}

/// Group identical-family failures and emit one line per family with up
/// to three examples. Each example is the canonical label of a failing
/// test extracted from the block header (`extract_block_label`).
///
/// When there's only one block (or all blocks fall under `Other`) the
/// function returns the input unchanged so the caller falls back to the
/// per-language compactor.
pub fn group_by_failure_family(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let separators: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_block_separator(l))
        .map(|(i, _)| i)
        .collect();
    if separators.len() < 2 {
        return text.to_string();
    }

    let preamble: String = lines[..separators[0]].join("\n");
    let mut groups: std::collections::BTreeMap<FailureFamily, Vec<String>> =
        std::collections::BTreeMap::new();
    for w in 0..separators.len() {
        let start = separators[w];
        let end = if w + 1 < separators.len() {
            separators[w + 1]
        } else {
            lines.len()
        };
        let block: String = lines[start..end].join("\n");
        let family = FailureFamily::classify(&block);
        let label = extract_block_label(&block).unwrap_or_else(|| "(anonymous)".into());
        groups.entry(family).or_default().push(label);
    }

    // If every block fell into `Other`, the taxonomy didn't help — bail.
    let non_other = groups
        .iter()
        .filter(|(k, _)| **k != FailureFamily::Other)
        .count();
    if non_other == 0 {
        return text.to_string();
    }

    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(preamble.trim_end());
        out.push('\n');
    }
    for (family, labels) in &groups {
        let n = labels.len();
        let examples = labels
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        let more = if n > 3 {
            format!(", +{} more", n - 3)
        } else {
            String::new()
        };
        let _ = writeln!(out, "[{}] ×{n}: {examples}{more}", family.label());
    }
    out.trim_end().to_string()
}

impl FailureFamily {
    /// Stable ordering for the BTreeMap output (alphabetical labels).
    fn ord(&self) -> u8 {
        match self {
            FailureFamily::Assertion => 0,
            FailureFamily::AsyncTimeout => 1,
            FailureFamily::MissingModule => 2,
            FailureFamily::Network => 3,
            FailureFamily::Other => 4,
            FailureFamily::Permission => 5,
            FailureFamily::SnapshotMismatch => 6,
            FailureFamily::Syntax => 7,
            FailureFamily::UndefinedAccess => 8,
        }
    }
}

impl PartialOrd for FailureFamily {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FailureFamily {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.ord().cmp(&other.ord())
    }
}
