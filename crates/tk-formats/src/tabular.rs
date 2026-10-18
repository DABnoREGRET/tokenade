//! Tabular-CLI compaction: `docker ps`, `docker images`, `ps aux`, `kubectl
//! get pods`, `lsof`, `netstat`, `df -h`, …
//!
//! These commands all output a fixed-column-position table:
//!
//! ```text
//! CONTAINER ID   IMAGE          COMMAND      CREATED       STATUS         PORTS   NAMES
//! abc12345…      nginx:latest   "/docker"    3 days ago    Up 10 min      80/tcp  web
//! …
//! ```
//!
//! For an agent the **signal** is usually `IMAGE`, `STATUS`, `NAMES`,
//! `COMMAND` — the rest (`CONTAINER ID` hash, `CREATED` relative time,
//! ephemeral port numbers) is per-row noise that scales linearly with the
//! number of rows.
//!
//! [`compact`] detects the header line by matching against a curated list of
//! known commands, picks a signal subset of columns by name, and emits a
//! compact version that preserves the relative ordering. Unknown headers
//! pass through unchanged (no false-positive damage).

/// A known command we have a column whitelist for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    DockerPs,
    DockerImages,
    KubectlGetPods,
    KubectlGetNodes,
    PsAux,
    PsEf,
    Lsof,
    NetstatAn,
    DfH,
}

impl Cmd {
    /// FULL column list in header order — required to compute correct slice
    /// boundaries for kept columns (a dropped column between two kept ones
    /// would otherwise leak its content into the previous kept slice).
    fn all_columns(self) -> &'static [&'static str] {
        match self {
            Cmd::DockerPs => &[
                "CONTAINER ID",
                "IMAGE",
                "COMMAND",
                "CREATED",
                "STATUS",
                "PORTS",
                "NAMES",
            ],
            Cmd::DockerImages => &["REPOSITORY", "TAG", "DIGEST", "IMAGE ID", "CREATED", "SIZE"],
            Cmd::KubectlGetPods => &["NAME", "READY", "STATUS", "RESTARTS", "AGE"],
            Cmd::KubectlGetNodes => &["NAME", "STATUS", "ROLES", "AGE", "VERSION"],
            Cmd::PsAux => &[
                "USER", "PID", "%CPU", "%MEM", "VSZ", "RSS", "TTY", "STAT", "START", "TIME",
                "COMMAND",
            ],
            Cmd::PsEf => &["UID", "PID", "PPID", "C", "STIME", "TTY", "TIME", "CMD"],
            Cmd::Lsof => &[
                "COMMAND", "PID", "USER", "FD", "TYPE", "DEVICE", "SIZE/OFF", "NODE", "NAME",
            ],
            Cmd::NetstatAn => &[
                "Proto",
                "Recv-Q",
                "Send-Q",
                "Local Address",
                "Foreign Address",
                "State",
            ],
            Cmd::DfH => &["Filesystem", "Size", "Used", "Avail", "Use%", "Mounted on"],
        }
    }

    /// Column names we keep, in the order they should appear in the output.
    /// Anything else in the input is dropped.
    fn signal_columns(self) -> &'static [&'static str] {
        match self {
            // `CONTAINER ID` is how `docker logs`, `docker exec` and every
            // daemon message refer to a container, and `--no-trunc` asks for
            // it whole (a real one, 2026-10-03, lost its 64-hex ids). Twelve
            // characters per row by default.
            Cmd::DockerPs => &[
                "CONTAINER ID",
                "IMAGE",
                "COMMAND",
                "STATUS",
                "PORTS",
                "NAMES",
            ],
            // `IMAGE ID` is the ONLY address of an untagged (`<none>`) image —
            // `docker rmi` / `docker run` need it; `DIGEST` (when `--digests`
            // asked for it) pins one. Measured 2026-10-03 on a real `docker
            // images --digests`: both ids and the column bounds were lost —
            // the digest only showed because it overflowed into `TAG`.
            Cmd::DockerImages => &["REPOSITORY", "TAG", "DIGEST", "IMAGE ID", "SIZE"],
            // `RESTARTS` is what tells a crash-looping pod from a healthy one.
            Cmd::KubectlGetPods => &["NAME", "READY", "STATUS", "RESTARTS"],
            Cmd::KubectlGetNodes => &["NAME", "STATUS", "ROLES", "VERSION"],
            // Numeric columns of `ps aux` are right-aligned and not safely
            // slice-extractable when data exceeds the header width. Keep
            // only COMMAND — it's the last column, always left-aligned at
            // its header start, and the most useful signal.
            Cmd::PsAux => &["COMMAND"],
            Cmd::PsEf => &["CMD"],
            Cmd::Lsof => &["COMMAND", "NAME"],
            Cmd::NetstatAn => &["Local Address", "Foreign Address", "State"],
            Cmd::DfH => &["Filesystem", "Size", "Used", "Avail", "Use%", "Mounted on"],
        }
    }
}

/// Try to recognise a known command from its header line. We look at the
/// **first** non-empty line — that's where every tool we care about puts the
/// header.
fn detect_cmd(header: &str) -> Option<Cmd> {
    let h = header.trim();
    if h.starts_with("CONTAINER ID") && h.contains("IMAGE") && h.contains("NAMES") {
        return Some(Cmd::DockerPs);
    }
    if h.starts_with("REPOSITORY") && h.contains("TAG") && h.contains("IMAGE ID") {
        return Some(Cmd::DockerImages);
    }
    if h.starts_with("NAME") && h.contains("READY") && h.contains("RESTARTS") && h.contains("AGE") {
        return Some(Cmd::KubectlGetPods);
    }
    if h.starts_with("NAME") && h.contains("STATUS") && h.contains("ROLES") && h.contains("VERSION")
    {
        return Some(Cmd::KubectlGetNodes);
    }
    if h.starts_with("USER") && h.contains("PID") && h.contains("%CPU") && h.contains("COMMAND") {
        return Some(Cmd::PsAux);
    }
    if h.starts_with("UID") && h.contains("PID") && h.contains("PPID") && h.contains("CMD") {
        return Some(Cmd::PsEf);
    }
    if h.starts_with("COMMAND") && h.contains("PID") && h.contains("FD") && h.contains("NAME") {
        return Some(Cmd::Lsof);
    }
    if h.starts_with("Proto") && h.contains("Recv-Q") && h.contains("Local Address") {
        return Some(Cmd::NetstatAn);
    }
    if h.starts_with("Filesystem") && h.contains("Use%") && h.contains("Mounted") {
        return Some(Cmd::DfH);
    }
    None
}

/// Per-signal-column `(name, start, end)` byte ranges, computed from the
/// header. `end` is the start position of the NEXT column in the FULL list
/// (so content of a dropped column between two kept ones is correctly
/// attributed to the dropped one and not leaked into the previous kept
/// slice).
fn column_ranges(
    header: &str,
    full: &[&str],
    signal: &[&str],
) -> Vec<(String, usize, Option<usize>)> {
    let mut all_pos: Vec<(String, usize)> = full
        .iter()
        .filter_map(|n| header.find(n).map(|i| ((*n).to_string(), i)))
        .collect();
    all_pos.sort_by_key(|p| p.1);
    all_pos
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| signal.contains(&name.as_str()))
        .map(|(i, (name, start))| (name.clone(), *start, all_pos.get(i + 1).map(|(_, s)| *s)))
        .collect()
}

/// Slice a data line for the column [start, end). Two real-world corrections:
///   1. **Overflow**: if `end` lands mid-token (data value wider than the
///      header column), extend it forward until the next whitespace, so the
///      slice captures the full value. (docker ps wide IMAGE case.)
///   2. **Suppress next column**: the extension stops at the FIRST single
///      whitespace it sees, which is the actual visual gap between fields
///      — even if the header position predicted the next column starts
///      elsewhere.
fn slice_col(line: &str, start: usize, end: Option<usize>) -> String {
    let bytes = line.as_bytes();
    if start >= bytes.len() {
        return String::new();
    }
    let mut e = end.map(|x| x.min(bytes.len())).unwrap_or(bytes.len());
    // Overflow extension: if both byte positions either side of `e` are
    // non-whitespace, we're mid-token — extend until whitespace.
    if e > start && e < bytes.len() {
        let mid_token = !bytes[e - 1].is_ascii_whitespace() && !bytes[e].is_ascii_whitespace();
        if mid_token {
            while e < bytes.len() && !bytes[e].is_ascii_whitespace() {
                e += 1;
            }
        }
    }
    // Char-boundary-safe: header-derived byte positions land mid-codepoint
    // when an earlier column holds a multi-byte char (unicode COMMAND names
    // in lsof/ps) — `line[start..e]` panicked there.
    let mut s0 = start;
    while s0 < bytes.len() && !line.is_char_boundary(s0) {
        s0 += 1;
    }
    let mut e0 = e.max(s0);
    while e0 < bytes.len() && !line.is_char_boundary(e0) {
        e0 += 1;
    }
    line.get(s0..e0).unwrap_or("").trim().to_string()
}

// #555 — `_re_unused` placeholder removed; the regex/OnceLock imports
// it pinned were no longer needed (if a future enhancement wants them,
// re-add the import then).

/// Compact a tabular CLI output. Returns the input verbatim if no known
/// header is found in the first non-empty line.
pub fn compact(text: &str) -> String {
    // L42 — strip ANSI. Many tabular CLIs (`docker ps`, `aws s3 ls`,
    // `kubectl get`, `lsblk`) colour headers and status columns.
    // Stripping upfront prevents the header detector from missing
    // known tables that start with a colour-wrapped column name.
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();
    let mut lines = text.lines().peekable();
    // Skip and re-emit any leading blank lines / pre-header banner that
    // some tools add (e.g. `lsof` doesn't, but `df -h` doesn't either —
    // none of our targets do, so this is just defensive).
    let mut preamble = String::new();
    while let Some(&line) = lines.peek() {
        if line.trim().is_empty() {
            preamble.push_str(line);
            preamble.push('\n');
            lines.next();
        } else {
            break;
        }
    }
    let Some(header) = lines.next() else {
        return text.to_string();
    };
    let Some(cmd) = detect_cmd(header) else {
        return text.to_string();
    };
    // Header positions mean something only on an ALIGNED table, and an
    // aligned header pads at least one gap. One space between every name is a
    // table already respaced field by field — `tokenade`'s own `ps aux`
    // (seen in practice) — where slicing at `COMMAND`'s offset cut each row mid-field:
    // `:17.64 /usr/libexec/opendirectoryd`, a value no column ever held.
    if !header.trim().contains("  ") {
        return text.to_string();
    }
    let cols = column_ranges(header, cmd.all_columns(), cmd.signal_columns());
    if cols.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len() / 2);
    out.push_str(&preamble);
    out.push_str(
        &cols
            .iter()
            .map(|(n, _, _)| n.as_str())
            .collect::<Vec<_>>()
            .join("  "),
    );
    out.push('\n');

    let mut table_ended = false;
    for line in lines {
        // A blank line terminates the table: these CLIs print contiguous
        // rows. Column-slicing whatever FOLLOWS (another command's output in
        // a mixed script) shredded it — pass trailing content verbatim.
        if table_ended {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            table_ended = true;
            out.push('\n');
            continue;
        }
        // Shape check: a data row reaches at least the SECOND column's start
        // (rows are aligned under the header). A shorter/unaligned line is
        // the next command's output (`docker ps; git status` prints no blank
        // separator) — column-slicing it produced garbage.
        let second_col_start = cols.get(1).map(|(_, st, _)| *st).unwrap_or(0);
        if second_col_start > 0 && line.len() <= second_col_start {
            table_ended = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let parts: Vec<String> = cols
            .iter()
            .map(|(_, start, end)| slice_col(line, *start, *end))
            .collect();
        out.push_str(&parts.join("  "));
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

// ─────────────────────────────────────────────────────────────────────────
// Generic dash-underline table — header + `---- ----` rule + data rows.
//
// The whitelist [`compact`] above only handles nine known Unix commands by
// column name. But a *huge* class of tools emits a table whose header is
// underlined by a rule of space-separated dash groups: PowerShell
// `Format-Table` (Get-Process, Get-Service, Get-NetAdapter, Get-Disk,
// Get-LocalUser, … — hundreds of cmdlets, most without a dedicated module),
// many SQL/REST/cloud CLIs, and assorted custom tools. The dash rule is a
// universal, OS- and language-agnostic structural cue, so a single generic
// fold (keep header + rule + first N rows + a tail count) covers all of them
// without a per-command whitelist — the same strategy as `docker_table`.
// ─────────────────────────────────────────────────────────────────────────

/// Data rows kept verbatim before folding the tail of a generic dash table.
/// Same order of magnitude as the per-command HEAD constants in `cmd/*`.
const DASH_HEAD: usize = 50;

/// True when `line` is a column-separator rule of the form
/// `------- ------    -----` — space-separated groups of dashes.
///
/// Deliberately strict so it cannot steal another format:
/// - every non-space char must be `-` (rejects markdown `| --- |` pipes and
///   diff `--- a/file` which carry other characters);
/// - at least TWO dash groups (rejects a setext heading underline `------`
///   and a decorative single-run separator, which are one group);
/// - each group ≥ 2 dashes and ≥ 6 dashes total (rejects stray short runs).
/// Columns are cut where the `====` line cuts them — an image name may hold
/// spaces (`System Idle Process`).
pub fn ruler_spans(sep: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (i, c) in sep.chars().chain(std::iter::once(' ')).enumerate() {
        match (c == '=', start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                spans.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    spans
}

/// Cell `k`, trimmed. The last column runs to the end of the line: a value
/// wider than its ruler overflows it.
pub fn ruler_cell(chars: &[char], spans: &[(usize, usize)], k: usize) -> String {
    let (from, to) = spans[k];
    let to = if k + 1 == spans.len() {
        chars.len()
    } else {
        to.min(chars.len())
    };
    chars
        .get(from.min(chars.len())..to)
        .map(|c| c.iter().collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Any rule character works: Windows console tools (`tasklist`,
/// `driverquery`, `schtasks`) underline with `=`.
fn is_rule_of(line: &str, rule: char) -> bool {
    let t = line.trim();
    if t.is_empty() || !t.chars().all(|c| c == rule || c == ' ') {
        return false;
    }
    let groups: Vec<&str> = t.split(' ').filter(|g| !g.is_empty()).collect();
    if groups.len() < 2 || groups.iter().any(|g| g.len() < 2) {
        return false;
    }
    groups.iter().map(|g| g.len()).sum::<usize>() >= 6
}

/// True when `header` plausibly labels the columns delimited by `sep` (a line
/// for which [`is_rule_of`] is true). Requires the header to contain
/// alphanumerics and to align with the dash groups: for at least half the
/// groups, the column where the group starts must sit (±1) under a non-space
/// char in the header. This rejects a dash rule that merely happens to follow
/// an unrelated line (blank, prose, another rule).
fn header_matches_rule(header: &str, sep: &str, rule: char) -> bool {
    if !header.chars().any(|c| c.is_alphanumeric()) {
        return false;
    }
    // Char-column alignment (not bytes): the header/rule we target are ASCII,
    // but cells elsewhere may be Unicode — staying on chars is panic-free.
    let hdr: Vec<char> = header.chars().collect();
    let sep_chars: Vec<char> = sep.chars().collect();
    let (mut groups, mut aligned, mut i) = (0usize, 0usize, 0usize);
    while i < sep_chars.len() {
        if sep_chars[i] == rule {
            groups += 1;
            let hit = [i.wrapping_sub(1), i, i + 1]
                .iter()
                .any(|&c| hdr.get(c).is_some_and(|ch| !ch.is_whitespace()));
            if hit {
                aligned += 1;
            }
            while i < sep_chars.len() && sep_chars[i] == rule {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    groups >= 2 && aligned * 2 >= groups
}

/// Locate the first dash table in a bounded prefix: a header line directly
/// followed by a dash rule, with at least one data row beneath. Returns the
/// 0-based `(header_idx, separator_idx)`. Bounded so a stray `---- ----` deep
/// in a log cannot trigger a misclassification.
fn find_dash_header(text: &str) -> Option<(usize, usize)> {
    find_ruled_header(text, '-')
}

fn find_ruled_header(text: &str, rule: char) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let scan = lines.len().min(12);
    for i in 1..scan {
        if is_rule_of(lines[i], rule)
            && header_matches_rule(lines[i - 1], lines[i], rule)
            && lines.iter().skip(i + 1).any(|l| !l.trim().is_empty())
        {
            return Some((i - 1, i));
        }
    }
    None
}

/// True when `text` opens with a generic dash-underline table.
pub fn detect_dash_table(text: &str) -> bool {
    find_dash_header(text).is_some()
}

/// Fold a generic dash-underline table: keep any preamble, the header, the
/// dash rule and the first [`DASH_HEAD`] data rows, then announce the folded
/// tail. Returns the input verbatim when there is no dash table or the data
/// rows are already ≤ `DASH_HEAD` (nothing to gain — the caller then keeps the
/// original or falls back to generic dedup).
pub fn compact_dash_table(text: &str) -> String {
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();
    let Some((hdr_idx, sep_idx)) = find_dash_header(text) else {
        return text.to_string();
    };
    let lines: Vec<&str> = text.lines().collect();
    let data: Vec<&str> = lines[sep_idx + 1..]
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if data.len() <= DASH_HEAD {
        return text.to_string();
    }
    // **Every row, while the table fits.** The rows ARE the answer of a
    // table nobody here knows (`Get-Process`, `Get-Service`, `docker`-like
    // tools): measured 2026-10-02 on a real `Get-Process` (Windows 10, 85
    // processes, 10 KB — under the delivery threshold), 33 processes and
    // their Ids were cut, and nothing pointed to the rest since nothing was
    // stashed. What a row costs is its column padding; it is respaced.
    let whole = respaced_dash_table(&lines[..hdr_idx], &lines[hdr_idx..=sep_idx], &data);
    if tk_base::delivery::fits(&whole) {
        return whole;
    }
    let total = data.len();
    let mut out = String::with_capacity(text.len() / 2);
    for l in &lines[..hdr_idx] {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(lines[hdr_idx]);
    out.push('\n');
    out.push_str(lines[sep_idx]);
    out.push('\n');
    for l in &data[..DASH_HEAD] {
        out.push_str(l);
        out.push('\n');
    }
    use std::fmt::Write;
    let _ = write!(out, "  ... (+{} more rows)", total - DASH_HEAD);
    if let Some(names) = identities_of_the_rest(
        &lines[hdr_idx..=sep_idx],
        &data,
        DASH_HEAD,
        tk_base::delivery::threshold_tokens().saturating_sub(tk_base::tokenizer::estimate(&out)),
    ) {
        let _ = write!(out, " — {names}");
    }
    out
}

/// The last column of the rows a cut leaves out, when it is an IDENTITY —
/// nearly all distinct and not numbers (`Name` of `Get-ChildItem`,
/// `DisplayName` of `Get-Service`): `Name: a · b · …`. Measured 2026-10-03 on
/// a real `Get-ChildItem C:\Windows\System32\drivers` (436 files, 9 760
/// tokens even respaced): the cut showed 50 and only counted the other 386,
/// whose names alone cost 1 300. Kept within `budget` tokens, the tail
/// counted (`… +K`) when even the names do not fit; `None` when the column
/// is not an identity (`ProcessName` repeats `svchost`).
fn identities_of_the_rest(
    head: &[&str],
    data: &[&str],
    shown: usize,
    budget: usize,
) -> Option<String> {
    let rows: Vec<Vec<char>> = head
        .iter()
        .chain(data)
        .map(|l| l.trim_end().chars().collect())
        .collect();
    let columns = columns_of(&rows);
    // The last column runs to the end of the line (a name may overflow it).
    let &(a, _) = columns.last()?;
    let cell = |r: &Vec<char>| {
        r.get(a.min(r.len())..)
            .map(|c| c.iter().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let label = cell(&rows[0]);
    let rest: Vec<String> = rows[head.len() + shown..].iter().map(cell).collect();
    let distinct = rest.iter().collect::<std::collections::HashSet<_>>().len();
    let words = rest.iter().all(|n| n.chars().any(|c| c.is_alphabetic()));
    if label.is_empty() || rest.is_empty() || !words || distinct * 10 < rest.len() * 9 {
        return None;
    }
    let mut out = format!("{label}: ");
    let mut spent = tk_base::tokenizer::estimate(&out);
    for (i, n) in rest.iter().enumerate() {
        let more = format!("{}{n}", if i == 0 { "" } else { " · " });
        let tail = format!(" · … +{}", rest.len() - i);
        let cost = tk_base::tokenizer::estimate(&more);
        if spent + cost + tk_base::tokenizer::estimate(&tail) > budget {
            if i == 0 {
                return None;
            }
            out.push_str(&tail);
            return Some(out);
        }
        spent += cost;
        out.push_str(&more);
    }
    Some(out)
}

/// A one-table output given WHOLE with its column padding collapsed — when
/// that fits the delivery threshold. `None` otherwise (no such table, or too
/// big): the caller's own ladder decides. For the listings whose rows are the
/// answer (`driverquery`: 371 drivers, the first 50 kept, under the
/// threshold). The table sits under a `----`/`====` rule, or — `wmic` — has
/// none: its first line is then the header, and must name every column the
/// rows' common blanks cut.
pub fn whole_column_table(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let whole = if let Some((hdr_idx, sep_idx)) =
        find_ruled_header(text, '-').or_else(|| find_ruled_header(text, '='))
    {
        let data: Vec<&str> = lines[sep_idx + 1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let rows: Vec<Vec<char>> = lines[hdr_idx..=sep_idx]
            .iter()
            .chain(&data)
            .map(|l| l.trim_end().chars().collect())
            .collect();
        respaced(&lines[..hdr_idx], &rows, 2, &columns_of(&rows))
    } else {
        let hdr_idx = lines.iter().position(|l| !l.trim().is_empty())?;
        let data: Vec<&str> = lines[hdr_idx + 1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let rows: Vec<Vec<char>> = std::iter::once(lines[hdr_idx])
            .chain(data.iter().copied())
            .map(|l| l.trim_end().chars().collect())
            .collect();
        // Left-aligned cells: a run the header does not label continues the
        // column on its left (`System Idle Process` alone reaches past a
        // blank every shorter name leaves).
        let header = &rows[0];
        let labelled = |&(a, b): &(usize, usize)| {
            header
                .get(a.min(header.len())..b.min(header.len()))
                .is_some_and(|c| c.iter().any(|c| !c.is_whitespace()))
        };
        let mut columns: Vec<(usize, usize)> = Vec::new();
        for run in columns_of(&rows) {
            match columns.last_mut() {
                Some(last) if !labelled(&run) => last.1 = run.1,
                None if !labelled(&run) => return None,
                _ => columns.push(run),
            }
        }
        // Without a rule, only the padding says "column": a table leaves two
        // blanks at least between columns, prose one (`Windows 10 Pro` would
        // be respaced into `Windows  10  Pro` — a fabrication).
        let padded = columns.windows(2).all(|w| w[1].0 >= w[0].1 + 2);
        if data.is_empty() || columns.len() < 2 || !padded {
            return None;
        }
        respaced(&lines[..hdr_idx], &rows, 1, &columns)
    };
    tk_base::delivery::fits(&whole).then_some(whole)
}

/// Columns are the runs of positions that are NOT blank in every row.
fn columns_of(rows: &[Vec<char>]) -> Vec<(usize, usize)> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let blank = |i: usize| rows.iter().all(|r| r.get(i).is_none_or(|c| *c == ' '));
    let mut columns: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for i in 0..=width {
        match (i < width && !blank(i), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                columns.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    columns
}

/// The table with its column padding collapsed: cells separated by two
/// spaces (a cell may hold one — `Memory Compression`). Columns are the runs
/// of positions that are NOT blank in every line, ruler included: a
/// right-aligned value may spill left of its dash segment (`578668` under
/// `-----`), so the segments themselves cannot cut it. An empty cell stays
/// empty between its two separators.
fn respaced_dash_table(preamble: &[&str], head: &[&str], data: &[&str]) -> String {
    let rows: Vec<Vec<char>> = head
        .iter()
        .chain(data)
        .map(|l| l.trim_end().chars().collect())
        .collect();
    respaced(preamble, &rows, head.len(), &columns_of(&rows))
}

/// `rows` (header first) cut at `columns`, cells joined by two spaces.
fn respaced(
    preamble: &[&str],
    rows: &[Vec<char>],
    head_len: usize,
    columns: &[(usize, usize)],
) -> String {
    let cells_of = |row: &Vec<char>| -> Vec<String> {
        columns
            .iter()
            .map(|&(a, b)| {
                row.get(a.min(row.len())..b.min(row.len()))
                    .map(|c| c.iter().collect::<String>())
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .collect()
    };
    let mut out = String::new();
    for l in preamble {
        out.push_str(l);
        out.push('\n');
    }
    // The rule under the header was as wide as the column's PADDING: once the
    // cells are respaced it underlines the header and nothing else (`pip
    // list`: 40 + 31 dashes, 2026-10-04) — PowerShell's is already that.
    let is_rule = |c: &String| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | '='));
    let mut head: Vec<Vec<String>> = rows[..head_len.min(rows.len())]
        .iter()
        .map(cells_of)
        .collect();
    for i in 1..head.len() {
        if head[i].iter().any(is_rule) && head[i].iter().all(|c| c.is_empty() || is_rule(c)) {
            let widths: Vec<usize> = head[i - 1]
                .iter()
                .map(|c| c.chars().count().max(3))
                .collect();
            for (j, c) in head[i].iter_mut().enumerate() {
                if is_rule(c) {
                    let ch = c.chars().next().unwrap_or('-');
                    *c = std::iter::repeat_n(
                        ch,
                        widths.get(j).copied().unwrap_or(3).min(c.chars().count()),
                    )
                    .collect();
                }
            }
        }
    }
    let data: Vec<Vec<String>> = rows[head_len.min(rows.len())..]
        .iter()
        .map(cells_of)
        .collect();
    // **A first column of few values groups the rows** (`Mode` of
    // `Get-ChildItem`: 419 `-a----` and 14 `d-----` in a real 436-row
    // listing, 2026-10-03; `Status` of `Get-Service`): said once per group
    // instead of on every row, as `tasklist` groups by session.
    let mut groups: Vec<&str> = Vec::new();
    for row in &data {
        let key = row.first().map(String::as_str).unwrap_or("");
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    // A CATEGORY, not a measure: every key carries a letter (`-a----`,
    // `Running`); a numeric column that happens to repeat is not grouped.
    let categorical = groups.iter().all(|k| k.chars().any(|c| c.is_alphabetic()));
    let grouped = columns.len() >= 3 && data.len() >= 20 && groups.len() <= 4 && categorical;
    let line = |cells: &[String]| cells.join("  ").trim_end().to_string();
    if !grouped {
        for row in head.iter().chain(&data) {
            out.push_str(&line(row));
            out.push('\n');
        }
        return out;
    }
    let label = head
        .first()
        .and_then(|h| h.first())
        .cloned()
        .unwrap_or_default();
    for row in &head {
        out.push_str(&line(&row[1..]));
        out.push('\n');
    }
    for key in groups {
        let members: Vec<&Vec<String>> = data.iter().filter(|r| r[0] == key).collect();
        out.push_str(&format!("[{label} {key} — {}]\n", members.len()));
        for row in members {
            out.push_str(&line(&row[1..]));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    fn is_rule_of_dash(line: &str) -> bool {
        is_rule_of(line, '-')
    }
    use super::*;
    use std::fmt::Write;

    /// `pip list` respaced: the rule shrinks with the padding it measured.
    #[test]
    fn a_respaced_rule_is_short() {
        let mut t = String::from("Package                                  Version\n---------------------------------------- -------------------------------\n");
        for i in 0..80 {
            let _ = writeln!(t, "{:<40} 1.{i}.0", format!("pkg-{i}"));
        }
        let out = compact_dash_table(&t);
        assert!(
            out.starts_with("Package  Version\n-------  -------\npkg-0  1.0.0\n"),
            "{out}"
        );
    }

    /// A header with one space between every name is not aligned: its
    /// offsets say nothing about where a row's fields sit.
    #[test]
    fn an_unaligned_table_is_not_sliced_by_header_offsets() {
        let t = "USER PID %CPU %MEM VSZ RSS TT STAT STARTED TIME COMMAND\n\
                 root 109 56.8 0.1 33827288 8200 ?? Rs 22Jan26 10:17.64 /usr/libexec/opendirectoryd\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn docker_ps_keeps_container_id_and_drops_created() {
        let t = "CONTAINER ID   IMAGE          COMMAND        CREATED        STATUS         PORTS                  NAMES\n\
                 abc12345def0   nginx:latest   \"/docker.sh\"   3 days ago     Up 10 minutes  0.0.0.0:80->80/tcp     web\n\
                 fed98765aaa1   redis:7        \"redis-srv\"    1 week ago     Up 5 minutes   6379/tcp               cache\n";
        let out = compact(t);
        assert!(out.contains("nginx:latest"));
        assert!(out.contains("redis:7"));
        assert!(out.contains("web"));
        assert!(
            out.contains("abc12345def0") && out.contains("fed98765aaa1"),
            "{out}"
        );
        // Dropped:
        assert!(!out.contains("3 days ago"));
    }

    #[test]
    fn kubectl_get_pods_keeps_restarts_and_drops_age() {
        let t = "NAME             READY   STATUS    RESTARTS   AGE\n\
                 web-7d4b8c-x42   1/1     Running   3          15d\n\
                 redis-0          1/1     Running   0          22d\n\
                 worker-aa-99q1   0/1     Pending   12         3h\n";
        let out = compact(t);
        assert!(out.contains("web-7d4b8c-x42"));
        assert!(out.contains("Pending"));
        assert!(out.contains("RESTARTS"), "{out}");
        assert!(
            out.lines()
                .any(|l| l.contains("worker-aa-99q1") && l.contains("12")),
            "{out}"
        );
        // Dropped.
        assert!(!out.contains("15d"));
        assert!(!out.contains("AGE"));
    }

    #[test]
    fn docker_images_keep_the_id_and_the_digest() {
        let t = "REPOSITORY                  TAG       DIGEST                                                                    IMAGE ID       CREATED        SIZE\n\
                 orbit-sandbox               1.4.0     <none>                                                                    792b234e1715   9 days ago     2.81GB\n\
                 redis                       7.2       sha256:ccd6aa8d45ff3f033d6fa15b8cc1a50579f65c89f38cf9bb607a954c4f2128ed   0002e855e2e6   3 weeks ago    38.6MB\n";
        let out = compact(t);
        assert!(
            out.contains("792b234e1715") && out.contains("0002e855e2e6"),
            "{out}"
        );
        assert!(out.contains("sha256:ccd6aa8d"), "{out}");
        assert!(!out.contains("9 days ago"), "{out}");
    }

    #[test]
    fn ps_aux_keeps_command() {
        // We deliberately keep ONLY the COMMAND column for `ps aux` — the
        // numeric columns are right-aligned and not safely slice-extractable.
        let t = "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n\
                 root         1  0.0  0.1 168000 12000 ?        Ss   Jan01   0:01 /sbin/init\n\
                 www-data  1234  2.5  3.2 500000 80000 ?        Sl   Jan02  10:34 nginx: worker process\n";
        let out = compact(t);
        assert!(out.contains("/sbin/init"));
        assert!(out.contains("nginx: worker process"));
        assert!(!out.contains("Jan01"));
        assert!(!out.contains("168000"));
        assert!(!out.contains("www-data"));
    }

    #[test]
    fn unknown_header_passes_through() {
        let t = "totally not a recognised header\nrow1\nrow2\n";
        let out = compact(t);
        assert_eq!(out, t);
    }

    // ── generic dash-underline table ──────────────────────────────────────

    #[test]
    fn dash_separator_shape() {
        assert!(is_rule_of_dash("------- ------    ----- -----"));
        assert!(is_rule_of_dash("---- ----"));
        // single group → setext heading underline / decorative rule
        assert!(!is_rule_of_dash("--------"));
        // carries non-dash chars → markdown pipes / diff header
        assert!(!is_rule_of_dash("| --- | --- |"));
        assert!(!is_rule_of_dash("--- a/file.rs"));
        // group too short / too few total dashes
        assert!(!is_rule_of_dash("- -"));
        assert!(!is_rule_of_dash(""));
    }

    #[test]
    fn powershell_get_process_table_folds() {
        // Real `Get-Process | Format-Table` shape: header + dash rule + rows.
        let mut t = String::from(
            "Handles  NPM(K)    PM(K)      WS(K)   CPU(s)     Id  SI ProcessName\n\
             -------  ------    -----      -----   ------     --  -- -----------\n",
        );
        // Aligned on the header as PowerShell aligns it, at every width.
        for i in 0..4000 {
            writeln!(
                t,
                "{:>7}  {:>6}    {:>5}    {:>7}   {:>6}  {:>5}  {:>2} proc{}",
                100 + i,
                12,
                45678,
                9012,
                "1.23",
                1000 + i,
                1,
                i
            )
            .unwrap();
        }
        assert!(detect_dash_table(&t));
        let out = compact_dash_table(&t);
        assert!(out.contains("Handles"));
        assert!(out.contains("ProcessName"));
        assert!(out.contains("-------  ------"));
        assert!(out.contains("proc0"));
        assert!(out.contains("proc49"));
        // Row 50 and beyond: their ROWS are folded, their names follow
        // (an identity column — every `procN` distinct).
        assert!(!out.contains(" 1050   1 proc50"), "a folded row was shown");
        assert!(
            out.contains("... (+3950 more rows) — ProcessName: proc50 · proc51 · "),
            "{}",
            &out[out.len().saturating_sub(300)..]
        );
        // The end-to-end path through the router is in
        // `tests/misc/a_format_is_reached_by_its_router.rs`.
    }

    /// A table that fits keeps every row, its padding collapsed — real
    /// `Get-Process` shapes: a value spilling left of its dashes, a name with
    /// a space, an empty CPU cell.
    #[test]
    fn a_dash_table_that_fits_keeps_every_row() {
        let mut t = String::from(
            "\nHandles  NPM(K)    PM(K)      WS(K)     CPU(s)     Id  SI ProcessName\n\
             -------  ------    -----      -----     ------     --  -- -----------\n\
             \x20     0       0       60          8                 0   0 Idle\n\
             \x20     0       0       40          0       0,00   1792   0 Memory Compression\n\
             \x20   878     313   578668     289876      15,77   2828   0 MsMpEng\n",
        );
        for i in 0..60 {
            writeln!(
                t,
                "    125       8     1456       5792       0,02  {:5}   0 svchost",
                1000 + i
            )
            .unwrap();
        }
        let out = compact_dash_table(&t);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "");
        assert_eq!(
            lines[1],
            "Handles  NPM(K)  PM(K)  WS(K)  CPU(s)  Id  SI  ProcessName"
        );
        assert_eq!(lines[3], "0  0  60  8    0  0  Idle");
        assert_eq!(lines[4], "0  0  40  0  0,00  1792  0  Memory Compression");
        assert_eq!(
            lines[5],
            "878  313  578668  289876  15,77  2828  0  MsMpEng"
        );
        assert_eq!(lines.len(), 66, "{out}");
        assert_eq!(lines[65], "125  8  1456  5792  0,02  1059  0  svchost");
    }

    /// `driverquery`: an `=` rule, a cell left empty (no link date).
    #[test]
    fn a_ruled_table_that_fits_is_given_whole() {
        let mut t = String::from(
            "\nModule Name  Display Name           Driver Type   Link Date\n\
             ============ ====================== ============= ======================\n\
             1394ohci     1394 OHCI Compliant Ho Kernel\n",
        );
        for i in 0..60 {
            writeln!(
                t,
                "drv{i:<9} Driver number {i:<8} Kernel        19/05/2015 00:28:03"
            )
            .unwrap();
        }
        let out = whole_column_table(&t).expect("fits");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[1],
            "Module Name  Display Name  Driver Type  Link Date"
        );
        assert_eq!(lines[3], "1394ohci  1394 OHCI Compliant Ho  Kernel");
        assert_eq!(
            lines[63],
            "drv59  Driver number 59  Kernel  19/05/2015 00:28:03"
        );
        assert_eq!(whole_column_table("no table here\njust prose\n"), None);
    }

    /// `wmic`: no rule — the first line is the header and names each column;
    /// a value may hold spaces (`System Idle Process`).
    #[test]
    fn a_column_table_without_a_rule_is_given_whole() {
        let mut t = String::from(
            "HandleCount  Name                    Priority  ProcessId  \n\
             0            System Idle Process     0         0          \n",
        );
        for i in 0..60 {
            writeln!(
                t,
                "180          svchost.exe             8         {:<5}      ",
                2000 + i
            )
            .unwrap();
        }
        let out = whole_column_table(&t).expect("fits");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "HandleCount  Name  Priority  ProcessId");
        assert_eq!(lines[1], "0  System Idle Process  0  0");
        assert_eq!(lines[61], "180  svchost.exe  8  2059");
        // Prose is not a table: its header would not name every column.
        assert_eq!(
            whole_column_table("Caption=Microsoft Windows 10 Pro\nVersion=10.0.19045\n"),
            None
        );
    }

    /// `Get-ChildItem`: a `Mode` of two values groups the rows.
    #[test]
    fn a_first_column_of_few_values_groups_the_rows() {
        let mut t = String::from(
            "\n    Directory: C:\\Windows\\System32\\drivers\n\n\nMode                 LastWriteTime         Length Name\n\
             ----                 -------------         ------ ----\n\
             d-----        30/05/2026     13:06                de-DE\n",
        );
        for i in 0..60 {
            writeln!(
                t,
                "-a----        07/12/2019     10:08         {:>6} drv{i}.sys",
                10_000 + i
            )
            .unwrap();
        }
        let out = compact_dash_table(&t);
        let lines: Vec<&str> = out.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("LastWriteTime"))
            .expect("header");
        assert_eq!(
            lines[at..at + 5].to_vec(),
            [
                "LastWriteTime  Length  Name",
                "-------------  ------  ----",
                "[Mode d----- — 1]",
                "30/05/2026     13:06    de-DE",
                "[Mode -a---- — 60]",
            ]
        );
        assert_eq!(
            lines.last(),
            Some(&"07/12/2019     10:08  10059  drv59.sys")
        );
    }

    /// A last column that repeats is no identity: the cut lists nothing.
    #[test]
    fn a_repeating_last_column_names_nothing_after_the_cut() {
        let mut t = String::from(
            "Handles  NPM(K)    PM(K)      WS(K)   CPU(s)     Id  SI ProcessName\n\
             -------  ------    -----      -----   ------     --  -- -----------\n",
        );
        for i in 0..4000 {
            writeln!(
                t,
                "{:>7}  {:>6}    {:>5}    {:>7}   {:>6}  {:>5}  {:>2} svchost",
                100 + i,
                12,
                45678,
                9012,
                "1.23",
                1000 + i,
                1
            )
            .unwrap();
        }
        let out = compact_dash_table(&t);
        assert!(
            out.ends_with("... (+3950 more rows)"),
            "{}",
            &out[out.len().saturating_sub(200)..]
        );
    }

    #[test]
    fn small_dash_table_passes_through() {
        // ≤ DASH_HEAD data rows → nothing to gain → verbatim.
        let mut t = String::from(
            "Name        Status\n\
             ----        ------\n",
        );
        for i in 0..10 {
            writeln!(t, "svc{}        Running", i).unwrap();
        }
        let out = compact_dash_table(&t);
        assert_eq!(out, t);
    }

    #[test]
    fn unified_diff_not_a_dash_table() {
        let t = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n-old\n+new\n";
        assert!(!detect_dash_table(t));
    }

    #[test]
    fn setext_markdown_not_a_dash_table() {
        // Single-group underline under a title — must not be claimed.
        let t = "My Document Title\n-----------------\n\nBody paragraph here.\n";
        assert!(!detect_dash_table(t));
    }

    #[test]
    fn markdown_pipe_table_not_a_dash_table() {
        let t = "| Col A | Col B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n";
        assert!(!detect_dash_table(t));
    }

    #[test]
    fn dash_rule_after_blank_line_rejected() {
        // A dash rule whose preceding line is blank/non-labelling is not a
        // header→rule pair.
        let t = "intro line\n\n---- ----\ndata row one\ndata row two\n";
        assert!(!detect_dash_table(t));
    }

    #[test]
    fn empty_and_garbage_safe() {
        assert_eq!(compact(""), "");
        let _ = compact("\nonly blank line at start\n\n");
    }

    #[test]
    fn savings_real_shape() {
        let mut t = String::from("NAME             READY   STATUS    RESTARTS   AGE\n");
        for i in 0..200 {
            let _ = writeln!(
                t,
                "web-7d4b8c-x{i:04}   1/1     Running   {}          {}d",
                i % 7,
                (i % 365) + 1
            );
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("web-7d4b8c-x0000"));
        // 19 %, not the >30 % this floor once read. Column padding is what this
        // compactor removes, and cl100k charges almost nothing for padding —
        // one multi-space token per gutter, not one per space. The old
        // estimator priced the gutters at 4 chars/token and handed this
        // compactor a third of its reported gain for free.
        assert!(sav.percent() > 18.0, "got {:?}", sav);
    }
}
