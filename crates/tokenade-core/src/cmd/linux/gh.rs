//! `gh` — GitHub CLI list subcommands (`gh pr/issue/run/repo/release/
//! workflow/gist/secret/alias/extension/codespace list`).
//!
//! In a non-TTY context — any captured pipe, which is how an agent runs it —
//! `gh ... list` emits one record per line as TAB-separated columns: e.g.
//! `gh pr list` → `<number>\t<title>\t<branch>\t<state>`, `gh run list` →
//! `<status>\t<conclusion>\t<name>\t<workflow>\t<branch>\t<event>\t<id>\t…`.
//! A busy repo lists dozens to hundreds of rows; that bulk is the cost.
//!
//! The state tokens gh prints (`OPEN`/`MERGED`/`completed`/…) are machine
//! constants — gh does NOT run them through gettext — but the title/name
//! columns are free text that can be non-ASCII in any language. So detection
//! keys ONLY on the locale-invariant TAB column structure, never on a state
//! label (unlike the dead `detect()` it replaces, which required a literal
//! `#`/`OPEN`/`DRAFT`). The compactor folds the middle of the row stream,
//! keeping the first/last rows verbatim — every id/number/name survives —
//! with an announced counter and the real total row count in the banner. It
//! declines on a short list so a handful of rows passes through untouched.

const KEEP_EDGE: usize = 4;
const MIN_ROWS: usize = 12;
/// A tab-separated record has at least this many non-empty fields
/// (`gh secret list` / `gh alias list` emit two columns; most emit more).
const MIN_COLS: usize = 2;

fn tab_fields(line: &str) -> usize {
    if !line.contains('\t') {
        return 0;
    }
    line.split('\t').filter(|f| !f.trim().is_empty()).count()
}

/// Structural detector: a dominant run of TAB-delimited rows. Never keyed on
/// a state label, so a localized title column folds identically.
pub fn detect(text: &str) -> bool {
    let mut nonblank = 0usize;
    let mut rows = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        nonblank += 1;
        if tab_fields(line) >= MIN_COLS {
            rows += 1;
        }
    }
    rows >= MIN_ROWS && rows * 10 >= nonblank * 7
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::fold_middle(text, KEEP_EDGE, "gh", "row(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use super::*;

    fn pr_list(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let _ = writeln!(s, "{num}\tFix issue number {i}\tfix-{i}\tOPEN", num = i + 1);
        }
        s
    }

    fn run_list(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let _ = writeln!(
                s,
                "completed\tsuccess\tCI Build {i}\tci.yml\tmain\tpush\t{id}\t1m20s\t2026-05-{day:02}T10:00:00Z",
                id = 1_000_000 + i,
                day = (i % 28) + 1
            );
        }
        s
    }

    #[test]
    fn detector_structural_not_keyword() {
        // 30 tab rows → detect, with NO `#`/`OPEN`/`DRAFT` anywhere.
        let body = "10\tein titel\tzweig\tx\n".repeat(30);
        assert!(detect(&body), "tab structure alone must trigger detection");
        assert!(!body.contains('#') && !body.contains("OPEN"));
        // Free prose without tabs must not look like a gh list.
        let prose = "This is a description of the project.\n\
                     It spans a couple of plain sentences.\n"
            .repeat(10);
        assert!(!detect(&prose), "tab-less prose must not match");
    }

    #[test]
    fn short_list_declines() {
        assert!(try_compact(&pr_list(5)).is_none());
        assert!(try_compact("13\tonly one row\tbranch\tOPEN\n").is_none());
    }

    #[test]
    fn gain_on_large_list() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let body = pr_list(100);
        let (out, sav) = compact_with_savings(&body);
        assert!(out.contains("[tokenade:gh]"));
        assert!(
            sav.ratio() >= 0.50,
            "expected >=50% savings on a 100-row list, got {:.2} ({}→{})",
            sav.ratio(),
            sav.tokens_before,
            sav.tokens_after
        );
    }

    #[test]
    fn quality_real_count_edges_verbatim_fold_announced() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let body = pr_list(100);
        let out = try_compact(&body).expect("100 rows must fold");
        // Real total in the banner.
        assert!(
            out.contains("100 row(s)"),
            "banner must carry real total: {}",
            out.lines().next().unwrap()
        );
        // First/last KEEP_EDGE rows preserved verbatim (PR numbers + titles).
        assert!(out.contains("1\tFix issue number 0\tfix-0\tOPEN"), "first row dropped");
        assert!(out.contains("100\tFix issue number 99\tfix-99\tOPEN"), "last row dropped");
        // Fold announced with a counter; folded = 100 - 2*KEEP_EDGE.
        let folded = 100 - KEEP_EDGE * 2;
        assert!(out.contains(&format!("folded {folded}")), "fold not announced: {out}");
        // A middle row is folded away (not leaked verbatim).
        assert!(!out.contains("Fix issue number 50"), "middle row leaked verbatim");
        // Accounting closes.
        assert_eq!(KEEP_EDGE * 2 + folded, 100);
    }

    #[test]
    fn run_list_shape_folds() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        // `gh run list` has more columns and different state constants —
        // structural detection covers it without any per-shape logic.
        let body = run_list(40);
        let out = try_compact(&body).expect("40 runs must fold");
        assert!(out.contains("[tokenade:gh]"));
        assert!(out.contains("\t1000000\t"), "first run id dropped");
        assert!(out.contains("\t1000039\t"), "last run id dropped");
        assert!(out.contains("40 row(s)"));
    }

    #[test]
    fn localized_titles_fold_identically() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        // A German/French issue list: NO English anywhere, non-ASCII titles,
        // labels and a dd.mm.yyyy date column. Structural detection and the
        // fold behave exactly as for the ASCII list, and the non-ASCII edge
        // rows survive byte-for-byte.
        let mut body = String::new();
        body.push_str("1\tBehebt den Anmeldefehler\tmerkmal/anmeldung\tFehler, Priorität-hoch\t01.05.2026\n");
        body.push_str("2\tCorrige la connexion réseau\tcorrectif/réseau\tbogue, sécurité\t02.05.2026\n");
        for i in 3..30 {
            let _ = writeln!(
                body,
                "{i}\tÄnderung Nummer {i} – Überarbeitung\tzweig/{i}\twartung\t{i:02}.05.2026"
            );
        }
        body.push_str("30\tDernière révision – clôturée\tfinal/clôture\tterminé\t31.05.2026\n");

        assert!(detect(&body), "structural detection must not need English");
        let out = try_compact(&body).expect("localized list must fold");
        assert!(
            out.contains("1\tBehebt den Anmeldefehler\tmerkmal/anmeldung\tFehler, Priorität-hoch\t01.05.2026"),
            "leading non-ASCII row mangled: {out}"
        );
        assert!(
            out.contains("30\tDernière révision – clôturée\tfinal/clôture\tterminé\t31.05.2026"),
            "trailing non-ASCII row mangled: {out}"
        );
        let total = body.lines().count();
        assert!(out.contains(&format!("{total} row(s)")), "real total missing for localized input");
        assert!(out.contains("folded "), "fold counter missing for localized input");
    }

    #[test]
    fn preserves_trailing_newline_state() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let with = pr_list(30);
        assert!(try_compact(&with).unwrap().ends_with('\n'));
        let without = with.trim_end_matches('\n').to_string();
        let out = try_compact(&without).expect("must still fold");
        assert!(!out.ends_with('\n'), "must mirror missing trailing newline");
    }

    #[test]
    fn returns_something_via_compact() {
        let _ = compact(&pr_list(30));
        let _ = compact("13\tonly one\tbranch\tOPEN\n");
    }
}
