//! Prose mined out of a page's structured data.
//!
//! When the rendered body is a JS shell, the readable text is often still
//! there as JSON-LD (`schema.org` article bodies) or as strings in a
//! rehydration blob. These walk that JSON and hand back the prose.

/// essence's 32 MB-stack parse guard (MIT — see module doc), adapted:
/// tokenade's HTML→markdown conversion is regex/string-based, not a
/// recursive DOM walk, so the case essence names (Amazon product pages
/// nesting 100+ `<div>` levels, overflowing an 8 MB thread stack) does not
/// apply to `html_to_markdown` itself. The real analog in THIS file is
/// [`mine_jsonld`]/[`mine_prose_strings`], which DO recurse — over
/// `serde_json::Value`, unbounded — while mining `__NEXT_DATA__`/JSON-LD
/// state off exactly the kind of JS-framework page essence is worried
/// about, and a Redux/Apollo cache dump can nest deep enough to overflow a
/// default stack for real (`serde_json::from_str::<Value>` recurses too).
///
/// Below 500 KB of JSON text, parse and mine on the current thread. Above,
/// run both on a thread with a 32 MB stack and join it; a stack overflow
/// aborts the thread (Rust cannot `catch_unwind` past one) but the PROCESS
/// survives, and the caller gets nothing mined rather than nothing served.
pub(super) fn parse_and_mine(
    text: String,
    mine: fn(&serde_json::Value, &mut dyn FnMut(&str, &str)),
) -> Vec<(String, String)> {
    const BIG_JSON_BYTES: usize = 500 * 1024;
    const GUARD_STACK_BYTES: usize = 32 * 1024 * 1024;
    let is_big = text.len() >= BIG_JSON_BYTES;
    let run = move || -> Vec<(String, String)> {
        let mut mined = Vec::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            mine(&v, &mut |key: &str, s: &str| {
                mined.push((key.to_string(), s.to_string()))
            });
        }
        mined
    };
    if !is_big {
        return run();
    }
    match std::thread::Builder::new()
        .stack_size(GUARD_STACK_BYTES)
        .spawn(run)
    {
        Ok(handle) => handle.join().unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Walk a JSON-LD value, yielding the schema.org fields that carry copy.
pub(super) fn mine_jsonld(v: &serde_json::Value, sink: &mut dyn FnMut(&str, &str)) {
    match v {
        serde_json::Value::Object(m) => {
            for key in ["name", "headline", "description", "articleBody", "text"] {
                if let Some(serde_json::Value::String(s)) = m.get(key) {
                    // The FIELD is returned with the value: `headline` is the
                    // article's title, and the caller could not tell it apart
                    // from the body once the string stood alone.
                    sink(key, s);
                }
            }
            for val in m.values() {
                mine_jsonld(val, sink);
            }
        }
        serde_json::Value::Array(a) => {
            for val in a {
                mine_jsonld(val, sink);
            }
        }
        _ => {}
    }
}

/// Walk arbitrary JSON, yielding string values that read as prose: long
/// enough, contain spaces, not URLs/paths/identifiers.
pub(super) fn mine_prose_strings(v: &serde_json::Value, sink: &mut dyn FnMut(&str, &str)) {
    match v {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.chars().count() >= 60
                && t.contains(' ')
                && !t.starts_with("http")
                && !t.starts_with('/')
                && !t.starts_with('{')
                // `flag_a=v1; flag_b=v1; …` is configuration, not text (the
                // feature flags of the Gumroad state).
                && t.split_whitespace().filter(|w| w.contains('=')).count() * 2 < t.split_whitespace().count()
            {
                // A rehydration blob has no named field: nothing to say about
                // the ROLE of this string.
                sink("", t);
            }
        }
        serde_json::Value::Object(m) => {
            for val in m.values() {
                mine_prose_strings(val, sink);
            }
        }
        serde_json::Value::Array(a) => {
            for val in a {
                mine_prose_strings(val, sink);
            }
        }
        _ => {}
    }
}

/// **A page state also carries LISTS, not only prose.**
///
/// `mine_prose_strings` keeps only strings of at least 60 characters: a
/// catalogue has none. Gumroad's "Discover" page (Inertia.js, the whole state
/// in `data-page`) lists 36 products named like `PES Master Plus`, and
/// returned 0 characters out of 116 KB (2026-10-04, field survey). An array of
/// at least `MIN_ITEMS` objects that almost all have a NAME (`name`/`title`)
/// and an http(s) ADDRESS (`url`/`href`) is a list of links: it comes out as
/// such, and its objects are not searched a second time.
pub(super) fn mine_page_state(v: &serde_json::Value, sink: &mut dyn FnMut(&str, &str)) {
    const MIN_ITEMS: usize = 3;
    const MAX_ITEMS: usize = 50;
    match v {
        serde_json::Value::Array(a) => {
            let link = |o: &serde_json::Value| -> Option<(String, String)> {
                let field =
                    |keys: &[&str]| keys.iter().find_map(|k| o.get(*k)?.as_str()).map(str::trim);
                let name = field(&["name", "title"]).filter(|n| !n.is_empty())?;
                let url = field(&["url", "href"])
                    .filter(|u| u.starts_with("http://") || u.starts_with("https://"))?;
                Some((name.to_string(), url.to_string()))
            };
            let links: Vec<(String, String)> = a.iter().filter_map(link).collect();
            if links.len() >= MIN_ITEMS && links.len() * 5 >= a.len() * 4 {
                let list: Vec<String> = links
                    .iter()
                    .take(MAX_ITEMS)
                    .map(|(n, u)| format!("- [{}]({u})", n.replace(['[', ']'], "")))
                    .collect();
                sink("list", &list.join("\n"));
                return;
            }
            for val in a {
                mine_page_state(val, sink);
            }
        }
        serde_json::Value::Object(m) => {
            for val in m.values() {
                mine_page_state(val, sink);
            }
        }
        other => mine_prose_strings(other, sink),
    }
}
