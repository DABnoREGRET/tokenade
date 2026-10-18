//! Several URLs or queries in ONE call.
//!
//! The point is the round-trip: N targets fetched concurrently pay one
//! context re-send instead of N. The cache saving that buys is credited
//! here, once, for both the web and search channels.

mod render;
mod search;
pub use render::*;
pub use search::*;

use super::*;

/// `tokenade serp <file|->` — compact a Google/Bing/DuckDuckGo SERP HTML page
/// into a deduped top-N list of `title / url / snippet`. Falls back to
/// `tokenade web` for non-SERP HTML so the caller doesn't need to detect.
/// `tokenade search "<query>"` — the CLI half of web search.
///
/// The WebSearch hook and this command run the SAME `tokenade_core::search`:
/// same engines, same breaker (same state file, so a wall one channel hits is a
/// wall the other already knows about), same validation, same ranking. The only
/// thing the hook has that this does not is a session, and therefore
/// cross-search dedup.
///
/// This exists because half the supported agents cannot be hooked — Codex,
/// Cursor, Grok, Droid and Aider reach tokenade through the shell alone — and
/// until now that meant they had NO search at all, while the hooked agents had
/// a five-engine union. A capability reachable from one channel of two is the
/// exact defect this refactor set out to remove.
pub(super) use tokenade_core::gain::batching_cache_saved;

/// The sizes that count toward a batch's cache saving: only the results the
/// agent can actually use.
///
/// The saving is "you paid ONE context re-send instead of N". A query that came
/// back with nothing usable bought no re-send — the agent will ask again — so
/// counting it credits work that did not happen.
///
/// The web batch already had this rule by construction: it pushes a size only
/// for a page that rendered, and a batch where every fetch failed books
/// nothing. The search batch pushed EVERY result, usable or not, and the two
/// halves of one feature disagreed. Measured 2026-08-28 on this machine's own
/// ledger: **603 search batches booked 1 056 089 tokens of cache saving while
/// the measured ledger holds ZERO `web-search` rows** — not one of those
/// batches ever produced a usable result, and every one of them was credited.
pub(super) fn usable_sizes<I: IntoIterator<Item = (usize, bool)>>(items: I) -> Vec<usize> {
    items
        .into_iter()
        .filter_map(|(sz, usable)| usable.then_some(sz))
        .collect()
}

/// Book a web/search batch's avoided re-sends (see [`batching_cache_saved`]).
pub(super) fn record_batch_cache_saving(source: &str, sizes: &[usize]) {
    tokenade_core::gain::record_batch_cache_saving(source, "batch", sizes);
}

/// Fetch several web targets CONCURRENTLY in one invocation (D11).
///
/// Routes each target through the SAME shared bricks the single-target path uses
/// — the site readers, `web_fetch::fetch`, `classify_body`, the docs extractor,
/// `distill_page` — so there is no forked pipeline. The one refinement it omits
/// is the prompt-scoped extract, which needs a question the batch has not been
/// given.
/// Run `each` over `items` at most `wave` at a time, and collect what came back
/// in the INPUT's order.
///
/// The wave is a cap on how many run at once, never a grouping of the results:
/// a caller reading the returned list cannot tell where one wave ended. Pass
/// `items.len()` for no cap, which is what the two page-fetching batches want —
/// nothing is shared between two page fetches. `tokenade search` is the one
/// that must cap: its queries contend for a fixed pool of engine locks and the
/// losers are refused outright.
///
/// A `wave` of 0 would divide the batch into no waves at all and drop every
/// item, so it floors at 1.
///
/// A panicking item is skipped, exactly as the three hand-rolled spawn/join
/// loops this replaces did with `h.join().ok()`.
fn in_waves<T, R>(items: &[T], wave: usize, each: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let wave = wave.max(1);
    let mut out = Vec::with_capacity(items.len());
    for chunk in items.chunks(wave) {
        let done: Vec<R> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk.iter().map(|it| sc.spawn(|| each(it))).collect();
            hs.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        out.extend(done);
    }
    out
}

pub(super) fn cmd_web_batch(
    targets: &[&str],
    max_input_tokens: usize,
    prompt: Option<&str>,
) -> i32 {
    // D13: the agent batched — reset the sequential-call counter.
    nudge_sequential_web_calls("cli-web", "fetch", true);
    // No cap: two page fetches share nothing, so every target goes at once.
    let rendered: Vec<(String, Result<render::Rendered, String>)> =
        in_waves(targets, targets.len(), |t| {
            (
                t.to_string(),
                web_batch_render_one(t, max_input_tokens, prompt),
            )
        });
    let mut code = 0;
    // Per-result output sizes, in emission order — the inputs to the batch
    // cache-saving figure (Σ sᵢ·(N-1-i)).
    let mut sizes: Vec<usize> = Vec::new();
    // The size gate is per PAGE, and the answer is the batch. Measured
    // 2026-09-06 on one content split two ways: as a single page it came back
    // 4 641 bytes, folded, stashed and pointed at; as eight pages it came back
    // 23 152 bytes with no fold, no stash and no pointer — five times the
    // tokens for the same information, decided by how the agent packaged the
    // request. Assemble first, then let the whole answer meet the same bar.
    let mut assembled = String::new();
    // Whether any page came back carrying a pointer — a replay fold, or its own
    // stash. Tracked as a FACT rather
    // than sniffed out of the text below: the note opens `[tokenade]` and the
    // string test there looks for `[tokenade:`, so a batch whose pages had all
    // folded would have read as "nothing is pointed at" and been summarised —
    // cutting the pointers out of the middle, which is the very thing that test
    // exists to prevent.
    let mut any_pointed = false;
    // The targets actually emitted, in order — to say afterwards which ones
    // the summary left out.
    let mut emitted: Vec<(String, String)> = Vec::new();
    for (t, out) in rendered {
        match out {
            Ok(r) => {
                // A page this session already delivered folds to a pointer
                // here too. It did not before: the batch neither folded nor
                // REMEMBERED, so a second `web u1 u2` came back byte-identical
                // and a single fetch after a batch came back whole as well.
                // One spelling, shared with the single-target path.
                let (md, op, booked) = match super::replay_fold_or_remember(&t, &r.md) {
                    Some((note, saved)) => {
                        any_pointed = true;
                        // A page is credited from its basis, on the pointer.
                        let saved = match r.basis {
                            Some(
                                b @ (tokenade_core::web_credit::WebBasis::Page { .. }
                                | tokenade_core::web_credit::WebBasis::Excerpt { .. }),
                            ) => tokenade_core::web_credit::credit_on_delivery(
                                b,
                                tokenade_core::tokenizer::estimate(&note),
                            ),
                            _ => saved,
                        };
                        (note, "web-replay", saved)
                    }
                    None => {
                        // The page's own stash, reported by the stash itself.
                        any_pointed |= r.pointed;
                        (r.md, r.op, r.booked)
                    }
                };
                // AFTER the fold: the batch cache figure prices what was
                // actually emitted, not the page a pointer replaced.
                sizes.push(tokenade_core::tokenizer::estimate(&md));
                // **The boundary between two pages is a HEADING.**
                //
                // It used to be written `===== {t} =====`, a line nothing
                // reads back: the batch is summarised as ONE document, the
                // outline keeps only headings, and the boundaries vanished
                // along with the target names. MEASURED 2026-09-09 on four
                // local pages: the delivered outline chained the BBC's
                // headings then aljazeera's without a line saying where one
                // ended, and **two of the four requested pages did not appear
                // at all**. A markdown heading survives the outline and makes
                // the question "is this target there?" exactly decidable.
                emitted.push((t.clone(), md.clone()));
                assembled.push_str(&format!("\n# {t}\n{md}"));
                // ONE ROW PER PAGE, through the SAME assiette the single-target
                // path books — so what a fetch is worth does not depend on how
                // the agent packaged it.
                //
                // This used to book ONE flat `credit_binary(total)` for the
                // whole batch: the document turn tax (800 tokens), for
                // ordinary web pages, once however many were fetched. The
                // single path books the per-agent WEB model, worth 7 057
                // tokens on the average of this machine's 204 measured pages —
                // so a three-page batch reported ~26x less than the same three
                // pages fetched one at a time, on the very call shape this
                // product's own guidance tells agents to prefer ("several URLs
                // in ONE call").
                log_gain("web", op, booked);
            }
            Err(why) => {
                eprintln!("tokenade web: {t}: {why}");
                code = 1;
            }
        }
    }
    // Only when NO part already carries one. A per-page stash means the gate
    // did its job there, and summarising the assembly would cut those pointers
    // out of the middle — the cut-pointer defect, one level up.
    //
    // Decided ENTIRELY on what the renders reported. This used to also search
    // the assembled text for `[stashed:` and `[tokenade:`, which is a claim the
    // CONTENT can make: measured 2026-09-06 on twelve local pages, putting the
    // literal `[tokenade:` in ONE page's body took the answer from 1 875 bytes,
    // folded and pointed at, to 18 997 with no fold at all — a fetched page
    // deciding whether this tool folds its own output.
    let already_pointed = any_pointed;
    let out = if already_pointed {
        assembled
    } else {
        // **The summary budget is SHARED between the pages.**
        //
        // The batch is a single document and its outline is cut at the head
        // and the tail: on four pages, the two in the MIDDLE vanished
        // entirely. The total budget does not change — each page gets its
        // share, so none can eat the others'.
        let body = fair_share_body(&emitted);
        let names: Vec<String> = emitted.iter().map(|(t, _)| t.clone()).collect();
        let summary = tokenade_core::disclose::maybe_stash_with_body(
            assembled,
            tokenade_core::disclose::SourceHint::web(),
            body,
        );
        name_the_pages_the_summary_left_out(summary, &names)
    };
    println!("{out}");
    // The cache-read tokens this ONE call saved versus N sequential fetches.
    record_batch_cache_saving("web", &sizes);
    code
}

#[cfg(test)]
include!("web_batch_tests.rs");
