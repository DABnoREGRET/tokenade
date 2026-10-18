//! `grep` — grep.
//!
//! Archetype: cap (cap=200) + common-path-prefix strip.
//!
//! Typical agent grep returns 10-100 lines all from the same
//! project tree (`crates/tokenade-core/src/foo.rs:42:`,
//! `crates/tokenade-core/src/bar.rs:99:`, …). The repeated path
//! prefix dominates the bytes. Detect the longest common
//! `path/segment/` prefix shared by ALL match lines and strip it
//! once; emit a single `[paths from <prefix>]` header so the
//! agent can still resolve full paths.

use std::fmt::Write;
pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

/// Above this many match lines, the cap alone is not an answer: 200 arbitrary
/// lines out of several thousand tell the agent nothing about WHERE the hits
/// are. A census does, and costs one line per file.
const CENSUS_MIN_LINES: usize = 400;

/// Bytes the census may spend naming files, most matches first. It was a
/// count of FIFTEEN files: measured 2026-10-03, `grep -rn 'fn ' crates/tk-fold/src`
/// (683 hits, 47 files, 66 KB) named 15 and said `+32 more` — 19 of the 47
/// files then appeared nowhere in the output, though all 47 names fit in
/// 1.4 KB. A file holding a hit is what a census is read for.
const CENSUS_BUDGET_BYTES: usize = 4_000;

pub fn compact(text: &str) -> String {
    compact_for(text, &[])
}

/// [`compact`], with the literal pieces of the searched pattern to centre a
/// clipped hit on (see [`clip_long_hits`]).
pub fn compact_for(text: &str, needles: &[String]) -> String {
    let clipped = clip_long_hits(text, needles);
    let text = clipped.as_str();
    let stripped = strip_common_path_prefix(text);
    let Some(census) = file_census(&stripped) else {
        return crate::archetypes::line_cap_results(&stripped, 200, "grep match(es)");
    };
    // The sample yields to the census, not the other way round: past the
    // delivery threshold the stage downstream cuts the MIDDLE of whatever it
    // is handed, and the census sits at the top of a 200-line sample.
    let mut cap = 200;
    loop {
        let out = format!("{census}{}", crate::archetypes::line_cap_results(&stripped, cap, "grep match(es)"));
        if cap <= 20 || tk_base::delivery::fits(&out) {
            return out;
        }
        cap -= 20;
    }
}

/// The path part of a grep match line, for both shapes grep emits:
/// `path:42:content` (with `-n`) and `path:content` (without).
///
/// The no-`-n` shape was previously unreadable here, which is why a plain
/// `grep -r pattern tree/` — the single most common form an agent types —
/// got neither the prefix strip nor any per-file accounting.
///
/// A path is only accepted when it looks like one: no whitespace, and either
/// a directory separator or a file extension. That rules out prose lines that
/// merely contain a colon, which is the whole risk of dropping the digit
/// requirement.
fn match_path(line: &str) -> Option<&str> {
    // `C:\\x\\services:10:echo` — the drive's colon is part of the path.
    let drive = line.as_bytes().get(1..3) == Some(b":\\".as_slice());
    let skip = if drive { 2 } else { 0 };
    let head = &line[..skip + line[skip..].find(':')?];
    if head.is_empty() || head.len() > 400 || head.contains(char::is_whitespace) {
        return None;
    }
    let has_sep = head.contains('/') || head.contains('\\');
    let has_ext = std::path::Path::new(head).extension().is_some();
    (has_sep || has_ext).then_some(head)
}

/// One line per matching file, most matches first — emitted ABOVE the capped
/// sample so the agent sees the distribution before the excerpt.
///
/// Returns `None` unless the output is big enough for the cap to be hiding
/// something (`CENSUS_MIN_LINES`) and nearly every line parses as a match, so
/// a census is never invented from a shape we did not understand.
fn file_census(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            !l.trim().is_empty()
                && !l.starts_with("[paths from ")
                && !l.starts_with("[all matches in ")
        })
        .collect();
    if lines.len() < CENSUS_MIN_LINES {
        return None;
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut parsed = 0usize;
    for l in &lines {
        if let Some(p) = match_path(l) {
            *counts.entry(p).or_default() += 1;
            parsed += 1;
        }
    }
    if parsed * 10 < lines.len() * 9 {
        return None;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    // Ties sort by path so the same output always prints the same census.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut out = format!(
        "[tokenade:grep] {} match(es) in {} file(s)\n",
        parsed,
        ranked.len()
    );
    let mut named = 0usize;
    for (path, n) in &ranked {
        let row = format!("  {n:>5}  {path}\n");
        if out.len() + row.len() > CENSUS_BUDGET_BYTES {
            break;
        }
        out.push_str(&row);
        named += 1;
    }
    if ranked.len() > named {
        // The rest, by directory: where the unnamed files are.
        let mut dirs: Vec<(&str, usize)> = Vec::new();
        for (path, _) in &ranked[named..] {
            let dir = path.rsplit_once('/').map_or(".", |(d, _)| d);
            match dirs.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, k)) => *k += 1,
                None => dirs.push((dir, 1)),
            }
        }
        dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let shown: Vec<String> = dirs.iter().take(10).map(|(d, k)| format!("{d}/ ({k})")).collect();
        let more = if dirs.len() > 10 { format!(" · +{} more dir(s)", dirs.len() - 10) } else { String::new() };
        let _ = writeln!(out, "  ... (+{} more file(s): {}{more})", ranked.len() - named, shown.join(" · "));
    }
    Some(out)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// When ALL non-empty lines start with `<path>:<line>:`, find the
/// longest path prefix shared by every line and strip it (emit
/// once at top as `[paths from <prefix>]`). Falls back to the
/// original text when:
///   - fewer than 3 match lines (banner overhead exceeds savings),
///   - no common prefix of meaningful length (≥10 chars),
///   - any line doesn't match the `<path>:<digit>:` shape.
fn strip_common_path_prefix(text: &str) -> String {
    // Collect non-empty lines + verify they're all grep -n shape.
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // Extract the path part (up to first `:` followed by a digit).
    // A hit the console wrapped (`Select-String` folds a long line at the
    // window's width) leaves a continuation that names no path — 46 of 200
    // lines on a real run. The text opens on a hit and hits are most of it;
    // only they lose the prefix, a continuation is kept whole.
    let paths: Vec<&str> = lines.iter().filter_map(|l| match_path(l)).collect();
    if lines.first().and_then(|l| match_path(l)).is_none() || paths.len() * 2 < lines.len() {
        return text.to_string();
    }
    // The decision — segments and never bytes, minimum length, profitability,
    // and the wording when everything comes from a single file — lives in
    // `filelist`, with the twenty listing commands that share it.
    let Some((header, strip_len)) = crate::filelist::common_prefix_header(&paths) else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    let _ = writeln!(out, "{header}");
    // The SAME rendering as the twenty listing commands: common prefix removed,
    // then each directory stated once. This loop only did the
    // first, and the second was added to it without it noticing.
    crate::filelist::push_grouped(&mut out, &lines, strip_len, &|l| match_path(l));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_hit(needle_at: usize) -> String {
        let mut line = String::from(".next/server/chunks/814.js:1:");
        for i in 0..needle_at {
            let _ = write!(line, "var q{i}=function(t){{return t.k{i}}};");
        }
        line.push_str("if(n.revived)return Revived(n);");
        for i in 0..900 {
            let _ = write!(line, "const z{i}=(e)=>e*{i};");
        }
        line
    }

    /// A 5 MB line of a Next.js chunk beside the source hits: the source hits
    /// come through whole, the chunk's line is cut round the match and says
    /// how much it lost on each side.
    #[test]
    fn a_bundle_hit_is_clipped_round_the_match_and_source_hits_stay() {
        let text = format!(
            "app/inbox/page.tsx:41:  {{row.revived && <Badge>relancé</Badge>}}\n{}\nlib/revive.ts:9:export const REVIVED_TTL_DAYS = 14;\n",
            bundle_hit(3000)
        );
        let needles = pattern_needles("revived\\|relancé");
        assert_eq!(needles, ["revived", "relancé"]);
        let out = compact_for(&text, &needles);
        assert!(out.contains("app/inbox/page.tsx:41:  {row.revived && <Badge>relancé</Badge>}"), "{out}");
        assert!(out.contains("lib/revive.ts:9:export const REVIVED_TTL_DAYS = 14;"), "{out}");
        let chunk = out.lines().find(|l| l.contains("814.js")).expect(&out);
        assert!(chunk.starts_with(".next/server/chunks/814.js:1:…[+"), "{chunk}");
        assert!(chunk.contains("if(n.revived)return Revived(n);"), "{chunk}");
        assert!(chunk.len() < 2 * HIT_CAP, "{}", chunk.len());
        let parts: Vec<usize> = chunk
            .match_indices("…[+")
            .map(|(i, m)| chunk[i + m.len()..].split(' ').next().unwrap().parse().unwrap())
            .collect();
        let kept = chunk.chars().count() - ".next/server/chunks/814.js:1:".len();
        let marks: usize = parts.iter().map(|n| format!("…[+{n} chars]").chars().count()).sum();
        let original = bundle_hit(3000).chars().count() - ".next/server/chunks/814.js:1:".len();
        assert_eq!(parts.iter().sum::<usize>() + kept - marks, original, "the counts must add up");
    }

    #[test]
    fn hits_with_a_long_line_needs_hit_shaped_lines() {
        let hits = format!("a/b.ts:3:x\n{}\n", bundle_hit(10));
        assert!(hits_with_a_long_line(&hits));
        let prose = format!("Note: see below\n{}\n", "word ".repeat(200));
        assert!(!hits_with_a_long_line(&prose));
        assert!(!hits_with_a_long_line("a/b.ts:3:x\na/c.ts:4:y\n"), "nothing long");
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn try_compact_some() {
        assert!(try_compact("sample line\n").is_some());
    }

    #[test]
    fn a_recursive_grep_without_line_numbers_still_strips_the_prefix() {
        // `grep -r pattern tree/` emits `path:content`, with no line number.
        // That shape used to fail the parser, so the single most common grep
        // an agent types got no prefix strip at all.
        let input = "\
crates/tokenade-core/src/auto.rs:    let foo = 1;
crates/tokenade-core/src/bar.rs:    let foo = 2;
crates/tokenade-core/src/baz.rs:    let foo = 3;
crates/tokenade-core/src/qux.rs:    let foo = 4;
";
        let out = strip_common_path_prefix(input);
        assert!(out.starts_with("[paths from crates/tokenade-core/src/]"), "got:\n{out}");
        assert!(out.contains("auto.rs:    let foo = 1;"));
    }

    #[test]
    fn prose_containing_a_colon_is_not_mistaken_for_a_path() {
        // The digit requirement was the old guard against this. Its
        // replacement must hold: no whitespace, and a separator or extension.
        let input = "\
note: this is prose
warning: so is this
error: and this
";
        assert_eq!(strip_common_path_prefix(input), input);
    }

    /// Past the census budget, the unnamed files are placed by directory.
    #[test]
    fn files_past_the_census_budget_are_placed_by_directory() {
        let mut input = String::new();
        for i in 0..600 {
            let d = if i % 3 == 0 { "crates/alpha/src/deep" } else { "crates/beta/src" };
            input.push_str(&format!("{d}/a_rather_long_module_name_{i}.rs:{i}:    let hit = {i};\n"));
        }
        let out = compact(&input);
        let end = out.find("more file(s):").expect("no directory tail");
        let tail_end = end + out[end..].find('\n').unwrap();
        let tail = &out[out[..end].rfind('\n').unwrap() + 1..tail_end];
        assert!(tail.contains("crates/beta/src/ (") && tail.contains("crates/alpha/src/deep/ ("), "{tail}");
        assert!(tail_end - tail.len() <= CENSUS_BUDGET_BYTES, "census over budget");
    }

    /// A hit that says `error` is a search result, not a failure of grep:
    /// it is not pulled out of order past the cap.
    #[test]
    fn a_hit_that_reads_like_an_error_is_not_rescued_out_of_order() {
        let _g = tk_base::delivery::threshold_on_this_thread(100_000);
        let mut input = String::new();
        for i in 0..300 {
            let word = if i == 250 { "fn des_erreurs_suivies() {" } else { "fn ordinary() {" };
            input.push_str(&format!("src/m{}.rs:{i}:    {word}\n", i % 7));
        }
        let out = compact(&input);
        assert!(!out.contains("look like failures"), "{out}");
        assert!(!out.contains("des_erreurs_suivies"), "{out}");
    }

    #[test]
    fn a_huge_grep_reports_where_the_matches_are() {
        let mut input = String::new();
        for i in 0..500 {
            let f = i % 40;
            input.push_str(&format!("src/mod{f}/file.rs:{i}:    let hit = {i};\n"));
        }
        let out = compact(&input);
        assert!(out.starts_with("[tokenade:grep] 500 match(es) in 40 file(s)"), "got:\n{}", &out[..120]);
        // Forty files fit the census budget: every one is named.
        for f in 0..40 {
            assert!(out.contains(&format!("  src/mod{f}/file.rs\n")), "mod{f} unnamed:\n{out}");
        }
        assert!(!out.contains("more file(s)"), "got:\n{out}");
    }

    #[test]
    fn a_small_grep_gets_no_census() {
        let input: String = (0..20)
            .map(|i| format!("src/a{i}.rs:{i}:hit\n"))
            .collect();
        assert!(!compact(&input).contains("[tokenade:grep]"));
    }

    #[test]
    fn strips_common_path_prefix() {
        let input = "\
crates/tokenade-core/src/auto.rs:42:    foo
crates/tokenade-core/src/auto.rs:99:    foo bar
crates/tokenade-core/src/bar.rs:15:    foo baz
crates/tokenade-core/src/baz.rs:101:    foo qux
";
        let out = compact(input);
        assert!(out.contains("[paths from crates/tokenade-core/src/]"));
        assert!(out.contains("auto.rs:42:    foo"));
        assert!(out.contains("bar.rs:15:    foo baz"));
        assert!(!out.contains("crates/tokenade-core/src/auto.rs:42"));
    }

    #[test]
    fn keeps_short_prefix_as_is() {
        // Common prefix `src` is too short (< 10 chars) — overhead
        // not worth the savings.
        let input = "src/a.rs:1:foo\nsrc/b.rs:2:bar\nsrc/c.rs:3:baz\n";
        let out = compact(input);
        assert!(!out.contains("[paths from"));
        assert!(out.contains("src/a.rs:1:foo"));
    }

    #[test]
    fn keeps_unrelated_paths_as_is() {
        // No common prefix → bail.
        let input = "\
/home/u/foo.rs:1:match
/tmp/bar.rs:2:match
/var/baz.rs:3:match
";
        let out = compact(input);
        assert!(!out.contains("[paths from"));
    }

    #[test]
    fn keeps_tiny_match_set_as_is() {
        // Only 2 lines — banner cost > savings even with long prefix.
        let input = "crates/tokenade-core/src/foo.rs:1:bar\ncrates/tokenade-core/src/baz.rs:2:bar\n";
        let out = compact(input);
        assert!(!out.contains("[paths from"));
    }

    #[test]
    fn handles_non_grep_shape_gracefully() {
        // Lines without `:digit:` shape (e.g. file-list output) bail.
        let input = "/home/u/file1\n/home/u/file2\n/home/u/file3\n";
        let out = compact(input);
        assert!(!out.contains("[paths from"));
    }

    #[test]
    fn saves_real_bytes_on_realistic_grep() {
        // 30 lines from the same project — should compress
        // significantly.
        let mut input = String::new();
        for i in 0..30 {
            let _ = writeln!(
                input,
                "crates/tokenade-core/src/cmd/linux/foo{i}.rs:{i}:    match here"
            );
        }
        let out = compact(&input);
        assert!(out.len() < input.len(), "must shrink: in={} out={}", input.len(), out.len());
        // Verify a typical original line is gone (replaced by stripped form).
        assert!(!out.contains("crates/tokenade-core/src/cmd/linux/foo5.rs"));
        // The stripped form must be present.
        assert!(out.contains("foo5.rs:5:    match here"));
    }
}

#[cfg(test)]
mod single_file_header_tests {
    #[test]
    fn one_file_is_named_as_a_file_not_a_directory() {
        // Every match from one file → the common prefix IS that file's path.
        // `[paths from …/hcl.rs/]` invited the reader to rejoin a residual to a
        // directory that does not exist (`crates/a/b/hcl.rs/128`).
        let t = "crates/a/b/hcl.rs:128:    let x = 1;\n\
                 crates/a/b/hcl.rs:132:    let y = 2;\n\
                 crates/a/b/hcl.rs:133:    let z = 3;\n";
        let out = super::compact(t);
        assert!(
            out.starts_with("[all matches in crates/a/b/hcl.rs]"),
            "single-file header wrong: {out}"
        );
        assert!(!out.contains("hcl.rs/]"), "file still shown as a dir: {out}");
        assert!(out.contains("128:    let x = 1;"));
    }

    #[test]
    fn several_files_keep_the_directory_wording() {
        let t = "crates/a/b/one.rs:1:x\ncrates/a/b/two.rs:2:y\ncrates/a/b/three.rs:3:z\n";
        let out = super::compact(t);
        assert!(
            out.starts_with("[paths from crates/a/b/]"),
            "multi-file header regressed: {out}"
        );
        assert!(out.contains("one.rs:1:x"));
    }
}

/// A match line longer than this is clipped.
pub const HIT_CAP: usize = 500;
/// Characters of a clipped hit kept around the match (each side), or from
/// the start when the match is not found.
const HIT_WINDOW: usize = 120;

/// Clip every match line over [`HIT_CAP`] to its `path:line:` and a window of
/// its content, each cut written `…[+N chars]`.
///
/// `grep -rn revived dashboard/` matched one line of a Next.js chunk — 5 MB on
/// one line — beside the source hits it was run for. Nothing here cut it, the
/// output did not shrink, and the safety net handed it to the auto router,
/// which saw a bundle: `[tokenade:minified] bundled source — 12759017 bytes`
/// and an import list, every source hit gone. Clipped, the bundle's line
/// says where the match is, and the source hits come through untouched.
///
/// The window is centred on the first of `needles` found (ASCII
/// case-insensitive, so byte offsets hold); with no needle found it is the
/// start of the content.
pub fn clip_long_hits(text: &str, needles: &[String]) -> String {
    if !text.lines().any(|l| l.len() > HIT_CAP) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len().min(64 * 1024));
    for line in text.lines() {
        if line.len() <= HIT_CAP {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let head_len = hit_prefix_len(line);
        let (head, content) = line.split_at(head_len);
        out.push_str(head);
        let lower = content.to_ascii_lowercase();
        let at = needles
            .iter()
            .filter(|n| !n.is_empty())
            .find_map(|n| lower.find(&n.to_ascii_lowercase()))
            .unwrap_or(0);
        let chars: Vec<(usize, char)> = content.char_indices().collect();
        let centre = chars.partition_point(|(b, _)| *b < at);
        let from = centre.saturating_sub(HIT_WINDOW);
        let to = (centre + HIT_WINDOW).min(chars.len());
        let byte = |i: usize| chars.get(i).map_or(content.len(), |(b, _)| *b);
        if from > 0 {
            let _ = write!(out, "…[+{from} chars]");
        }
        out.push_str(&content[byte(from)..byte(to)]);
        if to < chars.len() {
            let _ = write!(out, "…[+{} chars]", chars.len() - to);
        }
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// Bytes of `path:line:` (or `path:`) at the start of a match line; 0 when
/// the line is not one.
fn hit_prefix_len(line: &str) -> usize {
    let Some(path) = match_path(line) else {
        return 0;
    };
    let rest = &line[path.len() + 1..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && rest.as_bytes().get(digits) == Some(&b':') {
        path.len() + 1 + digits + 1
    } else {
        path.len() + 1
    }
}

/// The literal pieces of a grep pattern argument: `a\|b`, `a|b` split into
/// alternatives, each cut to its longest run of word characters (3+).
pub fn pattern_needles(pattern: &str) -> Vec<String> {
    pattern
        .split("\\|")
        .flat_map(|alt| alt.split('|'))
        .filter_map(|alt| {
            alt.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ' ' || c == '-'))
                .max_by_key(|run| run.len())
                .map(str::trim)
                .filter(|run| run.len() >= 3)
                .map(String::from)
        })
        .collect()
}

/// `grep -n` output (3 in 4 non-blank lines are `path:N:`) carrying at least
/// one hit over [`HIT_CAP`] — the shape the minified detector took for a
/// bundle when `grep … | head` reached the auto router with too few lines
/// for the search-output detector.
pub fn hits_with_a_long_line(text: &str) -> bool {
    let mut lines = 0usize;
    let mut hits = 0usize;
    let mut long = false;
    for l in text.lines().filter(|l| !l.trim().is_empty()).take(200) {
        lines += 1;
        let n = hit_prefix_len(l);
        if n > 0 && l.as_bytes()[n - 1] == b':' && l[..n - 1].ends_with(|c: char| c.is_ascii_digit()) {
            hits += 1;
        }
        long |= l.len() > HIT_CAP;
    }
    long && hits >= 2 && hits * 4 >= lines * 3
}

#[cfg(test)]
mod windows_select_string {
    /// A real `Select-String -Path C:\…\services -Pattern tcp`: grep's shape
    /// with a drive letter, read as grep — the path is the whole `C:\…`.
    #[test]
    fn select_string_is_read_as_grep() {
        let text = include_str!("../../../tests/fixtures/linux/select_string_services.txt");
        let hit = text.lines().find(|l| !l.trim().is_empty()).unwrap();
        assert_eq!(super::match_path(hit), Some("C:\\Windows\\System32\\drivers\\etc\\services"));
        let out = crate::cmd::dispatch_named("Select-String", text).expect("routed");
        assert!(out.len() * 3 < text.len() * 2, "{} vs {}", out.len(), text.len());
        assert!(out.contains("C:\\Windows\\System32\\drivers\\etc\\services]"), "{out}");
        assert!(out.lines().any(|l| l.starts_with("10:echo")), "{out}");
        assert!(out.lines().any(|l| l == "Protocol"), "a wrapped continuation stays: {out}");
    }

    /// Several files under one Windows folder: the folder is said once.
    #[test]
    fn a_shared_windows_folder_is_said_once() {
        let text = "C:\\proj\\src\\app\\a.cs:1:foo\nC:\\proj\\src\\app\\b.cs:2:bar\nC:\\proj\\src\\app\\c.cs:3:baz\n";
        let out = super::strip_common_path_prefix(text);
        assert!(out.starts_with("[paths from C:\\proj\\src\\app\\]"), "{out}");
        assert!(out.contains("a.cs:1:foo"), "{out}");
    }
}
