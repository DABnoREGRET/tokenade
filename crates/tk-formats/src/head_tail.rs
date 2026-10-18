//! **The head AND the tail of a long multi-line text.**
//!
//! A cut that keeps only the start loses the line a reader came for when it
//! is the LAST one: the final epoch of a training log (a notebook kept epochs
//! 1–9 of 50), the frame that raised in a Python traceback, the summary line of
//! a log stored in a JSON string. Whole lines from each end, half the budget
//! each, with what was left out counted between them; a single long line falls
//! back to a cut by characters.

pub fn head_and_tail(s: &str, cap: usize) -> String {
    let s = s.trim_end();
    let n = s.chars().count();
    if n <= cap {
        return s.to_string();
    }
    let lines: Vec<&str> = s.lines().collect();
    let half = cap / 2;
    let (mut head, mut used) = (0usize, 0usize);
    while head < lines.len() && used + lines[head].chars().count() < half {
        used += lines[head].chars().count() + 1;
        head += 1;
    }
    let (mut tail, mut used_t) = (lines.len(), 0usize);
    while tail > head && used_t + lines[tail - 1].chars().count() < half {
        used_t += lines[tail - 1].chars().count() + 1;
        tail -= 1;
    }
    if head == 0 && tail == lines.len() {
        // One long line: characters, not lines.
        let head: String = s.chars().take(cap).collect();
        return format!("{head}…[+{} chars]", n - cap);
    }
    let gap = lines[head..tail]
        .iter()
        .map(|l| l.chars().count() + 1)
        .sum::<usize>();
    format!(
        "{}\n…[{} line(s), {gap} chars]…\n{}",
        lines[..head].join("\n"),
        tail - head,
        lines[tail..].join("\n")
    )
}
