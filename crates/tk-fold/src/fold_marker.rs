//! "Has tokenade already folded this text?" — the one place that answers it.
//!
//! Every compaction channel needs this question. A hook must not re-fold what
//! the shell proxy already folded; a plugin must not re-fold what a hook did;
//! and the moment a proxy channel exists it will
//! re-see the entire conversation on every turn and must not fold — or
//! re-credit — a payload some earlier layer already handled.
//!
//! Before this module the answer lived in two places that each knew half of
//! it: `hookwrap` looked for the dedup substitution marker, `wrap::nudges`
//! looked for the recovery pointer. Neither knew about the other's signal.
//! That is the rule "one authority per payload" depending on a marker
//! nobody owned.
//!
//! Three signals exist in folded output, and they are NOT interchangeable:
//!
//! - a **recovery pointer** — `expand-ref <hash>` in a fold banner or footer.
//!   Means: bytes were dropped and are retrievable.
//! - a **dedup reference** — `§ref:<hash>§` substituted inline for a payload
//!   sent earlier in the session. Means: these bytes were replaced wholesale.
//! - a **fold banner or elision footer** — `[tokenade:…]`, `[... N more X
//!   elided]`. Means: a compactor ran and said so. This is the common case:
//!   most compactors emit only this, which is why the first two signals alone
//!   answered "not folded" for the majority of real folds.
//!
//! They are exposed separately because the existing callers each want exactly
//! one of them, and collapsing the two would silently change what they do.
//! [`already_folded`] is the combined predicate, and it is the one a NEW
//! channel should call: any new authority wants "did anyone touch this", not
//! "did anyone touch this in the specific way I happen to emit".

/// Length below which a run of hex digits is not a tokenade hash. Short
/// enough to accept every hash we emit, long enough that prose like
/// "expand-ref abc" does not read as one.
const MIN_HASH_HEX: usize = 12;

/// Does this text carry a live `expand-ref <hash>` recovery pointer?
///
/// Both spellings are matched: the CLI prints `expand-ref`, the MCP tool
/// name was `expand_ref`, and folded output from either era can still be
/// sitting in a transcript.
pub fn has_recovery_pointer(text: &str) -> bool {
    text.match_indices("expand-ref ")
        .chain(text.match_indices("expand_ref "))
        .any(|(i, m)| {
            text[i + m.len()..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .count()
                >= MIN_HASH_HEX
        })
}

/// Does this text contain a `§ref:<hash>§` dedup substitution?
///
/// `§` is two bytes in UTF-8 (0xC2 0xA7); matching on the `str` keeps this
/// on character boundaries, which the byte-scanning version of this check
/// elsewhere in the tree had to be careful about.
///
/// The hash is REQUIRED, for the same reason [`has_recovery_pointer`] requires
/// one: `§ref:` also appears in this tree's own source, inside the format
/// string that emits it (`"[tokenade:read-diff vs §ref:{prior_hash}§] …"`).
/// A bare `contains("§ref:")` made a grep over `crates/` read as already
/// folded — measured 2026-08-08, `grep -rn '[tokenade' crates/tokenade-cli/src`
/// went from 15 677 bytes to 22 940 (unfolded) because of a single matched
/// line. A template placeholder is not a hash.
pub fn has_dedup_ref(text: &str) -> bool {
    text.match_indices("§ref:").any(|(i, m)| {
        text[i + m.len()..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .count()
            >= MIN_HASH_HEX
    })
}

/// Does this text carry a tokenade fold banner or elision footer?
///
/// The third signal, and the one that covers the MAJORITY of folds. Measured
/// 2026-08-08: of `pip list`, `env` and `df -h` folded through the real
/// binary, **none** emitted a recovery pointer or a dedup ref — they carry a
/// `[tokenade…]` banner or an `… N more X elided` footer instead. So the two
/// signals above answered "not folded" for all three, and a second pass ran.
///
/// That second pass is not the harmless no-op the bridge plugins claim in
/// their comments. Measured on the same three: `pip list` folded 1 721 → 808,
/// and re-folding rewrote `[... 37 more package(s) elided]` into
/// `[... 1 more package(s) elided]` — the number of missing packages became
/// false, one byte shorter, therefore ACCEPTED by every plugin's
/// `< text.length` test. `env` dropped a variable the first pass had kept.
/// Neither carries a pointer, so the truth is not recoverable.
///
/// Three markers, all ours: the `[tokenade` banner prefix, the `[disclose:`
/// one, and an elision footer — a line that, once trimmed, OPENS with `[` or
/// `…` and carries both the house word `elided` and a count.
///
/// `[disclose:` was added 2026-08-08 and closes a narrow case rather than a
/// measured incident. A disclosure summary normally ends with an `expand-ref`
/// pointer, so `has_recovery_pointer` already recognised it — verified live:
/// `wrap`'s stashed `journalctl` output fed to the Bash hook came back
/// `reason: "already-folded"`. But the pointer is written only when the stash
/// SUCCEEDS (`[disclose:binary … stash=failed]` is the shape when it does
/// not), and a summary without a pointer would have been folded and credited
/// a second time. Recognising our own banner does not depend on a disk write
/// having worked.
///
/// The opening character is not decoration, it is the whole precision of this
/// predicate. A first draft accepted the word `elided` anywhere on a line, and
/// `grep -rn elided crates/` — 300 source lines quoting our own footers —
/// stopped folding: 815 bytes through `wrap` became 14 913. Every emitter in
/// the tree writes the footer at the START of its line (`  [... 37 more
/// package(s) elided]`, `… 41 more extension(s) elided`, `  [elided: 3
/// traceback line(s)]`); a grep hit puts a path there instead.
pub fn has_fold_banner(text: &str) -> bool {
    text.lines().any(|l| {
        // Compactors decorate their banner line with a comment or bullet
        // marker so it reads as metadata inside the output they fold —
        // `env` writes `# [tokenade:env] 0 secret(s) redacted, …`. Strip
        // that decoration, but nothing else: what must NOT be strippable
        // is a `path:line:` prefix, which is what a grep hit on our own
        // source looks like.
        let t = l.trim_start_matches([' ', '\t', '#', '*', '-', '/']);
        t.starts_with("[tokenade") || t.starts_with("[disclose:") || is_elision_footer(t)
    })
}

fn is_elision_footer(t: &str) -> bool {
    (t.starts_with('[') || t.starts_with('…') || t.starts_with("..."))
        && t.contains("elided")
        && t.contains(|c: char| c.is_ascii_digit())
}

/// Has ANY tokenade channel already folded this payload?
///
/// The predicate a new compaction channel should consult before folding or
/// crediting. Deliberately conservative: it answers "leave it alone" on any
/// known marker, because a missed fold costs tokens once while a double fold
/// costs correctness — the second pass folds an already-summarised text, and
/// what it drops is not recoverable from what the first pass kept.
pub fn already_folded(text: &str) -> bool {
    has_dedup_ref(text) || has_recovery_pointer(text) || has_fold_banner(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_real_fold_banner() {
        let banner = "output…\n[tokenade: 34789 tokens folded away — full output: \
                      `tokenade expand-ref 8cbb0d6ffff8` (`--grep <pattern>` to filter)]";
        assert!(has_recovery_pointer(banner));
        assert!(already_folded(banner));
        assert!(!has_dedup_ref(banner), "different signal");
    }

    #[test]
    fn recognises_the_dedup_substitution() {
        let s = "identical to a prior call — §ref:8cbb0d6ffff8§";
        assert!(has_dedup_ref(s));
        assert!(already_folded(s));
        assert!(!has_recovery_pointer(s), "different signal");
    }

    #[test]
    fn prose_mentioning_the_command_is_not_a_pointer() {
        // Documentation and nudge text name `expand-ref` constantly. Treating
        // that as "already folded" would make a channel skip real work — the
        // hash length is what separates a pointer from a mention.
        assert!(!has_recovery_pointer(
            "run `tokenade expand-ref <hash>` to recover"
        ));
        assert!(!has_recovery_pointer("expand-ref abc123"));
        assert!(!already_folded("nothing here was folded"));
    }

    #[test]
    fn accepts_the_underscore_spelling() {
        assert!(has_recovery_pointer(
            "see expand_ref 8cbb0d6ffff8 for the rest"
        ));
    }

    // The three shapes a real compactor emits, none of which carries a
    // recovery pointer or a dedup ref. Before `has_fold_banner` every one of
    // them read as "never folded", and a second pass rewrote the count of what
    // was missing (`[... 37 more package(s) elided]` → `[... 1 more …]`).
    #[test]
    fn recognises_the_banners_compactors_actually_emit() {
        assert!(already_folded("pkg 1.0\n  [... 37 more package(s) elided]"));
        assert!(already_folded(
            "# [tokenade:env] 0 secret(s) redacted, 2 long value(s) truncated"
        ));
        assert!(already_folded("a.txt  3\n  … 41 more extension(s) elided"));
        assert!(already_folded("[tokenade:git] 82 line(s)"));
    }

    // The precision that matters: a grep over tokenade's OWN source quotes
    // every one of those strings, and folding must still happen there. The
    // marker has to OPEN the line — a `path:line:` prefix disqualifies it.
    // Measured cost of getting this wrong: `grep -rn elided crates/` went from
    // 815 bytes through `wrap` to 14 913.
    #[test]
    fn a_grep_hit_on_our_own_source_is_not_a_fold() {
        assert!(!already_folded(
            "crates/x.rs:12:        \"  [... {} more package(s) elided]\""
        ));
        assert!(!already_folded(
            "crates/y.rs:321:        \"[tokenade uninstall]\""
        ));
        assert!(!already_folded(
            "the details were elided in 2019 for brevity"
        ));
        // A count is what separates a footer we printed from prose.
        assert!(!already_folded("[some rows elided]"));
        // Same class, older signal: the format string that EMITS the dedup
        // substitution lives in our source, and a grep hit on it is not a fold.
        assert!(!already_folded(
            "crates/z.rs:168:  \"[tokenade:read-diff vs §ref:{prior_hash}§] {file_path}\""
        ));
    }

    #[test]
    fn empty_and_huge_inputs_are_safe() {
        assert!(!already_folded(""));
        let big = "x".repeat(200_000);
        assert!(!already_folded(&big));
    }

    /// A disclosure summary is recognised WITHOUT its recovery pointer.
    ///
    /// With the pointer it was already recognised — that is `has_recovery_pointer`
    /// doing the work, and it is the normal case. The pointer is written only
    /// when the stash write succeeds, so the pointer-less summary is the one
    /// that could be folded and credited twice.
    #[test]
    fn a_disclosure_summary_is_recognised_without_its_recovery_pointer() {
        let with_pointer = "[disclose:summary kind=txt tokens=104897 lines=2494]\n             line one\n[stashed: /tmp/x.txt — recover with `tokenade expand-ref a90bd828055c`]";
        assert!(already_folded(with_pointer));

        let no_pointer = "[disclose:summary kind=txt tokens=104897 lines=2494]\nline one\nline two";
        assert!(
            !has_recovery_pointer(no_pointer),
            "fixture must have no pointer, else it proves nothing"
        );
        assert!(
            already_folded(no_pointer),
            "our own disclosure banner must be enough on its own"
        );

        // The false-positive that every widening of this predicate has to
        // survive: a grep hit on our own source puts `path:line:` first, so
        // the banner is no longer at the start of the line.
        assert!(!already_folded(
            "src/disclose.rs:585:    \"[disclose:summary kind={kind_ext} tokens={tokens}]\""
        ));
    }
}
