//! Token estimation — two modes, same call site.
//!
//! ## Default: heuristic (~3-5 % of cl100k on realistic mixed code+prose)
//!
//! We deliberately avoid pulling a multi-megabyte BPE vocabulary into the
//! default build — the whole point of STS is to ship as one tiny binary. For
//! measuring *relative* savings (which is what `Savings` records) a calibrated
//! heuristic is empirically within a few percent of tiktoken cl100k_base on
//! mixed code + English / French / k8s output / git diff.
//!
//! The estimate counts *pieces*, not characters, because that is what a BPE
//! merges into tokens: a word run, a camelCase break, a digit group, a
//! punctuation run, a whitespace run. A flat chars-per-token divisor cannot
//! tell a padded PowerShell table (11.8 real chars/token — cl100k has
//! multi-space tokens) from a base64 blob (1.9), and got both wrong by 2×.
//! Coefficients are fitted against cl100k_base over the repo's real corpora
//! (source, HTML, command output, compacted output, prose) — see
//! `tests/estimator_calibration_dump.rs` to refit and
//! `tests/estimator_gain_fidelity.rs` for the gate.
//!
//! ## Opt-in: `bpe` feature
//!
//! Build with `--features bpe` to switch the in-process counter to the actual
//! cl100k_base BPE via `tiktoken-rs`. Adds ~1.5 MB to the binary but the
//! estimate becomes the exact token count Claude/GPT see. Useful for the bench
//! crate; off in the shipping CLI/MCP build because the savings *ratios* are
//! what users care about and the heuristic gets those right.

#[cfg(feature = "bpe")]
use std::sync::OnceLock;

/// Estimate the number of tokens in `text` for the **default**, fast,
/// dependency-free heuristic. Always available.
/// What a lone QUOTE costs when it touches a word.
///
/// cl100k merges an opening or closing quote into the token beside it —
/// `"value` is one piece, not a quote plus a word — while the punctuation term
/// charges both. Measured 2026-08-27 against the real vocabulary: the same
/// words priced `key_42 = value_42` come out at **−0.5 %**, and priced
/// `"value number 42 here"` at **+21.6 %**. Only the quotes differ.
///
/// The scope is quotes ONLY, and that is the whole difference between a fix
/// and a regression. Applying the same merge to every lone mark — `,`, `:`,
/// `{`, `}` — moves the mean error from **+1.09 % to −4.94 %** and the worst
/// case from −22.0 % to −30.1 %, because those coefficients were fitted with
/// the compensations already in them. Restricted to quotes, the same factor
/// takes the mean to **+0.09 %**, leaves the worst case untouched, and makes
/// no shape's reported saving worse.
///
/// 0.5 rather than 0 because it MINIMISES the mean: 0.25 gives −0.41 % and 0.0
/// gives −0.91 %, and a negative mean is the harmful direction.
const MERGE_FACTOR: f64 = 0.5;

/// Real tokens the model `model` bills for ONE token of [`estimate`].
///
/// [`estimate`] is fitted on cl100k, OpenAI's vocabulary; a Claude model bills
/// with its own, denser one. MEASURED 2026-10-05 on 1 632 local Claude Code
/// transcripts (`tokenade-bench --bin tokenizer_drift`), two independent ways:
/// a text-only reply's billed `output_tokens` against the estimate of its text
/// (opus-5 **1.59**, n = 2 547, IQR 1.55–1.64; opus-5-5 1.60, n = 461), and the
/// context growth across a tool round against the estimate of its results
/// (Theil–Sen slope 1.57, n = 29 484; 1.56, n = 4 658). haiku-4-5, an older
/// vocabulary: 1.27 (n = 40). OpenAI's models stay at 1.0, and that is now a
/// measurement too: on local Codex sessions (`tokenizer_drift --codex`), the
/// context growth after a tool round against the estimate of its outputs gives
/// gpt-5.5 a Theil–Sen slope of **1.00** (IQR of pairwise slopes 0.89–1.09,
/// n = 47 rounds, 2026-10-05). A model nobody measured keeps 1.0 — an unknown
/// tokenizer is not inflated on a guess.
pub fn claude_tokens_per_estimated(model: &str) -> f64 {
    let m = model.to_ascii_lowercase();
    if !m.starts_with("claude-") {
        1.0
    } else if m.contains("haiku-4") {
        1.27
    } else {
        1.6
    }
}

pub fn estimate(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Non-Latin scripts tokenise FAR denser than Latin prose under cl100k, so a
    // Latin-tuned model under-counts them by 40–85 % (measured): CJK / kana /
    // Hangul / Devanagari / emoji ≈ ~1 token/char, Arabic/Hebrew ≈ ~1.5,
    // Cyrillic/Greek ≈ ~2.4. Left unfixed this breaks the disclosure threshold
    // for non-Latin text (a 5 000-token Chinese blob estimates at ~1 100 and
    // never stashes). Those chars are priced per script below; everything else
    // goes through the piece model.
    // Walked as BYTES, not as a materialised `Vec<char>`.
    //
    // The old loop collected every char first — 4 bytes per char, so 5 MB of
    // allocation to price a 1.28 MB output, plus a full decode pass before the
    // pricing pass could start. `estimate` runs 3-4x per command (before,
    // after, and again in the guards), so that copy was paid on every one.
    //
    // A byte cursor is exact here because every branch below except the script
    // tiers is ASCII-only, and an ASCII byte can never appear inside a
    // multi-byte UTF-8 sequence — so a byte compare cannot straddle a char.
    // Non-ASCII is decoded only when actually met. `estimate_reference` in the
    // tests keeps the original loop, and `same_count_as_the_char_vec_loop`
    // holds the two to the same answer.
    let mut cost = 0.0f64;
    let b = text.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let byte = b[i];
        if byte < 0x80 {
            // No ASCII scalar is in `script_cpt` (its lowest range starts at
            // U+0370), so the tier test the original ran first cannot fire.
            if byte.is_ascii_alphabetic() {
                // A word run is ~1 token, plus a slow length term for
                // identifiers. Past ~14 chars (base64, hashes, minified names)
                // the BPE stops merging and splits every few chars — the floor
                // takes over there.
                let start = i;
                let mut case_breaks = 0usize;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    if i > start && b[i].is_ascii_uppercase() && b[i - 1].is_ascii_lowercase() {
                        case_breaks += 1;
                    }
                    i += 1;
                }
                let len = (i - start) as f64;
                cost += (0.995 + 0.040 * len).max(len / 8.0) + 1.107 * case_breaks as f64;
                continue;
            }
            if byte.is_ascii_digit() {
                // cl100k emits digit groups of at most three.
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                cost += 1.512 * ((i - start) as f64 / 3.0).ceil();
                continue;
            }
            if !byte.is_ascii_whitespace() && byte != 0x0B {
                // A run of the SAME punctuation mark merges (`---`, `===`,
                // `///`); distinct marks each cost their own share.
                let start = i;
                while i < b.len() && b[i] == byte {
                    i += 1;
                }
                let mut c = 0.429 * ((i - start) as f64 / 3.0).ceil();
                // A LONE mark touching a word merges into it.
                if i - start == 1
                    && matches!(byte, b'"' | b'\'')
                    && (b.get(i).is_some_and(|n| n.is_ascii_alphanumeric())
                        || (start > 0 && b[start - 1].is_ascii_alphanumeric()))
                {
                    c *= MERGE_FACTOR;
                }
                cost += c;
                continue;
            }
            // ASCII whitespace falls through to the shared run below, which
            // must also swallow the non-ASCII kinds (U+00A0, U+2028, …).
        } else {
            let c = text[i..].chars().next().unwrap_or('\u{FFFD}');
            if let Some(tier) = script_cpt(c as u32) {
                cost += match tier {
                    ScriptTier::Heavy => 1.5,
                    ScriptTier::Dense => 1.0,
                    ScriptTier::Semi => 1.0 / 1.5,
                    ScriptTier::Mid => 1.0 / 2.4,
                };
                i += c.len_utf8();
                continue;
            }
            if !c.is_whitespace() {
                let w = c.len_utf8();
                let start = i;
                while i < b.len() && text[i..].starts_with(c) {
                    i += w;
                }
                cost += 0.429 * (((i - start) / w) as f64 / 3.0).ceil();
                continue;
            }
        }
        // A lone space is free — cl100k merges it into the word that follows.
        // Beyond that, column padding and indentation are still cheap
        // (multi-space tokens): a 20-space gutter is ~2 tokens, not 20. A
        // newline, however, almost always costs one. Counted in CHARS, which
        // is what the original priced.
        let mut run = 0usize;
        let mut newlines = 0usize;
        while i < b.len() {
            let (c, w) = if b[i] < 0x80 {
                (b[i] as char, 1)
            } else {
                let c = text[i..].chars().next().unwrap_or('\u{FFFD}');
                (c, c.len_utf8())
            };
            // NO script-tier test here, deliberately. U+3000 IDEOGRAPHIC
            // SPACE is both whitespace and a Dense tier, and the original
            // prices it as a tier only when it OPENS a position — once a run
            // is under way its loop tested `is_whitespace` alone and swallowed
            // it. Inconsistent, but it is the behaviour every calibrated
            // coefficient was fitted against; changing it here would move
            // reported savings, which is not what a speed rewrite may do.
            // Adding the tier test made `"a \u{a0}\n \u{3000} b"` price 5
            // instead of 4.
            if !c.is_whitespace() {
                break;
            }
            if c == '\n' {
                newlines += 1;
            }
            run += 1;
            i += w;
        }
        let padding = run.saturating_sub(newlines).saturating_sub(1);
        cost += 0.242 * (padding as f64 / 8.0).ceil() + 1.336 * newlines as f64;
        // A gutter that RIGHT-ALIGNS A NUMBER is not ordinary padding.
        //
        // The line above prices a run of spaces at ~0.24 tokens per eight,
        // which is right for indentation and for a gap between words: cl100k
        // has multi-space tokens and merges a lone space into the word after
        // it. It is wrong in front of a digit. `"     0"` does not tokenise as
        // cheap-spaces plus a number; the leading run breaks into its own
        // pieces and the digits into theirs, and the total is close to one
        // token per few spaces.
        //
        // Measured 2026-08-27 against real cl100k (`--features bpe`), 200
        // lines of four `{:>6}` integer columns: **2 193 estimated against
        // 2 945 actual, −25.5 %**. The same numbers separated by ONE space
        // come out at +2.8 %, and a 20-space gutter between two letters at
        // +3.4 % — so neither digits nor padding is wrong on its own. It is
        // the pair.
        //
        // That pair is what a numeric table IS, and it is the shape this tool
        // sees most: across the thirty fuzz corpora the whole under-counting
        // half of the spread is columnar output — vmstat −30.9 %, df −20.4 %,
        // netstat −15.1 %, ss −14.6 %, strace −13.2 %, kubectl −10.3 % —
        // while prose and JSON over-count. The mean was +0.1 %, which is what
        // a well-fitted model looks like right up until you ask it about one
        // shape.
        //
        // Under-counting is the harmful direction, and this function already
        // carries the same lesson for non-Latin scripts thirty lines above:
        // "a 5 000-token Chinese blob estimates at ~1 100 and never stashes".
        // A columnar dump that reads 30 % smaller than it is gets the same
        // treatment from every threshold keyed on this number.
        // `padding >= 2` is the best of THREE measured variants, not a guess.
        // Against real cl100k over the thirty corpora:
        //
        //   padding >= 2 (this)          mean +1.09 %   worst -22.0 %
        //   any gap before a digit       mean +6.36 %   worst +22.5 %
        //   + gaps BETWEEN two digits    mean +4.04 %   worst +19.3 %
        //
        // The two wider forms buy at most 2.7 points of worst case for three
        // to six times the mean error, and the mean is what most content
        // meets. A single space before a number is usually prose ("I have 3
        // apples"), where cl100k really does merge it cheaply; two or more is
        // a column.
        if padding >= 2 && b.get(i).is_some_and(u8::is_ascii_digit) {
            cost += 0.9;
        }
    }

    (cost.ceil() as usize).max(1)
}

// The `Vec<char>` loop [`estimate`] replaced, kept verbatim as the oracle for
// `same_count_as_the_char_vec_loop`. A rewrite for speed on a number the
// whole product's savings are computed from is only safe if something holds
// the two to the same answer.

/// Token-density class of a Unicode scalar for [`estimate`]. `None` = Latin /
/// ASCII / common punctuation (handled by the bracket-density tiers).
enum ScriptTier {
    Heavy, // ~1.5 tokens/char: Hangul — spaced like Latin, priced like CJK×1.5
    Dense, // ~1 token/char: CJK, kana, Devanagari, Thai, emoji & symbols
    Semi,  // ~1.5 chars/token: Arabic, Hebrew
    Mid,   // ~2.4 chars/token: Cyrillic, Greek
}

#[inline]
fn script_cpt(u: u32) -> Option<ScriptTier> {
    match u {
        // Cyrillic, Greek/Coptic — Latin-adjacent, moderate density.
        0x0370..=0x052F => Some(ScriptTier::Mid),
        // Arabic, Hebrew — abjads, ~1.5 token/char.
        0x0590..=0x05FF | 0x0600..=0x06FF | 0x0750..=0x077F => Some(ScriptTier::Semi),
        // Devanagari & other Indic, Thai — ~1 token/char.
        0x0900..=0x0E7F => Some(ScriptTier::Dense),
        // CJK symbols/punct, Hiragana, Katakana.
        0x3000..=0x30FF | 0x31F0..=0x31FF => Some(ScriptTier::Dense),
        // CJK Unified Ideographs (+ Ext A).
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Some(ScriptTier::Dense),
        // Hangul syllables / Jamo.
        0x1100..=0x11FF | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Some(ScriptTier::Heavy),
        // Emoji / pictographs / symbols (astral planes + BMP misc-symbols).
        0x2600..=0x27BF | 0x1F000..=0x1FAFF => Some(ScriptTier::Dense),
        // CJK Unified Ideographs Ext B+ (astral).
        0x20000..=0x3FFFF => Some(ScriptTier::Dense),
        _ => None,
    }
}

/// True for scripts written **without spaces between words** — Han, kana and
/// Thai — where whitespace/punctuation tokenisation collapses a whole run into
/// one token. Callers (e.g. the stash BM25) fall back to character n-grams for
/// these. Deliberately EXCLUDES Hangul (modern Korean spaces its words) and
/// Cyrillic/Greek/Arabic/Hebrew (all space-delimited), which tokenise fine.
pub fn is_wordless_script(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF |                                       // Hiragana, Katakana
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF |   // CJK ideographs (+ Ext A, compat)
        0x20000..=0x3FFFF |                                     // CJK ideographs Ext B+ (astral)
        0x0E00..=0x0E7F                                         // Thai
    )
}

/// Split one fragment at every change between a word-less script and anything
/// else, keeping each run's own bytes. `true` marks the word-less runs.
///
/// This is the boundary EVERY lexical tokenizer in the crate needs and none of
/// the Unicode character classes provide: Han, kana and Thai are
/// `is_alphanumeric()` and `XID_Continue`, so `修复run函数的报错` is ONE
/// alphanumeric run under every splitter we have. Whatever the caller does next
/// — bigrams, camelCase splitting, stemming — it can only see `run` as a symbol
/// name if the run is cut here first. MEASURED consequence of not cutting: the
/// ASCII symbol in a mixed CJK query never matched the index at all.
pub fn split_script_runs(frag: &str) -> Vec<(bool, String)> {
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in frag.chars() {
        let wordless = is_wordless_script(c);
        match runs.last_mut() {
            Some((w, buf)) if *w == wordless => buf.push(c),
            _ => runs.push((wordless, c.to_string())),
        }
    }
    runs
}

/// Emit a word-less run as overlapping character bigrams, à la Lucene's
/// `CJKBigramFilter` — a lone character stays a unigram.
///
/// Bigrams are what make such a run searchable at all: it carries no spaces, so
/// left whole it is one token that only an identical query reproduces.
pub fn push_wordless_bigrams(run: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = run.chars().collect();
    match chars.len() {
        0 => {}
        1 => out.push(chars[0].to_string()),
        _ => {
            for w in chars.windows(2) {
                out.push(w.iter().collect());
            }
        }
    }
}

/// Real BPE tokenization via cl100k_base. Available when the `bpe` feature
/// is enabled at build time; otherwise this falls back to [`estimate`].
///
/// Use this for absolute accuracy (e.g. comparing against tiktoken in the
/// bench crate). The default `estimate` is plenty for measuring savings.
#[cfg(feature = "bpe")]
pub fn estimate_exact(text: &str) -> usize {
    static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
    let bpe =
        BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("bundled cl100k vocab must load"));
    bpe.encode_with_special_tokens(text).len()
}

#[cfg(not(feature = "bpe"))]
pub fn estimate_exact(text: &str) -> usize {
    estimate(text)
}
#[cfg(test)]
include!("tokenizer_tests.rs");
