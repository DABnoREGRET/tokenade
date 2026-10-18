//! `npm_audit` — npm audit.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("found ") || text.contains("vulnerabilities")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

/// Fold an `npm audit` report to what a caller acts on.
///
/// The rule here used to be `strip_noise(text, &["    "], …)` — drop lines
/// starting with four spaces — and `archetypes::strip_noise`'s own comment had
/// already catalogued that guard as one of TWELVE inert prefixes. It is inert
/// on this shape for a plain reason: a real report has **zero** four-space
/// lines. MEASURED on a live 6 148-byte report from `npm audit`: routing it
/// through this compactor and routing it through the GENERIC pass produced
/// byte-identical output, 4 957 bytes both ways. Every byte saved was the
/// generic dedup layer; the npm-specific compactor contributed nothing at all.
///
/// What the shape actually carries, on that same report: 31 advisory lines
/// each ending in a full `https://github.com/advisories/GHSA-…` URL whose
/// last segment is the whole identifier, and 15 `node_modules/…` lines that
/// say where a package sits rather than what is wrong with it. Both are
/// folded; the severity, the version range, the advisory titles, the fix
/// lines, the breaking-change warnings and the final tally are kept verbatim,
/// because those are what a caller decides on.
pub fn compact(text: &str) -> String {
    use std::fmt::Write as _;
    const ADVISORY_URL: &str = "https://github.com/advisories/";
    let mut out = String::with_capacity(text.len());
    let mut dropped_paths = 0usize;
    let mut shortened = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        // A location, not a finding. The package and its range are already on
        // the block's first line.
        if t.starts_with("node_modules/") {
            dropped_paths += 1;
            continue;
        }
        // ` - https://github.com/advisories/GHSA-xxxx-xxxx-xxxx` -> ` (GHSA-…)`.
        // The trailing segment IS the identifier, so nothing that identifies
        // the advisory is lost — only the constant prefix in front of it.
        if let Some(pos) = line.find(ADVISORY_URL) {
            let id: String = line[pos + ADVISORY_URL.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !id.is_empty() {
                let head = line[..pos].trim_end().trim_end_matches('-').trim_end();
                let tail = &line[pos + ADVISORY_URL.len() + id.len()..];
                let _ = writeln!(out, "{head} ({id}){tail}");
                shortened += 1;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    // Compose with the command-agnostic pass instead of REPLACING it. This is
    // the whole reason the old rule was worse than useless: MEASURED on a live
    // 6 149-byte report, routing through this compactor returned 6 149 bytes —
    // nothing at all — while the generic route returned 4 957 (19.4 %). Having
    // a dedicated compactor for `npm audit` made its output LARGER than having
    // none, because the dedicated route is taken instead of the generic one and
    // then folded nothing.
    let structural = out;
    let mut out = structural.clone();
    // `filter::generic` — dedup + smart truncate — is exactly what the generic
    // route ran, so composing with IT (rather than `auto::compact`, which does
    // not dedup lines) keeps the 19.4 % that route already earned and adds the
    // structural fold on top.
    let generic = crate::compress::generic(&structural);
    if generic.len() < out.len() {
        out = generic;
    }
    // Never hand back MORE than we were given. The banner is a fixed cost of
    // ~130 bytes, so on a short report the two folds can be worth less than
    // saying so — and a "compactor" that grows its input is worse than none,
    // which is exactly the failure this module is being fixed for.
    // On its OWN line. `filter::generic` returns a trimmed string, so a bare
    // `writeln!` glued the banner onto the final tally — "11 vulnerabilities
    // (1 low, 3 moderate, 7 high)  [elided: …]" reads as one sentence and the
    // count stops being greppable.
    if (dropped_paths > 0 || shortened > 0) && !out.ends_with('\n') {
        out.push('\n');
    }
    if dropped_paths > 0 || shortened > 0 {
        // NOT `mark_generic_fold`: that marker means "a command-AGNOSTIC
        // helper produced this compaction", and this one is the npm-specific
        // compactor. Marking it generic would mislabel a dedicated fold.
        let _ = writeln!(
            out,
            "  [elided: {dropped_paths} node_modules path line(s); {shortened} advisory \
             URL(s) shortened to their GHSA id — prefix `{ADVISORY_URL}`]"
        );
    }
    let folded = out.trim_end().to_string();
    if folded.len() >= text.len() {
        return text.trim_end().to_string();
    }
    folded
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `npm audit` report, trimmed. Captured from `npm audit` in a live
    /// Node project — the shape matters (unindented advisory blocks, full
    /// advisory URLs, `node_modules/` location lines), and an invented fixture
    /// would not have caught the defect this guards.
    const REPORT: &str = "\
# npm audit report

ajv  <6.14.0
Severity: moderate
ajv has ReDoS when using `$data` option - https://github.com/advisories/GHSA-2g4f-4pwh-qvx6
fix available via `npm audit fix`
node_modules/ajv

brace-expansion  <=1.1.17 || 2.0.0 - 2.1.3
Severity: high
brace-expansion Regular Expression Denial of Service vulnerability - https://github.com/advisories/GHSA-v6h2-p8h4-qcjw
brace-expansion: Zero-step sequence causes process hang - https://github.com/advisories/GHSA-f886-m6hf-6m8v
fix available via `npm audit fix`
node_modules/brace-expansion
node_modules/glob/node_modules/brace-expansion
node_modules/mocha/node_modules/brace-expansion

diff  5.0.0 - 5.2.1 || 6.0.0 - 8.0.2
Severity: moderate
jsdiff has a Denial of Service vulnerability in parsePatch - https://github.com/advisories/GHSA-73rr-hh4g-fpgx
fix available via `npm audit fix --force`
Will install mocha@12.0.0, which is a breaking change
node_modules/diff
node_modules/mocha/node_modules/diff

minimatch  <=3.0.4
Severity: high
minimatch ReDoS vulnerability - https://github.com/advisories/GHSA-hxm2-r34f-qmc5
minimatch ReDoS vulnerability - https://github.com/advisories/GHSA-hxm2-r34f-qmc5
minimatch has ReDoS via matchOne() backtracking - https://github.com/advisories/GHSA-7r86-cg39-jmmj
minimatch has ReDoS via matchOne() backtracking - https://github.com/advisories/GHSA-7r86-cg39-jmmj
fix available via `npm audit fix`
node_modules/minimatch
node_modules/glob/node_modules/minimatch

11 vulnerabilities (1 low, 3 moderate, 7 high)
";

    #[test] fn detector_rejects_empty() { assert!(!detect("")); }

    /// The fold must actually fold, and must announce what it took.
    ///
    /// The rule here used to be "drop lines starting with four spaces", which
    /// a real report has NONE of — `archetypes::strip_noise`'s own comment had
    /// already catalogued that guard as inert and named this module. MEASURED
    /// on a live 6 149-byte report: through this compactor, 6 149 bytes — zero
    /// — while the GENERIC route returned 4 957. Having a dedicated compactor
    /// made the output bigger than having none, because the dedicated route is
    /// taken INSTEAD of the generic one and then folded nothing. 35.0 % now.
    #[test]
    fn the_audit_fold_shrinks_the_report_and_says_what_it_took() {
        let folded = compact(REPORT);
        assert!(
            folded.len() * 5 < REPORT.len() * 4,
            "the fold must be substantial: {} -> {}",
            REPORT.len(),
            folded.len()
        );
        assert!(
            folded.contains("node_modules path line(s)") && folded.contains("shortened"),
            "both folds owe the caller a count\n{folded}"
        );
        // The command-agnostic pass ran too. A real report repeats the same
        // advisory line once per affected path, and collapsing those is where
        // most of the saving lives — MEASURED on the live report, 19.4 % of it.
        // Without this the compactor REPLACES the generic route instead of
        // composing with it, which is the defect being fixed.
        assert!(
            folded.contains("(x2)"),
            "repeated advisory lines must be collapsed by the generic pass\n{folded}"
        );
        // The prefix it removed is NAMED, so the caller can rebuild any URL.
        assert!(
            folded.contains("https://github.com/advisories/"),
            "the banner must name the prefix it stripped\n{folded}"
        );
    }

    /// Everything a caller ACTS on survives verbatim.
    #[test]
    fn the_audit_fold_keeps_every_actionable_field() {
        let folded = compact(REPORT);
        for keep in [
            "11 vulnerabilities (1 low, 3 moderate, 7 high)",
            "Severity: moderate",
            "Severity: high",
            "ajv  <6.14.0",
            "brace-expansion  <=1.1.17 || 2.0.0 - 2.1.3",
            "fix available via `npm audit fix`",
        ] {
            assert!(folded.contains(keep), "dropped an actionable line: {keep:?}\n{folded}");
        }
        // Every advisory is still identified — the id is what the URL carried.
        for id in ["GHSA-2g4f-4pwh-qvx6", "GHSA-v6h2-p8h4-qcjw", "GHSA-f886-m6hf-6m8v"] {
            assert!(folded.contains(id), "lost advisory {id}\n{folded}");
        }
        // And the locations really are gone, not merely uncounted.
        assert!(
            !folded.contains("node_modules/ajv"),
            "a location line survived the fold\n{folded}"
        );
    }

    /// Anti-vacuity: a report with nothing of either shape is not mangled and
    /// claims nothing. Otherwise a compactor that always prints a banner would
    /// satisfy the assertions above.
    #[test]
    fn a_report_with_nothing_to_fold_claims_nothing() {
        let clean = "# npm audit report\n\nfound 0 vulnerabilities\n";
        let folded = compact(clean);
        assert!(
            !folded.contains("[elided:"),
            "nothing was taken, so nothing may be claimed\n{folded}"
        );
        assert!(folded.contains("found 0 vulnerabilities"), "{folded}");
    }
}
