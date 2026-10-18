//! Shared engine for the `ps` family (`ps`, `ps aux`, `ps -ef`, `ps -o …`).
//!
//! `ps aux` on a real host is long (hundreds of rows) and almost entirely
//! kernel threads pinned at `0.0  0.0` %CPU/%MEM — noise. What an agent
//! actually wants is the handful of processes burning CPU or memory. So we
//! parse the `%CPU`/`%MEM` columns *by header position* (those column
//! labels are ps invariants, never localised) and surface every process
//! above [`HOT_THRESHOLD`] first, then head-fold the inert remainder.
//!
//! When neither column is present (e.g. `ps -ef`, whose columns are
//! `UID PID PPID C STIME TTY TIME CMD`) there is nothing to rank, so we
//! fall back to a plain header-preserving head fold. The header line is
//! always kept verbatim; the column structure (`PID`, `CMD`/`COMMAND`)
//! is locale-independent.

use std::fmt::Write;

const HOT_THRESHOLD: f64 = 1.0;

/// Index of an exact-match column label among the header's whitespace tokens.
fn col_index(header: &str, name: &str) -> Option<usize> {
    header.split_whitespace().position(|t| t == name)
}

/// Parse the `nth` whitespace field of a row as a percentage value.
fn field_pct(row: &str, idx: usize) -> Option<f64> {
    row.split_whitespace().nth(idx)?.parse::<f64>().ok()
}

/// A ps table is a header carrying the (untranslated) `PID` column plus a
/// command column, followed by at least one data row.
pub fn detect(text: &str) -> bool {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return false;
    };
    let toks: Vec<&str> = header.split_whitespace().collect();
    // BSD `ps -o` titles a column by its keyword: `comm` → `COMM`, `ucomm`
    // → `UCOMM`, `args` → `ARGS`. Read as `COMMAND` only, a macOS
    // `ps -axo pid,%cpu,comm` missed the ranking and was cut by PID order.
    let has_shape = toks.contains(&"PID")
        && toks
            .iter()
            .any(|t| matches!(*t, "COMMAND" | "CMD" | "COMM" | "UCOMM" | "ARGS"));
    has_shape && lines.next().is_some()
}

/// Fold a ps table. Returns `None` when short enough that there is nothing
/// to fold (header + ≤ `head` rows).
/// Process states worth surfacing whatever the process is costing.
///
/// The fold ranks by `%CPU`/`%MEM` and cuts the tail, which is right for three
/// hundred idle daemons — and wrong for the two states someone actually runs
/// `ps` to find. A **zombie** (`Z`) burns nothing by definition, so it sorted
/// last and was cut: MEASURED, a `<defunct>` entry among 300 idle processes
/// never reached the reader. **`D`** (uninterruptible sleep) is the shape of a
/// hung I/O — also cheap, also the answer. **`T`/`t`** is stopped or traced.
///
/// The state field carries suffix flags (`Ss`, `S<`, `R+`, `Zs`); only the
/// leading letter names the state.
/// How many rows in an unhealthy state are shown before the rest become a
/// count. Enough to see WHAT they are, not enough to become the listing again.
const MAX_SICK_SHOWN: usize = 5;

fn state_needs_attention(field: &str) -> bool {
    matches!(field.chars().next(), Some('Z' | 'D' | 'T' | 't' | 'X'))
}

pub fn fold(text: &str, head: usize, label: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    // Whole when it fits, respaced: a cut at `head` rows named the rest only
    // as `+9 more` on a `ps -e` of 49 processes, 3.3 KB (macOS corpus,
    // 2026-10-03). Respaced, the table still folds, so it keeps the hand
    // against `auto`, which keeps COMMAND alone.
    // Already one space apart (a second pass, or `ps -o` with narrow
    // columns), it is handed back for what follows to judge — never cut.
    // `fits` judges what would be DELIVERED: the padding is a quarter of a
    // `ps -eo pid,comm,%mem` (625 rows, 18 KB raw, 13.7 KB respaced).
    let whole = respaced(&lines);
    if tk_base::delivery::fits(whole.as_deref().unwrap_or(text)) {
        return whole.filter(|w| *w != lines.join("\n"));
    }
    if lines.len() <= head + 1 {
        return None;
    }
    let header = lines[0];
    let rows = &lines[1..];
    let cpu = col_index(header, "%CPU");
    let mem = col_index(header, "%MEM");
    let stat = col_index(header, "STAT").or_else(|| col_index(header, "S"));

    let mut hot: Vec<&str> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    let mut sick_shown = 0usize;
    let mut sick_hidden = 0usize;
    if cpu.is_some() || mem.is_some() || stat.is_some() {
        for &r in rows {
            let c = cpu.and_then(|i| field_pct(r, i)).unwrap_or(0.0);
            let m = mem.and_then(|i| field_pct(r, i)).unwrap_or(0.0);
            let sick = stat
                .and_then(|i| r.split_whitespace().nth(i))
                .is_some_and(state_needs_attention);
            // A handful of zombies is a diagnosis; a hundred is a number.
            // MEASURED on a real machine carrying 113 of them: promoting every
            // one turned a 30-line fold into 132 lines that all say
            // `<defunct>`. The count is what informs — `top`'s own header
            // prints it — so keep a few as evidence and let the tail line say
            // how many there were.
            if sick && sick_shown >= MAX_SICK_SHOWN {
                sick_hidden += 1;
                continue;
            }
            if c >= HOT_THRESHOLD || m >= HOT_THRESHOLD || sick {
                if sick {
                    sick_shown += 1;
                }
                hot.push(r);
            } else {
                rest.push(r);
            }
        }
    } else {
        rest.extend_from_slice(rows);
    }

    let total = rows.len();
    let mut out = String::with_capacity(text.len() / 2);
    // Rows that already come RANKED (`--sort=-%mem`, `ps aux | sort -k4`) are
    // the order the reader asked for: a busy row moved to the top broke it
    // (measured 2026-10-04, an 11 %-CPU / 0 %-MEM row ahead of a memory
    // ranking). The head keeps the given order; what the cut would hide and
    // matters follows it.
    let ranked_by = [mem, cpu].into_iter().flatten().find(|&i| {
        let v: Vec<f64> = rows.iter().map(|r| field_pct(r, i).unwrap_or(0.0)).collect();
        // Four distinct values at least: a column that is all 0.1 then all
        // 0.0 is not a ranking anyone asked for.
        let steps = v.windows(2).filter(|w| w[0] > w[1]).count();
        steps >= 3 && v.windows(2).all(|w| w[0] >= w[1])
    });
    if let Some(rank) = ranked_by {
        // Past the head, a row high on the RANKED column is just the ranking
        // going on; high on the other one, or in a sick state, it is news.
        let other = [mem, cpu].into_iter().flatten().find(|&i| i != rank);
        let shown = rows.len().min(head);
        let later: Vec<&&str> = hot
            .iter()
            .filter(|r| !rows[..shown].contains(r))
            .filter(|r| {
                stat.and_then(|i| r.split_whitespace().nth(i)).is_some_and(state_needs_attention)
                    || other.and_then(|i| field_pct(r, i)).is_some_and(|v| v >= HOT_THRESHOLD)
            })
            .collect();
        let _ = writeln!(out, "[tokenade:{label}] {total} process(es), in the order given");
        out.push_str(header);
        out.push('\n');
        for r in &rows[..shown] {
            out.push_str(r);
            out.push('\n');
        }
        if !later.is_empty() {
            let _ = writeln!(out, "  further down, busy or in a state that needs attention:");
            for r in &later {
                out.push_str(r);
                out.push('\n');
            }
        }
        let hidden = total - shown - later.len();
        if hidden > 0 {
            let _ = writeln!(out, "  ... (+{hidden} more) ...");
        }
        return Some(out.trim_end().to_string());
    }
    if hot.is_empty() {
        let _ = writeln!(out, "[tokenade:{label}] {total} process(es)");
    } else {
        let _ = writeln!(
            out,
            "[tokenade:{label}] {total} process(es), {} busy or in a state that needs attention, first",
            hot.len()
        );
    }
    out.push_str(header);
    out.push('\n');
    for r in &hot {
        out.push_str(r);
        out.push('\n');
    }
    let shown = rest.len().min(head);
    for r in rest.iter().take(shown) {
        out.push_str(r);
        out.push('\n');
    }
    if rest.len() > shown || sick_hidden > 0 {
        let extra = if sick_hidden > 0 {
            format!(
                ", including {sick_hidden} more in a state that needs attention"
            )
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "  ... (+{} more{extra}) ...",
            rest.len().saturating_sub(shown) + sick_hidden
        );
    }
    Some(out.trim_end().to_string())
}

/// One space between fields. The header names N columns and only the last,
/// COMMAND, holds spaces — so the first N−1 whitespace-separated fields of a
/// row are its cells and the rest of the line, less one separator, is its
/// command, verbatim.
/// `None` when a row has fewer fields than the header, so a shape this does
/// not understand is never rewritten.
pub(crate) fn respaced(lines: &[&str]) -> Option<String> {
    let (header, rows) = lines.split_first()?;
    let n = header.split_whitespace().count();
    if n < 2 {
        return None;
    }
    let mut out = header.split_whitespace().collect::<Vec<_>>().join(" ");
    for row in rows {
        let mut cells: Vec<&str> = Vec::with_capacity(n);
        let mut rest: &str = row;
        for _ in 0..n - 1 {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace)?;
            cells.push(&rest[..end]);
            rest = &rest[end..];
        }
        // ONE separator goes; any more is the command's own indentation —
        // `ps axjf` draws its tree there (`  \_ sshd`).
        let sep = rest.chars().next()?.len_utf8();
        let last = rest[sep..].trim_end();
        if last.trim_start().is_empty() {
            return None;
        }
        cells.push(last);
        out.push('\n');
        out.push_str(&cells.join(" "));
    }
    Some(out)
}

// --- Shared-helper stubs (see df_table.rs for the rationale) ---------------
pub fn try_compact(_text: &str) -> Option<String> {
    None
}
pub fn compact(text: &str) -> String {
    crate::archetypes::passthrough(text)
}
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUX_HEADER: &str = "USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND";

    /// macOS `ps -axo pid,ppid,user,%cpu,%mem,comm` titles its last column
    /// `COMM`: the busy process far down the PID order must come first.
    #[test]
    fn a_bsd_comm_column_is_ranked_like_command() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = String::from(" PID PPID USER %CPU %MEM COMM\n");
        for i in 0..80 {
            let _ = writeln!(t, " {} 1 root 0.0 0.1 /usr/libexec/d{i}", 100 + i);
        }
        t.push_str(" 999 1 root 37.5 0.2 /usr/libexec/opendirectoryd\n");
        let out = crate::cmd::dispatch_named("ps", &t).unwrap();
        let second_row = out.lines().nth(2).unwrap();
        assert_eq!(second_row, " 999 1 root 37.5 0.2 /usr/libexec/opendirectoryd", "{out}");
    }

    fn aux(n: usize) -> String {
        let mut t = format!("{AUX_HEADER}\n");
        for i in 0..n {
            let _ = writeln!(t, "root {i} 0.0 0.0 27048 15224 ? Ss juin10 0:06 [kthread{i}]");
        }
        t
    }

    #[test]
    fn detects_aux() {
        assert!(detect(&aux(2)));
    }

    #[test]
    fn detects_ef() {
        assert!(detect(
            "UID PID PPID C STIME TTY TIME CMD\nroot 1 0 0 juin10 ? 00:00:06 /sbin/init\nroot 2 0 0 juin10 ? 00:00:00 [kthreadd]\n"
        ));
    }

    #[test]
    fn rejects_non_ps() {
        assert!(!detect("hello\nworld\n"));
    }

    /// `ps aux --sort=-%mem`: the ranking the reader asked for stays the
    /// order; what the cut would hide and matters is listed after it.
    #[test]
    fn a_ranked_listing_keeps_its_order() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = format!("{AUX_HEADER}\n");
        for i in 0..80 {
            let (cpu, cmd) = if i == 10 { (11.0, "hot-one") } else { (0.0, "idle") };
            let st = if i == 70 { "Z" } else { "S" };
            let _ = writeln!(t, "u {i} {cpu:.1} {:.1} 1 1 ? {st} 00:00 0:00 {cmd}-{i}", 8.0 - i as f64 * 0.1);
        }
        let out = fold(&t, 40, "ps-aux").unwrap();
        assert!(out.contains("in the order given"), "{out}");
        let (a, b, c) = (out.find("idle-9\n").unwrap(), out.find("hot-one-10").unwrap(), out.find("idle-11\n").unwrap());
        assert!(a < b && b < c, "the ranking was reordered:\n{out}");
        let further = out.find("further down").expect("the zombie past the cut is named");
        assert!(out.find("idle-70").unwrap() > further, "{out}");
        assert!(!out.contains("idle-50\n"), "the cut still cuts:\n{out}");
    }

    #[test]
    fn surfaces_busy_processes_first() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = aux(80);
        // A real CPU/MEM hog buried in the kthread noise.
        t.push_str("alice 4242 87.5 12.3 999999 888888 pts/0 R 10:00 5:00 cargo build\n");
        let out = fold(&t, 40, "ps-aux").unwrap();
        let hog = out.find("cargo build").unwrap();
        let fold = out.find("more) ...").unwrap();
        assert!(hog < fold, "busy process must precede fold:\n{out}");
        // The banner has to say the listing was REORDERED — an agent reading a
        // `ps` fold in input order would misread the promoted rows. The exact
        // wording moved when process STATE joined %CPU/%MEM as a reason to
        // promote, so this pins the promise, not the sentence.
        assert!(out.contains("first"), "the banner must say rows were promoted:\n{out}");
        assert!(out.contains("COMMAND"));
    }

    #[test]
    fn ef_without_cpu_columns_head_folds() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = String::from("UID PID PPID C STIME TTY TIME CMD\n");
        for i in 0..80 {
            let _ = writeln!(t, "root {i} 0 0 juin10 ? 00:00:00 [k{i}]");
        }
        let out = fold(&t, 40, "ps-ef").unwrap();
        assert!(out.contains("more) ..."));
        assert!(out.contains("CMD"));
    }

    /// `ps -e` on macOS, 49 processes, 3.3 KB: every command was cut after
    /// row 40 behind an anonymous `+9 more`. It fits, so every row ships.
    #[test]
    fn a_ps_listing_that_fits_ships_every_row_respaced() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/macos/ps_e.txt"),
        )
        .expect("la capture `ps -e` du corpus");
        let out = crate::cmd::dispatch_named("ps", &body).unwrap();
        assert!(out.starts_with("PID TTY TIME CMD\n"), "{out}");
        assert!(out.contains("\n1 ?? 8:42.48 /sbin/launchd\n"), "{out}");
        for cmd in body.lines().skip(1).filter_map(|l| l.split_whitespace().nth(3)) {
            assert!(out.contains(cmd), "{cmd} lost:\n{out}");
        }
        assert!(!out.contains("more"), "{out}");
        assert!(out.len() < body.len());
    }

    /// Whether it fits is asked of what would be DELIVERED, the respaced
    /// table — not of the padded capture, a third larger.
    #[test]
    fn fitting_is_judged_on_the_respaced_table() {
        let mut t = String::from("USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n");
        for i in 0..120 {
            t.push_str(&format!("root     {:>7}  0.0  0.0      0     0 ?        S    juin10   0:00 [kworker/{i}]\n", 100 + i));
        }
        let lines: Vec<&str> = t.lines().collect();
        let (raw, dense) = (crate::tokenizer::estimate(&t), crate::tokenizer::estimate(&respaced(&lines).unwrap()));
        assert!(dense < raw, "{dense} vs {raw}");
        let _g = tk_base::delivery::threshold_on_this_thread((raw + dense) / 2);
        let out = fold(&t, 40, "ps-aux").unwrap();
        assert!(!out.contains("more"), "{out}");
        assert!(out.contains("root 219 0.0"), "{out}");
    }

    /// The tree of `ps axjf` lives in the command's indentation.
    #[test]
    fn respacing_keeps_the_indentation_of_the_last_column() {
        let lines = [
            " PPID   PID COMMAND",
            "    1   812 /usr/sbin/sshd -D",
            "  812  4242  \\_ sshd: alice [priv]",
        ];
        assert_eq!(
            respaced(&lines).unwrap(),
            "PPID PID COMMAND\n1 812 /usr/sbin/sshd -D\n812 4242  \\_ sshd: alice [priv]"
        );
    }

    #[test]
    fn short_table_passes_through() {
        assert!(fold(&aux(3), 40, "ps-aux").is_none());
    }
}
