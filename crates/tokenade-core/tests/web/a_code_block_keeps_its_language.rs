//! **The page says which language its code block is in; tokenade kept quiet
//! about it.**
//!
//! A markdown fence carries an "info string" — ` ```yaml ` — and that is what
//! tells an agent whether it holds a manifest, a shell command or Python.
//! tokenade read it from `class="language-…"` and only there.
//!
//! MEASURED 2026-09-10 on the 39 pages, three families were missing:
//!  * **the unquoted attribute value** — HTML allows it and Hugo/Chroma
//!    writes it that way (`<code class=language-yaml data-lang=yaml>`): 39
//!    blocks of the Kubernetes documentation;
//!  * **the language carried by the WRAPPER** — Pygments, hence Sphinx,
//!    renders `<div class="highlight-python"><div class="highlight"><pre>`,
//!    and it is the `<pre>` alone that stashing takes: 84 blocks (Django 53,
//!    Python glossary 21, Wikipedia 6, Kubernetes 4);
//!  * **`brush: js`**, the SyntaxHighlighter convention MDN still writes.
//!
//! **95 fences gain their language** for 489 bytes, and every line that
//! changes in the five diffs is a fence — nothing else moves.

use std::path::PathBuf;

fn distilled_page(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pages")
        .join(format!("{name}.html"));
    let html = std::fs::read_to_string(&p).expect("the page");
    tokenade_core::web_pipeline::distill_page(
        &html,
        &format!("https://example.test/{name}"),
        Some("text/html; charset=utf-8"),
    )
    .md
}

/// Fence opening lines, at whatever indent: one inside a list item sits at the
/// item's content column.
fn fences(md: &str) -> Vec<&str> {
    md.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("```") && l.len() > 3)
        .collect()
}

#[test]
fn each_generator_convention_is_read() {
    for (page, expected, at_least) in [
        ("k8s_doc", "```shell", 40),       // unquoted attribute value
        ("py_glossary", "```python3", 20), // language on the Pygments wrapper
        ("django_leaf", "```pycon", 12),
        ("mdn_doc", "```js", 4),         // `brush: js`
        ("wikipedia_art", "```rust", 5), // `mw-highlight-lang-rust`
    ] {
        let md = distilled_page(page);
        let n = fences(&md).iter().filter(|l| **l == expected).count();
        assert!(
            n >= at_least,
            "{page}: {n} `{expected}` fence(s), expected at least {at_least}"
        );
    }
}

#[test]
fn nothing_claims_a_language_the_page_does_not_name() {
    // **`default` is not a language.** Sphinx writes it when the directive
    // names none, and announcing it would be a false claim — worse than a
    // bare fence. 37 blocks on the Django documentation.
    for page in [
        "django_leaf",
        "py_glossary",
        "k8s_doc",
        "wikipedia_art",
        "mdn_doc",
    ] {
        let md = distilled_page(page);
        for f in fences(&md) {
            let lang = &f[3..];
            assert!(
                !matches!(lang, "default" | "none" | "auto" | "nohighlight"),
                "{page} announces `{lang}`, which is not a language"
            );
            assert!(
                !lang.starts_with("lang-") && !lang.starts_with("language-"),
                "{page} announces `{lang}`: the prefix was not consumed"
            );
        }
    }
}
