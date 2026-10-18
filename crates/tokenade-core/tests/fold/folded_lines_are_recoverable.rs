//! **A fold that promises a hash must have one.**
//!
//! The fold footer comes from the world of COMMANDS: it points to a
//! `full output:` banner and, failing that, to `tokenade raw <cmd>`. A page
//! has neither. MEASURED 2026-09-09 on a 3,000-row catalogue: **2,824 folded
//! lines**, and the output pointed to a missing banner then to a command that
//! does not exist — no way to recover the rows.
//!
//! Hand-written fixture. This line used to say "**none of the corpus's 39
//! pages reaches the hard ceiling**" and that is no longer true: `pages/zhwiki`
//! reaches it, and that is why it entered the corpus. A Chinese article of
//! 86,000 characters is worth ~86,000 tokens — the estimator counts CJK at
//! ~1 token/character — where its English equivalent of 92,000 characters is
//! worth ~23,000. The fixture stays: it reaches the rule in three lines of
//! code and without a megabyte.
//!
//! And what the footer names has changed. It pointed to the body as fetched,
//! i.e. to HTML the caller must re-distil itself; it now points to the FULL
//! MARKDOWN, the requested form.

fn catalogue(rows: usize) -> String {
    let mut html = String::from("<html><head><title>Catalogue</title></head><body><h1>Catalogue</h1>\
                                 <table><thead><tr><th>Ref</th><th>Nom</th><th>Prix</th></tr></thead><tbody>");
    for i in 0..rows {
        html.push_str(&format!(
            "<tr><td>{i}</td><td>Produit {i} de la gamme professionnelle</td><td>{},50 €</td></tr>",
            i * 7
        ));
    }
    html.push_str("</tbody></table></body></html>");
    html
}

#[test]
fn a_page_that_folds_lines_says_where_every_line_is() {
    let d = tokenade_core::web_pipeline::distill_page(
        &catalogue(3_000),
        "https://exemple-test.com/catalogue",
        Some("text/html; charset=utf-8"),
    );
    // The fixture does REACH the rule.
    assert!(
        d.md.contains("[tokenade folded"),
        "nothing was folded, the fixture proves nothing:\n{:.300}",
        d.md
    );
    // The promise from the world of commands is gone — checked on the words
    // THEMSELVES and not only through `fold_footer_promise()`. An assertion
    // that queries the same function as the code does not see a sentence
    // shortened on both sides: the mutation that cut the command out of the
    // promise SURVIVED the first version of this test.
    for orphan in ["tokenade raw <cmd>", "full output:"] {
        assert!(
            !d.md.contains(orphan),
            "\"{orphan}\" remains in the output of a PAGE, which has no command:\n{}",
            d.md.lines().last().unwrap_or_default()
        );
    }
    assert!(
        !d.md
            .contains(&tokenade_core::elision::fold_footer_promise()),
        "the output still points to a banner and a command that do not exist:\n{}",
        d.md.lines().last().unwrap_or_default()
    );
    // …and what replaces it is TRUE: the named file carries a row the fold
    // removed.
    let last = d.md.lines().last().unwrap_or_default().to_string();
    assert!(last.contains("expand-ref"), "{last}");
    assert!(last.contains("--prompt"), "{last}");
    assert!(
        !last.contains("--raw"),
        "the footer must not push the dump: {last}"
    );
    let path = last
        .rsplit_once('(')
        .and_then(|(_, p)| p.split_once(')'))
        .map(|(p, _)| p.to_string())
        .unwrap_or_else(|| panic!("the footer names no file: {last}"));
    let stashed = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(
        stashed.contains("Produit 1500 de la gamme"),
        "the stash does not carry the folded row: {path}"
    );
}
