//! Dispatcher for `db`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::argv::{has_flag, has_any_flag};

pub fn dispatch(prog: &str, argv: &[String], output: &str) -> String {
    // JSON / vertical / batch flags.
    if has_flag(argv, "--json") || (prog == "mysql" && has_any_flag(argv, &["-E", "--vertical"])) {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    // A script run through the client prints one command tag per statement.
    // `psql -f migrate.sql` on a real migration is hundreds of `INSERT 0 1`
    // lines, all distinct enough that nothing folded them: measured 5 212
    // chars in, 5 212 out. They are receipts; the ERROR among them is why the
    // agent ran the file.
    if let Some(folded) = fold_command_tags(output) {
        return folded;
    }
    // Otherwise output is tabular or status line. Auto + generic.
    let (_, c, _) = crate::auto::compact(output);
    if c.len() < output.len() { c } else { generic(output) }
}

/// A SQL client's per-statement command tag: `INSERT 0 1`, `UPDATE 12`,
/// `CREATE TABLE`, `SET`, `COMMIT`, …
///
/// Matched on the leading VERB and a tail of nothing but digits/spaces, so a
/// line that carries a message — `ERROR: …`, `NOTICE: …`, `LINE 1: …`, a data
/// row — never qualifies. That guard is the point: the tags are the part with
/// no information, and everything else here is the reason the file was run.
fn is_command_tag(line: &str) -> bool {
    const VERBS: [&str; 14] = [
        "INSERT", "UPDATE", "DELETE", "SELECT", "COPY", "CREATE", "ALTER", "DROP", "TRUNCATE",
        "GRANT", "REVOKE", "SET", "BEGIN", "COMMIT",
    ];
    let t = line.trim();
    let Some(verb) = t.split_whitespace().next() else {
        return false;
    };
    if !VERBS.contains(&verb) {
        return false;
    }
    t[verb.len()..]
        .split_whitespace()
        .all(|w| w.chars().all(|c| c.is_ascii_digit()) || w.chars().all(|c| c.is_ascii_uppercase()))
}

fn fold_command_tags(output: &str) -> Option<String> {
    const MIN: usize = 20;
    let mut kept: Vec<&str> = Vec::new();
    let mut folded = 0usize;
    for line in output.lines() {
        if is_command_tag(line) {
            folded += 1;
        } else {
            kept.push(line);
        }
    }
    if folded < MIN {
        return None;
    }
    let mut out = format!("[tokenade:db] {folded} statement(s) completed without error\n");
    out.push_str(kept.join("\n").trim_end());
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_script_folds_its_statement_tags_but_keeps_the_error() {
        let mut input: String = (0..400).map(|i| format!("INSERT 0 {i}\n")).collect();
        input.push_str("psql:migrate.sql:412: ERROR:  relation \"frames\" does not exist\n");
        input.push_str("LINE 1: INSERT INTO frames VALUES (1);\n");
        let out = dispatch("psql", &argv(&["psql", "-f", "migrate.sql"]), &input);
        assert!(out.contains("400 statement(s) completed"), "got:\n{out}");
        assert!(out.contains("ERROR:  relation"), "the error survives:\n{out}");
        assert!(out.contains("LINE 1: INSERT INTO frames"), "and its context:\n{out}");
        assert!(out.len() * 10 < input.len(), "a real fold: {} vs {}", out.len(), input.len());
    }

    #[test]
    fn a_data_row_is_never_mistaken_for_a_command_tag() {
        // A SELECT's result rows start with values, not verbs — but a row like
        // `UPDATE 3 rows pending` would be data, and folding it would delete
        // the answer to the query.
        for line in ["INSERT INTO frames VALUES (1);", "UPDATE 3 rows pending", "SELECT name FROM t"] {
            assert!(!super::is_command_tag(line), "must not fold: {line}");
        }
        for line in ["INSERT 0 1", "UPDATE 12", "CREATE TABLE", "COMMIT", "SET"] {
            assert!(super::is_command_tag(line), "must fold: {line}");
        }
    }
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn json_flag_routes_to_auto() {
        // `psql --json` / `sqlite3 --json` style — auto handles JSON.
        let body = r#"[{"id":1,"name":"a"},{"id":2,"name":"b"},{"id":3,"name":"c"}]"#;
        let out = dispatch("psql", &argv(&["psql", "--json"]), body);
        assert!(out.contains("name") || out.contains("id"));
    }

    #[test]
    fn mysql_e_vertical_routes_to_auto() {
        // `mysql -E` / `mysql --vertical` emits key:value pairs per row.
        let body = "*************************** 1. row ***************************\n\
                    id: 1\nname: alice\n\
                    *************************** 2. row ***************************\n\
                    id: 2\nname: bob\n";
        for flag in &["-E", "--vertical"] {
            let out = dispatch("mysql", &argv(&["mysql", flag]), body);
            assert!(out.contains("alice") || out.contains("name"));
        }
    }

    #[test]
    fn non_mysql_does_not_use_vertical_path() {
        // The `-E` / `--vertical` route ONLY fires for mysql; same
        // flags on psql/sqlite3 don't trigger (psql -E means "echo
        // hidden queries", totally different).
        let body = "id | name\n1  | alice\n2  | bob\n";
        let out = dispatch("psql", &argv(&["psql", "-E"]), body);
        // Routes through the default path; content survives.
        assert!(!out.is_empty());
        assert!(out.contains("alice") || out.contains("id"));
    }

    #[test]
    fn default_auto_wins_on_structured() {
        // Default path tries auto first; on a 30-row JSON it folds.
        let body = format!(
            "[{}]",
            (0..30).map(|i| format!("{{\"id\":{i}}}")).collect::<Vec<_>>().join(",")
        );
        let out = dispatch("sqlite3", &argv(&["sqlite3", "db.sqlite"]), &body);
        // auto folds the array into a lossless TOON block.
        assert!(out.contains("toon"));
    }

    #[test]
    fn default_falls_back_to_generic() {
        // Prose-y output where auto can't shrink → generic preserves
        // content (and dedups).
        let body = "Connecting...\n\
                    Connecting...\n\
                    Connecting...\n\
                    Connected.\n\
                    QUERY OK, 1 row affected\n";
        let out = dispatch("mysql", &argv(&["mysql"]), body);
        assert!(!out.is_empty());
        assert_eq!(out.matches("Connecting").count(), 1, "generic must dedup");
    }
}
