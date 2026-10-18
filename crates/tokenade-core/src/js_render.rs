//! Read a page whose text only exists after its own scripts have run.
//!
//! A growing share of pages ship an empty HTML skeleton plus the real content
//! as *executable* JavaScript — a Relay payload pushed as doubly-encoded JSON,
//! a Next.js flight stream, a state object assembled at parse time. The tiered
//! extractor in [`crate::web`] sees markup and finds nothing, because at that
//! point there genuinely is nothing: the text is in code that never ran.
//!
//! This runs it. Not in a browser — in QuickJS with a shim DOM, which costs a
//! ~1 MB engine and tens of milliseconds instead of a headless Chrome. It is
//! deliberately NOT part of the normal path: [`crate::web::looks_like_spa_shell`]
//! gates it, so a page that already distilled fine never pays for it.
//!
//! What it does NOT do: fetch subresources, render a DOM tree, or run a
//! framework's client render. It executes the *inline* scripts and harvests the
//! state they leave behind. That covers preloaded-state pages and nothing else,
//! which is the honest boundary — see `experiments/cf-clearance/README.md` for
//! the corpus this was measured on.

use rquickjs::{Context, Function, Runtime};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SHIM_JS: &str = include_str!("js/shim.js");
const HARVEST_JS: &str = include_str!("js/harvest.js");
const DRIVER_JS: &str = include_str!("js/driver.js");

/// Wall-clock ceiling for one page. A hostile or pathological page can loop;
/// the interrupt handler below cuts it off rather than hanging the hook.
const BUDGET: std::time::Duration = std::time::Duration::from_millis(1500);
/// Measured rather than guessed: every page in the 63-page corpus
/// was re-run at descending limits to find the smallest that still reproduces
/// its full output. 52 of 63 need **≤ 4 MB**, 9 need ≤ 8 MB, x.com 10 MB, and
/// the worst — nuxt.com/blog, 1619 KB — needs **28 MB**. Scaled to `MAX_HTML`
/// that is ~34 MB, so 64 MB carries ~1.9x margin.
///
/// It was 192 MB, six times the worst real page, which is a large transient
/// spike to allow inside the agent's own process. Exceeding the limit fails
/// safe: `render` returns `None` and the caller keeps the generic extraction.
const MEM_LIMIT: usize = 64 * 1024 * 1024;
/// QuickJS's own recursion guard, and it has to sit **well below** the native
/// thread stack or it never fires: at 8 MB it matched the OS stack exactly, so
/// a page with deeply nested inline JSON smashed the real stack and aborted the
/// process instead of raising a catchable JS error. Measured: 20 000 nested
/// objects in a 120 KB script — comfortably inside `MAX_HTML` — crashed with
/// `thread 'main' has overflowed its stack`.
///
/// The limit is meaningful only against a stack whose size we KNOW, so the
/// engine no longer borrows the caller's — it runs on a thread this module
/// sizes itself (`ENGINE_STACK`). Expressing the guard as a byte count against
/// whatever stack happened to be underneath made its real meaning
/// platform-dependent: at 512 KB against Linux's 8 MB it fired with 15x
/// headroom, but macOS gives a secondary thread 512 KB by default — exactly the
/// limit, i.e. the process-abort failure mode this constant exists to prevent.
/// It also cost content: nesting that Linux read fine came back empty on both
/// macOS targets, because per-level frame cost differs enough between the two
/// to cross a fixed byte budget (caught by the release suite on the mac
/// builders, green on Linux and Windows).
///
/// 4 MB against a 16 MB stack keeps the same 4x headroom on every platform, and
/// is still two orders of magnitude more than any real page needs: the deepest
/// state tree in the 63-page corpus nests 50 levels.
const STACK_LIMIT: usize = 4 * 1024 * 1024;

/// The stack the JS engine gets, owned rather than inherited — see
/// `STACK_LIMIT`. One thread per render, and only for pages that already
/// reached the engine gate, so the spawn is noise next to `BUDGET`.
const ENGINE_STACK: usize = 16 * 1024 * 1024;

/// Assumed worst-case parse+harvest throughput, in microseconds per KB of HTML.
///
/// Re-derived against the whole 63-page corpus, not the four pages an
/// earlier pass happened to time. Those four gave ~180 µs/KB and a 550 figure with
/// "3x margin" — but nuxt.com/blog, never timed, ran at **763 µs/KB**. The
/// margin was not 3x, it was negative, and a 2.5 MB page would have run the
/// whole budget and returned nothing.
///
/// After the containment index (see `dedupeProse`) the corpus worst is nuxt at
/// **382 µs/KB** (1619 KB / 618 ms), and a later pass confirmed it on much bigger
/// real pages: lavenir.net 4153 KB / 1543 ms = 371, washingtonpost 298,
/// vedomosti 292, costco 154, cnbc 76.
///
/// The 2x safety multiplier this used to carry was **removed deliberately**, and
/// that is a deliberate trade rather than an oversight. The multiplier only ever
/// protected CPU: a page admitted and then interrupted returns `None` and the
/// caller keeps the generic extraction — the *same outcome* as refusing it up
/// front, just after spending the budget. Meanwhile it cost real wins. So the
/// ceiling is now the measured worst plus rounding, and `BUDGET` does the rest.
///
/// (The first figure's lesson, learned again: a ceiling is only as honest as the
/// slowest page you actually measured.)
/// **A later corpus falsified this figure, and it is left at 400 deliberately.**
/// doctolib.fr runs at **1 404 µs/KB** (1 574 KB / 2 211 ms) — 3.5x the number
/// below, which had been called the measured worst. The first figure had
/// been falsified the same way. The pattern is now established: *a throughput
/// constant derived from the corpus you have is falsified by the next corpus
/// you fetch.*
///
/// Raising it is not the fix. Measured across **2 022 pages: 3 exceed the
/// budget (0.15 %)**, and two of those are over `MAX_HTML` anyway. Exactly one
/// page in two thousand is both admissible and too slow. Sizing the ceiling for
/// it would refuse thousands of pages that finish comfortably, to save one page
/// a 1.5 s timeout whose outcome — `None`, fall back to generic — is the same
/// as refusing it.
///
/// So this constant is honestly a *typical* worst, not a true one, and `BUDGET`
/// is the real guard. `size_ceiling_is_reachable_within_the_budget` checks the
/// three constants agree with each other; nothing checks they describe the web,
/// and nothing can.
/// Consumed only by that test, which is the point: it exists to be compared
/// against the other two constants, not to be read at run time. Annotated so
/// the warning baseline stays meaningful — a permanently-unfixable warning
/// trains the reader to skip the list, and that is how a real one gets missed.
#[allow(dead_code)]
const WORST_US_PER_KB: usize = 400;

/// Size ceiling, derived from [`BUDGET`] rather than guessed: admitting a page
/// we cannot finish is the worst case going, since it runs the whole budget,
/// gets interrupted and returns nothing. Refusing up front costs microseconds.
///
/// Guarded by `size_ceiling_is_reachable_within_the_budget` — if `BUDGET` or the
/// throughput assumption moves, that test fails rather than this drifting.
/// 3.6 MB · 400 µs/KB ≈ 1.47 s, just inside the 1.5 s budget.
///
/// This was once set to 1.9 MB and justified with "the largest page the gate
/// ever fires on is youtube at 1.26 MB". **A later pass falsified that on real
/// data**: costco.com is 3.5 MB, the gate fires, the engine produces 2 344
/// chars in 556 ms — and the ceiling threw it away. Eleven of twelve
/// `JS != PROD` rows on the us/long-tail corpus were this refusal, on pages of
/// 2.2–4.2 MB that the engine finishes in 237–1175 ms.
const MAX_HTML: usize = 3_600 * 1024;

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn btoa(s: String) -> String {
    let b = s.chars().map(|c| c as u32 as u8).collect::<Vec<_>>();
    let mut o = String::new();
    for c in b.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | *c.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18 & 63) as usize] as char);
        o.push(B64[(n >> 12 & 63) as usize] as char);
        o.push(if c.len() > 1 {
            B64[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        o.push(if c.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    o
}

fn atob(s: String) -> String {
    let v: Vec<u8> = s
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
        .filter_map(|c| B64.iter().position(|x| *x == c).map(|p| p as u8))
        .collect();
    let mut o = Vec::new();
    for c in v.chunks(4) {
        let mut n = 0u32;
        for (i, x) in c.iter().enumerate() {
            n |= (*x as u32) << (18 - 6 * i);
        }
        o.push((n >> 16) as u8);
        if c.len() > 2 {
            o.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            o.push(n as u8);
        }
    }
    o.into_iter().map(|b| b as char).collect()
}

/// Deterministic PRNG. Pages seed jitter and ids from `Math.random`-alikes;
/// they only need *a* number, and a fixed stream keeps runs reproducible.
fn weak_random() -> f64 {
    use std::cell::Cell;
    thread_local!(static S: Cell<u64> = const { Cell::new(0x2545_F491_4F6C_DD1D) });
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// Execute `html`'s inline scripts and return the prose they materialise.
///
/// Returns `None` when the engine fails, times out, or the page had nothing to
/// give — every one of which means "fall back to what the extractor produced".
/// True if the page has at least one executable `<script>` — one whose `type`
/// is absent or a JS type. Scripts typed `application/json`, `ld+json`,
/// `importmap` (or `application/schema`) are inert data the engine can't run.
fn has_executable_script(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<script") {
        let start = pos + i;
        let tag_end = match lower[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let tag = &lower[start..tag_end];
        let inert = tag.contains("type")
            && (tag.contains("application/json")
                || tag.contains("ld+json")
                || tag.contains("importmap")
                || tag.contains("application/schema"));
        if !inert {
            return true;
        }
        pos = tag_end + 1;
    }
    false
}

pub fn render(html: &str, url: &str) -> Option<String> {
    // Cheap gates run on the CALLER's thread: whatever they reject costs no
    // spawn at all (`MAX_HTML` and the two classifiers reject the vast majority
    // of pages that get this far).
    if html.len() > MAX_HTML {
        return None;
    }
    // A bot wall trips the shell heuristic for the right reason — it really
    // does distil to nothing — but it is not a shell: there is no hidden
    // content to run out of it, only a CAPTCHA widget. Spending the engine
    // budget here buys nothing, and `stealth` already routes the caller.
    if crate::stealth::detect_and_remediate(html).is_some() {
        return None;
    }

    // No executable JS → nothing for the engine to run. A page whose only
    // scripts are inert data blocks (`application/json`, `ld+json`, `importmap`)
    // keeps its state where the embedded-state miners already read it; spinning
    // up QuickJS buys nothing (crw's data-vs-executable-script classifier).
    if !has_executable_script(html) {
        return None;
    }

    // Own the stack, so `STACK_LIMIT` means the same thing on every platform.
    // The runtime is created INSIDE the thread on purpose: QuickJS measures
    // depth against the stack pointer it sees at construction.
    let (owned_html, owned_url) = (html.to_string(), url.to_string());
    std::thread::Builder::new()
        .name("tokenade-js".to_string())
        .stack_size(ENGINE_STACK)
        .spawn(move || render_on_engine_thread(&owned_html, &owned_url))
        .ok()?
        .join()
        .ok()?
}

fn render_on_engine_thread(html: &str, url: &str) -> Option<String> {
    let rt = Runtime::new().ok()?;
    rt.set_memory_limit(MEM_LIMIT);
    rt.set_max_stack_size(STACK_LIMIT);

    let deadline = Instant::now() + BUDGET;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

    let ctx = Context::full(&rt).ok()?;

    let out: Option<String> = ctx.with(|ctx| {
        let g = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone()).ok()?;
        host.set(
            "now",
            Function::new(ctx.clone(), || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(0.0)
            })
            .ok()?,
        )
        .ok()?;
        host.set("random", Function::new(ctx.clone(), weak_random).ok()?)
            .ok()?;
        host.set("btoa", Function::new(ctx.clone(), btoa).ok()?)
            .ok()?;
        host.set("atob", Function::new(ctx.clone(), atob).ok()?)
            .ok()?;
        g.set("__host", host).ok()?;

        for src in [SHIM_JS, HARVEST_JS, DRIVER_JS] {
            ctx.eval::<(), _>(src.as_bytes()).ok()?;
        }
        let read_page: Function = g.get("readPage").ok()?;
        read_page.call((html, url)).ok()
    });

    let text = extract_text(&out?);
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Pull `text` out of the driver's JSON result. Hand-rolled rather than pulling
/// serde into this path: the shape is fixed and one field deep.
fn extract_text(json: &str) -> String {
    let Some(i) = json.find("\"text\":\"") else {
        return String::new();
    };
    let rest = &json[i + 8..];
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => break, // closing quote of the value
            '\\' => match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => {}
                Some('u') => {
                    let h: String = it.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(x) => out.push(x),
                None => break,
            },
            _ => out.push(c),
        }
    }
    out
}

/// Label JS-harvested text as what it is, before serving it.
///
/// This matters more than a provenance nicety: the engine harvests the page's
/// **state**, not its rendered DOM. The text can be partial (anything fetched
/// by XHR after load is absent), reordered relative to the visual page, and
/// mixed with strings the page would never display. An agent that knows this
/// can ask for the markup instead; an agent told nothing treats it as "the
/// page".
///
/// A function rather than a literal because it was needed in two places and
/// only ever written in one: the WebFetch hook prefixed it, `cmd_web` served
/// the bare harvest, so `tokenade web` on a SPA disclosed nothing. The
/// rule — the second time a value is needed at a second call site, it stops
/// being a literal.
pub fn label_as_js_harvest(url: &str, js_text: &str) -> String {
    // `https://example.invalid/` is the placeholder `location` handed to the
    // JS shim when the input is a FILE, not a URL — a page's own scripts may
    // read `location.href`, so the engine needs something. It must never be
    // shown: on a saved copy of nhs.uk/conditions/type-2-diabetes the output
    // opened with `> source: https://example.invalid/`, naming a source that
    // does not exist for content that is perfectly real.
    let provenance = if url.is_empty() || url.starts_with("https://example.invalid") {
        String::from("> ")
    } else {
        format!("> source: {url} ")
    };
    format!(
        "{provenance}(this page's markup distilled to nothing; the text below \
         was produced by running the page's own inline scripts)\n\n{js_text}"
    )
}

/// Should the JS engine even be tried, and is its output worth serving?
///
/// The gate is deliberately asymmetric: the engine only runs on a page the
/// extractor already gave up on, and its output only wins if it is decisively
/// bigger. A marginal gain is not worth serving text harvested from a state
/// blob over text extracted from markup — the markup version has structure.
pub fn beats_extraction(js_text: &str, extracted: &str) -> bool {
    js_text.len() > extracted.len().max(200) * 2
}
#[cfg(test)]
include!("js_render_tests.rs");
