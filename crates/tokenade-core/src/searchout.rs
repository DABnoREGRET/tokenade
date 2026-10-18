//! `grep` / `ripgrep` / `git grep` search-results compaction.
//!
//! Code-search sessions hit this constantly:
//!
//! ```text
//! src/main.rs:42:    let result = compute_thing();
//! src/main.rs:43:    println!("{result}");
//! src/lib.rs:15:    pub fn compute_thing() -> i32 {
//! src/lib.rs:16:        let x = 42;
//! tests/it.rs:8:    assert_eq!(compute_thing(), 42);
//! ```
//!
//! A `grep -r foo .` on a real repo can return 1 000 – 50 000 lines. The
//! agent reading it wants:
//!
//!   - the **list of files** that matched + per-file count
//!   - **sample matches** per file (first few lines)
//!   - **total** match count
//!
//! Detection: ≥10 lines matching `<path>:<digits>:<text>` AND those make
//! up ≥75 % of non-blank head lines. Also accepts `<path>:<digits>-<text>`
//! context lines (rg `--context=N` mode). Rejects log lines like
//! `2024-01-15T10:30:00:` since the second field needs to be pure digits
//! that fit `u32`.

use std::collections::BTreeMap;
use std::fmt::Write;

pub const FILES_CAP: usize = 50;
pub const MATCHES_PER_FILE_CAP: usize = 10;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    let mut grep_lines = 0usize;
    let mut total_nonblank = 0usize;
    for line in head.lines().take(80) {
        if line.trim().is_empty() {
            continue;
        }
        total_nonblank += 1;
        if parse_grep_line(line).is_some() {
            grep_lines += 1;
        }
    }
    grep_lines >= 10 && total_nonblank > 0 && (grep_lines * 100) / total_nonblank >= 75
}

/// Parse a single grep-style line into `(path, line_number, content)`.
/// Accepts three shapes:
///   - `path:linenum:content`    (default match)
///   - `path:linenum-content`    (context line)
///   - `path\0linenum:content`   (NUL-separated, `rg -0` / `git grep -z`)
///
/// The NUL variant is what rtk uses (`grep_cmd.rs:163-183`) because it
/// is the only way to correctly parse paths that contain `:` like
/// `src/ClassRegistry::init.rs`. Without it, the `:`-only parser
/// mis-anchors and drops legitimate matches.
fn parse_grep_line(line: &str) -> Option<(&str, u32, &str)> {
    // NUL-separator fast path. Real grep / rg never emits `\0` in the
    // middle of a path or content, so finding one means the user (or
    // their tool wrapper) opted into the unambiguous form.
    if let Some(nul) = line.find('\0') {
        let path = &line[..nul];
        if path.is_empty() {
            return None;
        }
        let rest = &line[nul + 1..];
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits_end == 0 {
            return None;
        }
        let line_num: u32 = rest[..digits_end].parse().ok()?;
        let after = &rest[digits_end..];
        let content = after
            .strip_prefix(':')
            .or_else(|| after.strip_prefix('-'))?;
        return Some((path, line_num, content));
    }
    // Look for the first `:` separating path from line-number.
    let first_colon = line.find(':')?;
    if first_colon == 0 {
        return None;
    }
    let path = &line[..first_colon];
    // Reject obvious non-paths: lines starting with a digit (could be
    // timestamp lines like `2024-01-15T...`).
    if path.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // Reject bracketed log prefixes like `[2026-05-28T10:00:00Z]` —
    // container-logs and journal lines often have this shape, and the
    // first `:` inside the bracket fooled us into parsing them as
    // `path:linenum:content`. Real grep output never bracket-wraps the
    // path.
    if path.starts_with('[') {
        return None;
    }
    // Reject paths with internal whitespace (real grep paths can't have
    // whitespace before the `:linenum`).
    if path.contains([' ', '\t']) {
        return None;
    }
    let rest = &line[first_colon + 1..];

    // Find the separator after the line-number: either ':' (match) or '-'
    // (context). Walk forward while we see digits; the first non-digit must
    // be one of those two.
    let digits_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits_end == 0 {
        return None;
    }
    let line_str = &rest[..digits_end];
    let line_num: u32 = line_str.parse().ok()?;
    let after_digits = &rest[digits_end..];
    if !after_digits.starts_with(':') && !after_digits.starts_with('-') {
        return None;
    }
    let content = &after_digits[1..];
    Some((path, line_num, content))
}

/// What a line becomes when the SAME line appears elsewhere in the fold.
enum Rendered {
    /// It appears only here: nothing to do.
    Once,
    /// First occurrence: the text, plus its tag.
    First(usize),
    /// A later one: the tag alone.
    Again(usize),
}

/// The lines that repeat among those the fold is going to EMIT.
///
/// MEASURED 2026-09-08 on the real capture of 400 matches
/// (`formats/grep_matches`, 45,355 bytes): the fold delivered 10,770 of them,
/// of which **5,577 (52%) are byte-for-byte repetitions** —
/// `pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {`
/// twenty-nine times, `pub fn compact(text: &str) -> String {` twenty-nine
/// times. A grep on a symbol returns the same line in thirty files, and the
/// compactor wrote it thirty times.
///
/// What matters stays WHOLE: every match keeps its file and its line number,
/// which are the information one came looking for. Only the text, already in
/// view, stops being copied again.
///
/// The margin is computed on what the fold DELIVERS, not on what the
/// repetition weighs: the tag costs twice, so a short line repeated twice
/// gains nothing and is not folded. `cargo run -p tokenade-bench
/// --bin searchout_headroom` gives the figure.
struct Repeats {
    /// Text → (tag, already written?)
    tagged: std::collections::HashMap<String, (usize, bool)>,
}

impl Repeats {
    fn over(emitted: &[&str]) -> Self {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for line in emitted {
            *counts.entry(line).or_default() += 1;
        }
        // Tag order follows EMISSION order, so that a top-to-bottom read
        // meets `[=1]` before `[=2]`.
        let mut tagged = std::collections::HashMap::new();
        let mut next = 0usize;
        for line in emitted {
            let n = counts.get(line).copied().unwrap_or(1);
            if n < 2 || tagged.contains_key(*line) {
                continue;
            }
            let tag = next + 1;
            // Folding is only worth it if it saves bytes: the text is written
            // once with its tag, and each repetition still costs the tag.
            let label = format!("[={tag}]").len() + 1;
            if line.len() * n <= line.len() + label * n {
                continue;
            }
            next = tag;
            tagged.insert((*line).to_string(), (tag, false));
        }
        Self { tagged }
    }

    /// No folding: the control rendering, the one the fold must beat.
    fn none() -> Self {
        Self {
            tagged: std::collections::HashMap::new(),
        }
    }

    fn any(&self) -> bool {
        !self.tagged.is_empty()
    }

    fn distinct(&self) -> usize {
        self.tagged.len()
    }

    fn render(&mut self, line: &str) -> Rendered {
        match self.tagged.get_mut(line) {
            None => Rendered::Once,
            Some((tag, written)) => {
                let tag = *tag;
                if *written {
                    Rendered::Again(tag)
                } else {
                    *written = true;
                    Rendered::First(tag)
                }
            }
        }
    }
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        // Too few hits to group, but routed here for a hit too long to
        // deliver whole (`hits_with_a_long_line`).
        if crate::cmd::linux::grep::hits_with_a_long_line(text) {
            return crate::cmd::linux::grep::clip_long_hits(text, &[]);
        }
        return text.to_string();
    }
    // L42 — strip ANSI. `rg` and `git grep` colourise matches by
    // default. Stripping upfront ensures `path:line:content`
    // splitting works on terminal output that wasn't piped through
    // `| cat`.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();

    // Per-file: vec of (line_num, content). Use BTreeMap for deterministic
    // ordering when two files have identical counts.
    let mut by_file: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    let mut total_matches = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let Some((path, line_num, content)) = parse_grep_line(raw) else {
            // Not a grep match line — another command's output (the window-
            // limited detector can fire on a doc with a trailing section).
            unmatched.push(raw.to_string());
            continue;
        };
        total_matches += 1;
        by_file
            .entry(path.to_string())
            .or_default()
            .push((line_num, content.to_string()));
    }

    if by_file.is_empty() {
        return text.to_string();
    }

    // Sort files by match count descending, then path ascending.
    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    let n_files = files_sorted.len();

    // **The caps only apply above the delivery threshold.**
    //
    // They used to apply whatever the size: `grep -n "@Controller\|@Post"`
    // over three controllers (73 matches, ~1k tokens grouped) returned ten
    // lines per file, and the agent asked again for the whole list through
    // `expand-ref --raw` — 4 `searchout` results out of 23 in the transcripts,
    // 13 `auto:grep` out of 93. Below `disclose::threshold_tokens()` the full
    // grouped rendering is delivered as-is by the next stage; cutting it only
    // bought a round-trip. Same rule as the requested raw output.
    let full = build(
        &files_sorted,
        total_matches,
        n_files,
        &unmatched,
        Caps::NONE,
    );
    if tk_base::delivery::fits(&full) {
        return full;
    }
    build(
        &files_sorted,
        total_matches,
        n_files,
        &unmatched,
        Caps::DEFAULT,
    )
}

/// How many files, and matches per file, a rendering shows.
#[derive(Clone, Copy)]
struct Caps {
    files: usize,
    per_file: usize,
}

impl Caps {
    const NONE: Caps = Caps {
        files: usize::MAX,
        per_file: usize::MAX,
    };
    const DEFAULT: Caps = Caps {
        files: FILES_CAP,
        per_file: MATCHES_PER_FILE_CAP,
    };
}

#[allow(clippy::type_complexity)]
fn build(
    files_sorted: &[(&String, &Vec<(u32, String)>)],
    total_matches: usize,
    n_files: usize,
    unmatched: &[String],
    caps: Caps,
) -> String {
    // The lines ACTUALLY emitted, after both caps — repetition is counted on
    // them, not on the input.
    let emitted: Vec<&str> = files_sorted
        .iter()
        .take(caps.files)
        .flat_map(|(_, matches)| {
            matches
                .iter()
                .take(caps.per_file)
                .map(|(_, c)| c.trim_start())
        })
        .collect();
    // Built TWICE: without folding, then with. The fold is delivered only if
    // it pays for its own banner — the gate shared with `template_fold`.
    // Without it, a 20-byte line repeated twice saved 10 bytes for a 95-byte
    // banner, and the output grew.
    let plain = render(
        files_sorted,
        total_matches,
        n_files,
        &mut Repeats::none(),
        unmatched,
        caps,
    );
    let mut repeats = Repeats::over(&emitted);
    if !repeats.any() {
        return plain;
    }
    let folded = render(
        files_sorted,
        total_matches,
        n_files,
        &mut repeats,
        unmatched,
        caps,
    );
    if crate::fold_gate::pays(plain.len(), folded.len(), 0) {
        folded
    } else {
        plain
    }
}

#[allow(clippy::type_complexity)]
fn render(
    files_sorted: &[(&String, &Vec<(u32, String)>)],
    total_matches: usize,
    n_files: usize,
    repeats: &mut Repeats,
    unmatched: &[String],
    caps: Caps,
) -> String {
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "{total_matches} match(es) in {n_files} file(s)");
    if repeats.any() {
        let _ = writeln!(
            out,
            "[tokenade:searchout] {} line(s) repeat; written once, later \
             occurrences shown as [={{n}}]",
            repeats.distinct()
        );
    }

    for (path, matches) in files_sorted.iter().take(caps.files) {
        let _ = write!(out, "\n{} ({}):\n", path, matches.len());
        for (ln, content) in matches.iter().take(caps.per_file) {
            // Trim leading whitespace from content for tighter display, but
            // keep it short.
            let trimmed = content.trim_start();
            match repeats.render(trimmed) {
                Rendered::First(tag) => {
                    let _ = writeln!(out, "  {ln}: {trimmed} [={tag}]");
                }
                Rendered::Again(tag) => {
                    let _ = writeln!(out, "  {ln}: [={tag}]");
                }
                Rendered::Once => {
                    let _ = writeln!(out, "  {ln}: {trimmed}");
                }
            }
        }
        if matches.len() > caps.per_file {
            let _ = writeln!(
                out,
                "  … {} more match(es) in this file elided",
                matches.len() - caps.per_file
            );
        }
    }
    if n_files > caps.files {
        let _ = write!(
            out,
            "\n… {} more file(s) with matches elided\n",
            n_files - caps.files
        );
    }

    if !unmatched.is_empty() {
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {

    fn input_line(buf: &mut String, i: usize, text: &str) {
        buf.push_str(&format!("crates/core/src/mod{i}.rs:{}:{text}\n", 10 + i));
    }

    /// A grep on a symbol returns the SAME line in thirty files, and the
    /// compactor wrote it thirty times. MEASURED 2026-09-08 on the real
    /// capture of 400 matches: 10,770 bytes delivered, of which 5,577 (52%)
    /// were byte-for-byte repetitions.
    #[test]
    fn a_line_repeated_across_files_is_written_once() {
        let long = "pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {";
        let mut input = String::new();
        for i in 0..12 {
            input_line(&mut input, i, long);
        }
        // `detect` requires at least ten match lines. With eight, the
        // compactor returned the input unchanged and the "nothing is folded"
        // assertions passed VACUOUSLY.
        assert!(detect(&input), "the fixture must reach the compactor");
        let out = compact(&input);
        assert_eq!(
            out.matches(long).count(),
            1,
            "the text must be written ONCE:\n{out}"
        );
        // What matters stays whole: every file and every line number
        // survives, it is the information one came looking for.
        for i in 0..12 {
            assert!(out.contains(&format!("mod{i}.rs")), "file lost:\n{out}");
            assert!(
                out.contains(&format!("  {}: ", 10 + i)),
                "line lost:\n{out}"
            );
        }
        assert_eq!(out.matches("[=1]").count(), 12, "one tag per occurrence");
        // Every fold announces itself.
        assert!(
            out.contains("[tokenade:searchout]"),
            "fold not announced:\n{out}"
        );
        // Compared with the SAME fold over twelve DISTINCT texts: it is the
        // repetition that gets folded, not the output in general. Comparing
        // with the input would mostly measure the file headers the fold adds,
        // which is the other axis (the common prefix) and not this one.
        let mut distinct = String::new();
        for i in 0..12 {
            input_line(&mut distinct, i, &format!("{long} // {i}"));
        }
        let control = compact(&distinct);
        assert!(
            out.len() * 2 < control.len(),
            "folded {} versus {} for distinct texts",
            out.len(),
            control.len()
        );
    }

    /// Folding is only worth it if it saves bytes. A SHORT line repeated twice
    /// costs the tag twice and saves nothing: it is left as-is, and nothing
    /// announces itself.
    #[test]
    fn a_short_line_repeated_twice_is_left_alone() {
        let mut input = String::from("a/b.rs:1:x()\nc/d.rs:2:x()\n");
        for i in 0..10 {
            input.push_str(&format!("z/f{i}.rs:{}:distinct_line_{i}()\n", i + 1));
        }
        assert!(detect(&input), "the fixture must reach the compactor");
        let out = compact(&input);
        assert_eq!(out.matches("x()").count(), 2, "no tag for nothing:\n{out}");
        assert!(!out.contains("[=1]"), "{out}");
        assert!(
            !out.contains("[tokenade:searchout]"),
            "nothing was folded:\n{out}"
        );
    }

    /// The fold must pay for its own banner. A single short line repeated
    /// twice saves about ten bytes, the banner costs nearly a hundred: the
    /// output WOULD GROW. Nothing is folded, and nothing announces itself.
    #[test]
    fn a_fold_that_does_not_pay_for_its_banner_is_not_delivered() {
        let repeated = "let x = compute(input, options, context);";
        let mut input = String::new();
        input.push_str(&format!("a/one.rs:1:{repeated}\n"));
        input.push_str(&format!("b/two.rs:2:{repeated}\n"));
        for i in 0..10 {
            input.push_str(&format!(
                "z/f{i}.rs:{}:a_distinct_and_fairly_long_line_{i}();\n",
                i + 1
            ));
        }
        assert!(detect(&input), "the fixture must reach the compactor");
        let out = compact(&input);
        assert!(
            !out.contains("[tokenade:searchout]"),
            "banner not earned:\n{out}"
        );
        assert_eq!(out.matches(repeated).count(), 2, "{out}");
    }

    /// The two gates do not do the same job, and it took a mutation to see
    /// it: the GLOBAL gate refuses a fold that does not earn its banner, the
    /// PER-LINE gate refuses to tag a repetition that loses even though the
    /// fold itself pays handsomely. Without it, the short line would be tagged
    /// and would cost bytes inside a winning fold, where nothing would flag
    /// it.
    #[test]
    fn a_short_repeat_is_left_alone_inside_a_fold_that_pays() {
        let long = "pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {";
        let short = "x()";
        let mut input = String::new();
        for i in 0..12 {
            input_line(&mut input, i, long);
        }
        input.push_str(&format!("q/short_a.rs:1:{short}\n"));
        input.push_str(&format!("q/short_b.rs:2:{short}\n"));
        assert!(detect(&input), "the fixture must reach the compactor");
        let out = compact(&input);
        // The fold did happen.
        assert_eq!(out.matches(long).count(), 1, "{out}");
        assert!(out.contains("[tokenade:searchout]"), "{out}");
        // And the short line was NOT tagged: it would have cost.
        assert_eq!(
            out.matches(short).count(),
            2,
            "the short one stays written twice:\n{out}"
        );
        assert!(!out.contains("[=2]"), "only one tag is justified:\n{out}");
    }

    /// Two different texts do not share a tag, and tag order follows reading
    /// order.
    #[test]
    fn two_repeated_texts_get_two_labels_in_reading_order() {
        let a = "pub fn alpha(text: &str) -> String { unreachable!() }";
        let b = "pub fn bravo(text: &str) -> String { unreachable!() }";
        let mut input = String::new();
        for i in 0..6 {
            input.push_str(&format!("z/a{i}.rs:{}:{a}\n", i + 1));
        }
        for i in 0..6 {
            input.push_str(&format!("z/b{i}.rs:{}:{b}\n", i + 1));
        }
        assert!(detect(&input), "the fixture must reach the compactor");
        let out = compact(&input);
        assert_eq!(out.matches(a).count(), 1, "{out}");
        assert_eq!(out.matches(b).count(), 1, "{out}");
        let first = out.find("[=1]").expect("tag 1");
        let second = out.find("[=2]").expect("tag 2");
        assert!(first < second, "tags follow reading order:\n{out}");
    }
    use super::*;

    const GREP_SAMPLE: &str = "src/main.rs:42:    let result = compute_thing();
src/main.rs:43:    println!(\"{result}\");
src/main.rs:50:    compute_thing();
src/lib.rs:15:    pub fn compute_thing() -> i32 {
src/lib.rs:16:        let x = 42;
src/lib.rs:20:    compute_thing();
src/lib.rs:25:    compute_thing();
tests/it.rs:8:    assert_eq!(compute_thing(), 42);
tests/it.rs:9:    compute_thing();
tests/it.rs:10:    compute_thing();
benches/perf.rs:5:    b.iter(|| compute_thing());
";

    #[test]
    fn detects_grep_output() {
        assert!(detect(GREP_SAMPLE));
    }

    #[test]
    fn detects_with_context_lines() {
        // Context lines use `-` between linenum and content (rg -C N).
        let s = "src/main.rs:40-    fn frob() {
src/main.rs:41-        let setup = init();
src/main.rs:42:    let result = compute_thing();
src/main.rs:43-        println!(\"{result}\");
src/lib.rs:13-    /// Computes a thing.
src/lib.rs:14-    #[inline]
src/lib.rs:15:    pub fn compute_thing() -> i32 {
src/lib.rs:16-        let x = 42;
src/lib.rs:17-        x
src/lib.rs:18-    }
";
        assert!(detect(s));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect("This is just prose with no grep output shape.\n"));
    }

    #[test]
    fn rejects_timestamped_logs() {
        // `2024-01-15T10:30:00:` starts with digits — first-field digit check rejects.
        let s = "2024-01-15T10:30:00: starting service
2024-01-15T10:30:01: handling request
2024-01-15T10:30:02: done
2024-01-15T10:30:03: idle
2024-01-15T10:30:04: handling request
2024-01-15T10:30:05: done
2024-01-15T10:30:06: idle
2024-01-15T10:30:07: handling
2024-01-15T10:30:08: done
2024-01-15T10:30:09: idle
2024-01-15T10:30:10: handled
";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_code() {
        assert!(!detect(
            "fn main() {\n    let path = \"./src/foo.rs\";\n    println!(\"{path}\");\n}\n"
        ));
    }

    #[test]
    fn extracts_per_file_grouping() {
        let out = compact(GREP_SAMPLE);
        // Header.
        /* L122: banner removed; assertion deleted */
        // Per-file headers.
        assert!(out.contains("src/lib.rs (4):"));
        assert!(out.contains("src/main.rs (3):"));
        assert!(out.contains("tests/it.rs (3):"));
        assert!(out.contains("benches/perf.rs (1):"));
        // Sample contents.
        assert!(out.contains("42: let result = compute_thing();"));
        assert!(out.contains("15: pub fn compute_thing() -> i32 {"));
        // Line numbers preserved.
        assert!(out.contains("8: assert_eq!"));
    }

    #[test]
    fn parses_match_and_context_lines() {
        // Match form: `:linenum:content`.
        assert_eq!(
            parse_grep_line("src/main.rs:42:    let x = 5;"),
            Some(("src/main.rs", 42, "    let x = 5;"))
        );
        // Context form: `:linenum-content`.
        assert_eq!(
            parse_grep_line("src/main.rs:41-    fn frob() {"),
            Some(("src/main.rs", 41, "    fn frob() {"))
        );
        // Reject lines without `:`.
        assert_eq!(parse_grep_line("plain text"), None);
        // Reject lines starting with digits (timestamps).
        assert_eq!(parse_grep_line("2024:01:15T..."), None);
        // Reject lines without digits after the first `:`.
        assert_eq!(parse_grep_line("src/main.rs:not-a-number"), None);
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not grep output";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn handles_content_with_colons() {
        // Real code often has `:` in content (e.g. type annotations, JSON).
        let s = "src/foo.rs:10:    let config: HashMap<String, String> = HashMap::new();
src/foo.rs:11:    let json: &str = r#\"{\"key\": \"value\"}\"#;
src/foo.rs:12:    let url: &str = \"http://example.com:8080/path\";
src/bar.rs:5:    fn baz(x: i32) -> i32 { x + 1 }
src/bar.rs:6:    fn qux(y: i32) -> i32 { y - 1 }
src/bar.rs:7:    fn rab(z: i32) -> i32 { z * 2 }
src/baz.rs:1:    use std::collections::HashMap;
src/baz.rs:2:    use std::collections::HashSet;
src/baz.rs:3:    use std::collections::BTreeMap;
src/baz.rs:4:    use std::collections::BTreeSet;
";
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("3 file(s)"));
        // Colons in content preserved.
        assert!(out.contains("HashMap::new"));
        assert!(out.contains("http://example.com:8080"));
        assert!(out.contains("z * 2"));
    }

    #[test]
    fn handles_unicode_in_paths_and_content() {
        let s = "src/café.rs:10:    let name = \"Müller\";
src/café.rs:11:    println!(\"déjà-vu\");
src/中文.rs:5:    fn 計算() -> i32 { 42 }
src/中文.rs:6:    fn 解析() -> i32 { 0 }
src/中文.rs:7:    let x = \"中文字符\";
src/🚀.rs:1:    // Rocket emoji file
src/🚀.rs:2:    fn launch() {}
src/🚀.rs:3:    let icon = \"🚀\";
src/🚀.rs:4:    fn land() {}
src/🚀.rs:5:    let speed = 1000;
";
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("café.rs"));
        assert!(out.contains("中文.rs"));
        assert!(out.contains("🚀.rs"));
        assert!(out.contains("Müller"));
        assert!(out.contains("計算"));
        assert!(out.contains("中文字符"));
    }

    #[test]
    fn caps_matches_per_file() {
        // Under the delivery threshold every match is kept: 73 route lines in
        // three controllers are what the grep was run for, and cutting them to
        // ten per file only bought a re-fetch.
        let verbs = ["Get", "Post", "Put", "Delete", "Patch"];
        let mut small = String::new();
        for (f, n) in [("calendar", 41), ("mail", 22), ("organizer", 10)] {
            for i in 0..n {
                let _ = writeln!(
                    small,
                    "src/connectors/{f}.controller.ts:{}:  @{}('{f}/op-{i}')",
                    80 + i * 9,
                    verbs[i % 5]
                );
            }
        }
        let out = compact(&small);
        assert!(out.contains("73 match(es) in 3 file(s)"), "{out}");
        assert!(out.contains("@Post('calendar/op-36')"), "{out}");
        assert!(!out.contains("elided"), "{out}");

        // Past it, the per-file cap still applies.
        let mut big = String::new();
        for i in 1..=4_000 {
            let _ = writeln!(
                big,
                "src/big.rs:{i}:    let x{i} = compute_{}({i}, \"{}\");",
                i % 13,
                i * 7
            );
        }
        assert!(crate::tokenizer::estimate(&big) > crate::disclose::threshold_tokens());
        let out = compact(&big);
        assert!(out.contains("4000 match(es) in 1 file(s)"));
        assert!(out.contains("1: let x1 = compute_1(1, \"7\");"), "{out}");
        assert!(out.contains("more match(es) in this file elided"));
        assert!(!out.contains("11: let x11 ="));
    }

    #[test]
    fn savings_huge_grep() {
        // 5 000-line grep output across 100 files × 50 matches per file.
        let mut s = String::with_capacity(300_000);
        for f in 0..100 {
            for m in 0..50 {
                let _ = writeln!(
                    s,
                    "src/file{f}/mod.rs:{m}:    fn called_thing() {{ thing() }}"
                );
            }
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("5000 match(es) in 100 file(s)"));
        assert!(sav.percent() > 80.0, "got {sav:?}");
    }
}
