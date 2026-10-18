//! Snapshot tests for compactor outputs.
//!
//! Inspired by rtk's testing methodology (see `rtk/.claude/rules/cli-
//! testing.md`). The idea: a compactor's job is to produce a *specific*
//! shrunken form; an unrelated refactor that happens to drop a few
//! more tokens but mangles the structure should FAIL these tests so we
//! catch the regression before it ships.
//!
//! Workflow:
//!   `cargo insta test`     run all snapshot tests
//!   `cargo insta review`   interactively accept/reject diffs
//!
//! Each test pairs a representative fixture with `assert_snapshot!`.
//!
//! The snapshots live in `misc/snapshots/` and their name carries the
//! `misc__` prefix: insta builds the path from the name of the test BINARY
//! then of the module. This subject was its own target
//! (`tests/snapshot_compactors.rs`), so the files were called
//! `snapshot_compactors__*.snap`; on entering the `misc` target they took the
//! binary's prefix. If this subject changes group, the eight files must
//! follow — insta prints the exact path it expected, so the failure reads in
//! a second.
//! Snapshots live under `tests/snapshots/` and are committed to source
//! control. Reviewing a diff is a one-line decision: "is the new
//! output STILL the right compaction?"

use tokenade_core::{auto, json as tokenade_json};

#[test]
fn snapshot_json_compactor_folds_array_to_lossless_toon() {
    // 60-item array → compactor folds it into a lossless TOON block
    // (every row preserved, keys hoisted to a single header).
    let mut input = String::from("{\"items\": [");
    for i in 0..60 {
        if i > 0 {
            input.push(',');
        }
        input.push_str(&format!(
            "{{\"id\":{i},\"name\":\"user_{i}\",\"plan\":\"pro\"}}"
        ));
    }
    input.push_str("], \"total\": 60}");
    let out = tokenade_json::compact(&input);
    insta::assert_snapshot!("json_60_items_array", out);
}

#[test]
fn snapshot_auto_dispatch_recognises_terraform_plan() {
    // Terraform plan with the canonical preamble — auto::detect routes
    // it to the terraform compactor, which folds the
    // `Refreshing state...` block.
    let mut input = String::new();
    for i in 0..30 {
        input.push_str(&format!(
            "module.web[{i}]: Refreshing state... [id=sg-{i}]\n"
        ));
    }
    input.push_str("\nTerraform will perform the following actions:\n\n");
    input.push_str("  # aws_instance.foo will be updated in-place\n");
    input.push_str("Plan: 0 to add, 1 to change, 0 to destroy.\n");
    let (kind, out, _sav) = auto::compact(&input);
    insta::assert_snapshot!(
        "auto_terraform_plan",
        format!("kind={:?}\n---\n{out}", kind)
    );
}

#[test]
fn snapshot_auto_dispatch_recognises_dig_output() {
    // dig output → DNS compactor groups by record type and shows count.
    let mut input = String::from("; <<>> DiG 9.18.0 <<>> example.com any\n;; ANSWER SECTION:\n");
    for r in ["A", "AAAA", "MX", "TXT"] {
        for i in 1..=6 {
            input.push_str(&format!("example.com. 3600  IN  {r}  value-{r}-{i}\n"));
        }
    }
    let (kind, out, _sav) = auto::compact(&input);
    insta::assert_snapshot!("auto_dig_dns", format!("kind={:?}\n---\n{out}", kind));
}
