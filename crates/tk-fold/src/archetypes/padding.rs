//! **Is the spacing DECORATIVE, or load-bearing?**
//!
//! Removing it where it aligns columns makes a table unreadable; keeping it
//! where it merely fills costs bytes on every line. The three
//! questions here — is the padding decorative, is the indentation
//! a nesting ladder, is an interior cell empty — are what
//! separates the two, and each carries its measurement.

/// Is this padding DECORATION?
///
/// One function for the question, two ways to answer yes, because
/// two very different shapes carry the same waste and neither
/// of them is read through a language.
///
/// 1. **A column-aligned `label: value` block.** Eight lines or more
///    whose value is pushed by at least two spaces — on one side OR THE
///    OTHER of the colon: French typography puts it on the left, and
///    `pacman -Qi` writes `Nom                      : bash` over twenty-one
///    lines, not one of which was counted when only the right side was checked.
///
/// 2. **A table in COLUMNS, without a single colon.** `Get-PSDrive`,
///    `netstat -nr`, `whoami /all` write a header, sometimes a `----`
///    rule, then lines padded to the column width. There
///    is nothing to recognise by text: what gives it away is the SHARE
///    of interior spaces. MEASURED by `compactor_review` on the outputs the
///    product still delivered almost whole: `Get-PSVersion` **79 %**
///    padding, `Get-PSDrive` 76 %, `Get-ChildItem` 71 %, `netstat -nr`
///    57 %, `whoami /all` 39 % — versus **0 %** for `mount`, `pgrep -a`,
///    `find` and `netstat -s`, which really have nothing to give.
///
/// The threshold is 20 %, well above what indented prose reaches and
/// well below the six targeted outputs. LEADING indentation does not
/// count: it is structure, it survives the fold, and counting it
/// would make source code pass for a table.
/// The ABSOLUTE padding floor: below it, the share alone decides.
///
/// A thousand bytes of decoration are worth removing whatever the size of the
/// document; below that, the fold is not worth the risk of taking for a
/// table something that is not one.
pub(super) const PADDING_FLOOR: usize = 1_000;

/// `text` with the `< ` / `> ` of a normal diff taken off the lines that carry
/// it, or `None` unless most non-empty lines do. Change commands and `---`
/// are the other lines of that format and stay as they are.
fn without_side_markers(text: &str) -> Option<String> {
    let (mut marked, mut other) = (0usize, 0usize);
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match line.strip_prefix("< ").or_else(|| line.strip_prefix("> ")) {
            Some(rest) => {
                marked += 1;
                out.push_str(rest);
            }
            None => {
                if !line.trim().is_empty() {
                    other += 1;
                }
                out.push_str(line);
            }
        }
        out.push('\n');
    }
    (marked > other).then_some(out)
}

pub fn padding_is_decorative(text: &str) -> bool {
    // **A line-number gutter is not a column.** Behind `   300\t` the code's
    // own indentation is no longer at the head of the line, so it counted as
    // interior padding — the confusion the paragraph above rules out for
    // un-numbered code — and the nesting-ladder guard read the gutter's
    // constant width instead of the code's. Measured 2026-10-02: `nl -ba
    // dispatch.rs | sed -n '300,500p'` came back with 0 of 201 lines still
    // indented, on 1.1.21 and HEAD alike. Judge what the gutter numbers.
    //
    // A NORMAL diff (`diff a b`) prefixes every changed line with `< ` or
    // `> `: the same confusion, measured the same day — a 185-line diff of a
    // test file came back with every changed line's indentation flattened.
    if let Some(body) =
        crate::line_gutter::without_gutter(text).or_else(|| without_side_markers(text))
    {
        return padding_is_decorative(&body);
    }
    let mut labelled_rows = 0usize;
    #[allow(unused_assignments)]
    let mut interior = 0usize;
    let mut total = 0usize;
    let mut column_lines = 0usize;

    for line in text.lines() {
        total += line.len() + 1;

        // (1) the `label: value` shape.
        if let Some((label, value)) = line.split_once(':') {
            if !label.contains(['"', '{', '}', ',', '[']) && !label.trim().is_empty() {
                let pushed_right = value.len() - value.trim_start().len() >= 2;
                let pushed_left = label.len() - label.trim_end().len() >= 2;
                if pushed_right || pushed_left {
                    labelled_rows += 1;
                }
            }
        }

        // (2) does the line carry COLUMNS? A run of at least two
        // spaces between two cells, i.e. not at the start of the line.
        let trimmed = line.trim_end();
        let mut run = 0usize;
        let mut started = false;
        let mut runs_here = 0usize;
        for c in trimmed.chars() {
            if c == ' ' || c == '\t' {
                run += 1;
            } else {
                if started && run > 1 {
                    runs_here += 1;
                }
                run = 0;
                started = true;
            }
        }
        if runs_here >= 1 {
            column_lines += 1;
        }
    }
    // **How much the fold would remove**, asked of the fold itself.
    //
    // This quantity was RECOUNTED here, and the two counts did not say
    // the same thing: the measurement ignored LEADING indentation, which the
    // rewrite nevertheless reduces to one space. A decision that estimates what
    // another function will do ends up estimating something else; it is the
    // "single declarer" rule applied to a number.
    interior = text
        .len()
        .saturating_sub(collapse_alignment_padding(text).len());

    if labelled_rows >= 8 {
        return true;
    }
    // Five lines at least, otherwise a pair of heavily padded lines would be enough
    // to make anything pass for a table.
    // **A SHARE without an absolute floor refuses a real gain on a big
    // document.** The one-fifth threshold exists so that a pair of heavily
    // padded lines does not make anything pass for a table — but
    // `column_lines >= 5` and the absence of an interior hole already say that.
    //
    // MEASURED 2026-09-09 on `ps aux`: fifty lines in columns, no
    // hole, **1,218 bytes of padding out of 7,868 — 15.5 %**, hence refused
    // by a point and a half, and nobody else takes them.
    // The mirror image of an earlier bug: there a rate without a SIZE floor
    // ranked the tiniest first, here a share without a BYTES floor refuses
    // the biggest.
    column_lines >= 5
        && total > 0
        && (interior * 5 >= total || interior >= PADDING_FLOOR)
        && no_interior_cell_is_empty(text)
        && !indentation_is_a_nesting_ladder(text)
}

/// **Multi-LEVEL indentation is structure.**
///
/// `collapse_alignment_padding` reduced every leading indent to ONE space:
/// the belonging relation survived, the DEPTH did not (since
/// 2026-10-04 it keeps one space per level — `lsusb -v` slipped under this guard). On a table in columns
/// that is harmless — there is only one level. On a nested document,
/// it is the same mistake as the empty cell in the middle: what carried
/// the information was the POSITION.
///
/// MEASURED 2026-09-09: as soon as the absolute floor let big
/// documents in, `defaults read com.apple.dock` — a plist with three levels of
/// indent — started going out flat, 1,161 → 770 bytes. No rare atom
/// had disappeared, so the richness ratchet did not see it; it was the
/// ROUTING floor that named it, and it was right to do so.
///
/// Three distinct widths at least: two levels is a heading and its
/// lines (`Correctif(s):` then `[01]: KB…`), and that one must keep
/// folding.
///
/// **And the indent must WEIGH more than the interior decoration.** Counting
/// widths is not enough: a RIGHT-aligned first column produces as many
/// (`    1`, `   10`, `  100`), and that is precisely the table the
/// fold was taught again to handle. MEASURED on both documents — the plist gives
/// **343 bytes of indent versus 48 inside**, the `ps -eo` table **7
/// versus 91**. The ratio separates them where the width count
/// confuses them.
pub(super) fn indentation_is_a_nesting_ladder(text: &str) -> bool {
    let mut widths = std::collections::BTreeSet::new();
    let (mut leading, mut inner) = (0usize, 0usize);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let w = line.chars().count() - trimmed.chars().count();
        widths.insert(w);
        // What the fold would remove on each side: the indent keeps one
        // space, and so do the interior runs.
        leading += w.saturating_sub(1);
        let body = trimmed.trim_end();
        inner += body.chars().count()
            - body
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .count();
    }
    widths.len() >= 3 && leading > inner
}

/// In a table in columns, an empty cell IN THE MIDDLE carries meaning, and
/// reducing its padding to one space destroys it with nothing
/// signalling it: `Get-PSDrive` writes `Alias` under `Name`, leaves `Used` and `Free`
/// empty, and `Alias Alias` no longer says which column the second belongs to.
/// Richness does not catch it — it counts rare atoms, and no atom
/// disappeared; it was the POSITION that carried the information.
///
/// An empty column at the END of a line does not pose that problem: fields are
/// read from the left and some are simply missing on the right. That is the
/// common case (`netstat -nr` leaves `Expire` empty almost everywhere), so
/// refusing it would cost the fold without protecting anything.
///
/// The shape is read on the HEADER: columns start where the first
/// non-empty line resumes after a run of at least two spaces. A line
/// is safe if, as soon as one of its columns is empty, all the following ones
/// are too. Counted in CHARACTERS, not bytes — a table may carry
/// accents, and a byte position would fall in the middle of an `é`.
pub(super) fn no_interior_cell_is_empty(text: &str) -> bool {
    // The header is not necessarily the first line: `netstat -nr` opens
    // with `Routing tables`, a blank line and `Internet:` before writing its
    // columns. Taking the first non-empty line refused the whole table
    // on a three-line preamble — a rule answering "it might
    // lie" where it simply did not look at the right line.
    //
    // The header is the first line carrying at least two runs of at
    // least two spaces: that is what distinguishes columns from a title.
    // The FIRST column starts where the header starts, not at zero.
    //
    // An indented header — `  PID  PPID USER  COMM`, the shape `ps`, `ioreg`
    // and `dir` write when their first column is RIGHT-aligned —
    // fabricated a phantom column at position 0, empty on every
    // data row while the next one is full. It read as
    // an empty cell IN THE MIDDLE, and the fold was refused on the whole table.
    let column_starts = |line: &str| {
        let first = line.chars().position(|c| !c.is_whitespace()).unwrap_or(0);
        let mut starts: Vec<usize> = vec![first];
        let mut run = 0usize;
        for (i, c) in line.chars().enumerate() {
            if c == ' ' || c == '\t' {
                run += 1;
            } else {
                if run >= 2 && i > first {
                    starts.push(i);
                }
                run = 0;
            }
        }
        starts
    };

    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).peekable();
    let mut starts: Vec<usize> = Vec::new();
    for line in lines.by_ref() {
        let s = column_starts(line);
        // TWO columns are enough to make a table (`Get-PSVersion` writes
        // `Name  Value`), and a preamble line has only one — the same
        // threshold therefore serves to pick the header and to skip the title.
        if s.len() >= 2 {
            starts = s;
            break;
        }
    }
    if starts.len() < 2 {
        return false;
    }

    for line in lines {
        let cells: Vec<char> = line.chars().collect();
        let filled: Vec<bool> = starts
            .iter()
            .enumerate()
            .map(|(n, &from)| {
                let to = starts.get(n + 1).copied().unwrap_or(cells.len());
                cells
                    .get(from..to.min(cells.len()))
                    .map(|s| s.iter().any(|c| !c.is_whitespace()))
                    .unwrap_or(false)
            })
            .collect();
        // From the first hole on, everything that follows must be empty too.
        if let Some(hole) = filled.iter().position(|f| !f) {
            if filled[hole..].iter().any(|f| *f) {
                return false;
            }
        }
    }
    true
}

/// The cells of a header whose own spacing does not say where they end.
///
/// `df` in French prints `Sys. de fichiers Taille Utilisé Dispo Uti% Monté
/// sur` — ONE space everywhere, nine words for six columns; only the data
/// rows below are aligned (2026-10-04). Each header word goes to the data
/// column it overlaps most (the nearest when none), so `Sys. de fichiers`
/// and `Monté sur` come back as cells. `None` unless the rows agree on a
/// column count, the header has more words than that, and every column
/// receives a word.
fn header_by_data_columns(text: &str) -> Option<(&str, Vec<String>)> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let (header, rows) = lines.split_first()?;
    let spans = |l: &str| -> Vec<(usize, usize)> {
        let chars: Vec<char> = l.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            out.push((start, i));
        }
        out
    };
    let mut counts = std::collections::BTreeMap::<usize, usize>::new();
    for r in rows {
        *counts.entry(spans(r).len()).or_default() += 1;
    }
    let (&k, &n) = counts.iter().max_by_key(|(_, n)| **n)?;
    if k < 3 || rows.len() < 2 || n * 5 < rows.len() * 4 {
        return None;
    }
    let mut cols = vec![(usize::MAX, 0usize); k];
    for r in rows {
        let sp = spans(r);
        if sp.len() == k {
            for (c, (a, b)) in cols.iter_mut().zip(sp) {
                *c = (c.0.min(a), c.1.max(b));
            }
        }
    }
    // Real columns: each one ends before the next begins, on every row.
    if cols.windows(2).any(|w| w[0].1 >= w[1].0) {
        return None;
    }
    let words = spans(header);
    if words.len() <= k {
        return None;
    }
    let hchars: Vec<char> = header.chars().collect();
    let mut cells: Vec<Vec<String>> = vec![Vec::new(); k];
    let mut last = 0usize;
    for (a, b) in words {
        let overlap = |c: &(usize, usize)| b.min(c.1).saturating_sub(a.max(c.0));
        let gap = |c: &(usize, usize)| {
            if b <= c.0 {
                c.0 - b
            } else {
                a.saturating_sub(c.1)
            }
        };
        let j = (0..k)
            .max_by_key(|&j| (overlap(&cols[j]), std::cmp::Reverse(gap(&cols[j]))))
            .unwrap_or(0)
            .max(last);
        last = j;
        cells[j].push(hchars[a..b].iter().collect());
    }
    if cells.iter().any(Vec::is_empty) {
        return None;
    }
    Some((header, cells.into_iter().map(|w| w.join(" ")).collect()))
}

/// Reduce every interior run of spaces to a single one, line by line.
///
/// First written in the `free` compactor, for `free` alone — "collapse
/// runs of spaces to one, which an agent never needs". Nothing in the rule is
/// specific to `free`: it holds for every aligned output, in every
/// language, and that is exactly what was missing when a named compactor
/// refused its program's output in a locale it did not read.
pub fn collapse_alignment_padding(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Each distinct indentation width is one LEVEL, kept as one space per
    // level: `lsusb -v` nests Device > Configuration > Interface > Endpoint
    // only by its 2/4/6/8-space indents, and a single space for all of them
    // flattened a 3 553-line tree (2026-10-04). One level still reads as one
    // space.
    let indent = |l: &str| {
        l.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 8 } else { 1 })
            .sum::<usize>()
    };
    let mut widths: Vec<usize> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(indent)
        .collect();
    widths.sort_unstable();
    widths.dedup();
    let ladder = std::iter::once(&0)
        .chain(&widths)
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[1] - w[0] <= 4);
    // A TABLE whose column NAMES hold spaces (`IMAGE  ID  DISK USAGE  CONTENT
    // SIZE` of `docker images`) keeps two spaces between its columns: one
    // would weld `DISK USAGE` to its neighbours, and the header no longer
    // says how many columns there are (2026-10-04). Single-word headers stay
    // single-spaced — the constant-column fold reads those.
    let gaps = |l: &str| {
        let t = l.trim();
        let (mut wide, mut run) = (0usize, 0usize);
        for c in t.chars() {
            if c == ' ' || c == '\t' {
                run += 1;
            } else {
                if run >= 2 {
                    wide += 1;
                }
                run = 0;
            }
        }
        wide
    };
    let body: Vec<usize> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(gaps)
        .collect();
    let tabular = body.iter().filter(|w| **w >= 2).count() * 2 >= body.len().max(1);
    // Multi-word names: the header has MORE words than any row has cells.
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let header_words = body.iter().position(|w| *w >= 2).is_some_and(|h| {
        let words = lines[h].split_whitespace().count();
        let widest = lines[h + 1..]
            .iter()
            .map(|l| l.split_whitespace().count())
            .max()
            .unwrap_or(usize::MAX);
        words > widest
    });
    let recelled = header_by_data_columns(text);
    let column_gap = if (tabular && header_words) || recelled.is_some() {
        "  "
    } else {
        " "
    };
    for line in text.trim_matches('\n').lines() {
        if let Some((h, cells)) = &recelled {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if line == *h || toks.len() == cells.len() {
                out.push_str(&if line == *h {
                    cells.join("  ")
                } else {
                    toks.join("  ")
                });
                out.push('\n');
                continue;
            }
        }
        let trimmed = line.trim_end();
        // An indent every line shares still reads as one level, as it did;
        // and only a LADDER (steps of at most 4 from the margin) is depth —
        // `systeminfo` indents its continuation lines to the VALUE column (27
        // and more), which is alignment, one space as before.
        let base = usize::from(widths.first().is_some_and(|w| *w > 0));
        let level = match widths.binary_search(&indent(trimmed)) {
            _ if trimmed.is_empty() => 0,
            Ok(0) | Err(_) if base == 0 => 0,
            Ok(r) if ladder => r + base,
            Ok(_) | Err(_) => 1,
        };
        for _ in 0..level {
            out.push(' ');
        }
        let trimmed = trimmed.trim_start();
        // A tree DRAWING (`│   └─vlc`) is depth written with box characters:
        // its spaces are columns of the drawing, and squeezed, a grandchild
        // read as a sibling of its parent (`pactree -r glibc`, 2026-10-04).
        // The drawing prefix stays as drawn.
        let drawing: usize = trimmed
            .char_indices()
            .take_while(|(_, c)| *c == ' ' || ('\u{2500}'..='\u{257F}').contains(c))
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        let (drawn, trimmed) = trimmed.split_at(drawing);
        out.push_str(drawn);
        // A blank line SEPARATES records and comes out intact: it
        // has no character to push, so the loop below only writes
        // its `\n`. (The `free` compactor dropped them, harmlessly — its
        // output has none — but the rule now serves any aligned
        // output.) An explicit guard existed here and was dead
        // code: emptying it did not change a byte of the corpus.
        // LEADING indentation is structure, not decoration:
        // under `Correctif(s):`, the twenty `[01]: KB…` lines only say they
        // belong to the line above through their indent. We keep one
        // space PER LEVEL (see above) — belonging survives, the
        // columns fall.
        let mut spaces = 0usize;
        for c in trimmed.chars() {
            if c == ' ' || c == '\t' {
                spaces += 1;
            } else {
                if spaces > 1 {
                    out.push_str(column_gap);
                } else if spaces == 1 {
                    out.push(' ');
                }
                spaces = 0;
                out.push(c);
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}
