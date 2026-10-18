//! Terraform / OpenTofu plan-output compaction.
//!
//! A real `terraform plan` against a non-trivial state is dominated by:
//!
//!   1. One `Refreshing state…` line per resource (hundreds, sometimes
//!      thousands) — pure progress chatter.
//!   2. Per-resource blocks that list **every attribute**, even the ones
//!      that haven't changed (`name = "old"` vs `~ name = "old" -> "new"`).
//!   3. A final `Plan: N to add, N to change, N to destroy.` summary —
//!      pure signal.
//!
//! The signal is: the resource header (`# aws_instance.foo will be …`),
//! the lines starting with `+`/`-`/`~`/`<=`, and the final plan summary.
//! Everything else is noise unless the agent specifically asked for it.
//!
//! [`compact`] strips the refresh chatter, drops unchanged attribute lines
//! inside resource blocks, and emits a compact `[tokenade:terraform] N
//! Refreshing lines folded` header so the agent knows what was dropped.

/// Strip a leading Terraform change marker (`~ `, `+ `, `- `, `+/- `, `<= `)
/// from a trimmed line, returning the bare content.
fn strip_tf_marker(s: &str) -> &str {
    for prefix in ["+/- ", "~ ", "+ ", "- ", "<= "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest;
        }
    }
    s
}

/// Compact a terraform / opentofu plan output.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 4);
    let mut refresh_count = 0usize;
    let mut in_resource_block = false;
    // Indent at which the `# ... will be ...` header appeared.
    let mut block_indent: usize = 0;
    // Whether the `resource "..." "..." {` declaration line has been seen.
    // Until it has, a top-level line (e.g. the Plan summary) closes the block
    // and falls through instead of being swallowed as an attribute.
    let mut block_opened = false;

    for raw in text.lines() {
        let line = raw;
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        // 1. Drop refresh chatter (one line per resource).
        if trimmed.ends_with("Refreshing state...")
            || trimmed.contains(": Refreshing state... [id=")
            || trimmed.contains(": Read complete after ")
            || trimmed.contains(": Reading...")
        {
            refresh_count += 1;
            continue;
        }

        // 2. Resource block tracking.
        //
        // Open block when we see the planner header comment.
        //   "  # aws_instance.web will be updated in-place"
        //   "  # aws_instance.gone will be destroyed"
        //   "  # aws_instance.new will be created"
        if trimmed.starts_with("# ")
            && (trimmed.contains(" will be ") || trimmed.contains(" must be replaced"))
        {
            in_resource_block = true;
            block_indent = indent;
            block_opened = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_resource_block {
            // The `resource "type" "name" {` declaration follows the `#`
            // header (with any change-marker prefix).  Accept it regardless
            // of indent and mark the block as open.
            let bare = strip_tf_marker(trimmed);
            if !block_opened && (bare.starts_with("resource ") || bare.starts_with("data ")) {
                block_opened = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }

            if indent <= block_indent {
                if trimmed.starts_with('}') || trimmed.is_empty() {
                    // Normal block close.
                    in_resource_block = false;
                    block_opened = false;
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
                if !block_opened {
                    // A top-level line arrived before the resource declaration
                    // (e.g. `Plan: N to add…` immediately after a `# … will
                    // be …` header with no actual block).  Close and fall
                    // through so the line is kept verbatim as outside-block.
                    in_resource_block = false;
                }
                // If block_opened but we're back at block_indent with a
                // non-} non-empty line, treat it as a new section boundary.
                if in_resource_block {
                    in_resource_block = false;
                    block_opened = false;
                }
            }

            if !in_resource_block {
                // Fell through from a block-close: emit as outside-block.
                out.push_str(line);
                out.push('\n');
                continue;
            }

            // Inside open block body: keep change-marker lines, drop the rest.
            if trimmed.starts_with("~ ")
                || trimmed.starts_with("+ ")
                || trimmed.starts_with("- ")
                || trimmed.starts_with("+/- ")
                || trimmed.starts_with("<= ")
                || trimmed.starts_with("# (")
                || trimmed.starts_with("# forces replacement")
                || trimmed.ends_with('{')
                || trimmed == "}"
            {
                out.push_str(line);
                out.push('\n');
            }
            // Drop unchanged attribute lines silently.
            continue;
        }

        // 3. Outside blocks: keep everything (errors, warnings, summaries,
        // diagnostics — all signal).
        out.push_str(line);
        out.push('\n');
    }

    // Emit the refresh-chatter summary at the top so the agent immediately
    // sees what was dropped.
    if refresh_count > 0 {
        let header =
            format!("[tokenade:terraform] {refresh_count} `Refreshing state…` line(s) folded\n",);
        return format!("{header}{}", out.trim_end());
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn folds_refresh_chatter() {
        let mut p = String::new();
        for i in 0..200 {
            let _ = writeln!(p, "aws_instance.host{i}: Refreshing state... [id=i-{i:08}]");
        }
        p.push_str("\nPlan: 0 to add, 0 to change, 0 to destroy.\n");
        let out = compact(&p);
        assert!(out.contains("200 `Refreshing state…` line(s) folded"));
        assert!(out.contains("Plan: 0 to add"));
        // Real per-instance lines dropped.
        assert!(!out.contains("Refreshing state..."));
    }

    #[test]
    fn keeps_change_markers_drops_unchanged_attributes() {
        let p = r#"Terraform will perform the following actions:

  # aws_instance.web will be updated in-place
  ~ resource "aws_instance" "web" {
        ami                          = "ami-0c2b8ca1dad447f8a"
        arn                          = "arn:aws:ec2:..."
        associate_public_ip_address  = true
      ~ instance_type                = "t3.micro" -> "t3.small"
        availability_zone            = "us-east-1a"
        cpu_core_count               = 1
    }

Plan: 0 to add, 1 to change, 0 to destroy.
"#;
        let out = compact(p);
        // Header + change line + summary preserved.
        assert!(out.contains("aws_instance.web will be updated"));
        assert!(out.contains("~ instance_type"));
        assert!(out.contains("Plan: 0 to add, 1 to change"));
        // Unchanged attributes dropped.
        assert!(!out.contains("ami-0c2b8ca1dad447f8a"));
        assert!(!out.contains("availability_zone"));
        assert!(!out.contains("cpu_core_count"));
        assert!(!out.contains("arn:aws:ec2:"));
    }

    #[test]
    fn create_destroy_replace_all_kept() {
        let p = r#"  # aws_a.create will be created
  + resource "aws_a" "create" {
      + id = (known after apply)
      + name = "new"
    }

  # aws_a.gone will be destroyed
  - resource "aws_a" "gone" {
      - id = "old-id" -> null
      - name = "gone"
    }

  # aws_a.swap must be replaced
+/- resource "aws_a" "swap" {
      ~ id = "X" -> (known after apply) # forces replacement
        name = "same"
    }

Plan: 1 to add, 0 to change, 1 to destroy.
"#;
        let out = compact(p);
        assert!(out.contains("create"));
        assert!(out.contains("destroyed"));
        assert!(out.contains("must be replaced"));
        assert!(out.contains("forces replacement"));
        // The `name = "same"` line in the swap block is unchanged → dropped.
        assert!(!out.contains("name = \"same\""));
    }

    #[test]
    fn no_panic_on_garbage() {
        let _ = compact("");
        let _ = compact("not terraform output at all\n");
    }

    #[test]
    fn savings_real_shape() {
        let mut p = String::new();
        for i in 0..500 {
            let _ = writeln!(p, "aws_instance.x{i}: Refreshing state... [id=i-{i:08}]");
        }
        p.push_str(
            "\nTerraform will perform the following actions:\n\n  # aws_instance.web will be updated in-place\n  ~ resource \"aws_instance\" \"web\" {\n",
        );
        for j in 0..50 {
            let _ = writeln!(p, "        attr_{j} = \"unchanged value {j}\"");
        }
        p.push_str(
            "      ~ instance_type = \"t3.micro\" -> \"t3.small\"\n    }\n\nPlan: 0 to add, 1 to change, 0 to destroy.\n",
        );
        let (out, sav) = compact_with_savings(&p);
        assert!(out.contains("~ instance_type"));
        assert!(out.contains("Plan: 0 to add"));
        assert!(sav.percent() > 85.0, "got {:?}", sav);
    }
}
