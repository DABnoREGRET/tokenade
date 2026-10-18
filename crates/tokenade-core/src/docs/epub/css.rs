//! **The classes a book's style sheet does not paint.**
//!
//! `<p class="gone">` with `.gone { display: none }` in the book's sheet:
//! the e-reader shows nothing, our reader served the text (it used to read only
//! the attributes). A deliberately narrow reading — what a selector says WITHOUT
//! ambiguity:
//! * a top-level rule only: those of an `@media` depend on
//!   the screen or print, those of an `@supports` on the engine;
//! * a selector made of ONE class, bare (`.gone`) or on an element
//!   (`p.gone`) — no descendant, child, pseudo-class or attribute,
//!   which only hide in a context;
//! * `display: none` in its declarations.

use std::collections::HashSet;

/// The classes hidden by a style sheet.
pub(super) fn hidden_classes(css: &str) -> HashSet<String> {
    let css = super::super::markup_marks::re(r"(?s)/\*.*?\*/").replace_all(css, "");
    let mut out = HashSet::new();
    let mut depth = 0usize;
    let mut prelude = String::new();
    let mut body = String::new();
    for c in css.chars() {
        match c {
            '{' => {
                depth += 1;
                if depth > 1 {
                    body.push(c);
                }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let decls: String = body
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .to_ascii_lowercase();
                    // The prelude of an `@media` is not a class, and its inner
                    // rules are not read: nothing comes out of it.
                    if decls.contains("display:none") {
                        out.extend(prelude.split(',').filter_map(single_class));
                    }
                    prelude.clear();
                    body.clear();
                } else {
                    body.push(c);
                }
            }
            // `@import url(…);` and its kind open no block.
            ';' if depth == 0 => prelude.clear(),
            c if depth == 0 => prelude.push(c),
            c => body.push(c),
        }
    }
    out
}

/// `.gone` or `p.gone` → `gone`; any other selector → `None`.
fn single_class(selector: &str) -> Option<String> {
    let s = selector.trim();
    let (element, class) = s.split_once('.')?;
    let is_name = |n: &str| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    (element.is_empty() || is_name(element)).then_some(())?;
    is_name(class).then(|| class.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_class_rule_that_says_display_none_hides() {
        let css = r#"
            @import url("base.css");
            /* .commented { display: none } */
            .gone, p.also-gone { color: red; display : NONE !important }
            .shown { display: block }
            div .descendant { display: none }
            .a.b { display: none }
            a.link:hover { display: none }
            @media print { .print-only { display: none } }
            @supports (display: grid) { .grid-fallback { display: none } }
            .after-blocks { display:none }
        "#;
        let mut got: Vec<String> = hidden_classes(css).into_iter().collect();
        got.sort();
        assert_eq!(got, ["after-blocks", "also-gone", "gone"]);
    }
}
