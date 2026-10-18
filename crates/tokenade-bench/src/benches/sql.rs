//! `sql` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Realistic 500-row psql query result with multi-column data and the
/// `(500 rows)` footer.
pub(crate) fn task_sql_psql(results: &mut Vec<RunResult>) {
    let mut t = String::from(
        " id |     name      |      email           |     created_at      | status\n----+---------------+----------------------+---------------------+--------\n",
    );
    for i in 0..500 {
        t.push_str(&format!(
            " {i:3} | user_{i:04}    | user_{i:04}@x.com    | 2024-01-01 12:34:56 | active\n"
        ));
    }
    t.push_str("(500 rows)\n");
    let (out, sav, ms) = measure(&t, sql::compact_with_savings);
    // Quality: header preserved, first rows kept, separator dropped, footer
    // preserved, cap marker present.
    let header_kept = out.contains("id | name | email | created_at | status");
    let row_kept = out.contains("0 | user_0000");
    let sep_dropped = !out.contains("----+");
    let footer_kept = out.contains("(500 rows)");
    let cap_marker = out.contains("more row(s) elided");
    let quality = if header_kept && row_kept && sep_dropped && footer_kept && cap_marker {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:sql-compact",
        "sql",
        "psql-500-rows",
        &t,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #37: a `SELECT *` returning binary-ish blobs in a column —
// every cell value (including `NULL`, empty strings, and emoji) must survive
// the column-split unchanged.
pub(crate) fn adv_sql_unicode_and_nulls(results: &mut Vec<RunResult>) {
    let t = " id |   name    | status\n----+-----------+--------\n  1 | alice 🦀  | active\n  2 | NULL      | NULL\n  3 |           | dormant\n  4 | café      | (null)\n(4 rows)\n";
    let (out, _sav, ms) = measure(t, sql::compact_with_savings);
    let quality = if out.contains("alice 🦀")
        && out.contains("café")
        && out.contains("(4 rows)")
        && !out.contains("----+")
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:sql:unicode-and-nulls",
        "sql",
        "adversarial",
        t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #38: a 5 000-row mysql-style result must compact > 90 % AND
// finish in under 500 ms.
pub(crate) fn adv_sql_huge_mysql(results: &mut Vec<RunResult>) {
    let mut t = String::from("+----+--------+\n| id | name   |\n+----+--------+\n");
    for i in 0..5000 {
        t.push_str(&format!("| {i:>4} | row_{i:04} |\n"));
    }
    t.push_str("+----+--------+\n5000 rows in set (0.12 sec)\n");
    let (out, sav, ms) = measure(&t, sql::compact_with_savings);
    let quality = if out.contains("0 | row_0000")
        && out.contains("more row(s) elided")
        && out.contains("5000 rows in set")
        && !out.contains("+----+")
        && sav.percent() > 90.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:sql:huge-mysql-5000",
        "sql",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}
