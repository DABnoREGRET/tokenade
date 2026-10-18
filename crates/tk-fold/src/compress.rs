//! Context compression: turn a big blob of source/text into a much smaller
//! representation that preserves what a model needs to act.
//!
//! Two strategies, both deterministic and reversible-enough for agents:
//!   * [`skeleton`] — keep declaration lines + signatures, drop bodies. Great
//!     for "show me the shape of this file" without the full implementation.
//!   * [`squeeze`] — collapse blank-line runs, trim trailing whitespace, and
//!     fold long literal blocks. Safe on any text.

/// Collapse whitespace noise without touching meaningful content.
pub fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

// ---------- headroom-inspired smart list folding ----------------------------------------

/// Error/warning keywords that make a line always worth keeping during
/// truncation (headroom SmartCrusher "error-first guarantee").
const ERROR_KEYWORDS: &[&str] = &[
    "error",
    "err:",
    // `npm ERR!` and `yarn ERR!` — the single most common JS build failure
    // banner, and `err:` does not match it. MEASURED lost from a 2 500-line
    // log before this line existed.
    "err!",
    // `--- FAIL: TestX` is how Go reports a failing test. Bare `fail` is far
    // too broad for a substring test (`failover`, `failsafe`, `fail-open` are
    // ordinary log words), so the marker is matched with its prefix.
    "--- fail",
    "fatal",
    "panic",
    "exception",
    "traceback",
    "failure",
    "failed",
    "abort",
    "sigsegv",
    "sigabrt",
    "warning",
    "warn:",
    "critical",
    "timeout",
    "timed out",
    "deadlock",
    "overflow",
    "out of memory",
    " oom ",
    "null pointer",
    "use after free",
    "undefined behavior",
    "undefined behaviour",
    // Measured 2026-09-06. A failure buried in a HETEROGENEOUS 800-line output
    // — one the template folder cannot collapse, so the middle is elided —
    // survives only if a keyword names it. Six real failure sentences were put
    // at line 401 of 800: the English `error: build failed` came back, and
    // FIVE did not. These four cover four of them.
    //
    // False positives were counted the same day, not assumed: over 305 283
    // lines of this repository's own source, `mismatch` appears 48 times,
    // `would destroy` 6, `erreur` 3, `terminated with` 2, `segmentation fault`
    // 1 — all inside identifiers and prose, and a passing test verdict is
    // already refused above this scan. `refus` was measured too and DROPPED:
    // 82 hits in one suite run, every one an English test name containing
    // "refuses".
    //
    // A checksum/signature line names no error and IS one.
    "mismatch",
    // `child process terminated with status 137` — a kill, with no keyword on
    // it. `sigsegv`/`sigabrt` cover the signal names, nothing covered the
    // sentence.
    "terminated with",
    "segmentation fault",
    "core dumped",
    // The plan-level twin of `will be destroyed` / `must be replaced` below:
    // `plan would DESTROY 3 resources in production` matched none of them.
    "would destroy",
    // French, because the tooling on a French machine answers in French and
    // this repository's own shell does: `échec`, `erreur`. Zero and three hits
    // respectively across the source corpus above.
    "échec",
    "erreur",
    // Irreversible infrastructure actions. Not errors — worse: a plan that
    // destroys a production database is the one line a human must see before
    // anyone types `apply`. The per-format compactor keeps them, and then the
    // generic cap that runs AFTER it cut them anyway, leaving only
    // `Plan: 200 to add, 0 to change, 1 to destroy.` — the agent learns that
    // something is destroyed and never what. MEASURED on a 200-resource plan.
    //
    // Multi-word on purpose: `destroy` alone is an ordinary word in logs,
    // `will be destroyed` is terraform/pulumi plan grammar and almost nothing
    // else.
    "will be destroyed",
    "must be replaced",
    "forces replacement",
];

/// A test-runner line whose OWN verdict says the test passed.
///
/// Test names are written in English and name what they defend:
/// `a_panic_is_reported_not_swallowed`, `e2e_web_batch_names_a_failed_target`,
/// `read_prompt_without_a_value_is_a_clean_error`. Scanning such a line for
/// error keywords finds the NAME and not the outcome, so a run in which
/// nothing failed came back saying lines were "kept below because they look
/// like failures" — inventing failures is the mirror of losing them, and this
/// product promises test failures always reach you.
///
/// MEASURED 2026-09-04 on a real 8 884-test `cargo nextest run` with zero
/// failures: 10 passing lines were rescued and announced as failures, every
/// one of them for a word inside the test's name.
///
/// Where a runner prints a verdict, the verdict decides. Only the passing
/// forms are listed: anything else keeps the keyword scan.
pub fn passing_test_verdict(line: &str) -> bool {
    let t = line.trim();
    // cargo-nextest: `PASS [   0.006s] (   1/8884) crate::suite name`
    if let Some(rest) = t.strip_prefix("PASS") {
        return rest.trim_start().starts_with('[');
    }
    // go test: `--- PASS: TestErrorHandling (0.00s)`
    if t.starts_with("--- PASS:") {
        return true;
    }
    // libtest: `test tests::a_panic_is_reported ... ok`
    if t.starts_with("test ") && (t.ends_with(" ... ok") || t.ends_with(" ... ignored")) {
        return true;
    }
    // pytest: `tests/x.py::test_error_handling PASSED   [ 20%]`. A pytest line
    // carries exactly one verdict, so a FAILED/ERROR anywhere on it wins.
    if t.contains(" PASSED") && !t.contains("FAILED") && !t.contains("ERROR") {
        return true;
    }
    // jest / vitest: `✓ handles a failed request (3 ms)`
    t.starts_with('\u{2713}') || t.starts_with('\u{221a}')
}

/// A test-runner line whose OWN verdict says the test failed.
///
/// The verdict is the whole message on these lines, and it does not have to
/// match an ordinary keyword to be one: `fail` alone is too broad to sit in
/// [`ERROR_KEYWORDS`] (`failover`, `failsafe`, `fail-open` are ordinary log
/// words), so `FAIL [   0.004s] (4/5) ntfix tests::this_one_really_fails` —
/// cargo-nextest's way of saying a test failed — matched NOTHING and was
/// eligible to be elided from the middle of a long run while passing tests
/// were rescued in its place.
pub fn failing_test_verdict(line: &str) -> bool {
    let t = line.trim();
    // cargo-nextest: `FAIL [ …]`, and `TRY 2 FAIL [ …]` after a retry.
    if let Some(rest) = t.strip_prefix("FAIL") {
        return rest.trim_start().starts_with('[');
    }
    if t.starts_with("TRY ") {
        if let Some((_, rest)) = t.split_once("FAIL") {
            return rest.trim_start().starts_with('[');
        }
    }
    // jest / vitest.
    t.starts_with('\u{2715}') || t.starts_with('\u{2717}')
}

/// True when `line` (case-insensitive) contains an error/warning keyword.
/// Lines matching this are never dropped by [`smart_fold_indices`].
///
/// A line carrying its own verdict is judged on THAT and not on the words
/// around it: see [`failing_test_verdict`] and [`passing_test_verdict`].
pub fn is_error_line(line: &str) -> bool {
    if failing_test_verdict(line) {
        return true;
    }
    if passing_test_verdict(line) {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    ERROR_KEYWORDS.iter().any(|kw| {
        lower
            .match_indices(kw)
            .any(|(at, _)| !inside_a_name(line, at, kw.len()))
    })
}

/// A keyword written in lower case INSIDE a name is that name, not a verdict:
/// `thiserror`, `quick-error`, `error-ex@1.3.4`, `connect_timeout`. Measured
/// 2026-10-04: `cargo tree` cut in the middle rescued nine `thiserror` /
/// `quick-error` lines as its "errors". A class name (`TypeError`,
/// `NullPointerException`) starts its keyword with a capital and an error
/// code (`ERROR_FILE_NOT_FOUND`) is upper case: both stay verdicts.
fn inside_a_name(line: &str, at: usize, len: usize) -> bool {
    let b = line.as_bytes();
    if !b[at].is_ascii_lowercase() {
        return false;
    }
    let before = at.checked_sub(1).map(|i| b[i]);
    let after = b.get(at + len).copied();
    matches!(before, Some(c) if c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        || matches!(after, Some(b'-' | b'_' | b'@'))
}

// ---------------------------------------------------------------------------
// Middle truncation lived in `filter.rs`, the command dispatcher,
// and it already called `compress::coverage_knee` from here: the two
// modules held hands in BOTH directions. It is context
// compression — "structure, dedup, smart truncation" —
// that is, this module's job, and its adaptive threshold is computed by
// its neighbour three lines up.
// ---------------------------------------------------------------------------

/// Floor for the adaptive HEAD count: never show fewer than this many head
/// lines even when bigram coverage saturates almost immediately.
const MIN_ADAPTIVE_HEAD: usize = 8;

/// Cap on how many error/warning lines [`truncate_middle`] rescues from the
/// elided middle.  Bounds output growth on pathological all-error input while
/// still surfacing the failures a plain head/tail window would drop.
const ERROR_MIDDLE_BUDGET: usize = 60;

/// Keep the first `head` and last `tail` lines, eliding the middle — but two
/// headroom-derived refinements over a blind window:
///
/// * **Adaptive head** — [`crate::compress::coverage_knee`] caps the head at
///   the point where bigram coverage saturates, so repetitive output (graphs,
///   disassembly, progress spam) shows fewer redundant head lines while diverse
///   output keeps the full `head`.  Floored at [`MIN_ADAPTIVE_HEAD`].
/// * **Error rescue** — up to [`ERROR_MIDDLE_BUDGET`] error/warning lines from
///   the elided middle are preserved (earliest first), so a failure buried
///   between the head and tail is never silently dropped.
/// [`truncate_middle`] for a LISTING whose every line is an answer (`git
/// reflog`, `git tag`, `git config --list`, `cargo tree`): given whole while
/// it fits the delivery threshold, cut only past it. Opted into per command,
/// on its own measurement — a build log keeps the plain cut. Measured
/// 2026-10-03: `git reflog -50` (6 KB) lost entries 40–44, the old ones a
/// reflog is read for; `cargo tree` (4 KB) lost 19 dependencies.
pub fn whole_or_truncate_middle(s: &str, head: usize, tail: usize) -> String {
    if tk_base::delivery::fits(s) {
        return s.to_string();
    }
    // No failure rescue: in a listing the word `error` is CONTENT — a real
    // `git reflog -80` cut in three places to keep commit subjects that said
    // `erreur` (2026-10-04). A failing run of these commands is a short
    // message, and a short message fits.
    cut_middle(s, head, tail, &[], false)
}

pub fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    truncate_middle_keeping(s, head, tail, &[])
}

/// [`truncate_middle`], also keeping the lines at `extra` (0-based indices)
/// wherever they fall in the elided middle — a caller that knows which lines
/// carry the STRUCTURE of the middle (a source file's declaration headers)
/// passes them, and the middle reads as an outline instead of one marker.
pub fn truncate_middle_keeping(s: &str, head: usize, tail: usize, extra: &[usize]) -> String {
    cut_middle(s, head, tail, extra, true)
}

fn cut_middle(s: &str, head: usize, tail: usize, extra: &[usize], rescue: bool) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let n = lines.len();
    if n <= head + tail + 1 {
        return s.to_string();
    }
    // Adaptive head: don't show more head lines than carry information.
    let head_eff = crate::compress::coverage_knee(&lines[..head.min(n)], head)
        .max(MIN_ADAPTIVE_HEAD.min(head))
        .min(head);
    let tail = tail.min(n.saturating_sub(head_eff));
    let mut kept: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for i in 0..head_eff {
        kept.insert(i);
    }
    for i in (n - tail)..n {
        kept.insert(i);
    }
    // Rescue error/warning lines from the elided middle, bounded.
    let mut rescued = 0usize;
    for (i, line) in lines
        .iter()
        .enumerate()
        .take(n - tail)
        .skip(head_eff)
        .filter(|_| rescue)
    {
        if rescued >= ERROR_MIDDLE_BUDGET {
            break;
        }
        if crate::compress::is_error_line(line) {
            kept.insert(i);
            rescued += 1;
            // …with the banner that says what it was about. Ansible prints
            // `TASK [role : migrate database]` and then the `fatal:` line;
            // rescuing only the second tells the agent that something failed
            // and not what was being attempted. Same rule as
            // `smart_fold_indices`, because the two folds serve the same
            // outputs and disagreeing about this would be arbitrary.
            let mut j = i;
            while j > 0 && lines[j - 1].trim().is_empty() {
                j -= 1;
            }
            if j > 0 && crate::compress::line_is_a_context_banner(lines[j - 1]) {
                kept.insert(j - 1);
            }
        }
    }
    kept.extend(extra.iter().copied().filter(|&i| i < n));
    let indices: Vec<usize> = kept.into_iter().collect();
    crate::elision::render_kept(&lines, &indices)
}

/// A line that only makes sense with the one above it — a banner naming what
/// the next line reports on.
///
/// Rescuing an error from the middle without it answers half the question:
/// ansible prints `TASK [role : migrate database]` and THEN
/// `fatal: [host-3]: FAILED! => …`, so keeping only the failure tells the agent
/// that something failed and not what was being attempted. Same shape as the
/// `go test` case `fold_table_priority` already handles, where the reason lives
/// on the line under `--- FAIL`.
pub(crate) fn line_is_a_context_banner(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("TASK [")
        || t.starts_with("RUNNING HANDLER [")
        || t.starts_with("PLAY [")
        || t.starts_with("---- ")
        || t.starts_with("____ ")
        || (t.starts_with("# ") && t.contains(" will be "))
}

/// Return the indices (into `lines`) to keep when folding a list to at most
/// `max` items.  This is a *strict* budget: the result is always ≤ `max`.
/// Priority order when filling the budget:
///
/// 1. Always keep the first `keep_first` lines (schema / context).
/// 2. Always keep the last `keep_last` lines (recency).
/// 3. Keep middle lines that pass [`is_error_line`], earliest first
///    (the first failure is usually the root cause), until the budget fills.
/// 4. Stride-sample the remaining middle to fill any leftover budget.
///
/// Returns a sorted, de-duplicated vec with length ≤ `max`.
pub fn smart_fold_indices(
    lines: &[&str],
    max: usize,
    keep_first: usize,
    keep_last: usize,
) -> Vec<usize> {
    fold_indices_rescuing(lines, max, keep_first, keep_last, &is_error_line)
}

/// [`smart_fold_indices`], naming what deserves to be RESCUED from the middle.
///
/// The selection had a notion of a precious line — an error line — and
/// it was HARD-WIRED. A page has no errors; it has HEADINGS, and a
/// fixed-step sampling carries them away like the rest. The predicate is the
/// only point of variation between the two uses, and passing it in avoids
/// copying sixty lines of selection to change one line.
///
/// The historical caller passes `is_error_line` and does not change behaviour.
pub fn fold_indices_rescuing(
    lines: &[&str],
    max: usize,
    keep_first: usize,
    keep_last: usize,
    worth_rescuing: &dyn Fn(&str) -> bool,
) -> Vec<usize> {
    let n = lines.len();
    if n <= max {
        return (0..n).collect();
    }
    let max = max.max(1);
    let kf = keep_first.min(max).min(n);
    let kl = keep_last
        .min(max.saturating_sub(kf))
        .min(n.saturating_sub(kf));
    let mut kept = std::collections::BTreeSet::new();
    for i in 0..kf {
        kept.insert(i);
    }
    for i in (n - kl)..n {
        kept.insert(i);
    }
    let mid_start = kf;
    let mid_end = n - kl;
    // Error-first: rescue middle error lines until the budget is exhausted.
    if kept.len() < max {
        for (i, line) in lines.iter().enumerate().take(mid_end).skip(mid_start) {
            if kept.len() >= max {
                break;
            }
            if worth_rescuing(line) {
                kept.insert(i);
                // Take the banner that says what this was, if one is sitting
                // just above. Walking back over blanks costs at most two
                // lookups and turns "something failed" into "migrating the
                // database failed".
                let mut j = i;
                while j > mid_start && lines[j - 1].trim().is_empty() {
                    j -= 1;
                }
                if j > mid_start && line_is_a_context_banner(lines[j - 1]) && kept.len() < max {
                    kept.insert(j - 1);
                }
            }
        }
    }
    // Stride-sample the middle to fill remaining budget.
    if kept.len() < max && mid_end > mid_start {
        let mid_len = mid_end - mid_start;
        let budget = max - kept.len();
        let step = ((mid_len as f64 / budget as f64).ceil() as usize).max(1);
        let mut i = mid_start;
        while i < mid_end && kept.len() < max {
            kept.insert(i);
            i += step;
        }
    }
    kept.into_iter().collect()
}

/// Minimum normalized "bulge above the diagonal" for a coverage curve to be
/// considered concave enough to have a real knee.  Below this the curve is
/// ~linear (every line adds fresh information) and we keep the full `max`.
const KNEE_MIN_BULGE: f64 = 0.10;

/// Compute an adaptive item count using bigram-coverage change-point
/// detection (headroom Kneedle pattern).
///
/// Plots cumulative unique-bigram coverage as items are added one-by-one,
/// normalizes both axes to `[0, 1]`, and returns the index of maximum
/// distance above the diagonal (the elbow of a concave-increasing curve) — the
/// point past which extra lines add little new information.
///
/// When the curve is ~linear (no saturation, bulge < [`KNEE_MIN_BULGE`]) there
/// is no real knee and the full `max` is returned, so diverse output is never
/// over-truncated.  The result is always clamped to `[1, max]`.
pub fn coverage_knee(lines: &[&str], max: usize) -> usize {
    let n = lines.len();
    if n <= 4 {
        return n.min(max);
    }
    let mut seen = std::collections::HashSet::new();
    let mut coverage = Vec::with_capacity(n);
    for &line in lines {
        let words: Vec<u32> = line.split_ascii_whitespace().map(fnv1a_word).collect();
        for pair in words.windows(2) {
            seen.insert((pair[0], pair[1]));
        }
        coverage.push(seen.len() as f64);
    }
    let total = coverage.last().copied().unwrap_or(0.0);
    if total <= 0.0 {
        // No bigrams at all (single-word lines): nothing to learn, keep all.
        return n.min(max);
    }
    // Kneedle: argmax over i of (y_i - x_i) with both axes normalized to [0,1].
    let mut best_i = n - 1;
    let mut best_bulge = f64::NEG_INFINITY;
    for (i, &cov) in coverage.iter().enumerate() {
        let x = i as f64 / (n as f64 - 1.0);
        let y = cov / total;
        let bulge = y - x;
        if bulge > best_bulge {
            best_bulge = bulge;
            best_i = i;
        }
    }
    if best_bulge < KNEE_MIN_BULGE {
        // ~linear coverage → genuinely diverse, do not over-truncate.
        return n.min(max);
    }
    (best_i + 1).clamp(1, max)
}

/// FNV-1a 32-bit hash for a single word (no external dep).
fn fnv1a_word(word: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in word.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

use crate::ansi::strip_ansi;
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// `dedup_lines` — fold runs of identical lines into `(xN)`. Text
// compression, not command dispatching: it lived in
// `filter.rs` and 29 sites climbed up to it from the formats and the commands.
// ---------------------------------------------------------------------------

/// Collapse runs of identical lines into `<line>  (xN)`.
pub fn dedup_lines(s: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<&str> = None;
    let mut count = 0usize;
    let flush = |out: &mut String, line: &str, count: usize| {
        if count == 0 {
            return;
        }
        out.push_str(line);
        if count > 1 {
            let _ = write!(out, "  (x{count})");
        }
        out.push('\n');
    };
    for line in s.lines() {
        match prev {
            Some(p) if p == line => count += 1,
            Some(p) => {
                flush(&mut out, p, count);
                prev = Some(line);
                count = 1;
            }
            None => {
                prev = Some(line);
                count = 1;
            }
        }
    }
    if let Some(p) = prev {
        flush(&mut out, p, count);
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// The GENERIC fold and its truncation: `generic` = dedup + ANSI stripped +
// `truncate_smart`. Nineteen command modules call it when their own
// compactor does not apply, and they climbed up into `filter`,
// the dispatcher, for it. That is not dispatching: it is the compactor of last
// resort, and this module is the compression one.
// ---------------------------------------------------------------------------

/// Generic fallback: strip ANSI, dedup, and smart-truncate huge outputs.
pub fn generic(output: &str) -> String {
    let cleaned = dedup_lines(&strip_ansi(output));
    // Whole when it fits, like every listing fold: a real `pip freeze` (371
    // packages, 6.9 KB) lost 52 of them to the 180-line sampling, two by two
    // wherever a name resembled its neighbour (2026-10-04).
    if tk_base::delivery::fits(&cleaned) {
        return cleaned;
    }
    truncate_smart(&cleaned, 180)
}

/// Error-aware truncation: uses [`crate::compress::smart_fold_indices`] to
/// preserve error/warning lines and stride-sample the middle rather than
/// blindly dropping it.  Total kept lines ≤ `max`.
pub fn truncate_smart(s: &str, max: usize) -> String {
    truncate_rescuing(s, max, &crate::compress::is_error_line)
}

/// [`truncate_smart`], naming what deserves to be rescued from the middle.
///
/// The middle of a COMMAND OUTPUT is judged by its errors; the middle of a
/// PAGE is judged by its headings. Same window, same elision dialect, same
/// footer — only the predicate changes, and it is the web route that supplies
/// another one. `generic`, hence the whole command path, still goes
/// through `truncate_smart` and sees no difference.
pub fn truncate_rescuing(s: &str, max: usize, worth_rescuing: &dyn Fn(&str) -> bool) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max {
        return s.to_string();
    }
    let keep_first = (max / 4).max(5).min(lines.len());
    let keep_last = (max / 8).max(3).min(lines.len().saturating_sub(keep_first));
    let indices =
        crate::compress::fold_indices_rescuing(&lines, max, keep_first, keep_last, worth_rescuing);
    crate::elision::render_kept(&lines, &indices)
}
#[cfg(test)]
include!("compress_tests.rs");
