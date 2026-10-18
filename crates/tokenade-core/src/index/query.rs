//! **Answering "where is this symbol?".**
//!
//! Search, rank, and return the result with just enough context that an agent
//! does not have to reopen the file. Ranking is the real subject: an exact
//! match, a match within the caller's scope and a substring match are not
//! worth the same, and it is this scoring that decides whether the answer fits
//! in a few hundred tokens.

use super::*;

impl Index {
    /// Find symbols whose name matches `query` (case-insensitive substring),
    /// returning at most `limit` hits as a compact, agent-ready listing.
    pub fn find(&self, query: &str, limit: usize) -> Vec<&Symbol> {
        self.find_with_total(query, limit).0
    }

    /// As [`Self::find`], plus how many symbols matched before the cap — see
    /// [`Self::find_ranked_with_total`] for why the caller needs it.
    /// The indexed symbols a caller most likely MEANT, when `find_with_total`
    /// returned nothing.
    ///
    /// `no symbol matching 'X'` is a refusal that names no way out, and it is
    /// the most common answer this command gives after a hit: MEASURED
    /// 2026-08-31 over this machine's transcripts, 28 of 196 real `tokenade
    /// query` calls (14%) came back empty — against 2% for `skeleton` and
    /// `impact` — and 247 of the 425 code-intel calls ran a `grep` in the SAME
    /// command, the caller hedging against exactly this answer and paying for
    /// both.
    ///
    /// Two ways to be close, because the real misses are of two kinds:
    ///
    /// * A TYPO — `typo_distance` ≤ 3, the same rule and the same helper the
    ///   unknown-flag refusal uses.
    /// * A WRONG WORD in an otherwise right name. `cmd_mcp_wrap` is 5 edits
    ///   from `cmd_mcp_proxy` and no edit-distance rule will ever pair them,
    ///   but they share the tokens `cmd` and `mcp`. Splitting on `_` and on
    ///   camelCase boundaries and counting shared tokens is what catches
    ///   `cmd_mcp_wrap` → `cmd_mcp_proxy` and `brave_search` → `web_search`,
    ///   which are the misses actually observed.
    ///
    /// Names nothing when nothing is close: a wrong suggestion on
    /// a refusal costs more than no suggestion, and "there is genuinely no
    /// such symbol" is itself the answer the caller needs.
    pub fn nearest_symbols(&self, query: &str, limit: usize) -> Vec<&str> {
        fn tokens(s: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut prev_lower = false;
            for c in s.chars() {
                if c == '_' || c == '-' || c == '.' || c == ':' {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    prev_lower = false;
                    continue;
                }
                if c.is_uppercase() && prev_lower && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                prev_lower = c.is_lowercase() || c.is_ascii_digit();
                cur.push(c.to_ascii_lowercase());
            }
            if !cur.is_empty() {
                out.push(cur);
            }
            out
        }
        let q = query.to_lowercase();
        let qt = tokens(query);
        // (score, name) — higher score first. Score is shared-token count,
        // with a typo hit worth one token so both kinds compete on one scale.
        let mut scored: Vec<(usize, usize, &str)> = Vec::new();
        for sym in &self.symbols {
            let name = sym.name.as_str();
            if name.eq_ignore_ascii_case(&q) {
                continue;
            }
            let (shared, longest) = if qt.is_empty() {
                (0, 0)
            } else {
                let nt = tokens(name);
                let hit: Vec<&String> = qt.iter().filter(|t| nt.contains(t)).collect();
                (hit.len(), hit.iter().map(|t| t.len()).max().unwrap_or(0))
            };
            // A RELEVANCE FLOOR, or the suggestion is worse than silence.
            // `zzz_no_such_thing_anywhere` shares the token `no` with `NoHome`
            // and `NoBlock`, and offering those as "closest" answers a question
            // nobody asked. Two shared tokens is enough on its own; ONE only
            // counts when the query is short enough for that token to be most
            // of what was asked, and long enough not to be filler.
            let relevant = shared >= 2 || (shared == 1 && qt.len() <= 2 && longest >= 4);
            let typo = crate::text_distance::typo_distance(&q, &name.to_lowercase()) <= 3;
            if !relevant && !typo {
                continue;
            }
            let score = usize::from(relevant) * shared + usize::from(typo);
            scored.push((score, name.len(), name));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
        scored.dedup_by(|a, b| a.2 == b.2);
        scored.into_iter().take(limit).map(|(_, _, n)| n).collect()
    }

    pub fn find_with_total(&self, query: &str, limit: usize) -> (Vec<&Symbol>, usize) {
        self.find_in(query, limit, &[])
    }

    /// [`find_with_total`], restricted to `scope`. See [`Index::in_scope`].
    /// How many indexed symbols carry EXACTLY this name, in the same scope
    /// [`find_in`] would search.
    ///
    /// [`find_in`] caps what it returns, and a caller counting exact matches
    /// among the rows it got back is counting its own display window. The
    /// caveat `query` prints — "N declarations share it, the call graph cannot
    /// tell them apart" — is a statement about how far the answer can be
    /// trusted, so it has to be the real N: measured 2026-09-07 on this tree,
    /// `try_compact` said 30 (the cap) in the same output that said "30 of 972
    /// matches shown", where 928 files declare it.
    pub fn count_exact_declarations(&self, name: &str, scope: &[String]) -> usize {
        self.symbols
            .iter()
            .filter(|s| s.name == name && Self::in_scope(s, scope))
            .count()
    }

    pub fn find_in(&self, query: &str, limit: usize, scope: &[String]) -> (Vec<&Symbol>, usize) {
        let q = query.to_lowercase();
        // #3 — match on the symbol name OR its decorators/bases, so an
        // agent can ask `tokenade query app.route` / `query Serialize`
        // and land on every handler / every `derive(Serialize)` type
        // without re-reading files. Name hits still outrank
        // annotation-only hits via `match_rank`.
        let mut hits: Vec<&Symbol> = self
            .symbols
            .iter()
            .filter(|s| Self::match_rank(s, &q).is_some() && Self::in_scope(s, scope))
            .collect();
        // Tier first (name-exact → name-substring → annotation-only),
        // then by file class (source outranks docs/test/generated at equal
        // relevance — the real source rises above its tests/fixtures),
        // then by name length (closer match), then file.
        hits.sort_by(|a, b| {
            let ar = Self::match_rank(a, &q).unwrap_or(u8::MAX);
            let br = Self::match_rank(b, &q).unwrap_or(u8::MAX);
            ar.cmp(&br)
                .then(
                    crate::ranking::classify_file(&a.file)
                        .cmp(&crate::ranking::classify_file(&b.file)),
                )
                .then(a.name.len().cmp(&b.name.len()))
                .then(a.file.cmp(&b.file))
        });
        let total = hits.len();
        hits.truncate(limit);
        (hits, total)
    }

    /// PageRank centrality per symbol name over this index's call graph.
    /// Computed on demand (callers should cache it for one CLI invocation).
    pub fn centrality(&self) -> std::collections::HashMap<String, f64> {
        crate::centrality::symbol_centrality(&self.calls)
    }

    /// Like [`find`], but breaks ties WITHIN a match tier by call-graph
    /// centrality (most depended-on symbol first) before the file-class /
    /// name-length fallbacks. `centrality` is the precomputed score map from
    /// [`centrality`](Self::centrality) — pass an empty map to get the exact
    /// legacy ordering. Centrality is applied before `truncate`, so a central
    /// symbol is never dropped by the limit in favour of a peripheral one.
    pub fn find_ranked(
        &self,
        query: &str,
        limit: usize,
        centrality: &std::collections::HashMap<String, f64>,
    ) -> Vec<&Symbol> {
        self.find_ranked_with_total(query, limit, centrality).0
    }

    /// Same ranking, but also reports how many symbols matched BEFORE the cap.
    /// The count is free — every hit is collected to rank them — and without it
    /// a caller cannot tell a complete answer from a truncated one. `query` is
    /// the command agents are told to use INSTEAD of reading files, so a list
    /// that silently stops at the cap reads as "these are all of them".
    pub fn find_ranked_with_total(
        &self,
        query: &str,
        limit: usize,
        centrality: &std::collections::HashMap<String, f64>,
    ) -> (Vec<&Symbol>, usize) {
        self.find_ranked_in(query, limit, centrality, &[])
    }

    /// [`find_ranked_with_total`], restricted to symbols living in one of
    /// `scope` — an empty scope means the whole index.
    ///
    /// The filter belongs HERE and not at the call site because the cap is
    /// applied after ranking: a common name like `Format` matches 219 symbols
    /// in this repository, so the thirty a caller sees can easily hold none
    /// from the file they asked about, and a filter applied afterwards would
    /// answer "nothing" about a file that has three. Measured 2026-09-01 over
    /// this machine's transcripts: six real calls spelled
    /// `query <symbol> --file <path>` and were refused, which is a caller
    /// reaching for exactly the right idea.
    ///
    /// A scope entry matches by path SUFFIX, so `docs.rs`,
    /// `src/docs.rs` and the full relative path all name the same file, and a
    /// directory prefix works by naming it with its separator.
    pub fn find_ranked_in(
        &self,
        query: &str,
        limit: usize,
        centrality: &std::collections::HashMap<String, f64>,
        scope: &[String],
    ) -> (Vec<&Symbol>, usize) {
        let q = query.to_lowercase();
        let mut hits: Vec<&Symbol> = self
            .symbols
            .iter()
            .filter(|s| Self::match_rank(s, &q).is_some() && Self::in_scope(s, scope))
            .collect();
        hits.sort_by(|a, b| {
            let ar = Self::match_rank(a, &q).unwrap_or(u8::MAX);
            let br = Self::match_rank(b, &q).unwrap_or(u8::MAX);
            // Centrality DESC as the primary tiebreak after tier: surface the
            // symbol the rest of the codebase actually leans on.
            let ac = centrality.get(&a.name).copied().unwrap_or(0.0);
            let bc = centrality.get(&b.name).copied().unwrap_or(0.0);
            ar.cmp(&br)
                .then(bc.partial_cmp(&ac).unwrap_or(std::cmp::Ordering::Equal))
                .then(
                    crate::ranking::classify_file(&a.file)
                        .cmp(&crate::ranking::classify_file(&b.file)),
                )
                .then(a.name.len().cmp(&b.name.len()))
                .then(a.file.cmp(&b.file))
        });
        let total = hits.len();
        hits.truncate(limit);
        (hits, total)
    }

    /// Does this symbol live in one of `scope`? An empty scope is everything.
    ///
    /// ONE predicate for both finders. `query` has three code paths — JSON,
    /// the `-o` modal renderer and the default listing — and the first version
    /// of `--file` filtered two of them: the default path went through the
    /// other finder and answered about the whole repository while the flag sat
    /// there looking honoured. A predicate that lives beside the matcher
    /// cannot be added to one caller and forgotten in the next.
    ///
    /// Matching is by path SUFFIX or directory PREFIX, so `docs.rs`,
    /// `src/docs.rs` and the full relative path all name the same file, and a
    /// directory scopes everything under it. Backslashes are folded so a
    /// Windows-shaped index answers the same as a POSIX one.
    fn in_scope(s: &Symbol, scope: &[String]) -> bool {
        if scope.is_empty() {
            return true;
        }
        let f = s.file.replace('\\', "/");
        scope.iter().any(|want| {
            let w = want.trim_start_matches("./").replace('\\', "/");
            let w = w.trim_end_matches('/');
            !w.is_empty()
                && (f == w || f.ends_with(&format!("/{w}")) || f.starts_with(&format!("{w}/")))
        })
    }

    /// Match tier for `find`: `0` exact name, `1` name substring, `2`
    /// matched only via a decorator/base (annotation). `None` = no match.
    fn match_rank(s: &Symbol, q: &str) -> Option<u8> {
        let name = s.name.to_lowercase();
        if name == *q {
            return Some(0);
        }
        if name.contains(q) {
            return Some(1);
        }
        let anno = s
            .decorators
            .iter()
            .chain(s.bases.iter())
            .chain(std::iter::once(&s.owner))
            .any(|a| !a.is_empty() && a.to_lowercase().contains(q));
        if anno {
            Some(2)
        } else {
            None
        }
    }

    /// Render `hits` as a grep-compatible output with `context_lines`
    /// lines of context around each match. `context_lines == 0`
    /// returns the legacy `<kind> <name> — <file>:<line>` form. With
    /// `context_lines > 0` we emit one block per hit, formatted
    /// exactly like `grep -n -A N -B N`:
    ///
    /// ```text
    /// src/foo.rs-42-    // some comment
    /// src/foo.rs:43:    pub fn compact(text: &str) -> String {
    /// src/foo.rs-44-        let mut out = String::new();
    /// --
    /// src/bar.rs-10- ...
    /// ```
    ///
    /// The `:` separator on the matched line and `-` on context
    /// lines are the standard grep convention. This lets
    /// `symbol_find` serve as a drop-in replacement for
    /// `grep -n -A N -B N 'fn identifier'` — the agent gets the
    /// surrounding context that used to be the reason for switching
    /// to grep. Files unreadable (deleted, permission denied) fall
    /// back to a single legacy line so the hit isn't lost.
    ///
    /// Per-file content is cached for the call, so multiple hits in
    /// the same file don't re-read it from disk.
    pub fn render_with_context(&self, hits: &[&Symbol], context_lines: usize) -> String {
        if context_lines == 0 {
            return hits
                .iter()
                .map(|s| s.location_line())
                .collect::<Vec<_>>()
                .join("\n");
        }
        let mut file_cache: std::collections::HashMap<String, Option<Vec<String>>> =
            std::collections::HashMap::new();
        let root = std::path::Path::new(&self.root);
        let mut out = String::new();
        for (i, s) in hits.iter().enumerate() {
            if i > 0 {
                out.push_str("--\n");
            }
            let lines = file_cache.entry(s.file.clone()).or_insert_with(|| {
                std::fs::read_to_string(root.join(&s.file))
                    .ok()
                    .map(|c| c.lines().map(String::from).collect())
            });
            let Some(lines) = lines.as_ref() else {
                let _ = writeln!(out, "{} (file unreadable)", s.location_line());
                continue;
            };
            let total = lines.len();
            let center = s.line.saturating_sub(1); // 1-based → 0-based
            if center >= total {
                let _ = writeln!(
                    out,
                    "{} (line out of range, file has {total} lines)",
                    s.location_line()
                );
                continue;
            }
            let start = center.saturating_sub(context_lines);
            let end = (center + context_lines + 1).min(total);
            for (offset, src) in lines[start..end].iter().enumerate() {
                let ln_num = start + offset + 1;
                let sep = if ln_num == s.line { ':' } else { '-' };
                let _ = writeln!(out, "{}{}{}{}{}", s.file, sep, ln_num, sep, src);
            }
        }
        out
    }
}
