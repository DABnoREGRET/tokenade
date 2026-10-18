//! Maven / Gradle / SBT JVM build-tool output compaction.
//!
//! A real `mvn install` on a non-trivial project emits 5 000 – 50 000 lines,
//! roughly 95 % of which are `[INFO]` chatter — "Downloading from central",
//! per-plugin invocation banners, "Copying N resources", per-module
//! "Building X" / dashes. What the agent reading the build log actually
//! needs is:
//!   * the **final BUILD status** (SUCCESS / FAILURE / ABORTED)
//!   * every **`[ERROR]`** and **`[WARNING]`** line
//!   * the **total time** and **# files compiled**
//!   * the **module list** (multi-module reactor projects)
//!   * any **test summary** (`Tests run: N, Failures: M, Errors: K`)
//!
//! Gradle is simpler: `> Task :name` per task, optional `FAILED` suffix,
//! `BUILD SUCCESSFUL` / `BUILD FAILED`, `N actionable tasks: …`, and a
//! `FAILURE: …` block when something dies. SBT uses lowercase `[info]` /
//! `[warn]` / `[error]` prefixes.
//!
//! Real-world compaction ratio: ≥95 % on production build logs.

use std::fmt::Write;

pub const ERRORS_CAP: usize = 100;
pub const WARNINGS_CAP: usize = 50;
pub const FAILURE_BLOCK_CAP: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Maven,
    Gradle,
    Sbt,
    Unknown,
}

pub fn detect(text: &str) -> bool {
    detect_tool(text) != Tool::Unknown
}

/// Does `needle` appear in `head` OUTSIDE a string literal?
///
/// "Outside" is approximated by the character before it: a marker written in
/// source code is quoted (`line.starts_with("[INFO] Total time:")`), one printed
/// by Maven is not.
///
/// The plain `contains` this replaces read a Rust file as a Maven build.
/// Measured 2026-08-26 on `rtk/src/cmds/jvm/mvn_cmd.rs` — a compactor for Maven
/// output, so its source quotes dozens of `[INFO]` lines — 20 000 bytes of Rust
/// came back as 2 462 bytes headed `Maven BUILD SUCCESS: 30 errors, 1 warnings`,
/// with every `pub fn`, `struct`, `impl` and `let` gone. This compactor rebuilds
/// from its parse, so a wrong claim does not fold the file, it REPLACES it.
///
/// Both halves of the test are checked this way, because source quotes both:
/// the `[INFO]` prefix and the Maven-only marker. Measured over 58 601 source
/// files and 10 219 log files in the corpus: source claims **9 → 3**, log
/// detection **15 → 15**. The rule costs nothing on real logs.
///
/// The three that remain write their markers in raw or multi-line literals,
/// where the quote is not the preceding character. Left standing rather than
/// widened: a rule tightened past its own format is the worse defect.
fn unquoted_contains(head: &str, needle: &str) -> bool {
    head.lines().any(|line| {
        // A doc comment is source, not output: `//! Apache Maven filter — …`
        // is the first line of the very file this rule exists to protect.
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            return false;
        }
        let b = line.as_bytes();
        line.match_indices(needle)
            .any(|(i, _)| i == 0 || !matches!(b[i - 1], b'"' | b'\''))
    })
}

fn detect_tool(text: &str) -> Tool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);

    // Maven: `[INFO]` lines + an unambiguous Maven-only marker.
    let has_maven_prefix =
        unquoted_contains(head, "[INFO] ") || unquoted_contains(head, "[INFO]\n");
    let has_maven_marker = unquoted_contains(head, "BUILD SUCCESS")
        || unquoted_contains(head, "BUILD FAILURE")
        || unquoted_contains(head, "BUILD ABORTED")
        || unquoted_contains(head, "Apache Maven")
        || unquoted_contains(head, "Reactor Summary")
        || unquoted_contains(head, "--- maven-")
        // The next three appear in the FIRST few lines of every real `mvn`
        // run, so they catch huge logs where the BUILD line lives past the
        // 16 KB head cap.
        || unquoted_contains(head, "[INFO] Scanning for projects")
        || unquoted_contains(head, "[INFO] Building ")
        || unquoted_contains(head, "Downloading from ");
    if has_maven_prefix && has_maven_marker {
        return Tool::Maven;
    }

    // Gradle: `> Task :…` is uniquely Gradle. ≥2 hits, or 1 hit + BUILD status.
    let task_count = head
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            !(t.starts_with("//") || t.starts_with('*') || t.starts_with("/*"))
        })
        .map(|line| {
            let b = line.as_bytes();
            line.match_indices("> Task :")
                .filter(|(i, _)| *i == 0 || !matches!(b[*i - 1], b'"' | b'\''))
                .count()
        })
        .sum::<usize>();
    let has_gradle_build =
        unquoted_contains(head, "BUILD SUCCESSFUL") || unquoted_contains(head, "BUILD FAILED");
    if task_count >= 2 || (task_count >= 1 && has_gradle_build) {
        return Tool::Gradle;
    }
    if unquoted_contains(head, " actionable task") {
        return Tool::Gradle;
    }

    // SBT: lowercase `[info]` / `[warn]` / `[error]` + SBT-specific marker.
    let has_sbt_prefix = unquoted_contains(head, "[info] ") || unquoted_contains(head, "[error] ");
    let has_sbt_marker = unquoted_contains(head, "[success] Total time")
        || unquoted_contains(head, "sbt:")
        || unquoted_contains(head, "[error] (Compile / compileIncremental)")
        || unquoted_contains(head, "[info] Loading project definition");
    if has_sbt_prefix && has_sbt_marker {
        return Tool::Sbt;
    }

    Tool::Unknown
}

pub fn compact(text: &str) -> String {
    match detect_tool(text) {
        Tool::Maven => compact_maven(text),
        Tool::Gradle => compact_gradle(text),
        Tool::Sbt => compact_sbt(text),
        Tool::Unknown => text.to_string(),
    }
}

// #555 — `short_status` removed; had no real caller, only the
// `_keep_short_status` wrapper that existed to silence the lint.

/// `[INFO] core ........ FAILURE [  2.1 s]` → (`core`, `FAILURE`).
fn reactor_row(line: &str) -> Option<(String, &'static str)> {
    let rest = line.strip_prefix("[INFO] ")?;
    let (name, tail) = rest.split_once(" ..")?;
    let tail = tail.trim_start_matches(['.', ' ']);
    let st = ["SUCCESS", "FAILURE", "SKIPPED"]
        .into_iter()
        .find(|s| tail.starts_with(s))?;
    Some((name.trim().to_string(), st))
}

fn compact_maven(text: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut build_status: Option<&str> = None;
    let mut total_time: Option<String> = None;
    let mut compiled_files: Vec<String> = Vec::new();
    let mut module_names: Vec<String> = Vec::new();
    let mut test_summaries: Vec<String> = Vec::new();
    let mut reactor: Vec<(String, &str)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(row) = reactor_row(line) {
            reactor.push(row);
        }
        if line.contains("[ERROR]") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(line.to_string());
            }
        } else if line.contains("[WARNING]") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(line.to_string());
            }
        }
        if build_status.is_none() {
            if line.contains("BUILD SUCCESS") {
                build_status = Some("BUILD SUCCESS");
            } else if line.contains("BUILD FAILURE") {
                build_status = Some("BUILD FAILURE");
            } else if line.contains("BUILD ABORTED") {
                build_status = Some("BUILD ABORTED");
            }
        }
        if total_time.is_none() && line.contains("Total time:") {
            total_time = Some(line.to_string());
        }
        if line.contains("[INFO] Compiling ") && line.contains(" source file") {
            compiled_files.push(line.to_string());
        }
        // Module headers from the reactor: `[INFO] Building name 1.0-SNAPSHOT`.
        // Skip the plugin-execution variant: `[INFO] --- plugin:version:goal @ name ---`.
        if let Some(rest) = line.strip_prefix("[INFO] Building ") {
            // Plugin/goal-execution lines have ` @ ` and ` --- ` in them.
            if !rest.contains(" @ ") && !rest.contains(" --- ") && rest.len() < 200 {
                module_names.push(rest.to_string());
            }
        }
        // Surefire / Failsafe test summary lines: `Tests run: 12, Failures: 1, Errors: 0, Skipped: 0`.
        if line.contains("Tests run: ") && line.contains("Failures:") {
            test_summaries.push(line.to_string());
        }
    }

    let mut out = String::with_capacity(4096);
    let status_str = build_status.unwrap_or("BUILD ?");
    let _ = writeln!(
        out,
        "Maven {status_str}: {total_errors} errors, {total_warnings} warnings"
    );
    if let Some(t) = &total_time {
        out.push_str(t);
        out.push('\n');
    }
    if !compiled_files.is_empty() {
        let _ = writeln!(out, "compiled ({}):", compiled_files.len());
        for c in compiled_files.iter().take(20) {
            out.push_str("  ");
            out.push_str(c);
            out.push('\n');
        }
    }
    if !module_names.is_empty() {
        let _ = write!(out, "\nmodules ({}):\n", module_names.len());
        // The Reactor Summary says WHICH module failed and which were skipped
        // behind it; a cap on names taken in build order dropped the 25th
        // module's FAILURE with the 24 successes. Every non-SUCCESS row stays.
        let status = |m: &str| {
            reactor
                .iter()
                .find(|(n, _)| {
                    m == n
                        || m.strip_prefix(n.as_str())
                            .is_some_and(|r| r.starts_with(' '))
                })
                .map(|(_, s)| *s)
        };
        let mut shown_ok = 0usize;
        for m in &module_names {
            match status(m) {
                Some(st) if st != "SUCCESS" => {
                    let _ = writeln!(out, "  {m}  {st}");
                }
                _ if shown_ok < 20 => {
                    shown_ok += 1;
                    out.push_str("  ");
                    out.push_str(m);
                    out.push('\n');
                }
                _ => {}
            }
        }
        let shown = shown_ok
            + module_names
                .iter()
                .filter(|m| status(m).is_some_and(|s| s != "SUCCESS"))
                .count();
        if module_names.len() > shown {
            let _ = writeln!(
                out,
                "  … {} more module(s) elided",
                module_names.len() - shown
            );
        }
    }
    if !test_summaries.is_empty() {
        let _ = write!(out, "\ntest summaries ({}):\n", test_summaries.len());
        for t in test_summaries.iter().take(50) {
            out.push_str("  ");
            out.push_str(t);
            out.push('\n');
        }
    }
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("errors ({} shown", errors.len()),
        &errors,
        total_errors,
        "error line(s)",
    );
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("warnings ({} shown", warnings.len()),
        &warnings,
        total_warnings,
        "warning line(s)",
    );
    out.trim_end().to_string()
}

fn compact_gradle(text: &str) -> String {
    let mut completed_tasks = 0usize;
    let mut module_names: Vec<String> = Vec::new();
    let mut task_names: Vec<String> = Vec::new();
    let mut failing_tasks: Vec<String> = Vec::new();
    let mut test_failures: Vec<String> = Vec::new();
    let mut failure_block: Vec<String> = Vec::new();
    let mut in_failure = false;
    let mut build_status: Option<&str> = None;
    let mut actionable_summary: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.starts_with("> Task :") {
            completed_tasks += 1;
            // The PROJECT a task belongs to. `> Task :svc-api:compileKotlin`
            // names `svc-api`; a root-level task (`> Task :build`) names
            // nothing and is skipped.
            //
            // Maven's arm has kept its module list since it was written, and
            // its test says why in as many words: "the module names are what
            // an agent needs from a reactor build". Gradle's arm counted the
            // tasks and kept only the FAILING ones, so a build that SUCCEEDS
            // came back as `Gradle BUILD SUCCESSFUL: 33 tasks (0 failed)` —
            // one line, 877 tokens to 11, with nothing in it an agent can act
            // on and no disclosure that thirty-three task lines went away.
            // One rule, two arms, and only one of them had it.
            if let Some(rest) = line.strip_prefix("> Task :") {
                match rest.split_once(':') {
                    Some((project, _)) if !project.trim().is_empty() => {
                        let project = project.trim();
                        if !module_names.iter().any(|m| m == project) {
                            module_names.push(project.to_string());
                        }
                    }
                    // A SINGLE-project build has no project segment at all —
                    // `> Task :compileKotlin`, not `> Task :app:compileKotlin`
                    // — and that is the common case. There the TASK is the
                    // identifying payload, so it takes the same block. Without
                    // this the summary is one status line naming nothing, and
                    // the floor correctly refuses it and hands back the raw
                    // log: safe, and 760 tokens where 60 would do.
                    _ => {
                        let task = rest
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_end_matches(':')
                            .trim();
                        if !task.is_empty() && !task_names.iter().any(|t| t == task) {
                            task_names.push(task.to_string());
                        }
                    }
                }
            }
            if line.contains(" FAILED") {
                failing_tasks.push(line.to_string());
            }
        }
        if build_status.is_none() {
            if line.contains("BUILD SUCCESSFUL") {
                build_status = Some("BUILD SUCCESSFUL");
            } else if line.contains("BUILD FAILED") {
                build_status = Some("BUILD FAILED");
            }
        }
        if line.contains(" actionable task") {
            actionable_summary = Some(line.to_string());
        }
        if line.starts_with("FAILURE:") {
            in_failure = true;
        }
        if in_failure && failure_block.len() < FAILURE_BLOCK_CAP {
            failure_block.push(line.to_string());
            // Heuristic terminator: blank line after we've gathered ≥3 lines.
            if failure_block.len() >= 3 && line.is_empty() {
                in_failure = false;
            }
        }
        // Test failure line shapes:
        //   `Test name FAILED`
        //   `Foo > bar FAILED`
        if (line.ends_with(" FAILED") && !line.starts_with("> Task ")) && line.contains(' ') {
            test_failures.push(line.to_string());
        }
    }

    let mut out = String::with_capacity(4096);
    let status_str = build_status.unwrap_or("BUILD ?");
    let _ = writeln!(
        out,
        "Gradle {status_str}: {completed_tasks} tasks ({} failed)",
        failing_tasks.len()
    );
    if let Some(s) = &actionable_summary {
        out.push_str(s);
        out.push('\n');
    }
    // One block, two possible identifiers: the projects of a multi-project
    // build, or the tasks of a single-project one. Never both — a build that
    // names projects has already said what ran, and listing every task on top
    // of that is the fan-out the summary exists to avoid.
    let (label, names) = if module_names.is_empty() {
        ("tasks", &task_names)
    } else {
        ("projects", &module_names)
    };
    if !names.is_empty() {
        let _ = write!(out, "\n{label} ({}):\n", names.len());
        for m in names.iter().take(20) {
            out.push_str("  ");
            out.push_str(m);
            out.push('\n');
        }
        if names.len() > 20 {
            let _ = writeln!(out, "  … {} more {label} elided", names.len() - 20);
        }
    }
    if !failing_tasks.is_empty() {
        let _ = write!(out, "\nfailing tasks ({}):\n", failing_tasks.len());
        for t in &failing_tasks {
            out.push_str("  ");
            out.push_str(t);
            out.push('\n');
        }
    }
    if !test_failures.is_empty() {
        let _ = write!(out, "\ntest failures ({}):\n", test_failures.len());
        for t in test_failures.iter().take(50) {
            out.push_str("  ");
            out.push_str(t);
            out.push('\n');
        }
        if test_failures.len() > 50 {
            let _ = writeln!(
                out,
                "  … {} more test failure(s) elided",
                test_failures.len() - 50
            );
        }
    }
    if !failure_block.is_empty() {
        out.push_str("\nfailure detail:\n");
        for l in &failure_block {
            out.push_str("  ");
            out.push_str(l);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn compact_sbt(text: &str) -> String {
    // Re-uses Maven shape but with lowercase prefixes.
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut success_line: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.contains("[error]") {
            total_errors += 1;
            if errors.len() < ERRORS_CAP {
                errors.push(line.to_string());
            }
        } else if line.contains("[warn]") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(line.to_string());
            }
        }
        if success_line.is_none() && line.contains("[success] Total time") {
            success_line = Some(line.to_string());
        }
    }
    let status = if success_line.is_some() {
        "[success]"
    } else if total_errors > 0 {
        "[error]"
    } else {
        "(unknown)"
    };
    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "SBT {status}: {total_errors} errors, {total_warnings} warnings"
    );
    if let Some(s) = &success_line {
        out.push_str(s);
        out.push('\n');
    }
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("errors ({} shown", errors.len()),
        &errors,
        total_errors,
        "error line(s)",
    );
    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("warnings ({} shown", warnings.len()),
        &warnings,
        total_warnings,
        "warning line(s)",
    );
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAVEN_SAMPLE: &str = r#"[INFO] Scanning for projects...
[INFO]
[INFO] ----------< com.example:my-project >----------
[INFO] Building my-project 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.2.0:resources (default-resources) @ my-project ---
[INFO] Using 'UTF-8' encoding to copy filtered resources.
[INFO] Copying 5 resources
[INFO]
[INFO] --- maven-compiler-plugin:3.8.1:compile (default-compile) @ my-project ---
[INFO] Changes detected - recompiling the module!
[INFO] Compiling 142 source files to /home/x/target/classes
[WARNING] /home/x/src/Foo.java:[15,30] [unchecked] unchecked conversion
[ERROR] /home/x/src/Bar.java:[42,15] cannot find symbol
[ERROR]   symbol:   variable foo
[ERROR]   location: class Bar
[INFO] BUILD FAILURE
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s
[INFO] Finished at: 2024-01-15T10:30:45+00:00
[INFO] ------------------------------------------------------------------------
[ERROR] Failed to execute goal org.apache.maven.plugins:maven-compiler-plugin:3.8.1:compile (default-compile) on project my-project: Compilation failure
[ERROR] -> [Help 1]
"#;

    const GRADLE_SAMPLE: &str = r#"> Task :compileJava
> Task :processResources
> Task :classes
> Task :test
Foo > testBar FAILED
    java.lang.AssertionError: expected:<1> but was:<2>
        at com.example.Test.testBar(Test.java:42)

> Task :test FAILED
> Task :build FAILED

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':test'.
> There were failing tests. See the report at: file:///home/x/build/reports/tests/test/index.html

BUILD FAILED in 3s
6 actionable tasks: 4 executed, 2 up-to-date
"#;

    const SBT_SAMPLE: &str = r#"[info] Loading project definition from /home/x/project
[info] Set current project to root
[info] compiling 12 Scala sources to /home/x/target...
[warn] /home/x/src/Foo.scala:15: unused import
[error] /home/x/src/Bar.scala:42: not found: value foo
[error] one error found
[error] (Compile / compileIncremental) Compilation failed
[error] Total time: 4 s, completed Jan 15, 2024 10:30:45
"#;

    #[test]
    fn a_failed_module_past_the_twentieth_keeps_its_status() {
        let mut s = String::new();
        for i in 0..30 {
            let _ = writeln!(s, "[INFO] Building core{i} 1.0-SNAPSHOT  [{}/31]", i + 1);
        }
        let _ = writeln!(s, "[INFO] Building core 1.0-SNAPSHOT  [31/31]");
        s.push_str("[INFO] Reactor Summary for parent 1.0-SNAPSHOT:\n");
        for i in 0..30 {
            let st = match i {
                24 => "FAILURE [  2.1 s]",
                25..=29 => "SKIPPED",
                _ => "SUCCESS [  0.4 s]",
            };
            let _ = writeln!(s, "[INFO] core{i} ............................ {st}");
        }
        s.push_str(
            "[INFO] core ............................ SUCCESS [  0.1 s]\n[INFO] BUILD FAILURE\n",
        );
        let out = compact(&s);
        assert!(
            out.contains("  core24 1.0-SNAPSHOT  [25/31]  FAILURE"),
            "{out}"
        );
        assert!(
            out.contains("  core29 1.0-SNAPSHOT  [30/31]  SKIPPED"),
            "{out}"
        );
        assert!(!out.contains("core3 1.0-SNAPSHOT  [4/31]  "), "{out}");
        assert!(out.contains("… 5 more module(s) elided"), "{out}");
    }
    #[test]
    fn detects_maven() {
        assert!(detect(MAVEN_SAMPLE));
    }

    #[test]
    fn detects_gradle() {
        assert!(detect(GRADLE_SAMPLE));
    }

    #[test]
    fn detects_sbt() {
        assert!(detect(SBT_SAMPLE));
    }

    #[test]
    fn rejects_java_source_with_bracketed_words() {
        let java_code = r#"public class Foo {
  // [INFO] just a comment
  String s = "[ERROR] not a real error";
  String t = "[WARNING] not a real warning";
}
"#;
        assert!(!detect(java_code));
    }

    #[test]
    fn rejects_rust_code() {
        assert!(!detect("fn main() {\n    let x = 42;\n}\n"));
    }

    #[test]
    fn maven_extracts_errors_warnings_modules() {
        let out = compact(MAVEN_SAMPLE);
        assert!(out.contains("Maven"));
        assert!(out.contains("BUILD FAILURE"));
        assert!(out.contains("5 errors"), "errors count: {out}");
        assert!(out.contains("1 warnings"), "warnings count: {out}");
        assert!(out.contains("Total time:  3.456 s"));
        assert!(out.contains("Compiling 142 source files"));
        assert!(out.contains("my-project 1.0-SNAPSHOT"));
        assert!(out.contains("cannot find symbol"));
        assert!(out.contains("unchecked conversion"));
        // Plugin-execution `Building` lines should NOT have been confused with module headers.
        assert!(!out.contains("maven-resources-plugin"));
    }

    #[test]
    fn gradle_extracts_tasks_and_failures() {
        let out = compact(GRADLE_SAMPLE);
        assert!(out.contains("Gradle"));
        assert!(out.contains("BUILD FAILED"));
        assert!(out.contains("6 tasks (2 failed)"));
        assert!(out.contains("> Task :test FAILED"));
        assert!(out.contains("> Task :build FAILED"));
        assert!(out.contains("Foo > testBar FAILED"));
        assert!(out.contains("6 actionable tasks: 4 executed, 2 up-to-date"));
        assert!(out.contains("Execution failed for task ':test'."));
    }

    #[test]
    fn sbt_extracts_errors_warnings() {
        let out = compact(SBT_SAMPLE);
        assert!(out.contains("SBT"));
        // SBT prints "[error] Total time:" on failure; the sample has 4 lines
        // matching `[error]` (3 diagnostic + 1 status marker).
        assert!(out.contains("4 errors"));
        assert!(out.contains("1 warnings"));
        assert!(out.contains("not found: value foo"));
        assert!(out.contains("unused import"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is unrelated text";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_huge_maven_log() {
        // 50 000-line synthetic Maven build with massive transitive download spam.
        let mut s = String::from("[INFO] Scanning for projects...\n[INFO] ----------< com.example:proj >----------\n[INFO] Building proj 1.0-SNAPSHOT\n");
        for i in 0..50_000 {
            let _ = writeln!(
                s,
                "[INFO] Downloading from central: https://repo/example/lib-{i}/1.0/lib-{i}-1.0.jar"
            );
        }
        s.push_str("[ERROR] Bar.java:[42,15] cannot find symbol\n");
        s.push_str("[INFO] BUILD FAILURE\n");
        s.push_str("[INFO] Total time:  12.345 s\n");
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("BUILD FAILURE"));
        assert!(out.contains("1 errors"));
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }
}

/// Source code that QUOTES build output is not build output.
#[cfg(test)]
mod source_quoting_a_build_log {
    use super::{compact, detect};

    /// The shape of `rtk/src/cmds/jvm/mvn_cmd.rs` — a Maven compactor, so its
    /// source quotes the very lines it matches, and its doc comment names the
    /// tool.
    const RUST: &str = "//! Apache Maven filter — Surefire block collapse.\n\nuse std::fmt::Write;\n\npub struct MavenFilter {\n    errors: usize,\n}\n\nimpl MavenFilter {\n    pub fn feed(&mut self, line: &str) {\n        if line.starts_with(\"[INFO] Total time:\") {\n            return;\n        }\n        if line.contains(\"BUILD SUCCESS\") || line.contains(\"BUILD FAILURE\") {\n            self.errors += 1;\n        }\n        let trimmed = line.trim_start();\n        if trimmed.starts_with(\"[INFO] Building \") {\n            self.errors = 0;\n        }\n    }\n}\n";

    #[test]
    fn a_maven_compactors_own_source_is_not_a_maven_log() {
        assert_ne!(
            super::detect_tool(RUST),
            super::Tool::Maven,
            "the file was read as the output it parses"
        );
        assert!(!detect(RUST));
    }

    /// The harm: this compactor REBUILDS from its parse, so a wrong claim does
    /// not fold the file, it replaces it.
    #[test]
    fn the_rebuild_would_have_replaced_the_code() {
        let out = compact(RUST);
        assert!(out.contains("pub fn feed"), "the code was replaced:\n{out}");
        assert!(out.contains("MavenFilter"), "the code was replaced:\n{out}");
    }

    /// A real Maven log still detects, including one a CI system prefixed with
    /// a timestamp — which is why the rule is "not quoted, not in a comment"
    /// and not "at column 0".
    #[test]
    fn a_real_maven_log_still_detects() {
        let log = "[INFO] Scanning for projects...\n[INFO] \n[INFO] ------< com.example:app >------\n[INFO] Building app 1.0.0\n[INFO] BUILD SUCCESS\n[INFO] Total time:  3.412 s\n";
        assert!(detect(log));
        let ci = "2026-08-26T10:00:01Z [INFO] Scanning for projects...\n2026-08-26T10:00:02Z [INFO] Building app 1.0.0\n2026-08-26T10:00:09Z [INFO] BUILD SUCCESS\n";
        assert!(detect(ci), "a timestamp-prefixed CI log stopped detecting");
    }
}
