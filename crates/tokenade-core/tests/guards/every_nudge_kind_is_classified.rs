//! **A piece of advice that asks for an action must be able to say whether
//! it was followed.**
//!
//! MEASURED over 30 days of real usage: **44.5% of nudge fires had no
//! outcome at all** — twelve kinds out of sixteen only write `fire`. The
//! dashboard rendered "—" for all of them, without distinguishing "nobody
//! measured" from "there is nothing to measure".
//!
//! Two rules, held here:
//!
//! 1. every kind passed to `record_nudge` is declared in `NUDGE_KINDS` —
//!    otherwise it shows up on the dashboard without anyone knowing what it is;
//! 2. a `Steer` writes `ok` AND `ignored` somewhere in the tree. Advice that
//!    cannot be evaluated does not improve and cannot justify itself.
//!
//! The second rule carries a counted DEBT: six `Steer`s do not hold it yet.
//! The ratchet forbids adding a seventh.

use tk_base::source_guard::{crates_dir, rust_sources_whole};

/// **Nudges live in `tokenade-cli`, which `all_rust_sources_whole` does not
/// read.** That enumerator stops at the library layers (`tk-*` + the facade):
/// its rule is written and it is right for what it serves. But the twenty-one
/// `record_nudge` sites are ALL in the binary, so a guard written on it reads
/// zero files — green by emptiness, and that is the worst of verdicts. The
/// reach floor below forbids that silence.
fn sources_including_the_cli() -> Vec<String> {
    let crates = crates_dir();
    let mut roots: Vec<std::path::PathBuf> = tk_base::source_guard::crate_srcs();
    roots.push(crates.join("tokenade-cli").join("src"));
    roots
        .iter()
        .flat_map(|r| rust_sources_whole(r))
        // Whitespace carries no meaning and `rustfmt` moves it: searched as
        // is, `record_nudge(\n    &kind,\n    "ignored",` does not contain
        // `&kind, "ignored"`.
        .map(|(_, c)| c.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// The kinds really passed to `record_nudge`, read from the source.
fn kinds_emitted() -> Vec<String> {
    let mut out = Vec::new();
    for code in sources_including_the_cli() {
        let mut rest = code.as_str();
        while let Some(i) = rest.find("record_nudge(") {
            rest = &rest[i + "record_nudge(".len()..];
            let Some(q) = rest.find('"') else { continue };
            // A `&kind` composed at run time is not a literal: the
            // `batch-*` family is declared by its three named callers.
            if rest[..q].contains(',') || rest[..q].contains('&') {
                continue;
            }
            let Some(end) = rest[q + 1..].find('"') else {
                continue;
            };
            out.push(rest[q + 1..q + 1 + end].to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_kind_passed_to_record_nudge_is_declared() {
    let emitted = kinds_emitted();
    // Reach floor: this repo emits at least a dozen literal kinds. Reading
    // zero means the guard does not look where they live, not that they are
    // all classified.
    assert!(
        emitted.len() >= 12,
        "only {} kinds read — the guard does not read the call sites: {emitted:?}",
        emitted.len()
    );
    let undeclared: Vec<String> = emitted
        .into_iter()
        .filter(|k| tokenade_core::gain::nature_of(k).is_none())
        .collect();
    assert!(
        undeclared.is_empty(),
        "these kinds reach the dashboard without being classified in \
         `NUDGE_KINDS` — advice or notice? {undeclared:?}"
    );
}

/// How many `Steer`s cannot say `ok`/`ignored` yet.
///
/// The number goes down, never the reverse. As of 2026-09-14: `batch-read`,
/// `batch-bash`, `batch-edit` and `loop-guard` are wired; the seven others
/// are not — `batch-questions`, `batch-web-calls`, `cat-edit`,
/// `large-file`, `write-rewrite`, `media-tooling`, `bypass`.
const STEERS_WITHOUT_AN_OUTCOME: usize = 7;

#[test]
fn a_steer_that_cannot_be_evaluated_is_a_debt_that_only_shrinks() {
    let sources = sources_including_the_cli();
    let writes = |kind: &str, verb: &str| -> bool {
        // The `batch-*` family composes its kind (`format!("batch-{family}")`)
        // and writes its verbs on the variable: the site is recognised by the
        // `&kind` + verb pair, in the file that carries the three.
        let literal = format!("\"{kind}\", \"{verb}\"");
        let composed = format!("&kind, \"{verb}\"");
        // `format!("batch-{family}")` produces ONLY those three — the callers
        // of `batching_family_nudge`. `batch-questions` and `batch-web-calls`
        // carry the same prefix and live elsewhere: crediting them with their
        // namesakes' wiring was a false positive, and it made two real debts
        // vanish.
        const COMPOSED_FAMILY: &[&str] = &["batch-read", "batch-bash", "batch-edit"];
        sources.iter().any(|c| {
            c.contains(&literal)
                || (COMPOSED_FAMILY.contains(&kind)
                    && c.contains("batch-{family}")
                    && c.contains(&composed))
        })
    };
    let blind: Vec<&str> = tokenade_core::gain::steers()
        .filter(|k| !(writes(k, "ok") && writes(k, "ignored")))
        .collect();
    assert!(
        blind.len() <= STEERS_WITHOUT_AN_OUTCOME,
        "{} pieces of advice without a measurable outcome, ceiling {STEERS_WITHOUT_AN_OUTCOME}: {blind:?}",
        blind.len()
    );
    assert!(
        blind.len() == STEERS_WITHOUT_AN_OUTCOME,
        "the debt went down to {} — lower `STEERS_WITHOUT_AN_OUTCOME` to that number: {blind:?}",
        blind.len()
    );
}
