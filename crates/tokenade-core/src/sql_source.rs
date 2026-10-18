//! SQL source-file compactor (distinct from `sql.rs` which handles
//! `psql`/result-set OUTPUT). Targets `.sql` migration files, schema
//! dumps, hand-written queries, ORM-generated migrations (Drizzle,
//! Alembic, Flyway, Liquibase).
//!
//! Real-world ratio: 15-35% on commented migration files / DDL
//! dumps. Multi-statement schema files with extensive comments
//! (think Sequel Pro exports, pgdump --inserts with explanatory
//! preamble) hit the upper end.
//!
//! What it strips:
//!   * `-- line comments`
//!   * `/* … */` block comments (multi-line state machine)
//!   * blank-line runs → one max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every statement (CREATE / INSERT / UPDATE / SELECT / …)
//!   * string literals (`'…'`) — comment markers inside survive
//!   * identifier quoting (`"…"` and backticks for MySQL)
//!   * indentation inside multi-line statements
//!
//! Note: psql output rows (the result of running a SELECT) are
//! handled by `sql.rs::compact` which is a TOTALLY different parser
//! (table-shape detection, row dedup). Routing is by content shape:
//! `+---+---+` borders → result, `SELECT … FROM …` keywords → source.

/// Common SQL statement keywords used for detection. We require ≥2
/// distinct hits within the first 40 non-comment non-blank lines.
/// Clause keywords. They never OPEN a statement, so they do not count towards
/// `kw_hits`, but they are just as much SQL when measuring how much of a file
/// is SQL at all.
const SQL_CLAUSES: &[&str] = &[
    "FROM", "WHERE", "JOIN", "VALUES", "SET", "ORDER", "GROUP", "HAVING", "LIMIT", "ON", "AND",
    "OR", "UNION", "INTO", "AS", "LEFT", "RIGHT", "INNER", "OUTER", "PRIMARY", "FOREIGN",
];

const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "REPLACE",
    "MERGE", "CALL", "EXEC", "EXECUTE", "WITH", "GRANT", "REVOKE", "BEGIN", "COMMIT", "ROLLBACK",
    "DECLARE", "EXPLAIN", "ANALYZE", "VACUUM", "REINDEX",
];

/// Heuristic detector. Distinguishes from `sql.rs` result-set output
/// by REJECTING tables (`+---+---+` borders, `( N rows)` footers).
pub fn detect(text: &str) -> bool {
    // Result-set table border — instant reject. The sql.rs (result)
    // compactor owns this shape.
    for line in text.lines().take(20) {
        let t = line.trim();
        if (t.starts_with("+-") && t.ends_with("-+"))
            || t.ends_with(" rows)")
            || t.ends_with(" row)")
        {
            return false;
        }
    }
    if text.trim_start().starts_with("<!DOCTYPE") || text.trim_start().starts_with("<?xml") {
        return false;
    }
    // COVERAGE. The window below reads the first 40 lines, which is right for
    // deciding "is this SQL" and wrong for deciding "is this a SQL FILE".
    // A Python test that sets up a schema —
    //
    //     CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
    //
    // — is genuinely SQL in those lines and Python everywhere else, and the
    // SQL comment stripper then applies `--` rules to the Python around it:
    // a docstring mentioning `git log --name-only` loses everything after the
    // dash (the same mechanism that once ate diff hunk headers).
    //
    // Measured 2026-08-26 over this corpus, share of non-blank lines opening
    // on a SQL keyword:
    //
    //     real `.sql` (37 files)    min 6   p10 9   median 24   p90 49   max 76
    //     source files (475)        min 0   p10 0   median  2   p90 10   max 46
    //
    // The two overlap, so this is a floor and not a separator: at 5 % every
    // real `.sql` in the corpus is kept and the half of the source files that
    // merely quote a statement or two is not. Files under 20 non-blank lines
    // are exempt — a short snippet is all island by construction.
    {
        const MIN_COVERAGE_PCT: usize = 5;
        const MIN_BODY_LINES: usize = 20;
        let body: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if body.len() >= MIN_BODY_LINES {
            let hits = body
                .iter()
                .filter(|l| {
                    let w: String = l
                        .trim_start()
                        .chars()
                        .take_while(|c| c.is_alphabetic())
                        .map(|c| c.to_ascii_uppercase())
                        .collect();
                    SQL_KEYWORDS.contains(&w.as_str()) || SQL_CLAUSES.contains(&w.as_str())
                })
                .count();
            if hits * 100 < body.len() * MIN_COVERAGE_PCT {
                return false;
            }
        }
    }
    let mut kw_hits = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(40) {
        considered += 1;
        let t = line.trim_start();
        if t.starts_with("--") || t.starts_with("/*") {
            continue;
        }
        let first: String = t
            .chars()
            .take_while(|c| c.is_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        // `commit` is a SQL keyword AND the first word of every `git log`
        // entry. MEASURED 2026-08-25 on 51 KB of real `git log`: two entries
        // are enough to reach the `kw_hits >= 2` threshold, so it was claimed
        // as SQL source. Nothing was corrupted — the SQL compactor only strips
        // comments — but nothing was folded either: 51 189 → 50 560 bytes
        // (0.988), against 3 072 (0.060) from the git-log compactor that
        // should have had it. A 16× loss on one of the most common payloads a
        // developer puts in front of an agent.
        //
        // Reordering the detectors is NOT the fix: `gitlog` is deliberately
        // checked after `UnifiedDiff` so `git log -p` reaches the diff
        // compactor, and SQL is checked before both. The confusion belongs to
        // this detector, so this is where it is refused.
        let is_commit_header = first == "COMMIT" && {
            let rest = t["commit".len()..].trim();
            rest.len() >= 7 && rest.len() <= 40 && rest.chars().all(|c| c.is_ascii_hexdigit())
        };
        // …and the run has to BE the word, not its prefix. `take_while(alphabetic)`
        // stops at the first `-` or `_`, so every hyphenated or snake_cased
        // identifier that begins with a SQL keyword read as that keyword:
        // `update-type`, `create_table`, `select_one`, `delete-me`,
        // `begin_frame`. MEASURED 2026-08-26 on `gin-gitlog.txt`, 8 788 bytes
        // of real `git log`: the only two hits in the window are
        // `update-type: version-update:semver-major` and its `semver-minor`
        // twin, both inside a dependabot commit body. That was enough to claim
        // the whole log as SQL source, and the SQL compactor — which only
        // strips comments — returned all 8 788 bytes where the git-log
        // compactor gives 415. A 21x loss on a fixture that has been in the
        // benchmark corpus the whole time.
        //
        // A keyword in real SQL is followed by whitespace, `;`, `(` or `*` —
        // never by an identifier character.
        let followed_by_identifier_char = t[first.len()..]
            .chars()
            .next()
            .is_some_and(|c| c == '-' || c == '_' || c.is_ascii_digit());
        // …and a SQL statement does not end like an English sentence.
        //
        // `EXPLAIN` is a SQL keyword AND the first word of half the questions a
        // person asks a coding agent. Measured 2026-08-26 on
        // `scrooge-mode/benchmarks/prompts/en-report.txt`, a corpus of one
        // English prompt per line: three lines begin `Explain …`, two are
        // enough, `Kind::SqlSource` won, and what came back was the file's `#`
        // preamble plus NOTHING — **0 of its 19 prompts survived**, 2 246 bytes
        // to 434. The comments were kept and the content was deleted, which is
        // the inversion this class keeps producing.
        //
        // No SQL statement ends on `.`, `?` or `!`; an English sentence
        // routinely does. Measured over 342 real `.sql` files: **296 detected
        // before, 296 after** — the rule costs nothing. Over 35 033 `.txt` and
        // `.md` files: **293 claimed before, 201 after**, so 92 documents stop
        // being read as SQL.
        let ends_like_a_sentence = t.trim_end().ends_with(['.', '?', '!']);
        // …and a SQL statement does not end its line with a colon, where a
        // Python block statement always does. `WITH` is a CTE opener in SQL
        // and the single most common statement opener in Python:
        // `with mock.patch.object(...):`, `with open(path) as f:`.
        let ends_like_a_block_header = t.trim_end().ends_with(':');
        // …and a CTE names a plain identifier before its `AS`:
        // `WITH recent_orders AS (`. Python's `with mock.patch.object(up, …)`
        // names a dotted expression and never reaches an `AS` of its own.
        // Measured 2026-08-26 on `jcodemunch-mcp/tests/test_v1_108_85.py`:
        // five `with mock.patch.object(…)` lines were five SQL hits, and the
        // SQL comment stripper then cut every docstring line that mentions a
        // CLI flag — `ran ``git log --name-only`` over the entire` loses
        // everything from `--name-only` on. The same mechanism, one language
        // over: 172 of the 826 source files claimed here were being edited.
        let with_is_not_a_cte = first == "WITH" && {
            let rest = t["with".len()..].trim_start();
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            ident.is_empty()
                || !rest[ident.len()..]
                    .trim_start()
                    .get(..2)
                    .is_some_and(|a| a.eq_ignore_ascii_case("as"))
        };
        if SQL_KEYWORDS.contains(&first.as_str())
            && !is_commit_header
            && !followed_by_identifier_char
            && !ends_like_a_sentence
            && !ends_like_a_block_header
            && !with_is_not_a_cte
        {
            kw_hits += 1;
        }
        if kw_hits >= 2 {
            return true;
        }
    }
    kw_hits >= 2 && considered > 0
}

/// Strip comments + collapse blank-runs + trim trailing ws.
pub fn compact(text: &str) -> String {
    // A migration's comments are its licence, its identifier, its lock warning
    // and its rollback statement. See `code_strip::declare_strip`.
    crate::code_strip::declare_strip(text, strip_only(text), "sql", None)
}

/// The strip WITHOUT its declaration.
///
/// **Not an output path.** Everything a user or agent sees goes through
/// `compact`, which declares what this removed and returns the file whole when
/// the note would cost more than the strip saved. This is public only so the
/// per-language mechanics can be asserted directly: the lesson is that a
/// guard aimed at a wrapper which short-circuits on short input passes against
/// broken code underneath, and these fixtures are one line each on purpose.
pub fn strip_only(text: &str) -> String {
    crate::code_strip::strip_block_language(
        text,
        "--",
        strip_block_comment,
        strip_inline_line_comment,
    )
}

/// `Savings`-yielding variant.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn strip_block_comment(line: &str, in_block: bool) -> (String, bool) {
    crate::code_strip::strip_block_comment_line(line, in_block, b"'\"`")
}

/// Cut the line at its first comment marker that is not inside a string:
/// `--`, in strings quoted with `'`, `"` or a backtick.
///
/// This was a 40-line hand-rolled scanner, and `hcl.rs`, `proto.rs` and
/// `sql_source.rs` each had one. They differed by their quote set and their
/// marker and by NOTHING else — same string tracking, same backslash skip,
/// same `line[..i].trim_end()` tail. `code_strip::strip_inline_comment_line`
/// already expresses all three; the two-byte markers go through its `confirm`
/// hook, which is what `ini.rs` uses it for.
fn strip_inline_line_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"-", b"'\"`", b"'\"`", true, |_, i| {
        line.as_bytes().get(i + 1) == Some(&b'-')
    })
}

#[cfg(test)]
mod tests {

    /// The scanner this module used to carry, kept as the ORACLE.
    ///
    /// The delegation to `code_strip` must agree with it on every line, not
    /// just on the cases someone thought to write down — a comment stripper
    /// that silently eats code is the failure mode this repo has already paid
    /// for once (45 % of a `.tf` lost with no marker, 2026-08-04).
    #[allow(dead_code)]
    fn reference_strip(line: &str) -> String {
        let bytes = line.as_bytes();
        let mut in_string: Option<u8> = None;
        let mut cut: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            match in_string {
                Some(q) => {
                    if c == q {
                        in_string = None;
                    }
                }
                None => match c {
                    b'\'' | b'"' | b'`' => in_string = Some(c),
                    b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                        cut = Some(i);
                        break;
                    }
                    _ => {}
                },
            }
            i += 1;
        }
        match cut {
            Some(i) => line[..i].trim_end().to_string(),
            None => line.to_string(),
        }
    }

    #[test]
    fn delegation_agrees_with_the_scanner_it_replaced() {
        for line in &[
            "",
            "plain line",
            "a # b",
            "a // b",
            "a -- b",
            "a / b",
            "a - b",
            "key = \"a # b\"",
            "key = \"a // b\"",
            "key = \"a -- b\"",
            "key = 'a # b'",
            "key = \"a\\\" # still string\"",
            "key = \"unterminated # b",
            "key = `a -- b`",
            "  indented = 1  // trailing",
            "\\# escaped at column zero",
            "x = 1 ## double",
            "x = 1 /// triple",
            "x = 1 --- triple",
        ] {
            assert_eq!(
                super::strip_inline_line_comment(line),
                reference_strip(line),
                "diverge sur {line:?}"
            );
        }
    }
    use super::*;

    const REAL_SQL: &str = "-- Migration 0010: analytics tables\n-- Generated 2026-05-29\n\n/* Multi-line preamble\n   describing the migration */\n\nCREATE TABLE IF NOT EXISTS \"analytics_session\" (\n    \"id\" varchar(255) PRIMARY KEY NOT NULL,  -- session id\n    \"fingerprintHash\" text NOT NULL,\n    \"firstSeenAt\" timestamp DEFAULT now() NOT NULL,\n    \"lastSeenAt\" timestamp DEFAULT now() NOT NULL\n);\n\nCREATE INDEX idx_session_first_seen ON analytics_session (firstSeenAt);\n\n-- Insert seed data\nINSERT INTO analytics_session (id, fingerprintHash, firstPath)\nVALUES ('seed-1', 'hash-1', '/'),\n       ('seed-2', 'hash-2', '/home');\n";

    #[test]
    fn detect_real_sql_migration() {
        assert!(detect(REAL_SQL));
    }

    #[test]
    fn detect_rejects_result_set_table() {
        let table =
            "+----+------+\n| id | name |\n+----+------+\n|  1 | a    |\n+----+------+\n( 1 row)";
        assert!(!detect(table));
    }

    #[test]
    fn detect_rejects_prose() {
        assert!(!detect("hello world\nthis is just prose\n"));
        assert!(!detect("<!DOCTYPE html>\n"));
    }

    #[test]
    fn compact_strips_line_comments() {
        let out = compact(REAL_SQL);
        assert!(!out.contains("Migration 0010"));
        assert!(!out.contains("Generated 2026"));
        assert!(!out.contains("Insert seed data"));
        assert!(!out.contains("session id"));
    }

    #[test]
    fn compact_strips_block_comments() {
        let out = compact(REAL_SQL);
        assert!(!out.contains("Multi-line preamble"));
        assert!(!out.contains("describing the migration"));
    }

    #[test]
    fn compact_preserves_string_literals_with_dashes() {
        let input = "INSERT INTO t (s) VALUES ('a -- not a comment');\n";
        let out = compact(input);
        assert!(out.contains("'a -- not a comment'"));
    }

    #[test]
    fn compact_preserves_block_marker_inside_string() {
        let input = "INSERT INTO t (s) VALUES ('/* literal */');\n";
        let out = compact(input);
        assert!(out.contains("/* literal */"));
    }

    #[test]
    fn compact_preserves_all_statements() {
        let out = compact(REAL_SQL);
        assert!(out.contains("CREATE TABLE IF NOT EXISTS \"analytics_session\""));
        assert!(out.contains("CREATE INDEX idx_session_first_seen"));
        assert!(out.contains("INSERT INTO analytics_session"));
        assert!(out.contains("VALUES ('seed-1'"));
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        let (_, sav) = compact_with_savings(REAL_SQL);
        assert!(
            sav.percent() >= 10.0,
            "expected ≥10% on commented .sql, got {:.1}%",
            sav.percent()
        );
    }
}

#[cfg(test)]
mod git_log_is_not_sql {
    use super::*;

    fn log(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "commit {:040x}\nAuthor: a <a@b.c>\nDate:   Mon Aug 25 10:00:00 2026\n\n    message {i}\n\n",
                    i
                )
            })
            .collect()
    }

    /// `commit` is a SQL keyword and the first word of every `git log` entry.
    /// Two entries reached the `kw_hits >= 2` threshold, so `git log` was
    /// claimed by the SQL compactor: no corruption, but no fold either —
    /// 0.988 against the 0.060 the git-log compactor gives.
    #[test]
    fn a_git_log_is_not_claimed_as_sql_source() {
        assert!(!detect(&log(2)), "two commits were enough to look like SQL");
        assert!(!detect(&log(25)));
        // A short sha (`git log --abbrev-commit`) is the same shape.
        assert!(!detect(
            "commit 40f1888\nAuthor: a\n\ncommit 4332b57\nAuthor: b\n"
        ));
    }

    /// …and a real `COMMIT;` statement is still SQL. The refusal keys on the
    /// hex payload after the word, not on the word.
    #[test]
    fn a_real_commit_statement_is_still_sql() {
        let sql = "BEGIN;\nINSERT INTO t VALUES (1);\nCOMMIT;\n";
        assert!(detect(sql), "the refusal swallowed real SQL");
        // `commit` followed by something that is not a bare hex id stays a hit.
        assert!(detect("BEGIN;\nCOMMIT WORK;\nINSERT INTO t VALUES (1);\n"));
    }
}

/// An English question is not a SQL statement.
#[cfg(test)]
mod english_is_not_sql {
    use super::detect;

    /// Trimmed from `scrooge-mode/benchmarks/prompts/en-report.txt`, one
    /// English prompt per line. Three begin with `Explain`, which is also a SQL
    /// keyword; two were enough. The delivered answer was the `#` preamble and
    /// nothing else — every prompt gone.
    const PROMPTS: &str = "# EN report corpus\n# One prompt per non-comment line.\n\n# --- debug ---\nExplain why a React form re-renders on every keystroke, and how to fix it.\nWhat causes a Node.js EADDRINUSE error on server start, and how do I resolve it?\nWhy does a useMemo recompute on every render?\n\n# --- explain ---\nExplain how JavaScript event delegation works and its benefits.\nExplain the difference between s-maxage and max-age in CDN caching.\nCompare the READ COMMITTED and REPEATABLE READ isolation levels.\n";

    #[test]
    fn a_corpus_of_english_prompts_is_not_sql() {
        assert!(
            !detect(PROMPTS),
            "`Explain …` at the start of a sentence was counted as SQL EXPLAIN"
        );
    }

    // The harm, stated as what a reader loses: the prompts, not the comments.
    // The harm was measured on the REAL file rather than guarded
    // here, and that is deliberate: the deletion could not be reproduced from
    // a trimmed fixture. `scrooge-mode/benchmarks/prompts/en-report.txt` came
    // back as its `#` preamble and nothing else — 2 246 bytes to 434, **0 of
    // its 19 prompts** — while a 4 439-byte fixture built from the same shapes
    // returns unchanged. The mechanism of the deletion is NOT established; the
    // misrouting that exposed the file to it is, and that is what these guards
    // hold.

    /// Real SQL still detects. A statement does not end on a full stop.
    #[test]
    fn real_sql_still_detects() {
        let sql = "-- monthly rollup\nSELECT id, sum(amount)\nFROM orders\nWHERE created_at > now() - interval '30 days'\nGROUP BY id;\n\nUPDATE accounts SET tier = 'gold' WHERE id IN (SELECT id FROM winners);\n";
        assert!(detect(sql), "a plain SQL file stopped detecting");
    }

    /// …including a real `EXPLAIN`, which is followed by a query and ends on a
    /// semicolon.
    #[test]
    fn a_real_explain_still_counts() {
        let sql = "EXPLAIN ANALYZE SELECT * FROM orders WHERE id = 1;\nEXPLAIN SELECT count(*) FROM users;\n";
        assert!(detect(sql), "a real EXPLAIN plan stopped detecting");
    }
}

#[cfg(test)]
mod python_with_is_not_a_cte {
    //! `WITH` opens a CTE in SQL and a block in Python. Five
    //! `with mock.patch.object(…):` lines were five SQL hits, and the SQL
    //! comment stripper then cut every docstring line mentioning a CLI flag:
    //! ``ran ``git log --name-only`` over the entire`` loses everything from
    //! `--name-only` on. The same mechanism, one language over.

    const PY: &str = "from __future__ import annotations\n\nfrom unittest import mock\n\n\ndef test_one() -> None:\n    with mock.patch.object(up, \"detect_install_mechanism\", return_value=(\"pipx\", \"x\")):\n        assert up.plan() == 1\n\n\ndef test_two() -> None:\n    with mock.patch.object(up, \"detect_install_mechanism\", return_value=(\"uv\", \"x\")):\n        assert up.plan() == 2\n\n\ndef test_three() -> None:\n    with mock.patch.object(up, \"detect_install_mechanism\", return_value=(\"pip\", None)):\n        assert up.plan() == 3\n";

    #[test]
    fn python_with_statements_are_not_sql() {
        assert!(!super::detect(PY), "`with x(...):` is a block, not a CTE");
    }

    #[test]
    fn a_docstring_mentioning_a_cli_flag_keeps_its_line() {
        let doc = format!("{PY}\n\ndef doc() -> None:\n    \"\"\"ran ``git log --name-only`` over the entire tree.\"\"\"\n");
        let out = crate::auto::compact(&doc).1;
        assert!(
            out.contains("over the entire tree"),
            "the SQL comment stripper cut the line at `--`: {out}"
        );
    }

    #[test]
    fn a_real_cte_still_detects() {
        let sql = "WITH recent_orders AS (\n  SELECT id FROM orders WHERE created_at > now() - interval '7 days'\n)\nSELECT * FROM recent_orders;\nINSERT INTO audit (what) VALUES ('ran');\n";
        assert!(super::detect(sql), "`WITH x AS (` is a CTE");
    }

    #[test]
    fn a_multiline_with_call_is_not_a_cte() {
        // The shape the colon rule cannot see, and the one real files use:
        // the `with` opens a parenthesised call that continues on the next
        // line, so its own line ends on `(` — exactly like `WITH x AS (`.
        let py = "import os\n\n\ndef a():\n    with mock.patch(\n        \"x\", return_value=1\n    ):\n        pass\n\n\ndef b():\n    with mock.patch(\n        \"y\", return_value=2\n    ):\n        pass\n\n\ndef c():\n    with mock.patch(\n        \"z\", return_value=3\n    ):\n        pass\n";
        assert!(!super::detect(py), "`with mock.patch(` names no CTE");
    }

    #[test]
    fn a_block_header_colon_does_not_count() {
        // The general half of the rule: a SQL statement never ends its line
        // on a colon, a Python block statement always does.
        let py = "import os\n\n\ndef a():\n    with open(os.devnull) as f:\n        pass\n\n\ndef b():\n    with open(os.devnull) as f:\n        pass\n\n\ndef c():\n    with open(os.devnull) as f:\n        pass\n";
        assert!(!super::detect(py));
    }
}

#[cfg(test)]
mod a_sql_island_does_not_make_a_sql_file {
    //! The keyword window decides "is this SQL"; it cannot decide "is this a
    //! SQL FILE". A Python test that sets up a schema is genuinely SQL in
    //! those four lines and Python in the other four hundred, and the SQL
    //! comment stripper then applied `--` rules to the Python around it.

    fn python_with_a_schema_island() -> String {
        let mut s = String::from("import sqlite3\nimport pytest\n\n\nSCHEMA = \"\"\"\nCREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);\nCREATE TABLE files (path TEXT PRIMARY KEY, hash TEXT);\nCREATE TABLE scip_meta (key TEXT PRIMARY KEY, value TEXT);\n\"\"\"\n\n\ndef test_schema(tmp_path):\n    \"\"\"Checks the loader.\n\n    It used to run ``git log --name-only`` over the entire tree.\n    \"\"\"\n    con = sqlite3.connect(tmp_path / \"x.db\")\n    con.executescript(SCHEMA)\n");
        for i in 0..80 {
            s.push_str(&format!(
                "\n\ndef test_case_{i}(tmp_path):\n    assert helper_{i}(tmp_path) is not None\n"
            ));
        }
        s
    }

    #[test]
    fn a_schema_island_in_a_python_test_is_not_a_sql_file() {
        assert!(
            !super::detect(&python_with_a_schema_island()),
            "three CREATE lines in four hundred are an island"
        );
    }

    #[test]
    fn the_docstring_keeps_everything_after_the_dashes() {
        let out = crate::auto::compact(&python_with_a_schema_island()).1;
        assert!(
            out.contains("over the entire tree"),
            "the SQL comment stripper cut the line at `--`"
        );
    }

    #[test]
    fn a_real_schema_file_is_still_sql() {
        let mut s = String::from("-- schema\n");
        for i in 0..30 {
            s.push_str(&format!(
                "CREATE TABLE t{i} (\n  id INTEGER PRIMARY KEY,\n  name TEXT\n);\n\nINSERT INTO t{i} (name) VALUES ('a');\n"
            ));
        }
        assert!(super::detect(&s), "a schema file is mostly SQL");
    }

    #[test]
    fn a_short_snippet_is_exempt() {
        // Under the body-line floor a file is all island by construction, so
        // the coverage rule must not reach it.
        let snippet = "SELECT id, name\nFROM users\nWHERE active = 1\nORDER BY name;\n\nINSERT INTO audit (what) VALUES ('read');\n";
        assert!(super::detect(snippet));
    }
}
