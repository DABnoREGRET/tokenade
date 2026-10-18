//! Package-manager install-output compaction.
//!
//! Covers the four package managers an agent meets most often:
//!
//!   * **pip** — `Collecting …`, `Downloading …`, per-package progress bars,
//!     `Installing collected packages: …`, `Successfully installed …`.
//!   * **npm** — `npm WARN deprecated …` (one per transitive dep), `npm ERR! …`,
//!     `added N packages, and audited M packages in Xs`, vulnerability counts,
//!     funding hints.
//!   * **yarn** (v1) — `[N/M] 🔍 Resolving packages`, `Done in Xs.`, warning
//!     lines starting with `warning `.
//!   * **pnpm** — `Progress: resolved …`, `+ package@version`, `WARN deprecated`,
//!     `Done in Xs`.
//!
//! Real `pip install -r requirements.txt` on a non-trivial app emits 500 –
//! 5 000 lines, ≥95 % of which are `Downloading`/`Collecting`/progress noise.
//! The agent reading the install log really only needs:
//!
//!   - **what was installed/added** (the success summary)
//!   - **every warning/deprecation/vulnerability**
//!   - **every error** verbatim — those are the diagnostic gold
//!   - **the final timing/audit line**

use std::fmt::Write;

pub const INSTALLED_CAP: usize = 200;
pub const WARNINGS_CAP: usize = 100;
pub const ERRORS_CAP: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PM {
    Pip,
    Npm,
    Yarn,
    Pnpm,
    Unknown,
}

pub fn detect(text: &str) -> bool {
    detect_pm(text) != PM::Unknown
}

/// npm's update notifier — `npm notice New minor version of npm available!
/// 11.6.4 -> 11.21.0` and its `Changelog` / `To update run` lines — is appended
/// to whatever `npx` or `npm` ran, about once a day. It says nothing about an
/// install, and it made every such output read as one: measured 2026-10-05 on
/// a real `npx slidev build` (a Vite build, 60 asset rows) routed to the
/// install compactor with an `npm: 0 warnings, 0 errors` header. Same for
/// `npx tsc`, `npx eslint` — any npx command on the day the check runs.
fn without_update_notifier(text: &str) -> String {
    let notifier = text
        .lines()
        .any(|l| l.starts_with("npm notice") && l.contains(" version of npm available"));
    if !notifier {
        return text.to_string();
    }
    text.lines()
        .filter(|l| !l.starts_with("npm notice"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn detect_pm(text: &str) -> PM {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    let without_notifier = without_update_notifier(head);
    let head = without_notifier.as_str();

    // pip: `Successfully installed X-1.0 Y-2.0` is unmistakable; otherwise
    // require BOTH `Collecting ` (≥1) AND one of the action markers.
    let has_pip_success =
        head.contains("\nSuccessfully installed ") || head.starts_with("Successfully installed ");
    let has_pip_collecting = head.contains("\nCollecting ") || head.starts_with("Collecting ");
    let has_pip_installing = head.contains("Installing collected packages:");
    let has_pip_downloading = head.contains("Downloading ") && has_pip_collecting;
    if has_pip_success || has_pip_installing || has_pip_downloading {
        return PM::Pip;
    }

    // npm: `npm WARN`, `npm ERR!`, `npm notice` prefixes are unique. Also
    // the final `added N packages` summary.
    let has_npm_prefix =
        head.contains("npm WARN ") || head.contains("npm ERR!") || head.contains("npm notice ");
    let has_npm_summary = head.contains("added ")
        && head.contains(" packages")
        && (head.contains("audited") || head.contains(" in "));
    if has_npm_prefix || has_npm_summary {
        return PM::Npm;
    }

    // yarn v1 (emoji-rich) and v2 (plain).
    let has_yarn = head.contains("yarn install v")
        || head.contains("\nyarn install")
        || head.contains("🔍  Resolving packages")
        || head.contains("✨  Done in ")
        || head.contains("success Saved lockfile.")
        || (head.contains("[1/") && head.contains("] Resolving packages"));
    if has_yarn {
        return PM::Yarn;
    }

    // pnpm.
    let has_pnpm = head.contains("Progress: resolved ")
        || head.contains("\npnpm install")
        || head.contains("pnpm install ")
        || (head.contains("WARN  deprecated") && head.contains("dependencies:"));
    if has_pnpm {
        return PM::Pnpm;
    }

    PM::Unknown
}

pub fn compact(text: &str) -> String {
    // L42 — strip ANSI before parsing. `npm install` and `pip
    // install` colourise progress bars + "Downloading" prefixes
    // heavily; without stripping, the progress lines wouldn't
    // template-match for dedup.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();
    match detect_pm(text) {
        PM::Pip => compact_pip(text),
        PM::Npm => compact_npm(text),
        PM::Yarn => compact_yarn(text),
        PM::Pnpm => compact_pnpm(text),
        PM::Unknown => text.to_string(),
    }
}

/// Lines that ARE pip's own progress noise — safe to fold into the summary.
/// Anything else in a pip-routed document is another program's output.
fn is_pip_noise(t: &str) -> bool {
    t.starts_with("Downloading ")
        || t.starts_with("Using cached ")
        || t.starts_with("Requirement already satisfied")
        || t.starts_with("Preparing metadata")
        || t.starts_with("Building wheel")
        || t.starts_with("Created wheel")
        || t.starts_with("Stored in directory")
        || t.starts_with("Attempting uninstall")
        || t.starts_with("Found existing installation")
        || t.starts_with("Uninstalling ")
        || t.starts_with("Successfully uninstalled")
        || t.starts_with("Getting requirements")
        || t.starts_with("Installing build dependencies")
        || t.starts_with("Running command")
        || t.starts_with("[notice]")
        || t.starts_with('━') // progress bars
        || t.starts_with("Looking in indexes")
        || t.chars().all(|c| c.is_ascii_whitespace() || matches!(c, '|' | '#' | '-' | '/' | '\\'))
        // pip's modern progress bar carries its metrics on the SAME line:
        // `---------------------------------------- 123.4/123.4 kB 14.2 MB/s
        // eta 0:00:00`. The all-characters clause above only catches a bar
        // with nothing after it, so on a 300-package install these 300 lines
        // fell through to `compact_remainder` — where the ARCHIVE detector
        // folded them as a directory listing, because `123.4/123.4` carries a
        // slash. It worked, for the wrong reason, and stopped working the
        // moment that detector learned a listing line ENDS on its path
        // pip's own noise belongs to pip's own compactor.
        //
        // Eight dashes, so a `---` markdown rule and a `--- a/file` diff
        // header are not bars.
        || is_progress_bar(t)
}

/// A drawn progress bar: eight or more bar characters in a row at the start of
/// the line, whatever trails them.
fn is_progress_bar(t: &str) -> bool {
    let head = t.trim_start();
    let run = head
        .chars()
        .take_while(|c| matches!(c, '-' | '=' | '#' | '\u{2501}' | '\u{2500}' | '\u{2588}'))
        .count();
    run >= 8
}

fn compact_pip(text: &str) -> String {
    let mut installed: Vec<String> = Vec::new();
    let mut collected = 0usize;
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut installing_line: Option<String> = None;
    let mut unmatched: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();

        if t.starts_with("Collecting ") {
            collected += 1;
        } else if let Some(rest) = t.strip_prefix("Successfully installed ") {
            // Multiple `Successfully installed` lines possible (e.g. -e
            // editable installs). Accumulate all of them.
            for pkg in rest.split_whitespace() {
                if installed.len() < INSTALLED_CAP {
                    installed.push(pkg.to_string());
                }
            }
        } else if let Some(rest) = t.strip_prefix("Installing collected packages: ") {
            installing_line = Some(rest.to_string());
        } else if t.starts_with("WARNING:") || t.starts_with("WARNING ") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(line.to_string());
            }
        } else if t.starts_with("ERROR:") || t.starts_with("ERROR ") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(line.to_string());
            }
        } else if !t.is_empty() && !is_pip_noise(t) {
            // NOT pip output: a failing build's gcc/traceback context, or
            // the next command in a `pip install -r reqs && pytest` chain.
            // These lines used to be silently destroyed — the compactor
            // kept only its own summary.
            unmatched.push(line.to_string());
        }
    }

    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "pip: {collected} collected, {} installed, {total_warnings} warnings",
        installed.len()
    );
    if !installed.is_empty() {
        let _ = write!(out, "\ninstalled ({}): ", installed.len());
        out.push_str(&installed.join(", "));
        out.push('\n');
    } else if let Some(line) = &installing_line {
        // pip didn't print Successfully installed (perhaps interrupted) —
        // fall back to the planned-install list.
        let _ = write!(out, "\nplanned: {line}\n");
    }
    crate::archetypes::capped_section(&mut out, "errors", &errors, total_errors, "error(s)");
    crate::archetypes::capped_section(
        &mut out,
        "warnings",
        &warnings,
        total_warnings,
        "warning(s)",
    );
    if !unmatched.is_empty() {
        // Non-pip lines (kept, then given one more compaction pass — the
        // remainder may be pytest/gcc output with its own compactor).
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

fn compact_npm(text: &str) -> String {
    let mut summary_lines: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut vuln_line: Option<String> = None;
    let mut funding_line: Option<String> = None;
    let mut audited_line: Option<String> = None;
    let mut unmatched: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();

        if let Some(rest) = t.strip_prefix("npm WARN ") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(rest.to_string());
            }
        } else if let Some(rest) = t.strip_prefix("npm ERR! ") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(rest.to_string());
            }
        } else if t.starts_with("added ") || t.starts_with("removed ") || t.starts_with("changed ")
        {
            if t.contains(" packages") {
                audited_line = Some(line.to_string());
            }
        } else if t.contains(" packages are looking for funding") {
            funding_line = Some(t.to_string());
        } else if t.contains(" vulnerabilities") {
            // E.g. "5 vulnerabilities (1 low, 4 moderate)".
            vuln_line = Some(t.to_string());
        } else if t.contains(" audited ") && t.contains(" packages") {
            summary_lines.push(line.to_string());
        } else if !t.is_empty()
            && !t.starts_with("npm ")
            && !t.starts_with("up to date")
            && !t.starts_with("run `npm")
            && !t.starts_with("To address ")
            && !t.contains("npm audit")
            && !t.contains("npm fund")
            && !t
                .chars()
                .all(|c| c.is_ascii_whitespace() || matches!(c, '|' | '#' | '-' | '/' | '\\'))
        {
            // Not npm's own output (`npm install && npm test` in one doc).
            unmatched.push(line.to_string());
        }
    }

    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "npm: {total_warnings} warnings, {total_errors} errors");
    if let Some(a) = &audited_line {
        out.push_str(a);
        out.push('\n');
    }
    if let Some(f) = &funding_line {
        out.push_str(f);
        out.push('\n');
    }
    if let Some(v) = &vuln_line {
        out.push_str(v);
        out.push('\n');
    }
    crate::archetypes::capped_section(&mut out, "errors", &errors, total_errors, "error(s)");
    crate::archetypes::capped_section(
        &mut out,
        "warnings",
        &warnings,
        total_warnings,
        "warning(s)",
    );
    if !unmatched.is_empty() {
        // Non-npm lines (`npm install && npm test` in one output) — kept,
        // with one more compaction pass over the remainder.
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

fn compact_yarn(text: &str) -> String {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut done_line: Option<String> = None;
    let mut version_line: Option<String> = None;
    let mut success_line: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();

        if t.starts_with("yarn install v") {
            version_line = Some(t.to_string());
        } else if let Some(rest) = t.strip_prefix("warning ") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(rest.to_string());
            }
        } else if let Some(rest) = t.strip_prefix("error ") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(rest.to_string());
            }
        } else if t.contains("Done in ") {
            done_line = Some(t.to_string());
        } else if t.starts_with("success ") {
            // Last `success` line tends to be the most informative.
            success_line = Some(t.to_string());
        }
    }

    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "yarn: {total_warnings} warnings, {total_errors} errors"
    );
    if let Some(v) = &version_line {
        out.push_str(v);
        out.push('\n');
    }
    if let Some(s) = &success_line {
        out.push_str(s);
        out.push('\n');
    }
    if let Some(d) = &done_line {
        out.push_str(d);
        out.push('\n');
    }
    crate::archetypes::capped_section(&mut out, "errors", &errors, total_errors, "error(s)");
    crate::archetypes::capped_section(
        &mut out,
        "warnings",
        &warnings,
        total_warnings,
        "warning(s)",
    );
    out.trim_end().to_string()
}

fn compact_pnpm(text: &str) -> String {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut done_line: Option<String> = None;
    let mut added: Vec<String> = Vec::new();
    let mut total_added = 0usize;

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();

        if t.starts_with("WARN ") || t.contains(" WARN  ") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(t.to_string());
            }
        } else if t.starts_with("ERR ") || t.contains(" ERR_  ") || t.starts_with("ERR_  ") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(t.to_string());
            }
        } else if t.contains("Done in ") {
            done_line = Some(t.to_string());
        } else if let Some(rest) = t.strip_prefix("+ ") {
            total_added += 1;
            if added.len() < INSTALLED_CAP {
                added.push(rest.to_string());
            }
        }
    }

    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "pnpm: {total_added} added, {total_warnings} warnings, {total_errors} errors"
    );
    if let Some(d) = &done_line {
        out.push_str(d);
        out.push('\n');
    }
    crate::archetypes::capped_section(&mut out, "added", &added, total_added, "package(s)");
    if !errors.is_empty() {
        let _ = write!(out, "\nerrors ({total_errors}):\n");
        for e in &errors {
            out.push_str("  ");
            out.push_str(e);
            out.push('\n');
        }
    }
    crate::archetypes::capped_section(
        &mut out,
        "warnings",
        &warnings,
        total_warnings,
        "warning(s)",
    );
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    /// A real `npx slidev build` on the day npm checks for an update: a Vite
    /// build, not an install. A real `npm i` with the same notice still is one.
    #[test]
    fn the_update_notifier_is_not_an_install() {
        let build = include_str!("../tests/fixtures/linux/npx_build_update_notice.txt");
        assert!(build.contains("npm notice New minor version of npm available"));
        assert!(!detect(build));
        let install = "\nadded 682 packages in 23s\nnpm notice\nnpm notice New minor version of npm available! 11.6.4 -> 11.21.0\nnpm notice\n";
        assert!(detect(install));
        // Without the notifier, `npm notice` is still npm's own voice.
        assert!(detect(
            "npm notice created a lockfile as package-lock.json. You should commit this file.\n"
        ));
    }

    use super::*;

    const PIP_SAMPLE: &str = r#"Collecting requests
  Downloading requests-2.31.0-py3-none-any.whl (62 kB)
     ---------------------------------------- 62.6/62.6 kB 3.2 MB/s eta 0:00:00
Collecting urllib3<3,>=1.21.1
  Downloading urllib3-2.0.4-py3-none-any.whl (123 kB)
     ---------------------------------------- 123.9/123.9 kB 14.2 MB/s eta 0:00:00
Collecting charset-normalizer<4,>=2
  Downloading charset_normalizer-3.2.0-cp310-cp310-manylinux2014_x86_64.whl (200 kB)
Collecting idna<4,>=2.5
  Downloading idna-3.4-py3-none-any.whl (61 kB)
Collecting certifi>=2017.4.17
  Downloading certifi-2023.7.22-py3-none-any.whl (158 kB)
Installing collected packages: certifi, idna, charset-normalizer, urllib3, requests
Successfully installed certifi-2023.7.22 charset-normalizer-3.2.0 idna-3.4 requests-2.31.0 urllib3-2.0.4
WARNING: You are using pip version 22.0.4; however, version 23.2.1 is available.
WARNING: pandas 2.1.0 has requirement numpy>=1.22.4, but you have numpy 1.21.0
"#;

    const NPM_SAMPLE: &str = r#"npm WARN deprecated request@2.88.2: request has been deprecated, see https://github.com/request/request/issues/3142
npm WARN deprecated har-validator@5.1.5: this library is no longer supported
npm WARN deprecated uuid@3.4.0: Please upgrade to version 7 or higher

added 234 packages, and audited 235 packages in 23s

50 packages are looking for funding
  run `npm fund` for details

5 vulnerabilities (1 low, 4 moderate)

To address all issues (including breaking changes), run:
  npm audit fix --force
"#;

    const YARN_SAMPLE: &str = r#"yarn install v1.22.19
[1/4] 🔍  Resolving packages...
[2/4] 🚚  Fetching packages...
[3/4] 🔗  Linking dependencies...
[4/4] 🔨  Building fresh packages...
warning lockfile has changed
warning eslint > inquirer > rxjs@6.5.5: rxjs@<7.0.0 is no longer supported
success Saved lockfile.
✨  Done in 12.34s.
"#;

    const PNPM_SAMPLE: &str = r#"Lockfile is up to date, resolution step is skipped
Progress: resolved 1234, reused 1230, downloaded 4, added 4, done

dependencies:
+ lodash 4.17.21
+ react 18.2.0
+ react-dom 18.2.0
+ typescript 5.0.4

WARN  deprecated har-validator@5.1.5: this library is no longer supported
WARN  deprecated request@2.88.2: deprecated

Done in 2.1s
"#;

    #[test]
    fn detects_pip() {
        assert!(detect(PIP_SAMPLE));
        assert_eq!(detect_pm(PIP_SAMPLE), PM::Pip);
    }

    #[test]
    fn detects_npm() {
        assert!(detect(NPM_SAMPLE));
        assert_eq!(detect_pm(NPM_SAMPLE), PM::Npm);
    }

    #[test]
    fn detects_yarn() {
        assert!(detect(YARN_SAMPLE));
        assert_eq!(detect_pm(YARN_SAMPLE), PM::Yarn);
    }

    #[test]
    fn detects_pnpm() {
        assert!(detect(PNPM_SAMPLE));
        assert_eq!(detect_pm(PNPM_SAMPLE), PM::Pnpm);
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect(
            "This is some prose mentioning Collecting taxes and Downloading torrents.\n"
        ));
    }

    #[test]
    fn rejects_python_code() {
        assert!(!detect(
            "def collect(items):\n    for item in items:\n        download(item)\n"
        ));
    }

    #[test]
    fn pip_extracts_installed_and_warnings() {
        let out = compact(PIP_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("5 collected"));
        assert!(out.contains("5 installed"));
        assert!(out.contains("2 warnings"));
        // All 5 packages preserved.
        assert!(out.contains("certifi-2023.7.22"));
        assert!(out.contains("requests-2.31.0"));
        assert!(out.contains("urllib3-2.0.4"));
        // Warnings preserved.
        assert!(out.contains("WARNING: You are using pip version 22.0.4"));
        assert!(out.contains("requirement numpy>=1.22.4"));
        // Progress bars / downloading lines dropped.
        assert!(!out.contains("Downloading"));
        assert!(!out.contains("kB/s eta"));
    }

    #[test]
    fn npm_extracts_summary_and_warnings() {
        let out = compact(NPM_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("3 warnings"));
        assert!(out.contains("added 234 packages"));
        assert!(out.contains("50 packages are looking for funding"));
        assert!(out.contains("5 vulnerabilities"));
        // Specific warnings.
        assert!(out.contains("deprecated request@2.88.2"));
        assert!(out.contains("har-validator"));
        // Boilerplate dropped.
        assert!(!out.contains("To address all issues"));
        assert!(!out.contains("npm audit fix --force"));
    }

    #[test]
    fn yarn_extracts_done_and_warnings() {
        let out = compact(YARN_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("yarn install v1.22.19"));
        assert!(out.contains("Done in 12.34s"));
        assert!(out.contains("success Saved lockfile"));
        assert!(out.contains("2 warnings"));
        assert!(out.contains("lockfile has changed"));
        assert!(out.contains("rxjs@<7.0.0 is no longer supported"));
        // Step indicators dropped.
        assert!(!out.contains("🔍  Resolving"));
        assert!(!out.contains("[1/4]"));
    }

    #[test]
    fn pnpm_extracts_added_and_done() {
        let out = compact(PNPM_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("4 added"));
        assert!(out.contains("Done in 2.1s"));
        assert!(out.contains("react 18.2.0"));
        assert!(out.contains("typescript 5.0.4"));
        assert!(out.contains("2 warnings"));
        assert!(out.contains("deprecated har-validator"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not an install output";
        assert_eq!(compact(t), t);
    }

    /// The bar is EIGHT characters or more. Three dashes open a unified diff
    /// header and a markdown rule, and both carry information a pip log has no
    /// business swallowing.
    #[test]
    fn a_diff_header_is_not_a_progress_bar() {
        assert!(!super::is_progress_bar("--- a/setup.py"));
        assert!(!super::is_progress_bar("--- "));
        assert!(super::is_progress_bar(
            "     ---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00"
        ));
        let mut s =
            String::from("Collecting pkg-0\n  Downloading pkg_0-1.0.0-py3-none-any.whl (120 kB)\n");
        s.push_str(
            "     ---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00\n",
        );
        s.push_str("--- a/setup.py\n");
        s.push_str("Successfully installed pkg-0-1.0.0\n");
        let out = super::compact(&s);
        assert!(
            out.contains("a/setup.py"),
            "a diff header was folded as noise: {out}"
        );
    }

    #[test]
    fn savings_huge_pip_install() {
        // 300-package install with a progress bar per package.
        let mut s = String::with_capacity(100_000);
        for i in 0..300 {
            let _ = writeln!(s, "Collecting pkg-{i}");
            let _ = writeln!(s, "  Downloading pkg_{i}-1.0.0-py3-none-any.whl (12{i} kB)");
            s.push_str("     ---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00\n");
        }
        s.push_str("Installing collected packages: ");
        for i in 0..300 {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "pkg-{i}");
        }
        s.push_str("\nSuccessfully installed ");
        for i in 0..300 {
            if i > 0 {
                s.push(' ');
            }
            let _ = write!(s, "pkg-{i}-1.0.0");
        }
        s.push('\n');
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("pip"));
        assert!(out.contains("300 collected"));
        assert!(out.contains("200 installed")); // capped at INSTALLED_CAP=200
        assert!(sav.percent() > 90.0, "got {sav:?}");
    }
}
