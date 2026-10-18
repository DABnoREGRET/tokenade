//! `table_fold` — the fold for TABLE output: keep the header and the first
//! `head` data rows, then announce the rest with a count.
//! `fold_table_priority` first lifts problem rows (Error, Failed,
//! CrashLoopBackOff, Evicted, Unknown, unmet, invalid, extraneous, missing:).
//! Returns `None` when there are `head` data rows or fewer.
//!
//! This module used to be called `cmd/linux/docker_table.rs`, and both halves of
//! that name were wrong. It knows nothing about Linux: nothing in this fold is
//! platform-specific, it groups columns and counts rows. It knows
//! nothing about Docker either: its callers are the 26 `kubectl_get_*`,
//! `helm list`, `npm list`, `cmd/spec.rs` and the primitives of
//! `archetypes` — 312 call sites in total, of which a minority are
//! `docker` commands. It was filed under the first command that needed it.
//!
//! The price was measurable: `archetypes`, which carries the fold primitives of
//! the WHOLE crate, had to reach up into `crate::cmd::linux::` to get to
//! a function that knows nothing about commands. A primitive that depends on
//! the command layer cannot be separated from it, and this was one of the edges
//! holding together the strongly connected component of 136 modules.
//!
//! Two copies existed before, under `cmd/macos/` and `cmd/windows/`, and they
//! had drifted: MEASURED on 2026-08-04, both lacked the extended set
//! of problem keywords AND the record-respecting fold, so
//! that a line followed by indented continuations was cut in two
//! on two platforms out of three. They were reduced to delegation,
//! then — here — deleted: a single module, reachable through a single path,
//! can no longer drift from itself.

use std::fmt::Write;
/// What a row's MESSAGE says when it only restates another of its own columns.
const RESTATED: &str = "=REVISION";

/// Collapse `Applied revision: <X>` when `<X>` is already elsewhere on the row.
///
/// `flux get kustomizations` prints the reconciled revision TWICE on every
/// healthy row: once in the `REVISION` column and again inside `MESSAGE` as
/// `Applied revision: <the same 40-char sha1>`. Measured 2026-09-06 on this
/// repo's captured fixture — 70 rows, complete, not truncated — that is 3 397
/// of 11 530 bytes saying nothing the row has not already said.
///
/// Index-independent on purpose: the match is "the text after
/// `Applied revision: ` also appears EARLIER on this line", so a column order
/// that differs between flux versions, or an extra `NAMESPACE` column, cannot
/// turn this into a wrong rewrite.
///
/// Returns the rewritten text and how many rows were touched. Zero rows means
/// the caller emits exactly what it always did.
pub fn fold_restated_revision(text: &str) -> (String, usize) {
    const LEAD: &str = "Applied revision: ";
    let mut n = 0usize;
    let out: Vec<String> = text
        .lines()
        .map(|l| {
            let Some(at) = l.find(LEAD) else {
                return l.to_string();
            };
            let rev = l[at + LEAD.len()..].trim_end();
            // Non-empty, and genuinely a repeat of something already on the row.
            if rev.is_empty() || !l[..at].contains(rev) {
                return l.to_string();
            }
            n += 1;
            format!("{}{LEAD}{RESTATED}", &l[..at])
        })
        .collect();
    (
        out.join("\n") + if text.ends_with('\n') { "\n" } else { "" },
        n,
    )
}

/// The sentence that makes [`fold_restated_revision`] readable, inserted after
/// a fold's banner. Nothing is lost, so this names a substitution, not an
/// elision.
pub fn restated_revision_note(rows: usize) -> String {
    format!("[tokenade] `{RESTATED}` in MESSAGE on {rows} row(s): the message only repeated that row's own REVISION")
}

/// Fold a docker table (header row + data rows).
/// Returns `None` when data rows ≤ `head`.
pub fn fold_table(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    // A MESSAGE that only repeats this row's own REVISION is collapsed before
    // anything else looks at the text. Touches a CELL, never the header — the
    // header line is a contract this module guards by name.
    let (rewritten, restated) = fold_restated_revision(text);
    let text: &str = &rewritten;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let header = lines[0];
    let data = &lines[1..];
    let total = data.len();
    if total <= head {
        return None;
    }
    let mut out = format!("[tokenade:{label}] {total} {noun}\n");
    if restated > 0 {
        out.push_str(&restated_revision_note(restated));
        out.push('\n');
    }
    out.push_str(header);
    out.push('\n');
    for l in &data[..head] {
        out.push_str(l);
        out.push('\n');
    }
    let _ = write!(out, "  ... (+{} more)", total - head);
    Some(out)
}

/// Fold a status table with problem rows sorted to the top.
///
/// Rows containing signals like "Error", "Failed", "CrashLoopBackOff",
/// "OOMKilled", "Evicted", "Unknown", "Pending", "BackOff", or "ImagePull"
/// are moved before healthy rows so an agent sees failures immediately.
/// Returns `None` when data rows ≤ `head`.
pub fn fold_table_priority(text: &str, head: usize, label: &str, noun: &str) -> Option<String> {
    // A MESSAGE that only repeats this row's own REVISION is collapsed before
    // anything else looks at the text. Touches a CELL, never the header — the
    // header line is a contract this module guards by name.
    let (rewritten, restated) = fold_restated_revision(text);
    let text: &str = &rewritten;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let header = lines[0];
    let data = &lines[1..];
    let total = data.len();
    if total <= head {
        return None;
    }

    let is_problem = |row: &str| {
        // A test that PASSED is not a problem, whatever its name says. Test
        // names are English and name what they defend, so the keyword scan
        // below reads the NAME and not the outcome: measured 2026-09-04 on a
        // 400-line `cargo nextest run` with exactly ONE failure, the banner
        // announced `98 problem(s) first`. Where the runner prints its own
        // verdict, the verdict decides — third time this scan has been fooled
        // by content that merely reads like a status (see the tar listing and
        // the disassembly below).
        if crate::compress::passing_test_verdict(row) {
            return false;
        }
        // A PATH is not a status. This hoist exists for tables with a
        // status column (`docker ps`, `kubectl get pods`), where the word
        // is the verdict on the row. In a table whose cells are file
        // names the same words are ordinary content: measured on a real
        // crate archive, `tar -tzvf` reported `10 problem(s) first` and
        // hoisted `quickjs/test262_errors.txt` to the top — a data file,
        // promoted over the listing's own tree order, which is the one
        // thing a listing is read for. Tokens carrying a separator are
        // therefore dropped before the words are looked for; a real
        // status never lives inside a path, and a genuine `ERROR
        // /var/log/app.log` keeps its verdict, which sits outside one.
        //
        // A COUNT OF ZERO says the opposite of a problem. `cargo nextest`
        // closes a fully green run with `400 tests run: 400 passed, 0 failed,
        // 0 skipped`, and the word `failed` alone promoted that line and made
        // the banner announce `1 problem(s) first` on a run where nothing went
        // wrong. A keyword whose immediately preceding token is the NUMBER
        // zero is dropped with it — the number, not merely a token that ends
        // in one: a first cut asked whether the previous token trimmed down
        // to `0`, and `pod-bad-0   0/1   CrashLoopBackOff` (a kubectl row
        // whose name ends in the index 0) then lost its verdict. The existing
        // `priority_sorts_problems_first` caught it.
        //
        // A NAME is not a status either. A word joined to others by `-`, `_`
        // or `@` is an identifier — a package, a test, a pod, a file — and a
        // problem word inside it is part of the name: a healthy `npm ls --all`
        // announced `170 problem(s) first` for `error-ex@1.3.4`,
        // `es-errors@1.3.0 deduped`, `json-parse-even-better-errors`
        // (measured 2026-10-04). A verdict stands alone — `Error`,
        // `CrashLoopBackOff`, `Init:Error`, `[ERROR]`, `level=error`.
        let toks: Vec<&str> = row
            .split_whitespace()
            .filter(|tok| !tok.contains('/') && !tok.contains('\\'))
            .filter(|tok| {
                let bare = tok.trim_matches(|c: char| "[](){}:;,.'\"!".contains(c));
                !(bare.contains('-') || bare.contains('_') || bare.contains('@'))
                    || bare.contains('=')
            })
            .collect();
        let r: String = toks
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                *i == 0
                    || toks[i - 1]
                        .trim_end_matches([',', ';', '.', ':'])
                        .parse::<u64>()
                        != Ok(0)
            })
            // The KEY of an option is not a verdict. `findmnt` prints
            // `rw,relatime,errors=remount-ro` on every ext4 and vfat mount —
            // the policy for errors that have not happened — and a real
            // 64-mount listing announced `18 problem(s) first`, hoisting 18
            // healthy mounts out of their tree (measured 2026-10-03). Of a
            // `key=value` only the VALUE can be a status: `level=error` still
            // is one.
            .map(|(_, tok)| {
                tok.split(',')
                    .map(|part| part.split_once('=').map_or(part, |(_, v)| v))
                    .collect::<Vec<_>>()
                    .join(",")
                    .to_ascii_lowercase()
            })
            .collect::<Vec<_>>()
            .join(" ");
        let r = r.as_str();
        r.contains("error") || r.contains("fail") || r.contains("crashloop")
            || r.contains("oomkill") || r.contains("evict") || r.contains("unknown")
            || r.contains("imagepull") || r.contains("backoff") || r.contains("pending")
            // Package managers say it their own way, and these are the whole
            // reason anyone reads a dependency listing.
            || r.contains("unmet") || r.contains("invalid") || r.contains("extraneous")
            || r.contains("missing:")
    };

    // Group each row with the INDENTED lines that continue it. Scoring rows
    // independently loses the detail of the very problem it promoted: in
    // `go test`, the reason a test failed is always an indented line under
    // `--- FAIL: TestX`, so the header floated to the top while
    // `engine_test.go:118: expected 42, got 41` sank among the passing
    // packages and fell off the cap. The agent learned THAT a test failed and
    // never why — from a fold whose whole purpose is to surface problems.
    //
    // A no-op on the tables this helper was written for: `docker ps` and
    // `kubectl get` rows are never indented.
    //
    // That premise is testable, and it has to be tested rather than assumed:
    // it holds only where indentation means NESTING. Where indentation is
    // column ALIGNMENT the grouping swallows an entire section into one
    // record, and every line of it inherits that record's verdict. Measured
    // 2026-09-02 on real output: `objdump -d /usr/bin/ls` indents 25 855 of
    // its 26 196 rows (98%) and collapses to 341 records, 75 of which carry a
    // word from the problem list — so the banner announced `14865 problem(s)`,
    // 57% of a disassembly, against 155 rows that contain such a word at all.
    // `nm -D` indents 116 of 130 (89%). The `go test` shape this was written
    // for indents ONE row in 302.
    //
    // So: if most rows are indented, the indentation carries no nesting
    // information and every row is its own record.
    let indented = data
        .iter()
        .filter(|r| r.starts_with(' ') || r.starts_with('\t'))
        .count();
    let indent_means_nesting = indented * 2 <= total;
    let mut records: Vec<Vec<&str>> = Vec::new();
    for row in data {
        let is_continuation =
            indent_means_nesting && (row.starts_with(' ') || row.starts_with('\t'));
        match records.last_mut() {
            Some(rec) if is_continuation => rec.push(row),
            _ => records.push(vec![*row]),
        }
    }
    // A record is a problem if ANY of its lines says so — the header usually
    // does, but a clean-looking row whose continuation carries the error
    // counts too.
    let (problem_recs, ok_recs): (Vec<_>, Vec<_>) = records
        .into_iter()
        .partition(|rec| rec.iter().any(|r| is_problem(r)));
    // RECORDS, not their lines. A record is one problem however many lines say
    // so, and summing the lines made the banner count a promoted failure's
    // explanation as a second failure.
    let n_problems: usize = problem_recs.len();

    let mut banner = format!("[tokenade:{label}] {total} {noun}");
    if n_problems > 0 {
        let _ = write!(banner, ", {n_problems} problem(s) first");
    }
    let mut out = banner;
    out.push('\n');
    if restated > 0 {
        out.push_str(&restated_revision_note(restated));
        out.push('\n');
    }
    out.push_str(header);
    out.push('\n');
    // `head` still bounds LINES, so the size contract is unchanged; a record
    // is only ever emitted whole, so a promoted problem never arrives
    // decapitated.
    let mut emitted = 0usize;
    for rec in problem_recs.iter().chain(ok_recs.iter()) {
        if emitted + rec.len() > head && emitted > 0 {
            break;
        }
        for l in rec {
            out.push_str(l);
            out.push('\n');
            emitted += 1;
        }
    }
    let _ = write!(out, "  ... (+{} more)", total.saturating_sub(emitted));
    Some(out)
}

// Required by the auto-dispatch table — this is a helper, not a command.
crate::archetypes::cmd_helper_module!();
#[cfg(test)]
include!("table_fold_tests.rs");
