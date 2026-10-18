//! `npm` — shared compactor for npm/pnpm/yarn/bun run/start/build output.
//!
//! Strips ANSI escapes, collapses `\r`-redrawn progress lines to their final
//! state, and RLE-folds consecutive identical lines.  Declines (returns None)
//! when none of those reductions apply — keeping small/clean output banner-free.

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut escape = false;
    let mut csi = false;
    for c in s.chars() {
        if escape && c == '[' {
            csi = true;
            escape = false;
        } else if escape {
            escape = false;
        } else if csi {
            if ('@'..='~').contains(&c) { csi = false; }
        } else if c == '\x1b' {
            escape = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapse a single raw input line that may contain `\r`-based terminal
/// redraws into its final visible state.  Returns `(text, redraws_folded)`.
fn collapse_cr(raw: &str) -> (String, usize) {
    let stripped = strip_ansi(raw);
    let states: Vec<&str> = stripped.split('\r').collect();
    let final_state = states.iter().rev()
        .find(|s| !s.trim().is_empty())
        .map(|s| s.trim_end().to_string())
        .unwrap_or_default();
    let redraws = states.len().saturating_sub(1);
    (final_state, redraws)
}

/// Core compaction.  Returns `Some(compacted)` when savings exist, else `None`.
fn compact_inner(text: &str) -> Option<String> {
    let raw_lines: Vec<&str> = text.lines().collect();
    let total_before = raw_lines.len();

    let mut processed: Vec<String> = Vec::new();
    let mut progress_folded: usize = 0;

    for raw in &raw_lines {
        let (final_line, redraws) = collapse_cr(raw);
        progress_folded += redraws;
        if !final_line.is_empty() {
            processed.push(final_line);
        }
    }

    // RLE-fold consecutive identical lines.
    let mut rle: Vec<(String, usize)> = Vec::new();
    for line in &processed {
        if let Some(last) = rle.last_mut() {
            if &last.0 == line { last.1 += 1; continue; }
        }
        rle.push((line.clone(), 1));
    }

    let dup_folded: usize = rle.iter().map(|(_, c)| c.saturating_sub(1)).sum();

    let formatted: Vec<String> = rle.into_iter().map(|(line, count)| {
        if count > 1 { format!("{line}  (\u{d7}{count})") } else { line }
    }).collect();

    // Byte-equality is the floor, not the ceiling: a build log repeats a
    // SENTENCE (one warning per route, per file, per module), never a line.
    // Folding those keeps every value and every pairing — only the sentence
    // stops being said N times.
    let plain = formatted.join("\n");
    let (body, mut template_folded) =
        crate::template_fold::fold_templated_runs_counted(&plain);
    // Folding trades long repeated lines for short value lines: judge it on
    // size, not on line count, and keep the plain text when it does not pay.
    let body = if body.len() < plain.len() {
        body
    } else {
        template_folded = 0;
        plain
    };

    if dup_folded == 0 && progress_folded == 0 && template_folded == 0 {
        return None;
    }

    let mut extras: Vec<String> = Vec::new();
    if dup_folded > 0 {
        extras.push(format!("{dup_folded} duplicate line(s) folded"));
    }
    if progress_folded > 0 {
        extras.push(format!("{progress_folded} progress-redraw state(s) folded"));
    }
    if template_folded > 0 {
        extras.push(format!("{template_folded} repeated sentence(s) folded"));
    }

    let header = if extras.is_empty() {
        format!("[tokenade:npm-run] {total_before} line(s)")
    } else {
        format!("[tokenade:npm-run] {total_before} line(s)\n{}", extras.join(", "))
    };

    Some(format!("{header}\n{body}"))
}

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    compact_inner(text)
}

pub fn compact(text: &str) -> String {
    compact_inner(text).unwrap_or_else(|| {
        // No savings: strip ANSI and trim but emit no banner.
        text.lines()
            .map(|l| collapse_cr(l).0)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}
