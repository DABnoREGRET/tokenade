// The tests of `tokenizer`.
//
// Spliced in by `include!` from `tokenizer.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
fn estimate_reference(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut cost = 0.0f64;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if let Some(tier) = script_cpt(c as u32) {
            cost += match tier {
                ScriptTier::Heavy => 1.5,
                ScriptTier::Dense => 1.0,
                ScriptTier::Semi => 1.0 / 1.5,
                ScriptTier::Mid => 1.0 / 2.4,
            };
            i += 1;
        } else if c.is_ascii_alphabetic() {
            let start = i;
            let mut case_breaks = 0usize;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                if i > start && chars[i].is_ascii_uppercase() && chars[i - 1].is_ascii_lowercase() {
                    case_breaks += 1;
                }
                i += 1;
            }
            let len = (i - start) as f64;
            cost += (0.995 + 0.040 * len).max(len / 8.0) + 1.107 * case_breaks as f64;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            cost += 1.512 * ((i - start) as f64 / 3.0).ceil();
        } else if c.is_whitespace() {
            let start = i;
            let mut newlines = 0usize;
            while i < chars.len() && chars[i].is_whitespace() {
                if chars[i] == '\n' {
                    newlines += 1;
                }
                i += 1;
            }
            let padding = (i - start).saturating_sub(newlines).saturating_sub(1);
            cost += 0.242 * (padding as f64 / 8.0).ceil() + 1.336 * newlines as f64;
            // Mirror of the right-aligning-gutter term in `estimate`. The
            // oracle only works while it stays a mirror, and it is what
            // caught this: the two `same_count_as_*` tests were the ONLY
            // things in 7 080 that noticed the coefficient had moved.
            if padding >= 2 && chars.get(i).is_some_and(char::is_ascii_digit) {
                cost += 0.9;
            }
        } else {
            let start = i;
            while i < chars.len() && chars[i] == c {
                i += 1;
            }
            let mut pc = 0.429 * ((i - start) as f64 / 3.0).ceil();
            // Mirror of the quote-merge term in `estimate`; the oracle only
            // works while it stays a mirror.
            if i - start == 1
                && matches!(c, '"' | '\'')
                && (chars.get(i).is_some_and(char::is_ascii_alphanumeric)
                    || (start > 0 && chars[start - 1].is_ascii_alphanumeric()))
            {
                pc *= MERGE_FACTOR;
            }
            cost += pc;
        }
    }
    (cost.ceil() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate(""), 0);
    }

    /// The byte cursor must price EXACTLY what the `Vec<char>` loop priced.
    /// Every savings figure the product reports, every disclosure threshold
    /// and every gate is computed from this number, so "faster" is only
    /// acceptable at "identical".
    ///
    /// The cases are chosen where a byte cursor could plausibly diverge from a
    /// char cursor: whitespace that is not ASCII, a whitespace char that is
    /// ALSO a script tier (U+3000), runs of a repeated multi-byte punctuation
    /// mark, and the vertical tab — whitespace to `char::is_whitespace` but
    /// NOT to `u8::is_ascii_whitespace`.
    /// A quoted word costs what the same word costs unquoted, plus a little.
    ///
    /// cl100k merges a quote into the piece beside it. Measured against the
    /// real vocabulary before this term: the same words as `key = value` came
    /// out at −0.5 % and as `"value number 42 here"` at **+21.6 %** — only the
    /// quotes differed. That over-count is why tokenade REPORTED 44.4 % saved
    /// on a JSON corpus where the truth was 29.3 %: the raw side inflates more
    /// than the folded side, so the gap between them looks bigger than it is.
    #[test]
    fn a_quote_merges_into_the_word_it_touches() {
        let quoted: String = (0..200)
            .map(|i| format!("\"value number {i} here\"\n"))
            .collect();
        let bare: String = (0..200)
            .map(|i| format!("value number {i} here\n"))
            .collect();
        let (q, b) = (estimate(&quoted) as f64, estimate(&bare) as f64);
        assert!(q > b, "quotes must still cost something: {q} vs {b}");
        // Two quotes per line over ~5 tokens of words: before the merge this
        // ratio was 1.18, and cl100k puts it at ~1.03.
        assert!(
            q / b < 1.12,
            "a quote is still priced as a standalone mark: ratio {:.3}",
            q / b
        );
    }

    /// …and the merge is for QUOTES only.
    ///
    /// The same factor applied to every lone mark moves the mean error across
    /// the thirty corpora from +1.09 % to −4.94 % and the worst case from
    /// −22.0 % to −30.1 %: those coefficients were fitted with their
    /// compensations already in them. A comma between two words must keep its
    /// full price.
    #[test]
    fn the_merge_does_not_reach_other_punctuation() {
        let commas: String = (0..200)
            .map(|i| format!("alpha{i},beta{i},gamma{i},delta{i}\n"))
            .collect();
        let spaced: String = (0..200)
            .map(|i| format!("alpha{i} beta{i} gamma{i} delta{i}\n"))
            .collect();
        let (c, sp) = (estimate(&commas) as f64, estimate(&spaced) as f64);
        // A comma between words is a real token; a space is nearly free. If
        // the merge reached commas this ratio would collapse toward 1.
        assert!(
            c / sp > 1.10,
            "a comma is being merged like a quote: ratio {:.3}",
            c / sp
        );

        // …and it is the TOUCHING that licenses it. A quote standing alone
        // between spaces merges with nothing and must keep its full price;
        // dropping the adjacency test would discount it too.
        let lone: String = (0..200).map(|i| format!("a{i} \" b{i}\n")).collect();
        let none: String = (0..200).map(|i| format!("a{i}  b{i}\n")).collect();
        let (lo, no) = (estimate(&lone) as f64, estimate(&none) as f64);
        // The threshold SEPARATES the two implementations rather than naming a
        // true value, as the gutter guard below does: 0.185 tokens per line as
        // written, −0.030 if the adjacency test is dropped and every lone
        // quote is discounted. 0.10 sits between them.
        assert!(
            (lo - no) / 200.0 >= 0.10,
            "a lone quote between spaces was discounted: {:.3} per line",
            (lo - no) / 200.0
        );
    }

    /// A right-aligned numeric column costs what cl100k charges for it.
    ///
    /// Measured against the real tokenizer (`--features bpe`) and pinned here
    /// as a RATIO rather than an absolute, so the test says the same thing
    /// whichever build runs it: padded numeric columns must not price far
    /// below the same numbers separated by a single space, because the digits
    /// are identical and only the gutter differs.
    ///
    /// Before this term they did: 200 lines of four `{:>6}` integers
    /// estimated 2 193 against 2 945 actual (−25.5 %), while the single-spaced
    /// form was +2.8 %. Under-counting is the harmful direction — the same
    /// symptom this module already records for non-Latin scripts, where "a
    /// 5 000-token Chinese blob estimates at ~1 100 and never stashes".
    #[test]
    fn a_right_aligned_numeric_column_is_not_priced_as_indentation() {
        let padded: String = (0..200)
            .map(|i| format!("{:>6}{:>6}{:>6}{:>6}\n", i, i * 7, i * 13, i * 29))
            .collect();
        let spaced: String = (0..200)
            .map(|i| format!("{} {} {} {}\n", i, i * 7, i * 13, i * 29))
            .collect();
        let (p, sp) = (estimate(&padded) as f64, estimate(&spaced) as f64);
        // The padded form carries ~4 000 more space characters and the SAME
        // digits, so it must cost more, not less.
        assert!(
            p > sp,
            "padded columns priced below single-spaced ones: {p} vs {sp}"
        );
        // …and the gutters must not be free: before the fix the ratio was
        // 1.10, which is what "0.24 tokens per eight spaces" buys.
        assert!(
            p / sp >= 1.20,
            "a right-aligning gutter is still priced as indentation: ratio {:.2}",
            p / sp
        );

        // …and the term must stay tied to the DIGIT, not to padding in
        // general. Indentation before code is the shape this function sees
        // more than any other, and cl100k really does merge it cheaply —
        // charging a gutter there would over-count every source file.
        // Measured against real cl100k: indented Rust is +11 % already.
        let indented: String = (0..200)
            .map(|i| format!("        let value_{i} = compute({i});\n"))
            .collect();
        let flush: String = (0..200)
            .map(|i| format!("let value_{i} = compute({i});\n"))
            .collect();
        let (ind, fl) = (estimate(&indented) as f64, estimate(&flush) as f64);
        // The threshold SEPARATES the two implementations rather than
        // claiming a true value: 1.024 as written, 1.113 if the digit
        // condition is dropped and every gutter is charged. Anything between
        // catches the widening, and 1.06 leaves the current figure room to
        // move without a false alarm.
        assert!(
            ind / fl < 1.06,
            "indentation is being charged like a numeric gutter: ratio {:.3}",
            ind / fl
        );
    }

    #[test]
    fn same_count_as_the_char_vec_loop() {
        let cases: Vec<String> = vec![
            String::new(),
            " ".into(),
            "\u{0B}".into(),
            "a\u{0B}\u{0B}b".into(),
            "hello world".into(),
            "fn parseHTTPResponse(x: &str) -> Result<(), Error> { /* … */ }".into(),
            "=== --- /// ***".into(),
            "1 12 123 1234 12345".into(),
            "\n\n\n    indented\t\tand padded   \r\n".into(),
            "日本語のテキストです".into(),
            "한국어 텍스트".into(),
            "Кириллица и Ελληνικά".into(),
            "العربية ועברית".into(),
            "emoji 🙂🙂🙂 and ☂☂".into(),
            // Non-ASCII whitespace, and the one that is also a script tier.
            "a\u{00A0}\u{00A0}b".into(),
            "a\u{2028}\u{2029}b".into(),
            "a\u{3000}\u{3000}b".into(),
            "a \u{00A0}\n \u{3000} b".into(),
            // Repeated multi-byte punctuation — the run compare is a byte
            // compare and must not straddle a char boundary.
            "«««".into(),
            "—————".into(),
            "café ☕☕☕ naïve".into(),
            "mixed 中文abc123 ---\n\t«»".into(),
        ];
        for c in &cases {
            assert_eq!(estimate(c), estimate_reference(c), "diverged on {c:?}");
        }
        // And on every fixture in the tree — real command output, HTML, logs,
        // a11y trees, in several languages. Hand-picked cases prove the ones
        // someone thought of; this proves the ones nobody did.
        let mut checked = 0usize;
        // Anchored on the CRATE, not on the process CWD. A relative path here
        // depends on global state that other tests move: the `Layers`
        // helper changes the working directory, and this walk raced it into
        // "fixture corpus not found — checked 12". An earlier sweep caught the
        // tests that ASK for the CWD; this one never named it, it just opened
        // a relative path — the same dependency, spelled differently.
        // The fixture corpus stayed in `tokenade-core`: that crate carries
        // the eight test targets. Named absolutely for the same reason
        // as that sweep — a relative path depends on the current directory.
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("tokenade-core")
            .join("tests")
            .join("fixtures")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(t) = std::fs::read_to_string(&p) {
                    assert_eq!(
                        estimate(&t),
                        estimate_reference(&t),
                        "diverged on {}",
                        p.display()
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 100,
            "fixture corpus not found — checked {checked}"
        );
    }

    proptest::proptest! {
        /// The corpus proves the shapes that exist. This proves the ones that
        /// do not exist yet — arbitrary text, including the boundaries where a
        /// byte cursor and a char cursor can part ways.
        #[test]
        fn same_count_on_arbitrary_text(s in "(?s).{0,300}") {
            proptest::prop_assert_eq!(estimate(&s), estimate_reference(&s));
        }

        /// Uniform Unicode almost never lands on the handful of scalars that
        /// actually distinguish the two walks, so draw from them directly:
        /// both whitespace families, the vertical tab (whitespace to `char`,
        /// NOT to `u8::is_ascii_whitespace`), U+3000 (whitespace AND a script
        /// tier), and multi-byte punctuation a byte compare must not straddle.
        #[test]
        fn same_count_on_adversarial_alphabet(
            v in proptest::collection::vec(
                proptest::sample::select(vec![
                    'a', 'Z', 'q', '1', '9', ' ', '\t', '\n', '\r', '\u{0B}', '\u{0C}',
                    '-', '=', '/', '.', 'é', '«', '—', '中', '한', 'Ω', 'ا', '☕',
                    '\u{00A0}', '\u{2028}', '\u{2029}', '\u{3000}',
                ]),
                0..250,
            )
        ) {
            let s: String = v.into_iter().collect();
            proptest::prop_assert_eq!(estimate(&s), estimate_reference(&s));
        }
    }

    #[test]
    fn scales_with_length() {
        let short = estimate("fn main() {}");
        let long = estimate(&"fn main() {}\n".repeat(100));
        assert!(long > short * 50);
    }

    #[test]
    fn rough_calibration_english() {
        // ~9 words of prose → cl100k yields 9-11 tokens; we should land
        // in a tight band around that.
        let t = estimate("the quick brown fox jumps over the lazy dog");
        assert!((9..=14).contains(&t), "got {t}");
    }

    #[test]
    fn handles_code_and_prose_within_reasonable_bounds() {
        // Both stay close to cl100k's actual count — verified manually via
        // tiktoken on a few real samples. Code: ~14 tokens, prose: ~15.
        let code = "let x = vec![1, 2, 3]; foo(x, &y);";
        let prose = "the quick brown fox jumps over the lazy dog";
        let tc = estimate(code);
        let tp = estimate(prose);
        assert!((10..=22).contains(&tc), "code estimate {tc} out of range");
        assert!((9..=18).contains(&tp), "prose estimate {tp} out of range");
    }

    #[cfg(feature = "bpe")]
    #[test]
    fn bpe_exact_within_25pct_of_heuristic_on_realistic_text() {
        // The heuristic intentionally errs on the side of slight
        // over-estimation so a compactor never claims more savings than the
        // real BPE would show. 25 % covers the noise on small (10-token)
        // inputs and is comfortably tighter on the longer samples that
        // dominate real workloads.
        let samples = [
            "fn main() { println!(\"hello world\"); }",
            "The quick brown fox jumps over the lazy dog.",
            "{\"name\":\"redis-0\",\"status\":{\"phase\":\"Running\",\"podIP\":\"10.0.0.1\"}}",
            include_str!("./tokenizer.rs"),
        ];
        for s in samples {
            let exact = estimate_exact(s);
            let heur = estimate(s);
            let err = (exact as f64 - heur as f64).abs() / exact.max(1) as f64;
            assert!(
                err <= 0.25,
                "heuristic should be within 25% of BPE; sample={:.30?} exact={} heur={} err={:.2}",
                s,
                exact,
                heur,
                err
            );
        }
    }

    #[cfg(feature = "bpe")]
    #[test]
    fn bpe_within_25pct_on_non_latin_scripts() {
        // Multi-langue guard: before the per-script CPT tiers, a Latin-tuned
        // cpt under-counted these by 40–85 % (CJK ~1 tok/char vs the 4.0
        // prose tier), which silently broke the disclosure threshold for
        // non-Latin text. Each sample is repeated to dampen small-input noise.
        let samples: [(&str, &str); 6] = [
            ("zh", "令牌优化是减少大型语言模型上下文消耗的关键技术。"),
            (
                "ja",
                "トークン最適化は、コンテキスト消費を削減する重要な技術です。",
            ),
            (
                "ko",
                "토큰 최적화는 컨텍스트 소비를 줄이는 핵심 기술입니다.",
            ),
            (
                "ru",
                "Оптимизация токенов сокращает потребление контекста моделями.",
            ),
            ("ar", "تحسين الرموز تقنية أساسية لتقليل استهلاك السياق."),
            ("hi", "टोकन अनुकूलन संदर्भ खपत को कम करने की एक प्रमुख तकनीक है।"),
        ];
        for (lang, base) in samples {
            let s = base.repeat(6);
            let exact = estimate_exact(&s);
            let heur = estimate(&s);
            let err = (exact as f64 - heur as f64).abs() / exact.max(1) as f64;
            assert!(
                err <= 0.25,
                "non-latin heuristic should be within 25% of BPE; lang={lang} exact={exact} heur={heur} err={err:.2}"
            );
        }
    }
}

#[cfg(test)]
mod billed_units {
    use super::*;

    /// The measured Claude ratios, and nothing invented for a model nobody
    /// measured.
    #[test]
    fn an_estimated_token_converts_to_the_model_s_own_units() {
        assert_eq!(claude_tokens_per_estimated("claude-opus-5-5"), 1.6);
        assert_eq!(claude_tokens_per_estimated("Claude-Sonnet-5"), 1.6);
        assert_eq!(
            claude_tokens_per_estimated("claude-haiku-4-5-20251001"),
            1.27
        );
        assert_eq!(claude_tokens_per_estimated("gpt-5.5"), 1.0);
        assert_eq!(claude_tokens_per_estimated(""), 1.0);
    }
}
