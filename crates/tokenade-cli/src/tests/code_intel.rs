//! Tests for `code_intel`, split out of the flat `tests.rs`.

// #944 lock-in — `parse_context_flag` must accept both
// `--context N` (space-separated) and `--context=N` (GNU
// equals-form). Pre-#944 only the space form worked; agents
// translating from `grep -A=N` hit silent failures.
#[test]
fn parse_context_flag_accepts_space_form() {
    let args = vec!["--context".to_string(), "3".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Ok(3));
}

#[test]
fn parse_context_flag_accepts_equals_form() {
    let args = vec!["--context=3".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Ok(3));
}

#[test]
fn parse_context_flag_accepts_short_equals_form() {
    let args = vec!["-C=5".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Ok(5));
}

#[test]
fn parse_context_flag_default_zero_when_absent() {
    let args = vec!["foo".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Ok(0));
}

#[test]
fn parse_context_flag_rejects_non_integer() {
    let args = vec!["--context=abc".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Err(2));
    let args = vec!["--context".to_string(), "abc".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Err(2));
}

#[test]
fn parse_context_flag_rejects_over_cap() {
    // Cap is 10 — keeps symbol_find from re-reading whole files.
    let args = vec!["--context=11".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Err(2));
    let args = vec!["--context".to_string(), "100".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Err(2));
}

#[test]
fn parse_context_flag_rejects_missing_value_space_form() {
    let args = vec!["--context".to_string()];
    assert_eq!(super::parse_context_flag(&args, "query"), Err(2));
}

// #944 lock-in for the validator's equals-form acceptance: pre-fix
// `--context=2 fn` was rejected as "unknown flag '--context=2'"
// (validator did exact-string match against the value-flag list).
#[test]
fn validator_accepts_equals_form_value_flag() {
    // `--context=2` is recognised as a value-flag (one slot),
    // leaving `fn` as the positional. Pre-fix this returned the
    // unknown-flag error.
    let args = vec!["--context=2".to_string(), "fn".to_string()];
    let r = super::validate_single_positional_with_values(
        &args,
        "query",
        &["--json"],
        &["--context", "-C"],
        "symbol",
    );
    assert_eq!(r, Ok("fn".to_string()));
}

#[test]
fn validator_still_accepts_space_form_value_flag() {
    let args = vec!["--context".to_string(), "2".to_string(), "fn".to_string()];
    let r = super::validate_single_positional_with_values(
        &args,
        "query",
        &["--json"],
        &["--context", "-C"],
        "symbol",
    );
    assert_eq!(r, Ok("fn".to_string()));
}

#[test]
fn validator_rejects_value_flag_followed_by_another_flag() {
    // #943 lock-in: `--context --json fn` must NOT silently
    // consume `--json` as the context value.
    let args = vec![
        "--context".to_string(),
        "--json".to_string(),
        "fn".to_string(),
    ];
    let r = super::validate_single_positional_with_values(
        &args,
        "query",
        &["--json"],
        &["--context", "-C"],
        "symbol",
    );
    assert_eq!(r, Err(2));
}

#[test]
fn validator_rejects_whitespace_only_positional() {
    // #946 lock-in: `tokenade skeleton "   "` (shell expansion of
    // $EMPTY_VAR) used to reach `fs::read_to_string("   ")`, which
    // returns a confusing bare-OS error. The central validator
    // trim-rejects so all 30+ callers get a consistent diagnostic.
    let args = vec!["   ".to_string()];
    let r = super::validate_single_positional(&args, "skeleton", &[], "file");
    assert_eq!(r, Err(2));
}

#[test]
fn validator_rejects_empty_positional() {
    // Sibling of the above: bare `""` (empty string) was also
    // accepted pre-#946. Same trim-reject branch covers it.
    let args = vec!["".to_string()];
    let r = super::validate_single_positional(&args, "skeleton", &[], "file");
    assert_eq!(r, Err(2));
}

#[test]
fn validator_accepts_normal_positional_post_946() {
    // Regression guard: trim-reject must not break the happy path.
    let args = vec!["crates/foo/src/lib.rs".to_string()];
    let r = super::validate_single_positional(&args, "skeleton", &[], "file");
    assert_eq!(r, Ok("crates/foo/src/lib.rs".to_string()));
}

/// `--budget N` caps what a code-intel command prints. A malformed value must
/// fail loudly: a caller who believes a ceiling is in force when it is not is
/// worse off than one with no ceiling at all.
#[test]
fn budget_flag_parses_both_spellings_and_rejects_junk() {
    use crate::code_intel::parse_budget;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(parse_budget(&s(&["a.rs"]), "skeleton"), Ok(None));
    assert_eq!(
        parse_budget(&s(&["--budget", "500", "a.rs"]), "skeleton"),
        Ok(Some(500))
    );
    assert_eq!(
        parse_budget(&s(&["--budget=500", "a.rs"]), "skeleton"),
        Ok(Some(500))
    );
    assert!(parse_budget(&s(&["--budget", "wat"]), "skeleton").is_err());
    assert!(parse_budget(&s(&["--budget"]), "skeleton").is_err());
    // `0` used to be rejected, back when NO budget was the default and zero
    // could only be a typo. The ceiling is a default now, so zero is the only
    // way to ask for every byte — a caller must not have to guess a number
    // large enough to mean "all".
    assert_eq!(
        parse_budget(&s(&["--budget", "0", "a.rs"]), "skeleton"),
        Ok(Some(0)),
        "0 is the explicit no-ceiling escape, not a typo"
    );
}

/// The packer takes the longest PREFIX that fits, never a subset: these lists
/// are ranked, so resuming after a skipped item would hand the caller ranks
/// 1-3 and 7-8 while looking exactly like a complete top-5.
#[test]
fn budget_packs_a_prefix_and_reports_what_it_withheld() {
    use crate::code_intel::{Admission, TokenBudget};
    let item = "a".repeat(400);
    // Derive the ceiling from the estimator rather than hard-coding one: this
    // test is about prefix semantics, and a recalibration of the token model
    // must not silently turn "room for two" into "room for three".
    let per = tokenade_core::tokenizer::estimate(&item);
    let mut b = TokenBudget::new(Some(per * 2 + per / 2));
    assert_eq!(
        b.admit_or_degrade(&item, &item),
        Admission::Full,
        "first item always fits"
    );
    assert_eq!(b.admit_or_degrade(&item, &item), Admission::Full);
    assert_eq!(
        b.admit_or_degrade(&item, &item),
        Admission::Refused,
        "third item overruns the ceiling"
    );
    // A later SMALL item must not sneak in behind a refused big one.
    assert_eq!(
        b.admit_or_degrade("x", "x"),
        Admission::Refused,
        "prefix semantics: no resuming after a cut"
    );
    let footer = b.footer();
    assert!(footer.contains("2 shown"), "got {footer}");
    assert!(footer.contains("2 withheld"), "got {footer}");
    assert!(footer.contains("--budget "), "suggests a raise: {footer}");
    assert!(
        !footer.contains("of 250"),
        "spend and ceiling must read as two facts, not a fraction that can invert: {footer}"
    );
}

/// An item larger than the whole budget still prints. A command that answers
/// nothing and explains why it answered nothing has spent tokens for no value.
#[test]
fn budget_never_yields_an_empty_answer() {
    use crate::code_intel::{Admission, TokenBudget};
    let huge = "z".repeat(40_000);
    let mut b = TokenBudget::new(Some(10));
    assert_eq!(b.admit_or_degrade(&huge, &huge), Admission::Full);
    assert!(b.footer().is_empty(), "nothing was withheld");
}

/// An item that does not fit shrinks to a one-line stand-in rather than
/// vanishing. A silent omission reads as "that file has nothing in it", which
/// is the one reading the caller cannot check.
#[test]
fn budget_degrades_an_item_it_cannot_afford_in_full() {
    use crate::code_intel::{Admission, TokenBudget};
    let full = "a".repeat(400);
    let short = "==> f <== (12 signatures, withheld)";
    let per = tokenade_core::tokenizer::estimate(&full);
    let mut b = TokenBudget::new(Some(per * 2 + per / 2));
    assert_eq!(b.admit_or_degrade(&full, short), Admission::Full);
    assert_eq!(b.admit_or_degrade(&full, short), Admission::Full);
    assert_eq!(
        b.admit_or_degrade(&full, short),
        Admission::Degraded,
        "third overruns in full but its summary fits"
    );
    // Quality only decreases: no later item comes back in full.
    assert_eq!(b.admit_or_degrade("tiny", short), Admission::Degraded);
    let footer = b.footer();
    assert!(
        footer.contains("reduced to a one-line summary"),
        "got {footer}"
    );
    assert!(footer.contains("4 shown"), "got {footer}");
}

/// When even the stand-in does not fit, the item is refused and every later
/// one with it — the prefix rule still holds below the degraded tier.
#[test]
fn budget_refuses_when_even_the_summary_overruns() {
    use crate::code_intel::{Admission, TokenBudget};
    let big = "a".repeat(4_000);
    let mut b = TokenBudget::new(Some(60));
    assert_eq!(
        b.admit_or_degrade(&big, &big),
        Admission::Full,
        "first always"
    );
    assert_eq!(b.admit_or_degrade(&big, &big), Admission::Refused);
    assert_eq!(b.admit_or_degrade("x", "x"), Admission::Refused);
    assert!(b.footer().contains("withheld"));
}

/// No `--budget` means the historical behaviour, byte for byte: everything
/// prints and no footer is appended.
#[test]
fn budget_absent_is_unbounded_and_silent() {
    use crate::code_intel::{Admission, TokenBudget};
    let mut b = TokenBudget::new(None);
    for _ in 0..1000 {
        let q = "q".repeat(1000);
        assert_eq!(b.admit_or_degrade(&q, &q), Admission::Full);
    }
    assert!(b.footer().is_empty());
}
