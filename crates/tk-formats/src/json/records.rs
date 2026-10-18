//! **Records: a list of same-kind objects, as a table of what varies.**
//!
//! Shared by the two places records arrive: one object per line (NDJSON —
//! `yt-dlp -j`, `jq -c`, API dumps) and a JSON array (`[{…}, …]`, or under a
//! payload key such as `entries`). Tolerant of a key missing on some records,
//! which the schema fold is not: a playlist where some videos carry no
//! `channel` fell out of it, and came back as minified JSON cut off after the
//! first record.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;

/// One level of a record flattened into columns: an object's scalars become
/// `user.login`, `user.type`; an array of scalars, one joined value; an array
/// of objects that all NAME themselves (`name`, `login`, `title`, `key`), the
/// joined names — `labels[].name`. On a real GitHub issue list the author and
/// the labels were `nested, not shown`, the first two things asked of an
/// issue. Anything deeper is named in `deep`.
fn flatten(
    o: &serde_json::Map<String, Value>,
    deep: &mut BTreeSet<String>,
) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for (k, v) in o {
        match v {
            Value::Object(inner) => {
                for (c, cv) in inner {
                    if cv.is_object() || cv.is_array() {
                        deep.insert(format!("{k}.{c}"));
                    } else {
                        out.insert(format!("{k}.{c}"), cv.clone());
                    }
                }
            }
            Value::Array(items) => {
                if items.iter().all(|i| !i.is_object() && !i.is_array()) {
                    let joined: Vec<String> = items.iter().filter_map(scalar_text).collect();
                    out.insert(format!("{k}[]"), Value::String(joined.join(", ")));
                } else if let Some(field) =
                    ["name", "login", "title", "key"].into_iter().find(|f| {
                        items
                            .iter()
                            .all(|i| i.get(f).is_some_and(|x| x.is_string()))
                    })
                {
                    let joined: Vec<String> = items
                        .iter()
                        .filter_map(|i| i.get(field).and_then(scalar_text))
                        .collect();
                    out.insert(format!("{k}[].{field}"), Value::String(joined.join(", ")));
                } else {
                    deep.insert(k.clone());
                }
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// A column of opaque tokens: every non-empty value one identifier-shaped
/// word of 16+ chars mixing letters and digits (node ids, hashes, base64), or
/// a number of 8+ digits (database ids). A timestamp has `:` and is not one;
/// an issue number (6 digits) is not one.
fn opaque(values: &[String]) -> bool {
    let present: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
    !present.is_empty()
        && present.iter().all(|v| {
            let token = v.chars().count() >= 16
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-+/=".contains(c))
                && v.chars().any(|c| c.is_ascii_digit())
                && v.chars().any(|c| c.is_ascii_alphabetic());
            let db_id = v.len() >= 8 && v.bytes().all(|b| b.is_ascii_digit());
            token || db_id
        })
}

/// A list of names, the first few then how many more.
fn few(names: &[String]) -> String {
    const SHOWN: usize = 8;
    let head = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        format!("{head} (+{} more)", names.len() - SHOWN)
    } else {
        head
    }
}

/// Columns a record table shows before naming the rest.
const RECORD_COLUMNS: usize = 8;

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".into()),
        _ => None,
    }
}

/// **A stream of RECORDS reads as a table of what varies.**
///
/// `yt-dlp --flat-playlist -j` writes one 2 KB object per video, nine tenths
/// of it the same on every line (`playlist_*`, the channel, three thumbnail
/// URLs); the log view kept five sample lines and a key list — the titles,
/// ids and durations of 241 of 246 videos were nowhere (measured on a real
/// playlist, 556 107 bytes). Here: the keys that never change, once, with
/// their value; the nested ones by name; then one row per record of the
/// short scalars that do change. Nothing a row could rebuild is lost.
pub fn records_table(
    objs: &[serde_json::Map<String, Value>],
    source: &str,
    noun: &str,
) -> Option<String> {
    if objs.len() < 10 {
        return None;
    }
    let source_keys = objs
        .iter()
        .flat_map(|o| o.keys())
        .collect::<BTreeSet<_>>()
        .len();
    let mut deep: BTreeSet<String> = BTreeSet::new();
    let flat: Vec<serde_json::Map<String, Value>> =
        objs.iter().map(|o| flatten(o, &mut deep)).collect();
    let objs = &flat[..];
    let mut order: Vec<String> = Vec::new();
    for o in objs {
        for k in o.keys() {
            if !order.contains(k) {
                order.push(k.clone());
            }
        }
    }
    // `serde_json` here keeps keys SORTED; the source's order (title first,
    // as the tool wrote it) is read back from the source text (`"key":`,
    // `"key" :` once pretty-printed — the first occurrence decides).
    let at = |k: &str| {
        let a = source.find(&format!("\"{k}\":")).unwrap_or(usize::MAX);
        let b = source.find(&format!("\"{k}\" :")).unwrap_or(usize::MAX);
        a.min(b)
    };
    // A flattened `user.login` sorts with its parent, then by the child's own
    // first appearance.
    order.sort_by_key(|k| match k.split_once('.') {
        Some((parent, child)) => (at(parent.trim_end_matches("[]")), at(child)),
        None => (at(k), 0),
    });
    let (mut constant, mut nested, mut varying, mut long) = (
        Vec::new(),
        deep.into_iter().collect::<Vec<_>>(),
        Vec::new(),
        Vec::new(),
    );
    for k in &order {
        let vals: Vec<Option<&Value>> = objs.iter().map(|o| o.get(k)).collect();
        if vals
            .iter()
            .any(|v| v.is_some_and(|v| v.is_object() || v.is_array()))
        {
            nested.push(k.clone());
            continue;
        }
        let texts: Vec<String> = vals
            .iter()
            .map(|v| v.and_then(scalar_text).unwrap_or_default())
            .collect();
        if texts.iter().all(|t| *t == texts[0]) && vals.iter().all(|v| v.is_some()) {
            // Said once — and SHORT: a constant 300-char body is still 300 chars.
            let v: String = texts[0].chars().take(60).collect();
            let more = if texts[0].chars().count() > 60 {
                "…"
            } else {
                ""
            };
            constant.push(format!("{k}={v}{more}"));
        } else if texts.iter().map(|t| t.chars().count()).sum::<usize>() / texts.len() > 100 {
            long.push(k.clone());
        } else {
            varying.push(k.clone());
        }
    }
    if varying.is_empty() {
        return None;
    }
    // Which columns: the ones that tell records apart (most distinct values,
    // shortest first), skipping one that only restates a column already in —
    // it CONTAINS it on every row (`url` = `…watch?v=` + `id`), or maps one to
    // one with it where values repeat (`duration_string` ↔ `duration`).
    // Shown in key order.
    let col = |k: &str| -> Vec<String> {
        objs.iter()
            .map(|o| o.get(k).and_then(scalar_text).unwrap_or_default())
            .collect()
    };
    let mut ranked: Vec<(String, usize, usize)> = varying
        .iter()
        .map(|k| {
            let c = col(k);
            let distinct = c.iter().collect::<BTreeSet<_>>().len();
            let avg = c.iter().map(|t| t.chars().count()).sum::<usize>() / c.len();
            (k.clone(), distinct, avg)
        })
        .collect();
    // An opaque identifier (`I_kwDOAAsO6M8…`, a 40-hex hash) tells rows
    // apart but says nothing to a reader: it goes last, after the columns
    // that carry meaning — the author, the labels, the comment count.
    ranked.sort_by(|a, b| {
        // Then words before bare numbers: of `user.login` and `user.id`,
        // which restate each other, the one kept is the one a reader knows.
        let numeric = |k: &str| {
            col(k).iter().all(|v| {
                v.is_empty()
                    || v.bytes()
                        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            })
        };
        let (oa, ob) = (opaque(&col(&a.0)), opaque(&col(&b.0)));
        oa.cmp(&ob)
            .then(numeric(&a.0).cmp(&numeric(&b.0)))
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
    });
    let restates = |k: &[String], c: &[String]| -> bool {
        // Containment needs an id-like value: `Challenge 7` contains `7`.
        let contains = k
            .iter()
            .zip(c)
            .all(|(a, b)| b.chars().count() >= 6 && a.len() > b.len() && a.contains(b.as_str()));
        let mut fwd: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        let mut back: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        let bijection = k.iter().zip(c).all(|(a, b)| {
            *fwd.entry(a.as_str()).or_insert(b.as_str()) == b.as_str()
                && *back.entry(b.as_str()).or_insert(a.as_str()) == a.as_str()
        });
        // All-unique columns map one to one with ANY other all-unique column
        // (`id` ↔ `title`): only a column that repeats can show it restates.
        let repeats = k.iter().collect::<BTreeSet<_>>().len() < k.len();
        k == c || contains || (bijection && repeats)
    };
    // The KEY first: the shortest column whose values are all distinct and
    // readable (`number` of an issue, `playlist_index` of a video). Words
    // before numbers would otherwise push it out of the table.
    if let Some(pos) = ranked
        .iter()
        .enumerate()
        .filter(|(_, (k, d, _))| {
            *d == objs.len() && !opaque(&col(k)) && col(k).iter().all(|v| !v.is_empty())
        })
        .min_by_key(|(_, (_, _, avg))| *avg)
        .map(|(i, _)| i)
    {
        let key = ranked.remove(pos);
        ranked.insert(0, key);
    }
    // A column that WRAPS another on every row (`url` = `…/issues/` +
    // `number`) restates it whatever their rank: decided over all pairs, so
    // the wrapped value — the short one — is the one kept.
    let wraps = |k: &[String], c: &[String]| {
        k.iter()
            .zip(c)
            .all(|(a, b)| b.chars().count() >= 6 && a.len() > b.len() && a.contains(b.as_str()))
    };
    let columns: Vec<(String, Vec<String>)> =
        ranked.iter().map(|(k, _, _)| (k.clone(), col(k))).collect();
    let wrappers: BTreeSet<String> = columns
        .iter()
        .filter(|(k, v)| columns.iter().any(|(c, w)| c != k && wraps(v, w)))
        .map(|(k, _)| k.clone())
        .collect();
    let mut chosen: Vec<(String, Vec<String>)> = Vec::new();
    let mut redundant: Vec<String> = Vec::new();
    for (k, _, _) in &ranked {
        let c = col(k);
        if wrappers.contains(k) || chosen.iter().any(|(_, prev)| restates(&c, prev)) {
            redundant.push(k.clone());
        } else if chosen.len() < RECORD_COLUMNS {
            chosen.push((k.clone(), c));
        }
    }
    let shown: Vec<String> = varying
        .iter()
        .filter(|k| chosen.iter().any(|(c, _)| c == *k))
        .cloned()
        .collect();
    let mut out = String::new();
    let _ = writeln!(out, "{} {noun} · {source_keys} keys", objs.len());
    if !constant.is_empty() {
        let _ = writeln!(out, "same on every record: {}", constant.join(" · "));
    }
    let mut unshown: Vec<String> = varying
        .iter()
        .filter(|k| !shown.contains(k) && !redundant.contains(k))
        .cloned()
        .collect();
    unshown.extend(long);
    if !redundant.is_empty() {
        let _ = writeln!(
            out,
            "restating a column (not repeated): {}",
            few(&redundant)
        );
    }
    if !unshown.is_empty() {
        let _ = writeln!(out, "also varying, not in the table: {}", few(&unshown));
    }
    if !nested.is_empty() {
        let _ = writeln!(out, "nested, not shown: {}", few(&nested));
    }
    let _ = writeln!(
        out,
        "\n| {} |\n|{}",
        shown.join(" | "),
        " --- |".repeat(shown.len())
    );
    for o in objs {
        let row: Vec<String> = shown
            .iter()
            .map(|k| {
                o.get(k)
                    .and_then(scalar_text)
                    .map(|t| t.replace('|', "\\|").replace('\n', " "))
                    .unwrap_or_default()
            })
            .collect();
        let _ = writeln!(out, "| {} |", row.join(" | "));
    }
    // The bound the JSON folds already hold to (`showing_anomalies_stays_
    // bounded`): a third of the source. Rows that each carry their own long,
    // differently-keyed fields stay with the folds that bound them; a real
    // playlist dump is at 4 %.
    let out = out.trim_end().to_string();
    (out.len() * 3 < source.len()).then_some(out)
}
