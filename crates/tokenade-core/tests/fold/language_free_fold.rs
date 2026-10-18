//! The fold must not depend on the LANGUAGE of the output.
//!
//! A language test placed in a compactor's `detect()` is a GATE: one English
//! word missing and the compactor does not run at all. The output then goes
//! out whole. It is the most repeated class of defect in this repo, and the
//! measurement that sizes it lives in `richness_corpus.rs`
//! (`how_many_compactors_depend_on_the_language`).
//!
//! The structural answer is not to translate the keywords: it is that the
//! information is ALWAYS IN THE SAME PLACE. A column-aligned `label: value`
//! block is an aligned block in French, in German and in a language nobody
//! speaks — the padding pushing values to the right is decoration in all of
//! them. `auto::Kind::AlignedBlock` is that fallback: the last rung before
//! `Unknown`, without a single word of vocabulary.
//!
//! MEASURED 2026-09-07 on real captures this repo carries: FRENCH
//! `systeminfo` 4,765 → 2,562 bytes (−46%) where the generic one gave 4,696
//! (−1.4%), and `vm_stat` (macOS, no named compactor) 1,136 → 607. Richness
//! **1.0000 unchanged** in both cases: what goes is padding, not information.

// The corpus is mounted by `main.rs` for the whole binary — see the header
// of `fold/main.rs`. This subject takes it, it does not re-mount it.
use crate::corpus::{self as corpus, mangle_labels, normalise_newlines, Route};

fn fixture(key: &str) -> String {
    corpus::fixtures()
        .into_iter()
        .find(|f| f.key == key)
        .unwrap_or_else(|| panic!("fixture {key} missing from the corpus"))
        .body
}

fn deliver(prog: &str, body: &str) -> String {
    tokenade_core::filter::filter_command(&[prog.to_string()], body).output
}

/// The capture is in FRENCH and no named compactor claims it. Before the
/// language-free rung it went out whole.
#[test]
fn a_french_key_value_block_is_folded_by_a_compactor_that_reads_no_words() {
    let raw = fixture("windows/systeminfo");
    // The capture was taken on a console in code page 850: its accents are
    // already replacement characters in the file. So it is anchored on French
    // WITHOUT accents, which is also what a language-dependent compactor
    // should have recognised and did not.
    assert!(raw.contains("Fuseau horaire"), "the capture must be French");
    let out = deliver("systeminfo", &raw);
    // MEASURED: 4,765 → 2,562 bytes, i.e. −46%. The threshold is at −40%,
    // which leaves room for the noise of a neighbouring capture and still
    // refuses the generic fallback that served this output before (4,696,
    // i.e. −1.4%).
    assert!(
        out.len() * 10 <= raw.len() * 6,
        "a French aligned block must lose at least 40% of its bytes: {} → {}",
        raw.len(),
        out.len()
    );
}

/// The heart of the property: the SAME output, with not one word of French
/// left, must be folded to the same size. A compactor that reads words fails
/// here; this one does not look at them.
#[test]
fn mangling_every_label_changes_the_fold_by_nothing() {
    // Both sides go through the same line-ending normalisation: what must
    // differ between them is the LANGUAGE, and nothing else.
    let raw = normalise_newlines(&fixture("windows/systeminfo"));
    let mangled = mangle_labels(&raw);
    assert!(
        !mangled.contains("Fuseau horaire"),
        "the scrambling must have taken the language away"
    );
    let plain = deliver("systeminfo", &raw).len();
    let unknown_tongue = deliver("systeminfo", &mangled).len();
    // rot13 preserves length, so the equality is exact, not approximate.
    assert_eq!(
        plain, unknown_tongue,
        "the fold changed size although only the LANGUAGE changed"
    );
}

/// What goes is PADDING. Every value to the right of a colon must be found
/// as is: that is what allows folding without reading.
#[test]
fn every_value_survives_the_collapse() {
    let raw = fixture("windows/systeminfo");
    let out = deliver("systeminfo", &raw);
    for line in raw.lines() {
        let Some((_, value)) = line.split_once(':') else {
            continue;
        };
        // The value is compared WITH its runs of spaces reduced: that is
        // exactly what the fold does, and a value can itself carry a colon
        // (`Mémoire virtuelle : taille maximale:   12 913 Mo`), so splitting
        // on the first one is not enough to isolate it.
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if value.len() < 4 {
            continue;
        }
        let value = value.as_str();
        assert!(
            out.contains(value),
            "value lost by the language-free fold: {value:?}"
        );
    }
}

/// LEADING indentation is structure: under `Correctif(s):`, the twenty
/// `[01]: KB…` lines only say they belong to the line above through their
/// indent. The fold keeps ONE space — 34 bytes on this capture.
#[test]
fn leading_indentation_survives_as_a_single_space() {
    let raw = fixture("windows/systeminfo");
    let out = deliver("systeminfo", &raw);
    assert!(
        out.lines().any(|l| l.starts_with(" [01]:")),
        "the continuation line lost its attachment to the line above"
    );
    assert!(
        !out.lines().any(|l| l.starts_with("  ")),
        "the columns, though, must indeed have dropped"
    );
}

/// A blank line SEPARATES records — it is the only structure an output like
/// `pacman -Qi` carries, where the field name repeats from one block to the
/// next. The fold only reduces spaces WITHIN a line; dropping blank lines
/// would weld the records together. (The `free` compactor dropped them,
/// without visible consequence: its output has none.)
#[test]
fn a_blank_line_separates_records_and_survives() {
    use tokenade_core::archetypes::collapse_alignment_padding;
    let two_records = "Name:      alpha\nVersion:   1.0\n\nName:      beta\nVersion:   2.0";
    let out = collapse_alignment_padding(two_records);
    assert_eq!(out, "Name: alpha\nVersion: 1.0\n\nName: beta\nVersion: 2.0");
    assert_eq!(
        out.lines().filter(|l| l.trim().is_empty()).count(),
        1,
        "the separation between the two records disappeared"
    );
}

/// The padding can be on one side OR THE OTHER of the colon, and French
/// typography puts it on the left: `pacman -Qi` in French writes
/// `Nom                      : bash` over twenty-one lines. Looking only to
/// the right of the colon means not seeing as aligned what is most visibly
/// so — and missing it exactly on the locales this rung exists for.
#[test]
fn the_padding_may_sit_on_either_side_of_the_colon() {
    use tokenade_core::archetypes::{collapse_alignment_padding, padding_is_decorative};
    let left: String = (0..8)
        .map(|i| format!("Champ{i}                 : valeur{i}\n"))
        .collect();
    assert!(
        padding_is_decorative(&left),
        "a block aligned to the LEFT of the colon is not recognised"
    );
    assert_eq!(
        collapse_alignment_padding("Nom                      : bash"),
        "Nom : bash"
    );
    // And a single space on each side remains prose, not columns.
    let prose: String = (0..30).map(|i| format!("Champ{i} : valeur{i}\n")).collect();
    assert!(!padding_is_decorative(&prose));
}

/// The language-free rung is the LAST before `Unknown` and must stay
/// demanding: too broad, it steals routing from shapes that other compactors
/// serve better. Two ways in, two thresholds.
#[test]
fn a_handful_of_colons_is_not_an_aligned_block() {
    use tokenade_core::archetypes::padding_is_decorative;
    // The "labelled block" path requires EIGHT lines. Light padding, so the
    // "share of spaces" path does not answer in its place.
    let row = |i: usize| format!("label{i}:  a value long enough to dilute the padding\n");
    let seven: String = (0..7).map(row).collect();
    assert!(!padding_is_decorative(&seven));
    let eight: String = (0..8).map(row).collect();
    assert!(padding_is_decorative(&eight));
    // Aligned by ONE single space: that is `k: v` prose, not columns.
    let single: String = (0..30).map(|i| format!("label{i}: value{i}\n")).collect();
    assert!(!padding_is_decorative(&single));
    // JSON carries colons galore and is not an aligned block.
    let json: String = (0..30).map(|i| format!("  \"key{i}\":   {i},\n")).collect();
    assert!(!padding_is_decorative(&json));
}

/// The second path: a table in COLUMNS, without a single colon. Nothing to
/// recognise in it by text — what gives it away is the share of inner
/// spaces, and that is true in every language.
#[test]
fn a_column_table_without_a_single_colon_is_decorative_padding() {
    use tokenade_core::archetypes::padding_is_decorative;
    let table = "Name           Used    Free Provider\n\
                 ----           ----    ---- --------\n\
                 C             44,38    5,05 FileSystem\n\
                 D              0,05    0,00 FileSystem\n\
                 E             12,00    3,00 FileSystem\n\
                 F              7,50    1,25 FileSystem\n\
                 G              2,00    0,75 FileSystem\n";
    assert!(padding_is_decorative(table));

    // The SAME table with holes IN THE MIDDLE is refused: `Alias` under `Name`
    // and nothing under `Used`/`Free` gives `Alias Alias` once the padding is
    // reduced, and nothing says any more which column the second belongs to.
    // Richness does not catch it — no atom disappeared, it is the POSITION
    // that carried the information. It is the real output of `Get-PSDrive`.
    let holed = "Name           Used    Free Provider\n\
                 ----           ----    ---- --------\n\
                 Alias                        Alias\n\
                 C             44,38    5,05 FileSystem\n\
                 Cert                         Certificate\n\
                 Env                          Environment\n\
                 HKCU                         Registry\n";
    assert!(
        !padding_is_decorative(holed),
        "an empty column IN THE MIDDLE was folded: the fold lies about position"
    );

    // An empty column at the END of the line does not raise that problem —
    // fields read again from the left — and it is the common case, so
    // refusing it would cost the fold without protecting anything.
    let trailing = "Destination     Gateway         Flags     Netif Expire\n\
                    default         10.0.2.2        UGScg     en0\n\
                    10.0.2/24       link#4          UCS       en0\n\
                    127             127.0.0.1       UCS       lo0\n\
                    169.254         link#4          UCS       en0\n\
                    192.168.1       link#4          UCS       en0\n";
    assert!(padding_is_decorative(trailing));

    // And NOT on prose or indented code: leading indentation is structure,
    // it does not count, otherwise any source would pass for a table.
    let code = "fn main() {\n\
                \x20   let x = 1;\n\
                \x20   if x > 0 {\n\
                \x20       println!(\"hello\");\n\
                \x20   }\n\
                }\n";
    assert!(!padding_is_decorative(code));
    let prose = "The quick brown fox jumps over the lazy dog.\n\
                 It did so twice, and then it stopped.\n\
                 Nothing here is aligned to any column.\n\
                 There is exactly one space between words.\n\
                 Five lines, and no padding at all.\n";
    assert!(!padding_is_decorative(prose));
}

/// A SUCCESS that yields nothing blocks exactly what a REFUSAL would have
/// let through. Refusal has always been handled — `folded_or_fallthrough`
/// falls back to the ladder — but a named compactor that claims the output
/// and returns it nearly intact has "succeeded", and the ladder no longer
/// applies.
///
/// MEASURED by `tokenade-bench --bin compactor_review` on 554,148 crossings
/// of the corpus: twenty real pairs in this case, across the three
/// platforms. `file` on a 43,664-byte output returned **43,663**, where the
/// language-free fallback returns 31,770.
#[test]
fn a_name_that_folds_nothing_does_not_shadow_a_fallback_that_folds() {
    let raw = fixture("linux/file_usr_lib");
    let out = deliver("file", &raw);
    assert!(
        out.len() * 4 <= raw.len() * 3,
        "the named compactor claims and folds nothing, and the fallback stays \
         masked: {} → {}",
        raw.len(),
        out.len()
    );
    // And it lost nothing by folding: what goes is padding.
    assert_eq!(
        tokenade_core::richness::richness(&raw, &out),
        1.0,
        "the fallback cost rare information"
    );
}

/// The fallback is only taken if it does STRICTLY better. A named compactor
/// that returns its output unchanged because it was ALREADY minimal keeps the
/// hand: replacing it with a generic one that does no better would trade an
/// informative name for a vague one, without a byte gained.
#[test]
fn a_fallback_that_does_no_better_does_not_take_over() {
    // Three lines no ladder can fold.
    let tiny = "total 0\ndrwxr-xr-x 2 u g 40 Jan 1 00:00 .\ndrwxr-xr-x 3 u g 60 Jan 1 00:00 ..\n";
    let r = tokenade_core::filter::filter_command(&["ls".to_string()], tiny);
    assert!(
        r.output.len() >= tiny.len() - 8,
        "there was nothing to fold here: {:?}",
        r.output
    );
}

/// A document can carry SEVERAL tables of different geometries.
/// `netstat -nr` writes `Routing tables`, then an `Internet:` table of five
/// columns, then an `Internet6:` table whose columns fall elsewhere. Judged
/// as one block it looks like nothing — the second's lines measured against
/// the first's columns have holes everywhere — and the fold was refused on a
/// document of which **57% of the bytes** are padding.
#[test]
fn each_table_of_a_multi_table_document_is_folded_on_its_own() {
    let raw = fixture("macos/netstat_nr");
    let out = deliver("netstat", &raw);
    assert!(
        out.len() * 2 < raw.len(),
        "the two tables should each have been folded on their own: {} → {}",
        raw.len(),
        out.len()
    );
    assert_eq!(
        tokenade_core::richness::richness(&raw, &out),
        1.0,
        "the per-segment fold cost rare information"
    );
    // The preamble, which is not a table, comes out intact.
    assert!(
        out.starts_with("Routing tables"),
        "{}",
        &out[..40.min(out.len())]
    );
    // And both headers are there: no segment was eaten by the other.
    assert!(
        out.contains("Internet:") && out.contains("Internet6:"),
        "{out}"
    );
    // The blank line that SEPARATES the segments survives: it is what says
    // where one table ends and the next begins, and without it the two
    // geometries read as one incoherent table.
    assert!(
        out.contains("\n\nInternet:") && out.contains("\n\nInternet6:"),
        "the separators disappeared, the two tables are welded:\n{out}"
    );
}

/// The per-segment fold only returns something if a segment let itself be
/// folded: otherwise, a document without a single qualifying block would be
/// rebuilt for nothing, and the caller could not tell "nothing to do" from
/// "done, but without effect".
#[test]
fn a_document_with_no_decorative_segment_yields_nothing() {
    use tokenade_core::archetypes::collapse_decorative_segments;
    let prose = "Une phrase.\n\nUne autre phrase, sans colonnes.\n\nEt une troisième.\n";
    assert_eq!(collapse_decorative_segments(prose), None);
    // And a document of which ONE segment qualifies returns the whole, the other intact.
    let mixed = format!(
        "Prose libre, aucune colonne ici.\n\n{}",
        (0..8)
            .map(|i| format!("Champ{i}                 : valeur{i}\n"))
            .collect::<String>()
    );
    let out = collapse_decorative_segments(&mixed).expect("a segment qualifies");
    assert!(out.starts_with("Prose libre, aucune colonne ici."));
    assert!(out.contains("Champ0 : valeur0"), "{out}");
    assert!(
        out.contains("ici.\n\nChamp0"),
        "the blank line between the two segments was eaten: {out:?}"
    );
}

/// The census, as a guard rather than a measurement. When the NAMED
/// compactor drops out because the words changed, something must still fold
/// — "a little less good, but already compacting". The shapes the
/// language-free ladder does not cover yet are named one by one: it is the
/// work list, and it must only shrink.
#[test]
fn no_fixture_goes_from_folded_to_untouched_when_its_words_change() {
    // COLUMN tables without colons (header, `----` rule, right-padded lines).
    // The language-free ladder does not see them yet: it is an archetype
    // distinct from the `label: value` block, to be handled next.
    const NOT_YET_COVERED: [&str; 7] = [
        // Not a language at all: `codex exec` prints fixed markers (`codex`,
        // `exec`, `tokens used`) from an untranslated CLI, and a review is
        // prose the language-free ladder has nothing to fold in. Mangling the
        // markers invents an output Codex never prints.
        "linux/codex_review",
        "windows/ps_get_childitem",
        "windows/ps_get_psdrive",
        "windows/ps_get_psversion",
        "windows/ps_get_service",
        // Same family, on the other platform: `lsblk` draws a tree in columns
        // (`NAME  MAJ:MIN  RM  SIZE  RO  TYPE  MOUNTPOINTS`) of which no line
        // is a `label: value`. This case was INVISIBLE until the fallback
        // stopped being masked by a named compactor that folded nothing: the
        // name did not change, so the guard passed without checking anything.
        "linux/lsblk_full",
        "linux/lsblk_full_en",
    ];

    let mut untouched: Vec<String> = Vec::new();
    for f in corpus::fixtures() {
        if f.route != Route::Command {
            continue;
        }
        let Some(prog) = f.program() else {
            continue;
        };
        let before = tokenade_core::filter::filter_command(std::slice::from_ref(&prog), &f.body);
        if !tokenade_core::filter::filter_name_is_command_specific(&before.filter) {
            continue;
        }
        let mangled = mangle_labels(&f.body);
        // A one-line output has nothing to fold and is no proof:
        // `macos/nslookup_apple` is 104 bytes, of which the named compactor
        // gained TWO. Requiring a fold there would be requiring noise.
        if mangled.len() < 512 {
            continue;
        }
        let after = tokenade_core::filter::filter_command(&[prog], &mangled);
        if after.filter == before.filter {
            continue;
        }
        // The named compactor dropped out. The fallback must have bitten.
        if after.output.len() * 100 >= mangled.len() * 98
            && !NOT_YET_COVERED.contains(&f.key.as_str())
        {
            untouched.push(format!(
                "{}: {} → {} ({} / {} bytes)",
                f.key,
                before.filter,
                after.filter,
                after.output.len(),
                mangled.len()
            ));
        }
    }
    untouched.sort();
    assert!(
        untouched.is_empty(),
        "output(s) returned as is because their words changed:\n  {}",
        untouched.join("\n  ")
    );
}
