//! Dispatcher for `ansible`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::compress::dedup_lines;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    let verbosity_level = argv
        .iter()
        .filter(|a| a.starts_with("-v") && a.chars().all(|c| c == '-' || c == 'v'))
        .map(|a| a.matches('v').count())
        .max()
        .unwrap_or(0);
    let s = strip_ansi(output);
    if verbosity_level >= 2 {
        // Very verbose — keep only PLAY/TASK headers + failed/changed lines.
        let mut keep = Vec::new();
        for line in s.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("PLAY ") || t.starts_with("TASK ")
                || t.contains("failed:") || t.contains("changed:") || t.contains("unreachable:")
                || t.starts_with("PLAY RECAP") || t.contains("FAILED!")
            {
                keep.push(line);
            }
        }
        if keep.is_empty() {
            return generic(output);
        }
        return truncate_middle(&keep.join("\n"), 80, 20);
    }
    // Default: keep all lines, dedup, truncate.
    let cleaned = dedup_lines(&s);
    truncate_middle(&cleaned, 60, 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_mode_dedups_and_strips_ansi() {
        // No `-v` flag → default path: strip ANSI, dedup runs, truncate.
        let body = "\x1b[32mok\x1b[0m\n\
                    repeated\nrepeated\nrepeated\n\
                    done\n";
        let out = dispatch(&argv(&["ansible-playbook", "site.yml"]), body);
        assert!(!out.contains("\x1b["), "must strip ANSI");
        assert_eq!(
            out.matches("repeated").count(),
            1,
            "default must dedup; got: {out:?}"
        );
        assert!(out.contains("ok") && out.contains("done"));
    }

    #[test]
    fn single_v_uses_default_path_not_signal_filter() {
        // `-v` is level 1 (< 2 threshold). Must NOT apply the
        // PLAY/TASK-only filter — should dedup like default mode.
        let body = "Gathering facts...\n\
                    ok: [host1]\n\
                    ok: [host2]\n\
                    Some arbitrary detail line\n";
        let out = dispatch(&argv(&["ansible-playbook", "-v", "site.yml"]), body);
        // The "Some arbitrary detail line" doesn't match PLAY/TASK/failed
        // etc., so under -vv+ it would be dropped. Under -v it survives.
        assert!(
            out.contains("Some arbitrary detail line"),
            "-v must NOT engage the signal-only filter; got: {out}"
        );
    }

    #[test]
    fn vv_engages_signal_line_filter() {
        // `-vv` (level 2) → keep only PLAY/TASK/failed/changed/etc.
        let body = "PLAY [Deploy] *********\n\
                    Gathering facts...\n\
                    TASK [install nginx] **\n\
                    ok: [host1]\n\
                    changed: [host2]\n\
                    failed: [host3]\n\
                    Some chatter detail line\n\
                    PLAY RECAP ************\n\
                    host1: ok=2 changed=0\n";
        let out = dispatch(&argv(&["ansible-playbook", "-vv", "site.yml"]), body);
        // Signal lines kept.
        assert!(out.contains("PLAY [Deploy]"));
        assert!(out.contains("TASK [install nginx]"));
        assert!(out.contains("changed:"));
        assert!(out.contains("failed:"));
        assert!(out.contains("PLAY RECAP"));
        // Chatter dropped.
        assert!(!out.contains("Gathering facts"));
        assert!(!out.contains("chatter detail"));
        // Note: `ok: [host1]` doesn't match any signal predicate
        // (no "ok:" branch), so it's dropped too — that's intentional;
        // the dispatcher prioritizes failures + changes over OKs.
    }

    #[test]
    fn vvv_and_vvvv_also_use_filter() {
        // Higher verbosity levels also engage the filter — same threshold.
        let body = "TASK [foo] ****\nFAILED! something\n\
                    Gathering facts...\n";
        for flag in &["-vvv", "-vvvv"] {
            let out = dispatch(&argv(&["ansible-playbook", flag, "site.yml"]), body);
            assert!(out.contains("TASK [foo]"));
            assert!(out.contains("FAILED!"));
            assert!(!out.contains("Gathering facts"));
        }
    }

    #[test]
    fn version_like_flag_does_not_inflate_verbosity() {
        // `-vault-password-file` STARTS with `-v` but contains non-v
        // alphabetics, so the all(|c| ...) gate must reject it.
        // Otherwise this would be counted as 2 `v` chars → engage the
        // signal filter when the user really just wanted default mode.
        let body = "Gathering facts...\n\
                    Some detail to keep\n";
        let out = dispatch(
            &argv(&[
                "ansible-playbook",
                "-vault-password-file",
                "/etc/vault",
                "site.yml",
            ]),
            body,
        );
        // Detail line survives → default path engaged (not filter).
        assert!(
            out.contains("Some detail to keep"),
            "-vault-password-file must NOT count as verbosity; got: {out}"
        );
    }

    #[test]
    fn max_verbosity_wins_when_multiple_flags() {
        // ansible accepts multiple `-v` repeats; take the MAX.
        let body = "TASK [x] ****\nFAILED! err\ndetail line\n";
        let out = dispatch(
            &argv(&["ansible-playbook", "-v", "-vvv", "-v", "site.yml"]),
            body,
        );
        // Max is 3 (-vvv) → engage filter → "detail line" dropped.
        assert!(out.contains("TASK [x]"));
        assert!(out.contains("FAILED!"));
        assert!(!out.contains("detail line"));
    }

    #[test]
    fn vv_with_no_signal_lines_falls_back_to_generic() {
        // -vv engaged but no PLAY/TASK/failed/etc. lines → signal
        // filter yields empty → fallback to generic so we don't
        // return an empty result.
        let body = "Random output\n\
                    no recognized ansible markers\n\
                    just prose\n";
        let out = dispatch(&argv(&["ansible-playbook", "-vv", "site.yml"]), body);
        assert!(!out.is_empty(), "generic fallback must not be empty");
        assert!(
            out.contains("Random output") || out.contains("prose"),
            "generic must preserve content; got: {out}"
        );
    }
}
