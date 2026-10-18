//! **`tokenade query`: where this symbol is, and who calls it.**
//!
//! The hard part is not finding, it is ATTRIBUTING: a name can be declared in
//! several places, and returning the wrong declaration sends the agent off to
//! edit a namesake. When only one declaration exists, we say so; when several
//! exist, we do not choose for the agent.

use super::*;

/// The file of the ONE declaration matching `name`, or `None` when there are
/// several (or none).
///
/// Extracted so it can be falsified. Left inline, the mutation "never compute
/// it" survived every test: the callers-line tests call `callers_line`
/// directly and never enter `cmd_query`, so the two lines that decide WHICH
/// question gets asked were the untested part of the fix. Same lesson as the
/// clause deleted for being unreachable from one consumer — a behaviour is
/// only guarded where a test can actually reach it.
/// The spelling the index actually holds for this query.
///
/// `find_in` matches case-INSENSITIVELY, so `query run` can be answered
/// entirely by rows named `Run`. Everything downstream compares the name
/// EXACTLY — the declaration count, the sole-declaration test, and the call
/// graph — so asking them for `run` on a tree that spells it `Run` returns
/// nothing at all, and the callers line simply disappears. Measured
/// 2026-09-07 on a six-file Go tree: `query Run` answered `called by: Caller
/// (main.go:6), Other (main.go:10) [6 declarations share it]` while `query
/// run` listed the same six declarations and said nothing about callers. On a
/// command whose help promises "where is this defined, and who calls it",
/// that silence reads as "nothing calls it" — the very thing the
/// `graph_available` branch below exists to refuse.
///
/// Only a SINGLE spelling resolves. If the tree really holds both `Run` and
/// `run` they are different symbols, and the query stays exact rather than
/// picking one.
pub(crate) fn resolved_spelling<'a>(
    hits: &[&'a tokenade_core::index::Symbol],
    q: &'a str,
) -> &'a str {
    let lq = q.to_lowercase();
    let mut found: Option<&str> = None;
    for s in hits {
        if s.name.to_lowercase() != lq {
            continue;
        }
        match found {
            None => found = Some(s.name.as_str()),
            Some(seen) if seen == s.name => {}
            Some(_) => return q,
        }
    }
    found.unwrap_or(q)
}

/// Everything the callers line needs to be about the right symbol: the name
/// the index holds, how many declarations carry it, and the single file to
/// pin attribution to when there is exactly one. One unit, because the three
/// have to agree — a count taken for one spelling beside a file taken for
/// another is how the caveat and the attribution end up describing different
/// symbols.
#[derive(Debug, PartialEq)]
pub(crate) struct Attribution<'a> {
    /// The spelling the INDEX holds, never the one that was typed.
    pub(crate) name: &'a str,
    pub(crate) declarations: usize,
    pub(crate) decl_file: Option<&'a str>,
}

/// Both renderers below take this and nothing else, so neither can be handed a
/// raw query again. The text half was fixed first and the `--json` half kept
/// the typed name for one iteration — `query run --json` answered
/// `"callers": []` beside `"count": 6`, an empty array being this module's own
/// documented way of saying "nothing calls this". Making the argument a type
/// only `attribution_for` builds is what closes the class rather than the
/// second instance of it.
pub(crate) fn attribution_for<'a>(
    idx: &tokenade_core::index::Index,
    hits: &[&'a tokenade_core::index::Symbol],
    q: &'a str,
    scope: &[String],
) -> Attribution<'a> {
    let name = resolved_spelling(hits, q);
    Attribution {
        name,
        declarations: declarations_for(idx, hits, name, scope),
        decl_file: sole_declaration_file(hits, name),
    }
}

/// How many declarations the ambiguity caveat should name.
///
/// Over the INDEX, never over `hits`. `hits` is what `find_in` chose to SHOW
/// — capped — so counting exact matches in it reports the display window as
/// the ambiguity, and the caveat exists to say how far the caller may trust
/// the callers line. Measured 2026-09-07 on tokenade's own tree: `query
/// try_compact` printed "30 of 972 matches shown" and then "30 declarations
/// share it" on a name 927 symbols declare.
pub(crate) fn declarations_for(
    idx: &tokenade_core::index::Index,
    hits: &[&tokenade_core::index::Symbol],
    name: &str,
    scope: &[String],
) -> usize {
    let _ = hits;
    idx.count_exact_declarations(name, scope)
}

pub(crate) fn sole_declaration_file<'a>(
    hits: &[&'a tokenade_core::index::Symbol],
    name: &str,
) -> Option<&'a str> {
    let mut exact = hits.iter().filter(|s| s.name == name);
    match (exact.next(), exact.next()) {
        (Some(s), None) => Some(s.file.as_str()),
        _ => None,
    }
}

/// How many callers `query` names before it falls back to a count.
///
/// The call graph is name-keyed, so this answers "who calls this NAME", not
/// "who calls this particular declaration" — stated in the line itself when
/// the name has several declarations, because the honest answer to a question
/// the data cannot separate is to say so, not to pick one.
pub(crate) const QUERY_CALLERS_SHOWN: usize = 5;

/// The `callers` / `callers_total` pair for `--json`.
///
/// `null`, not `[]`, when the graph could not be read: an empty array is the
/// answer "nothing calls this", and a consumer cannot tell that from "we could
/// not look". Mined from `entroly` (2026-08-28) — an absent scan and a clean
/// scan must not serialise to the same thing.
pub(crate) fn callers_json(
    idx: &tokenade_core::index::Index,
    at: &Attribution<'_>,
    graph_available: bool,
) -> (serde_json::Value, serde_json::Value) {
    if !graph_available {
        return (serde_json::Value::Null, serde_json::Value::Null);
    }
    let (callers, total) = match at.decl_file {
        Some(f) => idx.callers_of_declaration(at.name, f, QUERY_CALLERS_SHOWN),
        None => idx.callers_with_total(at.name, QUERY_CALLERS_SHOWN),
    };
    let arr: Vec<serde_json::Value> = callers
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.caller_name,
                "file": c.file,
                "line": c.line,
            })
        })
        .collect();
    (
        serde_json::Value::Array(arr),
        serde_json::Value::from(total),
    )
}

/// `called by: …` — the half of `tokenade query --help`'s own promise
/// ("where is this defined, and who calls it") that was never rendered.
///
/// The call graph has been built, stored, loaded and parsed on every command
/// since it was added — 39.7 MB of a 51.0 MB index on a 3 980-file repository,
/// 60 % of the time spent parsing it — and `Index::callers` had no caller in
/// production at all.
pub(crate) fn callers_line(
    idx: &tokenade_core::index::Index,
    at: &Attribution<'_>,
    graph_available: bool,
) -> String {
    // Silence here means "nothing calls it". When the graph could not be read
    // that is a claim about the code made from an absence of data, so the
    // absence is what gets printed instead.
    if !graph_available {
        return "called by: unknown — the call graph could not be read, so this is \
                NOT \"nothing calls it\"\n"
            .to_string();
    }
    // ONE declaration means the caveat below is not printed and these read as
    // fact, so they have to BE fact — see `Index::callers_of_declaration`.
    let (hits, total) = match at.decl_file {
        Some(f) => idx.callers_of_declaration(at.name, f, QUERY_CALLERS_SHOWN),
        None => idx.callers_with_total(at.name, QUERY_CALLERS_SHOWN),
    };
    if hits.is_empty() {
        return String::new();
    }
    let named: Vec<String> = hits
        .iter()
        .map(|c| format!("{} ({}:{})", c.caller_name, c.file, c.line))
        .collect();
    let more = total.saturating_sub(hits.len());
    let mut line = format!("called by: {}", named.join(", "));
    if more > 0 {
        line.push_str(&format!(" +{more} more"));
    }
    if at.declarations > 1 {
        let declarations = at.declarations;
        line.push_str(&format!(
            " [by NAME — {declarations} declarations share it, the call graph cannot tell them apart]"
        ));
    }
    line.push('\n');
    line
}
