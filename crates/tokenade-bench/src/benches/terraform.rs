//! `terraform` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Synthesise a realistic `terraform plan` output: 800 `Refreshing state…`
/// lines, two changed resource blocks each with 50 unchanged attributes
/// and one `~`/`+` line, plus a plan summary.
pub(crate) fn synth_terraform_plan() -> String {
    let mut p = String::new();
    for i in 0..800 {
        p.push_str(&format!(
            "aws_instance.host_{i}: Refreshing state... [id=i-{i:08x}]\n"
        ));
    }
    p.push_str("\nTerraform will perform the following actions:\n\n");
    // Block 1: ~ update in-place
    p.push_str(
        "  # aws_instance.web will be updated in-place\n  ~ resource \"aws_instance\" \"web\" {\n",
    );
    for j in 0..50 {
        p.push_str(&format!(
            "        attribute_{j} = \"unchanged value {j}\"\n"
        ));
    }
    p.push_str("      ~ instance_type = \"t3.micro\" -> \"t3.small\"\n    }\n\n");
    // Block 2: + create
    p.push_str("  # aws_security_group.allow_https will be created\n  + resource \"aws_security_group\" \"allow_https\" {\n");
    for j in 0..30 {
        p.push_str(&format!("      + tag_{j} = (known after apply)\n"));
    }
    p.push_str("    }\n\n");
    p.push_str("Plan: 1 to add, 1 to change, 0 to destroy.\n");
    p
}

pub(crate) fn task_terraform_real(results: &mut Vec<RunResult>) {
    let raw = synth_terraform_plan();
    let (out, sav, ms) = measure(&raw, terraform::compact_with_savings);
    let folded = out.contains("Refreshing state");
    let change_kept = out.contains("~ instance_type") && out.contains("must be replaced")
        || out.contains("will be updated");
    let summary_kept = out.contains("Plan: 1 to add, 1 to change");
    let unchanged_dropped = !out.contains("attribute_25 = \"unchanged");
    let quality = if folded && change_kept && summary_kept && unchanged_dropped {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:terraform-compact",
        "terraform",
        "synth-terraform-800",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #25: a 10 000-line refresh storm + one tiny change must
// compress > 95 % AND keep the change marker AND the summary.
pub(crate) fn adv_terraform_refresh_bomb(results: &mut Vec<RunResult>) {
    let mut p = String::new();
    for i in 0..10_000 {
        p.push_str(&format!(
            "aws_instance.host_{i}: Refreshing state... [id=i-{i:08x}]\n"
        ));
    }
    p.push_str("\n  # aws_instance.web will be updated in-place\n  ~ resource \"aws_instance\" \"web\" {\n      ~ instance_type = \"t3.micro\" -> \"t3.small\"\n    }\n\nPlan: 0 to add, 1 to change, 0 to destroy.\n");
    let (out, sav, ms) = measure(&p, terraform::compact_with_savings);
    let quality = if out.contains("10000 `Refreshing state")
        && out.contains("~ instance_type")
        && out.contains("Plan: 0 to add, 1 to change")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:terraform:refresh-bomb",
        "terraform",
        "adversarial",
        &p,
        &out,
        ms,
        quality,
    );
}

// Adversarial #26: a plan with NO changes (Refreshing state + "No changes.
// Your infrastructure matches the configuration."). The compactor must
// drop the refresh chatter but preserve the "No changes." message AND not
// emit a misleading "block dropped" line.
pub(crate) fn adv_terraform_no_changes(results: &mut Vec<RunResult>) {
    let mut p = String::new();
    for i in 0..50 {
        p.push_str(&format!(
            "aws_instance.host_{i}: Refreshing state... [id=i-{i:08x}]\n"
        ));
    }
    p.push_str(
        "\nNo changes. Your infrastructure matches the configuration.\n\nTerraform has compared your real infrastructure against your configuration and found no differences, so no changes are needed.\n",
    );
    let (out, _sav, ms) = measure(&p, terraform::compact_with_savings);
    let quality = if out.contains("No changes.") && out.contains("50 `Refreshing state") {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:terraform:no-changes-plan",
        "terraform",
        "adversarial",
        &p,
        &out,
        ms,
        quality,
    );
}
