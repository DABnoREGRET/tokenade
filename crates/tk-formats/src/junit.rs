//! JUnit XML test reports — the CI artefact every project produces.
//!
//! Measured 2026-08-26 across a corpus of cloned repositories: of 279 `.xml` files between 4 KB
//! and 3 MB, **257 are JUnit reports** and they carry 21.6 MB. The generic XML
//! compactor strips comments and whitespace, so it returned every one of them
//! whole; the corpus sweep put `xml` second only to markdown among the things
//! tokenade hands back unchanged, at 1.16 MB over 118 files.
//!
//! What an agent needs from a test report is not the report. It is: how many
//! ran, how many failed, WHICH ones, and why — and when thirty-eight tests fail
//! with the same message and the same stack, that is ONE fact, not thirty-eight.
//! The real file that motivated this is exactly that: 38 identical
//! `traitCtx.createStore is not a function`, 22 671 bytes.
//!
//! Passing tests are a COUNT. Their names are in the file, and the file is on
//! disk; an agent looking at a red build is not looking for them.

/// Attribute value from an opening tag, unescaped for the five XML entities.
fn attr(line: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    // The attribute NAME must start where it says it does. `find("name=\"")`
    // matches the tail of `classname="…"`, so every `<testcase classname="a"
    // name="b">` reported `a` as the test — the same substring-for-a-token
    // mistake as the SQL keyword read out of `update-type`.
    let mut from = 0usize;
    let i = loop {
        let rel = line[from..].find(&pat)?;
        let at = from + rel;
        let ok = at == 0 || matches!(line.as_bytes()[at - 1], b' ' | b'\t' | b'<');
        if ok {
            break at + pat.len();
        }
        from = at + 1;
    };
    let rest = &line[i..];
    let end = rest.find('"')?;
    Some(unescape(&rest[..end]))
}

/// The five predefined XML entities. A test name reads `A &gt; B` in the file
/// and `A > B` to a person, and this output is a summary for a reader, not XML.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A JUnit report: a `<testsuite`/`<testsuites` root with `<testcase` inside.
///
/// Both are required. `<testsuite` alone appears in unrelated schemas, and a
/// document with no `<testcase` has nothing this compactor knows how to say.
pub fn detect(text: &str) -> bool {
    let head: String = text.lines().take(80).collect::<Vec<_>>().join("\n");
    (head.contains("<testsuites") || head.contains("<testsuite ")) && text.contains("<testcase")
}

#[derive(Default)]
struct Suite {
    name: String,
    tests: usize,
    failures: usize,
    errors: usize,
    skipped: usize,
    time: String,
    /// (kind, message, body) -> (count, first test name)
    groups: Vec<(String, String, String, usize, String)>,
}

/// How many lines of a failure body to keep. A stack trace's value is its top
/// frames; past that it is framework noise, and the file is on disk.
const BODY_LINES: usize = 8;

/// Scan by TAG, not by line.
///
/// The first version walked lines, which every pretty-printed report satisfies
/// and pytest's does not: measured 2026-08-26, a 1 707 011-byte pytest report
/// puts its prolog, its `<testsuites>`, its `<testsuite>` and all 11 792
/// `<testcase/>` elements on ONE line, so the line walk found nothing and the
/// whole file came back. **125 of the 257 reports in the corpus were that
/// shape, and they carried 16.3 MB of the 21.6.** XML has no lines; assuming
/// it does is assuming a formatter.
pub fn compact(text: &str) -> String {
    let b = text.as_bytes();
    let mut suites: Vec<Suite> = Vec::new();
    let mut case_name = String::new();
    let mut i = 0usize;
    while i < b.len() {
        let Some(rel) = text[i..].find('<') else {
            break;
        };
        let open = i + rel;
        let Some(close) = end_of_tag(text, open) else {
            break;
        };
        let tag = &text[open..=close];
        let name = tag_name(tag);
        match name {
            "testsuites" | "testsuite" => suites.push(Suite {
                name: attr(tag, "name").unwrap_or_default(),
                tests: attr(tag, "tests").and_then(|v| v.parse().ok()).unwrap_or(0),
                failures: attr(tag, "failures")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                errors: attr(tag, "errors")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                skipped: attr(tag, "skipped")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                time: attr(tag, "time").unwrap_or_default(),
                groups: Vec::new(),
            }),
            "testcase" => case_name = attr(tag, "name").unwrap_or_default(),
            "failure" | "error" | "skipped" => {
                let kind = match name {
                    "failure" => "FAIL",
                    "error" => "ERROR",
                    _ => "SKIP",
                };
                let msg = attr(tag, "message").unwrap_or_default();
                let mut body = String::new();
                let mut after = close + 1;
                if !tag.ends_with("/>") {
                    let closing = format!("</{name}>");
                    if let Some(rel) = text[after..].find(&closing) {
                        let raw = &text[after..after + rel];
                        for l in raw
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(BODY_LINES)
                        {
                            body.push_str(l.trim_end());
                            body.push('\n');
                        }
                        after += rel + closing.len();
                    }
                }
                // The BODY stays as it is in the file. Unescaping it made
                // `guarded`'s error-preservation pass fail to find its own
                // lines — it matches by substring — so it re-attached every one
                // of them, duplicating what the summary already said. The
                // MESSAGE and the test names are attributes and ARE unescaped:
                // those are what a person reads.
                let body = body.trim_end().to_string();
                if let Some(g) = suites.last_mut() {
                    // Group on the MESSAGE, not on the message plus the body.
                    // Thirty-eight tests failing on the same error differ in
                    // the last frame of their trace — the line inside the test
                    // file — so keying on the body gave thirty-eight groups of
                    // one and folded nothing. Measured on the real report:
                    // 22 676 bytes to 12 421 with the strict key, 1 102 with
                    // this one. The message is the fact; the first trace is an
                    // example of it.
                    match g
                        .groups
                        .iter_mut()
                        .find(|(k, m, _, _, _)| k == kind && *m == msg)
                    {
                        Some(e) => e.3 += 1,
                        None => g.groups.push((
                            kind.to_string(),
                            msg.clone(),
                            body.clone(),
                            1,
                            case_name.clone(),
                        )),
                    }
                }
                i = after;
                continue;
            }
            _ => {}
        }
        i = close + 1;
    }
    if suites.is_empty() {
        return text.to_string();
    }
    render(&suites, text)
}

/// Index of the `>` that closes the tag opening at `open`, skipping any `>`
/// inside a quoted attribute value.
fn end_of_tag(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = open + 1;
    let mut quote = 0u8;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The element name of an opening tag, lowercased ASCII. Empty for closing
/// tags, comments, the prolog and CDATA — none of which this compactor reads.
fn tag_name(tag: &str) -> &str {
    let rest = tag.strip_prefix('<').unwrap_or(tag);
    if rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('!') {
        return "";
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    &rest[..end]
}

fn render(suites: &[Suite], text: &str) -> String {
    let mut out = String::with_capacity(1024);
    // The header comes from the root when the root carries counts, and from
    // the SUM of the suites when it does not. `<testsuites name="pytest
    // tests">` has a name and nothing else — pytest puts the numbers on the
    // inner `<testsuite>` — so reading the root unconditionally reported a
    // 11 792-test run as "0 test(s), 0 failed".
    let sum = Suite {
        name: suites[0].name.clone(),
        tests: suites.iter().skip(1).map(|s| s.tests).sum(),
        failures: suites.iter().skip(1).map(|s| s.failures).sum(),
        errors: suites.iter().skip(1).map(|s| s.errors).sum(),
        skipped: suites.iter().skip(1).map(|s| s.skipped).sum(),
        time: suites
            .iter()
            .skip(1)
            .find(|s| !s.time.is_empty())
            .map(|s| s.time.clone())
            .unwrap_or_default(),
        groups: Vec::new(),
    };
    let root = if suites[0].tests > 0 || suites.len() == 1 {
        &suites[0]
    } else {
        &sum
    };
    out.push_str(&format!(
        "[tokenade:junit] {} test(s), {} failed, {} error(s), {} skipped{}\n",
        root.tests,
        root.failures,
        root.errors,
        root.skipped,
        if root.time.is_empty() {
            String::new()
        } else {
            format!(" in {}s", root.time)
        }
    ));
    for s in suites.iter().skip(if suites.len() > 1 { 1 } else { 0 }) {
        if s.tests == 0 && s.groups.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{} — {} test(s), {} failed\n",
            if s.name.is_empty() {
                "(unnamed suite)"
            } else {
                &s.name
            },
            s.tests,
            s.failures
        ));
        for (kind, msg, body, n, first) in &s.groups {
            out.push_str(&format!("  x{n} {kind}  {msg}\n"));
            for l in body.lines() {
                out.push_str("       ");
                out.push_str(l);
                out.push('\n');
            }
            // Name a test whenever one is known, not only when the group has
            // more than one member. A single failure is the case where the
            // name matters MOST — it is the whole answer to "which test broke"
            // — and it was the one case that dropped it. Found by
            // `junit_reporter_routes_to_auto`, whose report has exactly one.
            if !first.is_empty() {
                out.push_str(&format!(
                    "       {}: {first}\n",
                    if *n > 1 { "first" } else { "test" }
                ));
            }
        }
    }
    out.push_str(
        "[tokenade:junit] passing test names and the XML around them are in the file on disk\n",
    );
    // Never bigger than what it describes: a report with one test and a long
    // message can render longer than the file, and a summary that costs more
    // than the thing it summarises is not a summary.
    if out.len() >= text.len() {
        return text.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(cases: usize, distinct: usize) -> String {
        let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n");
        s.push_str(&format!(
            "<testsuites name=\"vitest tests\" tests=\"{cases}\" failures=\"{cases}\" errors=\"0\" time=\"0.056911\">\n"
        ));
        s.push_str(&format!(
            "    <testsuite name=\"tests/pair.test.ts\" tests=\"{cases}\" failures=\"{cases}\" errors=\"0\" skipped=\"0\">\n"
        ));
        for i in 0..cases {
            s.push_str(&format!(
                "        <testcase classname=\"tests/pair.test.ts\" name=\"Modifiers &gt; case {i}\">\n"
            ));
            s.push_str(&format!(
                "            <failure message=\"variant {} is not a function\" type=\"TypeError\">\n",
                i % distinct.max(1)
            ));
            s.push_str("TypeError: not a function\n ❯ registerTrait src/trait/trait.ts:102:25\n");
            s.push_str("            </failure>\n        </testcase>\n");
        }
        s.push_str("    </testsuite>\n</testsuites>\n");
        s
    }

    #[test]
    fn a_report_is_recognised() {
        assert!(detect(&report(4, 1)));
        assert!(!detect(
            "<project><modelVersion>4.0.0</modelVersion></project>"
        ));
        // `<testsuite` with no `<testcase` is some other schema.
        assert!(!detect("<testsuite name=\"x\"><properties/></testsuite>"));
    }

    /// Thirty-eight tests failing with the same message and the same stack is
    /// ONE fact. The real file that motivated this is exactly that shape.
    #[test]
    fn identical_failures_collapse_to_one_line_with_a_count() {
        let t = report(38, 1);
        let out = compact(&t);
        assert!(out.len() * 8 < t.len(), "{} of {}", out.len(), t.len());
        assert!(out.contains("x38 FAIL"), "{out}");
        assert!(out.contains("38 test(s), 38 failed"), "{out}");
        // …and the reason survives, once.
        assert_eq!(out.matches("registerTrait").count(), 1, "{out}");
    }

    /// Distinct failures are distinct facts and must all survive.
    #[test]
    fn every_distinct_failure_survives() {
        let out = compact(&report(30, 5));
        for v in 0..5 {
            assert!(
                out.contains(&format!("variant {v} is not a function")),
                "variant {v} was dropped:\n{out}"
            );
        }
    }

    /// XML entities are decoded: the output is a summary for a reader, not XML.
    #[test]
    fn entities_are_decoded_in_names_and_messages() {
        let out = compact(&report(2, 1));
        assert!(!out.contains("&gt;"), "{out}");
    }

    /// A summary that costs more than the file is not a summary.
    #[test]
    fn a_tiny_report_is_returned_as_is() {
        let t = "<testsuites tests=\"1\" failures=\"0\"><testsuite name=\"a\" tests=\"1\" failures=\"0\"><testcase name=\"t\"/></testsuite></testsuites>";
        assert_eq!(compact(t), t);
    }

    /// `find("name=\"")` matches the tail of `classname="…"`. Every testcase
    /// in a vitest or pytest report carries both, so the summary named the
    /// FILE where it meant to name the test — the same substring-for-a-token
    /// mistake as the SQL keyword read out of `update-type`.
    #[test]
    fn an_attribute_name_is_not_matched_inside_a_longer_one() {
        let line = r#"<testcase classname="tests/pair.test.ts" name="the real name">"#;
        assert_eq!(attr(line, "name").as_deref(), Some("the real name"));
        assert_eq!(
            attr(line, "classname").as_deref(),
            Some("tests/pair.test.ts")
        );
        // …and an attribute that genuinely is not there stays absent.
        assert_eq!(attr(line, "time"), None);
    }

    /// Failures with the same message differ in the LAST frame of their trace —
    /// the line inside the test file. Keying the group on the body gave
    /// thirty-eight groups of one and folded nothing.
    #[test]
    fn traces_that_differ_only_in_their_last_frame_still_group() {
        let mut s = String::from("<testsuites tests=\"20\" failures=\"20\" errors=\"0\">\n<testsuite name=\"a.ts\" tests=\"20\" failures=\"20\" errors=\"0\">\n");
        for i in 0..20 {
            s.push_str(&format!(
                "<testcase classname=\"a.ts\" name=\"case {i}\">\n<failure message=\"boom\" type=\"TypeError\">\nTypeError: boom\n at run a.ts:{}:5\n</failure>\n</testcase>\n",
                10 + i
            ));
        }
        s.push_str("</testsuite>\n</testsuites>\n");
        let out = compact(&s);
        assert!(out.contains("x20 FAIL"), "{out}");
    }

    // The test that went through `auto::compact` — "the summary does not come back
    // with its errors attached twice" — is in
    // `tests/misc/a_format_is_reached_by_its_router.rs`.

    /// A green report still reports: counts are the answer when nothing failed.
    #[test]
    fn a_passing_report_gives_its_counts() {
        let mut s = String::from(
            "<testsuites name=\"s\" tests=\"400\" failures=\"0\" errors=\"0\" time=\"12.5\">\n",
        );
        s.push_str("  <testsuite name=\"suite/a.ts\" tests=\"400\" failures=\"0\" errors=\"0\" skipped=\"2\">\n");
        for i in 0..400 {
            s.push_str(&format!(
                "    <testcase classname=\"suite/a.ts\" name=\"case {i}\"/>\n"
            ));
        }
        s.push_str("  </testsuite>\n</testsuites>\n");
        let out = compact(&s);
        assert!(out.contains("400 test(s), 0 failed"), "{out}");
        assert!(out.len() * 10 < s.len(), "{} of {}", out.len(), s.len());
    }
}

#[cfg(test)]
mod corpus_gain {
    #[test]
    #[ignore = "measurement"]
    fn what_the_junit_corpus_costs_now() {
        let Ok(root) = std::env::var("SWEEP_ROOT") else {
            return;
        };
        let mut stack = vec![std::path::PathBuf::from(&root)];
        let (mut n, mut inb, mut outb) = (0usize, 0usize, 0usize);
        let (mut bail, mut bailb, mut big, mut mid) = (0usize, 0usize, 0usize, 0usize);
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let nm = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if nm.starts_with('.') || nm == "target" || nm == "node_modules" {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().unwrap_or_default() != "xml" {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if t.len() < 4_000 || t.len() > 3_000_000 {
                    continue;
                }
                if !super::detect(&t) {
                    continue;
                }
                n += 1;
                inb += t.len();
                let o = super::compact(&t);
                outb += o.len();
                let r = o.len() as f64 / t.len() as f64;
                if r >= 0.99 {
                    bail += 1;
                    bailb += t.len();
                } else if r <= 0.2 {
                    big += 1;
                } else {
                    mid += 1;
                }
            }
        }
        eprintln!(
            "JUNIT n={n} in={inb} out={outb} ratio={:.3}",
            outb as f64 / inb.max(1) as f64
        );
        eprintln!("JUNIT unchanged={bail} ({bailb} bytes)  big<=0.2={big}  mid={mid}");
    }
}

#[cfg(test)]
mod unformatted {
    use super::*;

    /// pytest writes its whole report on ONE line. The line-based first
    /// version found nothing in it and returned all 1 707 011 bytes; 125 of
    /// the 257 reports in the corpus were that shape and carried 16.3 MB of
    /// the 21.6. XML has no lines.
    #[test]
    fn a_report_with_no_newlines_is_still_read() {
        let mut t = String::from(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><testsuites name=\"pytest tests\">\
             <testsuite name=\"pytest\" errors=\"0\" failures=\"0\" skipped=\"1699\" tests=\"11792\" time=\"101.36\">",
        );
        for i in 0..2000 {
            t.push_str(&format!(
                "<testcase classname=\"tests/test_{i}.py\" name=\"case_{i}\" time=\"0.001\" />"
            ));
        }
        t.push_str("</testsuite></testsuites>");
        assert!(detect(&t));
        let out = compact(&t);
        assert!(out.contains("11792 test(s), 0 failed"), "{out}");
        assert!(out.contains("1699 skipped"), "{out}");
        assert!(out.len() * 100 < t.len(), "{} of {}", out.len(), t.len());
    }

    /// …and failures inside a one-line report still group and keep their body.
    #[test]
    fn failures_in_a_one_line_report_group_and_keep_their_trace() {
        let mut t = String::from(
            "<testsuites tests=\"12\" failures=\"12\" errors=\"0\"><testsuite name=\"a.py\" tests=\"12\" failures=\"12\" errors=\"0\">",
        );
        for i in 0..12 {
            t.push_str(&format!(
                "<testcase classname=\"a.py\" name=\"case_{i}\"><failure message=\"assert 1 == 2\" type=\"AssertionError\">E  assert 1 == 2\na.py:{}: AssertionError</failure></testcase>",
                10 + i
            ));
        }
        t.push_str("</testsuite></testsuites>");
        let out = compact(&t);
        assert!(out.contains("x12 FAIL"), "{out}");
        assert!(out.contains("assert 1 == 2"), "{out}");
        assert!(out.contains("first: case_0"), "{out}");
    }

    /// A `>` inside a quoted attribute value must not end the tag — vitest
    /// escapes it, but a writer is free not to.
    #[test]
    fn a_bracket_inside_an_attribute_does_not_end_the_tag() {
        let tag = r#"<testcase classname="a.ts" name="a > b > c" time="0.1">"#;
        assert_eq!(attr(tag, "name").as_deref(), Some("a > b > c"));
        assert_eq!(end_of_tag(tag, 0), Some(tag.len() - 1));
    }
}
