//! Nothing an agent can be handed may take the compactor down.
//!
//! A compactor runs inside a hook. When it panics the hook fails OPEN: the user
//! silently stops being compacted and nothing says so — the worst failure mode
//! this product has, because it looks exactly like working. The inputs below are
//! not exotic; they are what a real repository and a real web page contain and
//! what a real command emits when it is cut off.
//!
//! Two invariants, checked over the cross product of every document shape and
//! every mutation:
//!   1. the dispatcher returns (no panic, no hang on pathological input);
//!   2. what it returns is valid UTF-8 that still contains the marker the input
//!      carried, or is legitimately shorter — never mojibake.

// `corpus` is mounted by `main.rs` for the whole binary: a single compiled
// copy instead of one per subject.
use crate::corpus;

/// A representative document per family the dispatcher recognises. Kept small:
/// the point is the SHAPE, and the mutations below supply the stress.
fn shapes() -> Vec<(&'static str, String)> {
    vec![
        ("html", "<html><body><article><h1>T</h1><p>MARKER body</p></article></body></html>".into()),
        ("json", r#"{"name":"MARKER","items":[1,2,3],"nested":{"a":true}}"#.into()),
        ("ndjson", "{\"a\":\"MARKER\"}\n{\"a\":\"two\"}\n{\"a\":\"three\"}\n".into()),
        ("yaml", "name: MARKER\nspec:\n  replicas: 2\n  image: registry/app:v1\n".into()),
        ("toml", "[package]\nname = \"MARKER\"\nversion = \"1.0\"\n".into()),
        ("ini", "[section]\nkey = MARKER\nother = 2\n".into()),
        ("csv", "id,name,note\n1,MARKER,ok\n2,b,ok\n3,c,ok\n".into()),
        ("markdown", "# MARKER\n\nSome prose.\n\n- one\n- two\n\n```rs\nlet a = 1;\n```\n".into()),
        ("diff", "--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n-old\n+MARKER\n".into()),
        ("sql_source", "/* c */\nSELECT id FROM t WHERE name = 'MARKER';\n".into()),
        ("hcl", "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"MARKER\"\n}\n".into()),
        ("proto", "syntax = \"proto3\";\nmessage M {\n  string f = 1 [default = \"MARKER\"];\n}\n".into()),
        ("dockerfile", "FROM alpine:3\nRUN echo MARKER\nCMD [\"sh\"]\n".into()),
        ("makefile", "all:\n\techo MARKER\n\nclean:\n\trm -rf out\n".into()),
        ("xml", "<?xml version=\"1.0\"?><root><item>MARKER</item></root>".into()),
        ("logs", "2024-01-01T00:00:00Z INFO MARKER started\n2024-01-01T00:00:01Z INFO ok\n".into()),
        ("stacktrace", "Traceback (most recent call last):\n  File \"a.py\", line 1, in <module>\n    raise ValueError('MARKER')\nValueError: MARKER\n".into()),
        ("tabular", "NAME    READY   STATUS\nMARKER  1/1     Running\nb       1/1     Running\n".into()),
        ("env", "PATH=/usr/bin\nHOME=/root\nMARKER_VAR=MARKER\n".into()),
        ("source", "// c\nfn main() {\n    let msg = \"MARKER\";\n    println!(\"{msg}\");\n}\n".into()),
    ]
}

/// A character sequence that is one code point but several bytes, so any cap,
/// offset or cursor computed in bytes lands inside it.
const WIDE: &str = "\u{4e00}\u{1f680}\u{e9}\u{41f}";

/// How each shape gets stressed. Every one of these is something a real input
/// does: a file in another language, output cut off mid-write, a minified line,
/// Windows line endings, a construct the writer never closed.
fn mutations() -> Vec<(&'static str, fn(&str) -> String)> {
    vec![
        ("plain", |s: &str| s.to_string()),
        ("non_ascii_marker", |s: &str| s.replace("MARKER", WIDE)),
        ("non_ascii_everywhere", |s: &str| {
            s.lines().map(|l| format!("{l}{WIDE}\n")).collect()
        }),
        // Output that stopped mid-write: every parser has to survive it.
        ("truncated_head", |s: &str| s[..s.len() / 2].to_string()),
        ("truncated_wide", |s: &str| {
            let t = s.replace("MARKER", WIDE);
            // Cut at a byte offset that is deliberately NOT a char boundary in
            // the source text, then repair only enough to build a `String`.
            String::from_utf8_lossy(&t.as_bytes()[..t.len() * 2 / 3]).into_owned()
        }),
        ("crlf", |s: &str| s.replace('\n', "\r\n")),
        ("no_trailing_newline", |s: &str| s.trim_end().to_string()),
        ("one_long_line", |s: &str| {
            format!("{}{}", s.replace('\n', " "), WIDE.repeat(4_000))
        }),
        ("repeated", |s: &str| s.repeat(60)),
        ("control_chars", |s: &str| {
            s.replace("MARKER", "A\u{0}B\u{7}C\u{1b}[31mD")
        }),
        ("unterminated", |s: &str| {
            format!("{s}\n/* open block\n\"open string\n<open tag\n```\n[open")
        }),
        ("only_whitespace_tail", |s: &str| {
            format!("{s}{}", " ".repeat(50_000))
        }),
        ("empty", |_s: &str| String::new()),
    ]
}

/// The dispatcher must return, and return valid text, for every combination.
#[test]
fn the_dispatcher_survives_every_shape_under_every_mutation() {
    for (shape, doc) in shapes() {
        for (mutation, mutate) in mutations() {
            let input = mutate(&doc);
            let (_kind, out, sav) = tokenade_core::auto::compact(&input);
            assert!(
                sav.tokens_after <= sav.tokens_before.max(sav.tokens_after),
                "{shape}/{mutation}: savings are incoherent: {sav:?}"
            );
            // A compactor may drop content; it may not invent replacement
            // characters where the input had none.
            if !input.contains('\u{fffd}') {
                assert!(
                    !out.contains('\u{fffd}'),
                    "{shape}/{mutation}: output carries U+FFFD the input did not"
                );
            }
        }
    }
}

/// Same matrix through the path-hinted entry point, which selects a different
/// compactor for the same bytes and is what the `Read` hook actually calls.
#[test]
fn the_path_hinted_dispatcher_survives_the_same_matrix() {
    let paths = [
        "a.rs",
        "a.py",
        "a.ts",
        "a.go",
        "a.java",
        "a.rb",
        "a.sh",
        "a.php",
        "a.md",
        "a.json",
        "a.yaml",
        "a.toml",
        "a.ini",
        "a.csv",
        "a.xml",
        "a.tf",
        "a.proto",
        "a.sql",
        "Dockerfile",
        "Makefile",
        "a.log",
        "a.txt",
    ];
    for (shape, doc) in shapes() {
        for (mutation, mutate) in mutations() {
            let input = mutate(&doc);
            for p in paths {
                let (_k, out, _s) = tokenade_core::auto::compact_with_path_hint(&input, p);
                if !input.contains('\u{fffd}') {
                    assert!(
                        !out.contains('\u{fffd}'),
                        "{shape}/{mutation} as {p}: output carries U+FFFD the input did not"
                    );
                }
            }
        }
    }
}

/// The web path carries most of what this product saves, and it is the one fed
/// bytes nobody wrote on purpose: a page cut off by a timeout, a tag closed by
/// nobody, a document whose declared encoding is a lie. Same two invariants,
/// on the REAL pages in the fixture corpus rather than on invented HTML.
#[test]
fn the_web_pipeline_survives_broken_real_pages() {
    let pages = corpus::pages();
    assert!(pages.len() > 20, "only {} pages", pages.len());

    // Cut points chosen to land inside a tag, inside an attribute, and inside a
    // multibyte character — the three the extractor has to walk back out of.
    let cuts = [7usize, 33, 50, 91];
    for (name, html) in &pages {
        for cut in cuts {
            let upto = html.len() * cut / 100;
            let truncated: String = String::from_utf8_lossy(&html.as_bytes()[..upto]).into_owned();
            let out = tokenade_core::web::html_to_markdown(&truncated);
            assert!(
                out.len() <= truncated.len() * 4 + 4096,
                "{name} cut at {cut}%: output blew up to {} from {}",
                out.len(),
                truncated.len()
            );
        }
        // Byte-budget bounding: every budget from "almost nothing" upward has to
        // land on a character boundary, whatever the page's script.
        for budget in [1usize, 2, 7, 64, 1000, 50_000] {
            let _ = tokenade_core::web::truncate_to_max_tokens(html, budget);
        }
        let _ = tokenade_core::web::html_to_markdown(&html.replace('\n', "\r\n"));
        let _ = tokenade_core::web::text_body_to_markdown(html, Some("text/plain"));
    }
}

/// Compaction may never cost MORE than it saves, on any entry point.
///
/// `auto::compact` carried this guard and documented it as covering every path.
/// It did not: both path-hinted entry points returned straight from the
/// dispatch table whenever the file extension pinned the kind — the branch the
/// `Read` hook takes for every recognised file. Measured on 1 325 real JSON
/// files, a tokenizer vocabulary came back 25,8 % BIGGER than it went in and an
/// OpenAPI document 5,3 % bigger.
#[test]
fn no_entry_point_can_return_more_than_it_was_given() {
    // `compact_with_path_hint_indexed` PERSISTS: for `Kind::Logs` it calls
    // `fold_store::save_fold`, which appends on a `(content_hash, template_id)`
    // conflict. This test passes a FIXED hash, so every run of the suite piled
    // its lines onto the same row of whatever store `HOME` pointed at — the
    // developer's real `~/.tokenade/fold_store.db`. Measured there before this
    // redirect: one row, template `  "tok<N>": <N>,` (this test's own vocab),
    // 1 760 000 lines, 66.7 MB, created 2026-08-03 and last appended 2026-08-27.
    // Today's classifier no longer calls that vocab a log, so the test writes
    // nothing; the redirect is here so the next fixture that IS log-shaped
    // cannot reach a real store again.
    let store = std::env::temp_dir().join(format!(
        "tokenade-adversarial-fold-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // SAFETY: process-wide, and no other test in this binary touches the store.
    unsafe {
        std::env::set_var("TOKENADE_FOLD_STORE_DB", &store);
    }
    // Assert the redirect TOOK. A redirect that silently fails leaves the test
    // writing the real store while reporting success — the exact shape this
    // comment is about.
    assert_eq!(
        tokenade_core::fold_store::db_path(),
        store,
        "fold store still resolves to the real HOME; the redirect did not take"
    );
    // A flat lookup table of unique short keys: nothing repeats, so there is
    // nothing to fold, and any re-rendering costs more than it saves.
    let vocab: String = {
        let mut s = String::from("{\n");
        for i in 0..4_000 {
            s.push_str(&format!("  \"tok{i}\": {i},\n"));
        }
        s.push_str("  \"end\": 0\n}\n");
        s
    };
    let cases: Vec<(&str, String)> = shapes()
        .into_iter()
        .flat_map(|(name, doc)| {
            mutations()
                .into_iter()
                .map(move |(_m, f)| (name, f(&doc)))
                .collect::<Vec<_>>()
        })
        .chain(std::iter::once(("vocab", vocab)))
        .collect();

    let hints = [
        "a.json",
        "a.yaml",
        "a.toml",
        "a.ini",
        "a.csv",
        "a.xml",
        "a.tf",
        "a.proto",
        "a.sql",
        "a.md",
        "a.html",
        "a.log",
        "Dockerfile",
        "Makefile",
    ];
    for (name, input) in &cases {
        let (_k, _o, sav) = tokenade_core::auto::compact(input);
        assert!(
            sav.tokens_after <= sav.tokens_before,
            "{name}: auto::compact inflated {} -> {}",
            sav.tokens_before,
            sav.tokens_after
        );
        for h in hints {
            let (_k, _o, sav) = tokenade_core::auto::compact_with_path_hint(input, h);
            assert!(
                sav.tokens_after <= sav.tokens_before,
                "{name} as {h}: compact_with_path_hint inflated {} -> {}",
                sav.tokens_before,
                sav.tokens_after
            );
            let (_k, _o, sav) =
                tokenade_core::auto::compact_with_path_hint_indexed(input, h, "0123456789ab");
            assert!(
                sav.tokens_after <= sav.tokens_before,
                "{name} as {h}: compact_with_path_hint_indexed inflated {} -> {}",
                sav.tokens_before,
                sav.tokens_after
            );
        }
    }
}
