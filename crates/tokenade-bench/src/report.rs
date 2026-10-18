//! **Making the figures readable, without damaging them.**
//!
//! A generated document is written from a single place, and re-read: a line
//! break swallowed in a markdown table makes a measurement row disappear
//! without anything saying so. Number formatting is here for the same
//! reason — it is the last thing that touches a reading before it is believed.

use super::*;

/// Write a GENERATED document, after re-reading it.
///
/// It was recorded that a Python heredoc eats the `\` continuation of a
/// Rust literal; it happened again, and the value ranking came out
/// with fourteen spaces in the middle of every sentence. Fixing the instance
/// closes nothing: three instruments write markdown and the fourth
/// will too.
///
/// The guard bears on what is OBSERVABLE in the finished product rather than
/// on how it was written: outside a code block, a markdown never wants
/// three spaces in the middle of a line. Measured at the time of writing,
/// **0 violations on the repo's three generated documents** — so the rule is
/// exact and not merely plausible.
pub fn write_generated_doc(path: &std::path::Path, md: &str) -> std::io::Result<()> {
    if let Some(bad) = swallowed_continuation(md) {
        return Err(std::io::Error::other(format!(
            "{}: a line carries a swallowed continuation blank, the document is not written.\n  {bad}",
            path.display()
        )));
    }
    std::fs::write(path, md)
}

/// The first line outside a code block carrying 3 or more spaces anywhere
/// but at its start, with enough context to find it again.
pub fn swallowed_continuation(md: &str) -> Option<String> {
    let mut in_fence = false;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let body = line.trim_start();
        if let Some((at, end)) = find_run_of_three_spaces(body) {
            // Show BOTH edges: the blank alone cannot be seen, and the
            // text before it alone is not enough to find the line again.
            let from = floor_char_boundary(body, at.saturating_sub(30));
            let to = floor_char_boundary(body, (end + 30).min(body.len()));
            return Some(body[from..to].to_string());
        }
    }
    None
}

/// The index of the first "at least three spaces" followed by something.
/// A line that ENDS with spaces is not this fault, and two
/// spaces are a legitimate markdown line break.
pub(crate) fn find_run_of_three_spaces(body: &str) -> Option<(usize, usize)> {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b' ' {
            let start = i;
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
            if i - start >= 3 && i < b.len() {
                return Some((start, i));
            }
        } else {
            i += 1;
        }
    }
    None
}

/// The largest index <= `at` that opens a character. A slice taken in the
/// middle of a UTF-8 byte panics, and these documents may contain non-ASCII text.
pub(crate) fn floor_char_boundary(s: &str, at: usize) -> usize {
    let mut i = at.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// **What a measurement covered, and through which path.** An instrument header
/// is built here, never by hand.
///
/// The sentence was put in [`Stages::banner`] because a module
/// comment had announced two stages for eight iterations. The same
/// fault was then found one level below: `json_headroom --delivered` was titled
/// "DELIVERY PATH (two stages, real CLI)" above a total computed
/// on ONE stage and over the 16,391 files, while its own
/// verification block, three lines higher, said **10,791,546 at stage 1 versus
/// 11,287 actually delivered** — a factor of 956. The flag added to fix
/// an 813× overestimate recreated a 956× one, in the same
/// instrument, because the sentence was written at the call site.
///
/// Here the sentence and the numbers come out together: one cannot describe a
/// total one did not measure.
pub struct Measured {
    /// The path ACTUALLY taken for the numbers below.
    pub stages: Stages,
    /// What was measured (files, captures…), and its plural name.
    pub items: usize,
    pub unit: &'static str,
    pub before: usize,
    pub after: usize,
    /// Which way the measurement went, in the instrument's words.
    ///
    /// The stage-count sentence no longer names it: it used to, and
    /// `json_headroom` was then titled "(`filter <prog>`)" while it
    /// goes through `auto::compact_with_path_hint`. A header that names a
    /// path the measurement did not take is the very fault it was fixing,
    /// one level below.
    pub via: &'static str,
}

impl Measured {
    pub fn header(&self) -> String {
        let pct = 100 * self.before.saturating_sub(self.after) / self.before.max(1);
        format!(
            "{} ({})\n{} {} · {} → {} bytes (−{} %)\n",
            self.stages.banner(),
            self.via,
            thousands(self.items as u64),
            self.unit,
            thousands(self.before as u64),
            thousands(self.after as u64),
            pct
        )
    }
}

/// Groups digits by thousands, with the thin space this repo uses
/// everywhere. Written here because three instruments each carried a copy.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}
