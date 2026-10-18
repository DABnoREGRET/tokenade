//! HTTP/`curl -v` and generic log compaction.
//!
//! A typical API-debug session ends in 80–200 lines of `curl -v` output,
//! 60 % of which is **TLS handshake noise** (cipher negotiation, certificate
//! chain dump, ALPN selection) that no agent ever reads. The signal is the
//! request line, the request/response headers (`> ` and `< `), and the
//! response body.
//!
//! [`compact_curl_verbose`] keeps:
//!   - the first `* Trying …` and `* Connected to …` lines (so the agent
//!     knows the host + IP),
//!   - the cert *subject* line (drops the rest of the cert chain),
//!   - **every** request (`> `) and response (`< `) line — these are the
//!     contract under test,
//!   - the response body,
//!
//! and folds the run of `* TLS…` / `* ALPN…` / `*  …` lines as
//! `* [N TLS-handshake lines elided]`.
//!
//! [`compact_generic_logs`] dedupes consecutive identical lines and folds
//! runs of same-prefix lines (e.g. 1000 identical `ERROR: timeout` lines →
//! `ERROR: timeout  (×1000)`).

use crate::tokenizer;

/// Compact `curl -v` (or any tool that uses the same `*`/`>`/`<` markers).
pub fn compact_curl_verbose(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut tls_run: Vec<String> = Vec::new();
    let mut cert_run = 0usize;
    let mut kept_trying = false;
    let mut kept_connected = false;

    let flush_tls = |out: &mut Vec<String>, tls: &mut Vec<String>| {
        if tls.is_empty() {
            return;
        }
        // Keep the FIRST line of the TLS run for context ("TLSv1.3 (OUT), …"),
        // fold the rest. This preserves enough that the agent can see the
        // protocol negotiation outcome without reading all 30 handshake lines.
        let first = tls.first().cloned().unwrap_or_default();
        if tls.len() == 1 {
            out.push(first);
        } else {
            out.push(format!(
                "{first}  [+ {} TLS-handshake lines elided]",
                tls.len() - 1
            ));
        }
        tls.clear();
    };

    for raw in text.lines() {
        let line = raw.to_string();
        let trimmed = line.trim_start();

        // Recognise TLS / ALPN noise inside the `* ` channel.
        let is_tls = trimmed.starts_with("* ")
            && (trimmed.contains("TLSv1")
                || trimmed.contains("TLS handshake")
                || trimmed.starts_with("* ALPN")
                || trimmed.starts_with("* SSL")
                || trimmed.starts_with("* successfully set certificate verify locations")
                || trimmed.starts_with("* CApath")
                || trimmed.starts_with("* CAfile"));

        // Recognise certificate-chain dump (keep only `subject:` line).
        let is_cert_field = trimmed.starts_with("* ")
            && (trimmed.contains("start date:")
                || trimmed.contains("expire date:")
                || trimmed.contains("issuer:")
                || trimmed.contains("SSL certificate verify ok")
                || trimmed.contains("certificate ")
                || trimmed.contains("subjectAltName:"));

        if is_tls {
            tls_run.push(line);
            continue;
        }
        if is_cert_field {
            cert_run += 1;
            continue;
        }
        flush_tls(&mut out, &mut tls_run);
        if cert_run > 0 {
            out.push(format!("* [{} cert-chain lines elided]", cert_run));
            cert_run = 0;
        }

        // Keep first Trying / Connected only.
        if trimmed.starts_with("*   Trying") {
            if !kept_trying {
                out.push(line);
                kept_trying = true;
            }
            continue;
        }
        if trimmed.starts_with("* Connected to") {
            if !kept_connected {
                out.push(line);
                kept_connected = true;
            }
            continue;
        }

        // Drop trailing connection-cleanup chatter.
        if trimmed.starts_with("* Connection ") && trimmed.contains("left intact") {
            continue;
        }
        if trimmed.starts_with("* Closing connection") || trimmed.starts_with("* Mark bundle") {
            continue;
        }

        out.push(line);
    }
    // Tail flush in case the file ends mid-TLS-run / mid-cert.
    flush_tls(&mut out, &mut tls_run);
    if cert_run > 0 {
        out.push(format!("* [{} cert-chain lines elided]", cert_run));
    }

    out.join("\n")
}

pub fn compact_curl_verbose_with_savings(text: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let out = compact_curl_verbose(text);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

/// Compact a generic log file by folding consecutive identical lines AND
/// runs of lines that share the same prefix up to the first variable token
/// (timestamps, request IDs).
///
/// Fold rendering (Option C, 2026-05-29): a folded run is emitted as
///
/// ```text
/// <inlined_template>  [×N lines · distinct TS=10,N=7 · e.g. N=10.0.0.5, UUID=a3f1b22c…]
/// ```
///
/// The "inlined template" replaces variable tokens with `<TS>`/`<N>`/
/// `<UUID>`/`<HEX>` placeholders ONLY for slots that actually varied
/// across the run. Slots with a single distinct value across all
/// folded lines are inlined as their literal value — so the model can
/// tell `from <N>` (7 distinct IPs) from `from 10.0.0.5` (one IP, ten
/// requests). The `distinct` summary lists the unique-value count per
/// varying slot; `e.g.` gives a sample value per varying slot from
/// the first observed line, so the model has concrete grounding
/// without us emitting every value.
pub fn compact_generic_logs(text: &str) -> String {
    compact_generic_logs_internal(text, None).0
}

/// Same as [`compact_generic_logs`] but also returns one
/// [`crate::fold_store::FoldRecord`] per folded run, AND each
/// fold-output line is annotated with an `expand:
/// tokenade expand-fold <template_id>` hint so the model
/// knows how to drill in. `content_hash` is the 12-char hash of the
/// raw file bytes — the same id `dedup_ref` would emit in a
/// `§ref:HASH§` marker. Callers persist via
/// `fold_store::save_fold(content_hash, rec)`.
pub fn compact_generic_logs_indexed(
    text: &str,
    content_hash: &str,
) -> (String, Vec<crate::fold_store::FoldRecord>) {
    compact_generic_logs_internal(text, Some(content_hash))
}

/// **An id that happens to be all digits is still the id.** A request id
/// written in hex is `<HEX>` when it holds a letter and `<N>` when it does
/// not (`92276658`): one line in six of a request log broke its run, printed
/// alone between two folded groups of the SAME line shape — a 60 000-line log
/// came back as hundreds of identical templates (2026-10-03). A line whose
/// template differs from one the log already holds only by such a slot takes
/// that template, and its slot is counted as `HEX` like its neighbours'.
fn adopt_hex_templates(
    lines: &[&str],
    templates: &mut [String],
    slots: &mut [Vec<(SlotKind, String)>],
) {
    let long_digits = |v: &str| v.len() >= 8 && v.bytes().all(|b| b.is_ascii_digit());
    let present: std::collections::HashSet<String> = templates
        .iter()
        .filter(|t| t.contains("<HEX>"))
        .cloned()
        .collect();
    if present.is_empty() {
        return;
    }
    for (k, line) in lines.iter().enumerate() {
        if !slots[k]
            .iter()
            .any(|(kind, v)| *kind == SlotKind::Num && long_digits(v))
        {
            continue;
        }
        let alt = walk_line(line, |kind, v, _| {
            if kind == SlotKind::Num && long_digits(v) {
                "<HEX>".to_string()
            } else {
                kind.placeholder().to_string()
            }
        });
        if alt != templates[k] && present.contains(&alt) {
            templates[k] = alt;
            for (kind, v) in slots[k].iter_mut() {
                if *kind == SlotKind::Num && long_digits(v) {
                    *kind = SlotKind::Hex;
                }
            }
        }
    }
}

fn compact_generic_logs_internal(
    text: &str,
    content_hash: Option<&str>,
) -> (String, Vec<crate::fold_store::FoldRecord>) {
    let stripped = crate::ansi::strip_ansi(text);
    let text = stripped.as_str();
    let lines: Vec<&str> = text.lines().collect();
    let mut templates: Vec<String> = lines.iter().map(|l| template_of(l)).collect();
    let mut slot_lists: Vec<Vec<(SlotKind, String)>> = lines.iter().map(|l| slots_of(l)).collect();
    adopt_hex_templates(&lines, &mut templates, &mut slot_lists);
    let mut out: Vec<String> = Vec::new();
    // The same pass with every run COUNTED, never listed: what the regroup
    // below has to beat. Since runs list their values (2026-10-02/03), the
    // in-order pass grew while the regroup — whose large groups summarise —
    // did not, and a 500-line log was reordered for a margin made of the
    // values the in-order pass kept (`e2e_a_two_stage_fold…`, 2026-10-03).
    let mut counted: Vec<String> = Vec::new();
    let mut records: std::collections::BTreeMap<String, crate::fold_store::FoldRecord> =
        std::collections::BTreeMap::new();
    let indexed = content_hash.is_some();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && templates[j] == templates[i] {
            j += 1;
        }
        let run = j - i;
        if run == 1 {
            out.push(lines[i].to_string());
            counted.push(lines[i].to_string());
        } else {
            counted.push(format_folded_run(
                &lines[i..j],
                &slot_lists[i..j],
                None,
                false,
            ));
            let template_id = if indexed {
                Some(crate::fold_store::template_id(&templates[i]))
            } else {
                None
            };
            out.push(format_folded_run(
                &lines[i..j],
                &slot_lists[i..j],
                template_id.as_deref(),
                true,
            ));
            if let Some(tid) = template_id {
                let rec =
                    records
                        .entry(tid.clone())
                        .or_insert_with(|| crate::fold_store::FoldRecord {
                            template_id: tid.clone(),
                            template: templates[i].clone(),
                            slot_kinds: slot_lists[i]
                                .iter()
                                .map(|(k, _)| k.label().to_string())
                                .collect(),
                            raw_lines: Vec::new(),
                            slot_values: Vec::new(),
                        });
                for (line, slots) in lines[i..j].iter().zip(slot_lists[i..j].iter()) {
                    rec.raw_lines.push((*line).to_string());
                    rec.slot_values
                        .push(slots.iter().map(|(_, v)| v.clone()).collect());
                }
            }
        }
        i = j;
    }
    let primary = out.join("\n");
    // A run-based fold sees only ADJACENT repeats, and interleaving is the
    // normal shape of build output: `make -j8`, a Maven reactor, a CI matrix.
    // Measured 2026-08-26 on the same five line-shapes and the same 60
    // iterations, grouped versus interleaved:
    //
    //   grouped      k=1 0.036  k=2 0.029  k=3 0.029  k=4 0.030  k=5 0.031
    //   interleaved  k=1 0.036  k=2 1.000  k=3 1.000  k=4 1.000  k=5 1.000
    //
    // Not a weaker fold from two shapes onward: NONE. Every line has a run of
    // one and comes out verbatim.
    if let Some(regrouped) = regroup_scattered(&lines, &templates, &slot_lists, &counted.join("\n"))
    {
        return (regrouped, records.into_values().collect());
    }
    (primary, records.into_values().collect())
}

/// Fold repeats that are NOT adjacent, emitting each shape where it FIRST
/// appeared.
///
/// A competing candidate, never a replacement, on the same terms as the
/// `+fold` candidate in `filter::best_auto_candidate`: it runs only when the
/// run-based pass achieved essentially nothing, and it has to win by a clear
/// margin. Gathering scattered lines under one shape reorders what the agent
/// reads, which is a real cost — it is worth paying against no fold at all,
/// and not worth paying against a fold that already worked.
///
/// The reordering is DECLARED. An agent reading a build log has to be able to
/// tell "these happened in this order" from "these were collected for you".
/// Whether these lines are SOURCE CODE, for the one question that needs it:
/// may the fold reorder them?
///
/// Narrow on purpose, and biased toward declining, because regrouping source
/// replaces the exact bytes an agent asked for with a template. Only signals
/// that are code in every language that has them — a comment marker, a
/// declaration keyword, a statement terminator. Deliberately NOT `[INFO]`-style
/// log tags, `# ` headings or `path:line` matches: those are what
/// `stealth::looks_like_source_or_doc` counts, for a different question.
fn looks_like_source_code(lines: &[&str]) -> bool {
    let mut considered = 0usize;
    let mut code = 0usize;
    for line in lines.iter().take(200) {
        let l = line.trim_start();
        if l.is_empty() {
            continue;
        }
        considered += 1;
        // #298 — membership from `source_signals`, policy kept here. This list
        // knew `#include`, `func ` and `=> {` and not `use std::`, `pub struct`
        // or `//!`; the shared one is the union of all three that existed.
        let is_code = crate::source_signals::line_opens_source(l)
            || l.ends_with(';')
            || l.ends_with('{')
            || l.ends_with("=> {");
        if is_code {
            code += 1;
        }
    }
    // A quarter is enough. A build log has none of these; a source slice is
    // rarely under half, and the cost of declining on a log that happens to
    // print semicolons is one missed fold, while the cost of accepting a
    // source slice is bytes the agent cannot trust.
    considered >= 8 && code * 4 >= considered
}

mod template;
pub(crate) use template::*;

fn regroup_scattered(
    lines: &[&str],
    templates: &[String],
    slot_lists: &[Vec<(SlotKind, String)>],
    primary: &str,
) -> Option<String> {
    // SOURCE CODE is two interleaved shapes like any build log — a comment and
    // a statement, a docstring and a `def` — and regrouping it replaces the
    // exact bytes the agent asked for with a template. Caught 2026-08-26 by
    // `a_big_sed_slice_of_source_gets_the_code_strip`: 300 comment/statement
    // pairs folded, and `let value_7 = compute_something(i, 7);` became
    // `let value_<N1> = compute_something(i, <N2>);`.
    //
    // `Kind` cannot make this call — measured the same day, a `make -j8` log, a
    // Rust slice and a page of prose ALL detect as `Kind::Unknown`, so the
    // discriminator has to come from the CONTENT.
    //
    // `stealth::looks_like_source_or_doc` looked like the reuse, and is NOT:
    // its name says "source or doc" but its contract is "not an anti-bot
    // challenge page", so it counts `[INFO]` / `[WARN]` / `[ERROR]` as
    // POSITIVE evidence. Wiring it here made a four-shape build log decline —
    // caught by `interleaved_shapes_fold_like_grouped_ones` at k=4, the only
    // one of its cases carrying an `[INFO]` line. Reusing a helper on the
    // strength of its NAME rather than its contract.
    if looks_like_source_code(lines) {
        return None;
    }
    const MIN_LINES: usize = 12;
    // Two COST early-outs, not correctness gates. Mutation-tested 2026-08-26:
    // removing either leaves every guard green, because the margin check at the
    // bottom refuses the same inputs on its own — a payload the run pass folded
    // well, and a document that repeats no shape, both fail to beat it. They
    // are here so the common case does not build a group map to be told what a
    // length comparison already knew. The ONE load-bearing rule is the margin.
    // Tried when the run pass achieved essentially nothing — or when what it
    // achieved still does not FIT: a 20 000-line request log interleaved with
    // WARN and ERROR lines folds in order into hundreds of identical INFO
    // groups, 24 KB that delivery then cuts, and every error past the cut was
    // gone (2026-10-03). Reordering is declared; truncation is not recoverable
    // in place.
    let primary_did_nothing = primary.len() * 4 >= text_len(lines) * 3;
    if lines.len() < MIN_LINES || (!primary_did_nothing && tk_base::delivery::fits(primary)) {
        return None;
    }
    // **A record is gathered WHOLE.** When the log's lines open on a
    // timestamp, a line that does not — an exception, its `at …` frames —
    // continues the record above it. Gathered line by line, two different
    // stack traces shared their `OrderController.checkout` frame and each
    // came back with a frame missing or borrowed from the other: a trace that
    // never happened (2026-10-03, a Java service log). Records are grouped by
    // the shape of ALL their lines, and a multi-line one keeps its order.
    // Without timestamps (a journal's messages once their prefix is gone), an
    // INDENTED line continues the record, and so does the exception line that
    // closes a Python `Traceback` after its indented frames: two tracebacks
    // through the same `views.py` frame were pooled the same way, and one of
    // them lost its `total = apply_discount(cart)` line outright.
    let stamped = templates.iter().filter(|t| t.starts_with("<TS>")).count() * 2 > lines.len();
    let indented = |l: &str| l.starts_with([' ', '\t']);
    let mut records: Vec<std::ops::Range<usize>> = Vec::new();
    for i in 0..lines.len() {
        let closes_traceback = |r: &std::ops::Range<usize>| {
            lines[r.start].trim_start().starts_with("Traceback (")
                && r.end > r.start + 1
                && indented(lines[i - 1])
                && !indented(lines[i])
                && lines[i].split(':').next().is_some_and(|n| {
                    !n.is_empty()
                        && n.chars()
                            .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
                })
        };
        let continues = match records.last() {
            None => false,
            Some(_) if stamped => !templates[i].starts_with("<TS>"),
            Some(r) => indented(lines[i]) || closes_traceback(r),
        };
        match records.last_mut() {
            Some(r) if continues => r.end = i + 1,
            _ => records.push(i..i + 1),
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<std::ops::Range<usize>>> =
        Default::default();
    for r in &records {
        let k = templates[r.clone()].join("\n");
        let e = groups.entry(k.clone()).or_default();
        if e.is_empty() {
            order.push(k);
        }
        e.push(r.clone());
    }
    // Nothing repeats — this is a document, not a log. Cost early-out; see
    // the note above.
    if order.len() * 2 >= records.len() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for k in &order {
        let members = &groups[k];
        if members.len() == 1 {
            out.extend(members[0].clone().map(|i| lines[i].to_string()));
            continue;
        }
        // Position by position: the first line carries the count; a later
        // line that is the same in every member is written once, verbatim.
        for pos in 0..members[0].len() {
            let at: Vec<usize> = members.iter().map(|r| r.start + pos).collect();
            if pos > 0 && at.iter().all(|&i| lines[i] == lines[at[0]]) {
                out.push(lines[at[0]].to_string());
                continue;
            }
            let group_lines: Vec<&str> = at.iter().map(|&i| lines[i]).collect();
            let group_slots: Vec<Vec<(SlotKind, String)>> =
                at.iter().map(|&i| slot_lists[i].clone()).collect();
            out.push(format_folded_run(&group_lines, &group_slots, None, true));
        }
    }
    let body = out.join("\n");
    // Has to beat the run-based answer by a clear margin, or the reordering
    // buys nothing and costs the reader its chronology.
    if body.len() * 2 >= primary.len() {
        return None;
    }
    Some(format!(
        "[tokenade:fold] {} shape(s) gathered from throughout the output — line order is BY SHAPE, not as printed\n{body}",
        order.len()
    ))
}

fn text_len(lines: &[&str]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

/// Render one folded run with the Option C annotation. When
/// `template_id` is provided, append an `expand:
/// tokenade expand-fold <template_id>` hint so the model
/// knows how to drill back into the raw lines.
fn format_folded_run(
    group_lines: &[&str],
    group_slots: &[Vec<(SlotKind, String)>],
    template_id: Option<&str>,
    list_values: bool,
) -> String {
    use std::collections::HashSet;
    let run = group_lines.len();
    let slot_count = group_slots[0].len();
    let expand_hint = template_id
        .map(|id| format!(" · expand: tokenade expand-fold {id}"))
        .unwrap_or_default();
    // No variable tokens at all → the lines are byte-identical.
    if slot_count == 0 {
        return format!(
            "{}  [×{run} identical lines{}]",
            group_lines[0], expand_hint
        );
    }
    let mut distinct: Vec<HashSet<String>> = vec![HashSet::new(); slot_count];
    for slots in group_slots {
        for (idx, (_k, v)) in slots.iter().enumerate() {
            distinct[idx].insert(v.clone());
        }
    }
    let varying: Vec<(usize, SlotKind)> = (0..slot_count)
        .filter(|i| distinct[*i].len() > 1)
        .map(|i| (i, group_slots[0][i].0))
        .collect();
    // Slots are named by their KIND, so a line with two numbers produced
    // `distinct N=60000,N=97` — two counts, and nothing saying which is which.
    // MEASURED on a real 60 000-line log: the reader cannot tell whether it is
    // the worker id or the latency that takes 97 values, which is the only
    // question the line is read for.
    //
    // Ordinals ONLY when a kind repeats among the varying slots, so the common
    // single-slot case keeps the shorter `<N>` it has always printed. The
    // template and the summary are labelled from the same map, so `<N1>` in the
    // line and `N1=` in the counts can never drift apart.
    let mut per_kind: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    for (_, k) in &varying {
        *per_kind.entry(k.label()).or_insert(0) += 1;
    }
    let mut seen: std::collections::HashMap<&'static str, usize> = std::collections::HashMap::new();
    let mut label_of: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for (idx, k) in &varying {
        let l = k.label();
        if per_kind.get(l).copied().unwrap_or(0) > 1 {
            let n = seen.entry(l).or_insert(0);
            *n += 1;
            label_of.insert(*idx, format!("{l}{n}"));
        } else {
            label_of.insert(*idx, l.to_string());
        }
    }
    let inlined = walk_line(group_lines[0], |kind, val, idx| {
        if idx < distinct.len() && distinct[idx].len() <= 1 {
            val.to_string()
        } else {
            match label_of.get(&idx) {
                Some(l) => format!("<{l}>"),
                None => kind.placeholder().to_string(),
            }
        }
    });
    if varying.is_empty() {
        return format!(
            "{}  [×{run} identical lines{}]",
            group_lines[0], expand_hint
        );
    }
    // **One value that differs on EVERY line is the content, not noise.**
    // `coupé   orbit/<N>` ×6 (the six workflow ids disabled), `+ id=<N>` ×12,
    // `sort -r` of 400 numbered lines: measured 2026-10-02 on six weeks of
    // transcripts outside this repo, 141 of 154 folds were this shape, and the
    // `e.g.` kept ONE value of the run. Listed in order, the run is still one
    // line and every original line can be rebuilt from it. A timestamp is the
    // exception the fold exists for, and a list past the budget keeps the
    // summary — as does a list that would not be shorter than the lines.
    if let (true, &[(idx, kind)]) = (list_values, &varying[..]) {
        if kind != SlotKind::Ts && distinct[idx].len() == run {
            let values: Vec<&str> = group_slots.iter().map(|s| s[idx].1.as_str()).collect();
            // Consecutive integers say themselves in two numbers, still without
            // loss: `req 0 … req 799` is `N= 0…799`, not eight hundred values.
            let listed = range_of(&values).unwrap_or_else(|| values.join(", "));
            if listed.len() <= LISTED_VALUES_BUDGET {
                let line = format!(
                    "{inlined}  [×{run} lines · {}= {listed}{expand_hint}]",
                    label_of[&idx]
                );
                let raw: usize = group_lines.iter().map(|l| l.len() + 1).sum();
                return if line.len() < raw {
                    line
                } else {
                    group_lines.join("\n")
                };
            }
        }
    }
    // The same rule when SEVERAL slots vary together: one id the tokenizer
    // split in two (`run-docker-netns-<N><HEX>`), a clock read as two numbers
    // (`05:<N1>:<N2>`). Measured 2026-10-03 on a real `journalctl -n 60`: each
    // run of four kept one `e.g.` — three ids and three times out of four gone.
    // Listed column by column, in line order, every line rebuilds; two slots
    // that always agree (`module <N1> into out<N2>.o`) are one column.
    if list_values && varying.len() > 1 && varying.iter().all(|(_, k)| *k != SlotKind::Ts) {
        let column = |i: usize| {
            group_slots
                .iter()
                .map(|s| s[i].1.as_str())
                .collect::<Vec<_>>()
        };
        let mut columns: Vec<(Vec<&str>, Vec<&str>)> = Vec::new();
        for (i, _) in &varying {
            let values = column(*i);
            match columns.iter_mut().find(|(_, v)| *v == values) {
                Some((labels, _)) => labels.push(label_of[i].as_str()),
                None => columns.push((vec![label_of[i].as_str()], values)),
            }
        }
        // One column has to NAME each line on its own — a clock, an id. Two
        // indices that only together tell lines apart are a grid (`suite_<N1>
        // ::case_<N2>`, 30 × 12): listing it is 360 pairs where the counts
        // `N1=30,N2=12` are the answer.
        let names_each_line = columns
            .iter()
            .any(|(_, v)| v.iter().collect::<HashSet<_>>().len() == run);
        let unambiguous = columns
            .iter()
            .all(|(_, v)| v.iter().all(|x| !x.contains(',')));
        let listed: Vec<String> = columns
            .iter()
            .map(|(labels, v)| {
                format!(
                    "{}= {}",
                    labels.join("="),
                    range_of(v).unwrap_or_else(|| v.join(", "))
                )
            })
            .collect();
        let listed = listed.join(" · ");
        if unambiguous && names_each_line && listed.len() <= LISTED_VALUES_BUDGET {
            let line = format!("{inlined}  [×{run} lines · {listed}{expand_hint}]");
            let raw: usize = group_lines.iter().map(|l| l.len() + 1).sum();
            return if line.len() < raw {
                line
            } else {
                group_lines.join("\n")
            };
        }
    }
    // **A slot with a handful of values NAMES them, with their counts.**
    // `N3=3` said a request log held three statuses and not which, nor how
    // the 60 000 lines split — the first question asked of it, and the 500s
    // were nowhere (2026-10-03). Six values or fewer, each short: listed,
    // most frequent first, like a category column in a table profile.
    const NAMED_MAX: usize = 6;
    let distinct_str: Vec<String> = varying
        .iter()
        .map(|(idx, k)| {
            let d = &distinct[*idx];
            // A timestamp slot says WHEN: first and last. A gathered ERROR shape
            // read `TS=74 · e.g. TS=12:00:00.021Z` — the first occurrence only,
            // nothing on whether it was still failing at the end (2026-10-03).
            // ISO timestamps sort as text.
            if *k == SlotKind::Ts {
                if let (Some(lo), Some(hi)) = (d.iter().min(), d.iter().max()) {
                    return format!("{}={} ({lo} … {hi})", label_of[idx], d.len());
                }
            }
            if d.len() <= NAMED_MAX
                && run >= 10 * d.len()
                && d.iter().all(|v| v.chars().count() <= 32)
            {
                let mut counts: Vec<(&str, usize)> = d
                    .iter()
                    .map(|v| {
                        (
                            v.as_str(),
                            group_slots.iter().filter(|s| s[*idx].1 == *v).count(),
                        )
                    })
                    .collect();
                counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                let named: Vec<String> = counts.iter().map(|(v, n)| format!("{v} ×{n}")).collect();
                format!("{}={} ({})", label_of[idx], d.len(), named.join(" · "))
            } else {
                format!("{}={}", label_of[idx], d.len())
            }
        })
        .collect();
    let sample_str: Vec<String> = varying
        .iter()
        .map(|(idx, _k)| {
            format!(
                "{}={}",
                label_of[idx],
                shorten_for_sample(&group_slots[0][*idx].1)
            )
        })
        .collect();
    format!(
        "{inlined}  [×{run} lines · distinct {} · e.g. {}{}]",
        distinct_str.join(","),
        sample_str.join(", "),
        expand_hint
    )
}

/// `a…b` when `values` are three or more integers counting by one, up or down,
/// written without leading zeros or all at one width (`000…199`, a
/// `worker-{i:03}`) — so the range rebuilds every one of them.
fn consecutive_range(values: &[&str]) -> Option<String> {
    let width = values.first()?.len();
    let padded = values.iter().all(|v| v.len() == width);
    let nums: Vec<i64> = values
        .iter()
        .map(|v| {
            v.parse::<i64>()
                .ok()
                .filter(|n| n.to_string() == *v || (padded && !v.starts_with(['-', '+'])))
        })
        .collect::<Option<_>>()?;
    let step = nums.get(1)? - nums[0];
    let counts = nums.len() >= 3
        && (step == 1 || step == -1)
        && nums.windows(2).all(|w| w[1] - w[0] == step);
    counts.then(|| format!("{}…{}", values[0], values[values.len() - 1]))
}

/// [`consecutive_range`], or the same set listed in NAME order.
///
/// `git status`, `ls`, `find | sort` sort their lines as STRINGS, so a run of
/// `report_<N>.csv` comes out `1, 10, 100, 101, …, 99` — consecutive as a SET,
/// never in line order, and the fold listed all 300 (measured on a real
/// `git status`). When the values are exactly the integers min…max and the
/// lines are in their string order, `1…300, in name order` still rebuilds
/// every line: sort the numbers as text.
fn range_of(values: &[&str]) -> Option<String> {
    if let Some(r) = consecutive_range(values) {
        return Some(r);
    }
    if values.len() < 3
        || values.iter().any(|v| {
            v.is_empty()
                || !v.bytes().all(|b| b.is_ascii_digit())
                || (v.len() > 1 && v.starts_with('0'))
        })
    {
        return None;
    }
    if !values.windows(2).all(|w| w[0] < w[1]) {
        return None;
    }
    let mut nums: Vec<u64> = values
        .iter()
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    nums.sort_unstable();
    nums.windows(2)
        .all(|w| w[1] == w[0] + 1)
        .then(|| format!("{}…{}, in name order", nums[0], nums[nums.len() - 1]))
}

/// The most a run's listed values may cost (bytes) before the fold falls back
/// to its `distinct … e.g.` summary — about a thousand tokens.
const LISTED_VALUES_BUDGET: usize = 4_000;

fn shorten_for_sample(v: &str) -> String {
    const CAP: usize = 32;
    if v.chars().count() > CAP {
        let truncated: String = v.chars().take(CAP).collect();
        format!("{truncated}…")
    } else {
        v.to_string()
    }
}

pub fn compact_generic_logs_with_savings(text: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(text);
    let out = compact_generic_logs(text);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

#[cfg(test)]
include!("logs_tests.rs");
