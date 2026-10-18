//! The route table a `next build` prints — reached through whatever script
//! actually ran it (`pnpm build`, `npm run build`, `yarn build`, `bun run
//! build`), not through the literal argv `next build` that nobody types.
//!
//! MEASURED in production (13 runs, 2026-07-14, one real site): `pnpm build`
//! handed back **26 135 tokens** with a **9.7 %** cut — the generic
//! `npm run` fold only collapses runs of *byte-identical consecutive* lines,
//! and a route table has none: every line carries a different path. tokenade's
//! own nudge had already flagged it (`'pnpm' has run 5× recently with only
//! generic compaction`).
//!
//! There *is* a built-in `next_build` spec, and it would not have helped: it
//! strips the `info  - ` prefix Next.js stopped emitting at v13, and it is
//! keyed off an argv (`next build`) that a package.json script hides.
//!
//! ## What the table looks like, and where the tokens are
//!
//! ```text
//! Route (app)                        Size     First Load JS
//! ┌ ○ /                              5.2 kB   123 kB
//! ├ ƒ /savings/[slug]                1.06 kB  193 kB
//! ├ ○ /comparisons/a-vs-b            279 B    106 kB
//! ├ ○ /comparisons/c-vs-d            279 B    106 kB
//! …one line per prerendered page, a thousand of them on a content site…
//! └ ƒ /welcome                       5.65 kB  216 kB
//! + First Load JS shared by all      192 kB
//! ```
//!
//! On a site that generates its pages from content, most of those lines report
//! **the same two sizes**. The path differs; the measurement does not. So the
//! fold groups routes by `(size, first-load, kind)` and, for any group big
//! enough to matter, emits ONE line carrying the count, the shared sizes and
//! the common path prefix. Every *distinct* measurement still gets its own
//! line — which is what a build table is read for.
//!
//! Routes are alphabetical, so same-size routes are usually NOT adjacent; the
//! grouping is table-wide and each group is emitted at the position of its
//! first member, so the surviving lines stay in the order Next printed them.
//!
//! Nothing outside the route rows is touched: the header, the shared-chunk
//! breakdown, `ƒ Middleware` and the legend all come through verbatim. The
//! full text stays recoverable through the usual stash reference.

/// The three box-drawing characters Next.js opens a route row with.
const BOX: [char; 3] = ['┌', '├', '└'];
/// Route kind markers: static, SSG, dynamic, and the older lambda marker.
const KIND: [char; 4] = ['○', '●', 'ƒ', 'λ'];
/// Below this, folding a group costs more than it saves and hides paths for
/// nothing. Four rows collapse to one line plus a prefix that names them all.
const MIN_GROUP: usize = 4;

/// A parsed route row: `(kind, path, size, first_load)`. `None` for any line
/// that is not a route row — including the `  ├ chunks/…` shared-chunk rows,
/// which are indented and carry only ONE size column.
fn split_route(line: &str) -> Option<(char, &str, &str, &str)> {
    let mut chars = line.chars();
    if !BOX.contains(&chars.next()?) {
        return None;
    }
    let rest = chars.as_str().trim_start();
    let mut chars = rest.chars();
    let kind = chars.next()?;
    if !KIND.contains(&kind) {
        return None;
    }
    // Columns are separated by runs of 2+ spaces; `279 B` keeps its single
    // inner space, and a URL path never contains two in a row.
    let cols: Vec<&str> = chars
        .as_str()
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    match cols[..] {
        [path, size, first] if !path.is_empty() => Some((kind, path, size, first)),
        _ => None,
    }
}

/// Longest shared prefix of `paths`, cut back to the last `/` so the result is
/// a real route prefix and not half a slug. `/` when they share nothing.
fn common_prefix(paths: &[&str]) -> String {
    let Some(first) = paths.first() else {
        return "/".to_string();
    };
    // The shared DIRECTORY, so the last segment never counts: two routes
    // that are the same route share their whole selves, and the label for
    // that group is still the directory above. The byte-wise version this
    // replaces cut back to the last `/` for the same reason, but bytes cut
    // `/api/user-a` and `/api/user-b` at `/api/user-`, inside a segment.
    let segments = first.split('/').count();
    let n = crate::filelist::common_path_segments(paths).min(segments.saturating_sub(1));
    if n == 0 {
        return "/".to_string();
    }
    format!("{}/", first.split('/').take(n).collect::<Vec<_>>().join("/"))
}

pub fn detect(text: &str) -> bool {
    // Both halves are required: the header phrase alone appears in prose about
    // Next.js, and box-drawn rows alone are any tree output.
    (text.contains("First Load JS") || text.contains("Route (app)") || text.contains("Route (pages)"))
        && text.lines().filter(|l| split_route(l).is_some()).count() >= MIN_GROUP * 2
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    // Group table-wide by measurement, remembering where each group started.
    let mut order: Vec<(char, String, String)> = Vec::new();
    let mut members: Vec<Vec<&str>> = Vec::new();
    let mut group_of: Vec<Option<usize>> = Vec::with_capacity(lines.len());
    for line in &lines {
        let Some((kind, path, size, first)) = split_route(line) else {
            group_of.push(None);
            continue;
        };
        let key = (kind, size.to_string(), first.to_string());
        let idx = match order.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                order.push(key);
                members.push(Vec::new());
                order.len() - 1
            }
        };
        members[idx].push(path);
        group_of.push(Some(idx));
    }

    let mut emitted = vec![false; order.len()];
    let mut folded_rows = 0usize;
    let mut folded_groups = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (line, g) in lines.iter().zip(&group_of) {
        let Some(g) = *g else {
            out.push((*line).to_string());
            continue;
        };
        if members[g].len() < MIN_GROUP {
            out.push((*line).to_string());
            continue;
        }
        if emitted[g] {
            continue; // already represented by its group line
        }
        emitted[g] = true;
        folded_groups += 1;
        folded_rows += members[g].len();
        let (kind, size, first) = &order[g];
        out.push(format!(
            "├ {kind} {} routes under {} — {size} / {first} each",
            members[g].len(),
            common_prefix(&members[g])
        ));
    }
    if folded_groups == 0 {
        return None;
    }
    out.insert(
        0,
        format!(
            "[tokenade:next] {folded_rows} route row(s) collapsed into {folded_groups} group(s) \
             of identical size; every distinct measurement is still listed below."
        ),
    );
    let joined = out.join("\n");
    // Never hand back MORE than we were given: on a table whose routes all
    // differ in size the banner would be pure cost.
    (joined.len() < text.len()).then_some(joined)
}

pub fn compact(text: &str) -> String {
    try_compact(text).unwrap_or_else(|| text.to_string())
}

/// Shared test fixture: a real `next build` route table whose `content_routes`
/// rows share a measurement.
///
/// `pub(crate)` and not a second copy: `npm_run_script` pins the
/// content-vs-name competition against the SAME shape this module folds, and
/// two fixtures drifting apart would let one of the two guards test nothing.
#[cfg(test)]
pub(crate) fn real_table(content_routes: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::from(
        "\n> tokenade.net@0.1.0 build\n> next build\n\n\
         Route (app)                                                  Size     First Load JS\n\
         ┌ ○ /                                                        5.2 kB          123 kB\n",
    );
    for i in 0..content_routes {
        let _ = writeln!(
            s,
            "├ ○ /comparisons/tool-{i}-vs-tokenade                          279 B           106 kB"
        );
    }
    s.push_str(
        "├ ƒ /review                                                  3.71 kB         195 kB\n\
         ├ ○ /robots.txt                                              0 B                0 B\n\
         ├ ƒ /savings/[slug]                                          1.06 kB         193 kB\n\
         ├ ƒ /signup                                                  4.1 kB          230 kB\n\
         └ ƒ /welcome                                                 5.65 kB         216 kB\n\
         + First Load JS shared by all                                192 kB\n  \
         ├ chunks/5181-2223d6506232b70e.js                          134 kB\n  \
         └ other shared chunks (total)                               4.16 kB\n\n\n\
         ƒ Middleware                                                 142 kB\n\n\
         ○  (Static)   prerendered as static content\n\
         ƒ  (Dynamic)  server-rendered on demand\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// Byte-for-byte the shape captured from a real `pnpm build` on
    /// 2026-07-14 (recovered from the disclosure stash), extended with the
    /// content routes that make a real site's table long.
    #[test]
    fn parses_the_real_row_shapes() {
        assert_eq!(
            split_route("├ ƒ /savings/[slug]                                          1.06 kB         193 kB"),
            Some(('ƒ', "/savings/[slug]", "1.06 kB", "193 kB"))
        );
        assert_eq!(
            split_route("└ ƒ /welcome                                                 5.65 kB         216 kB"),
            Some(('ƒ', "/welcome", "5.65 kB", "216 kB"))
        );
        // A shared-chunk row is indented and has ONE size column — not a route.
        assert!(split_route("  ├ chunks/5181-2223d6506232b70e.js                          134 kB").is_none());
        // `+ First Load JS shared by all` opens with no box character.
        assert!(split_route("+ First Load JS shared by all                                192 kB").is_none());
    }

    #[test]
    fn folds_the_repeated_measurement_and_keeps_every_distinct_one() {
        let raw = real_table(400);
        let out = compact(&raw);
        assert!(out.len() < raw.len() / 3, "barely folded: {} -> {}", raw.len(), out.len());
        assert!(
            out.contains("400 routes under /comparisons/ — 279 B / 106 kB each"),
            "group line missing or malformed: {}",
            out.lines().take(4).collect::<Vec<_>>().join("\n")
        );
        // Every DISTINCT measurement still has its own row, verbatim.
        for kept in ["/review", "/robots.txt", "/savings/[slug]", "/signup", "/welcome", "/  "] {
            assert!(out.contains(kept), "dropped a one-off route: {kept}");
        }
        // Everything that is not a route row survives untouched.
        for kept in [
            "> next build",
            "Route (app)",
            "+ First Load JS shared by all",
            "chunks/5181-2223d6506232b70e.js",
            "ƒ Middleware",
            "(Static)",
        ] {
            assert!(out.contains(kept), "dropped non-route context: {kept}");
        }
    }

    #[test]
    fn the_banner_states_the_real_counts() {
        let out = compact(&real_table(400));
        assert!(
            out.contains("400 route row(s) collapsed into 1 group(s)"),
            "banner does not carry the real counts: {}",
            out.lines().next().unwrap_or("")
        );
    }

    #[test]
    fn declines_when_every_route_differs() {
        // A small app where no two routes share a measurement: folding would
        // only add a banner, so the table must come back untouched.
        let mut s = String::from("Route (app)                    Size     First Load JS\n");
        for i in 0..12 {
            let _ = writeln!(s, "├ ○ /page-{i}                      {i}.5 kB          10{i} kB");
        }
        assert_eq!(compact(&s), s, "folded a table with no repeated measurement");
    }

    #[test]
    fn declines_on_output_that_is_not_a_route_table() {
        for raw in [
            "",
            "error: Cannot find module 'next'\nnpm ERR! missing script: build\n",
            "Creating an optimized production build ...\nCompiled successfully\n",
            // Tree output with box characters but no route table.
            "┌ a\n├ b\n├ c\n└ d\n",
        ] {
            assert!(try_compact(raw).is_none(), "fired on non-table input: {raw:?}");
        }
    }

    #[test]
    fn a_group_prefix_never_claims_more_than_the_routes_share() {
        assert_eq!(common_prefix(&["/blog/a", "/blog/b"]), "/blog/");
        assert_eq!(common_prefix(&["/blog/a", "/docs/b"]), "/");
        // A shared prefix that stops mid-slug is cut back to the last slash,
        // so the line never names a path segment that does not exist.
        assert_eq!(common_prefix(&["/api/user-a", "/api/user-b"]), "/api/");
    }

    /// The whole point: the argv says `pnpm`, never `next`. If this test goes
    /// red the compactor still works and is unreachable again — which is the
    /// state production was measured in.
    #[test]
    fn the_package_manager_argv_reaches_it() {
        let raw = real_table(400);
        for cmd in [
            vec!["pnpm", "build"],
            vec!["npm", "run", "build"],
            vec!["yarn", "build"],
            vec!["bun", "run", "build"],
        ] {
            let argv: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
            let r = crate::filter::filter_command(&argv, &raw);
            assert!(
                r.output.contains("routes under /comparisons/"),
                "{cmd:?} did not reach the route-table fold: {}",
                r.output.lines().take(3).collect::<Vec<_>>().join(" | ")
            );
            assert!(
                r.savings.tokens_after * 3 < r.savings.tokens_before,
                "{cmd:?} cut less than two thirds: {} -> {}",
                r.savings.tokens_before,
                r.savings.tokens_after
            );
        }
    }

}
