// The tests of [`crate::redact`], moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `redact`, so every `super::` and `super::super::` keeps the meaning it
// had. Rewriting the paths would have been a change of
// semantics disguised as a file move — and it broke
// at the first `super::super::` it met.
//
// The split comes from `srcsweep::production_lines`, the declarer of
// "the half of a file that ships": it tracks braces AND raw
// strings. A hand-written delimiter saw ONE block in
// `auto.rs` where there are ten.

#[cfg(test)]
mod metering_tests {
    use super::*;

    #[test]
    fn redact_counting_counts_new_masks_only() {
        let (out, n) =
            redact_counting("API_KEY=sk-ant-SECRET123\nAuthorization: Bearer TOPSECRET456\n");
        assert!(n >= 2, "expected ≥2 masks, got {n}: {out}");
        // Idempotent: a second pass adds nothing (already-masked text).
        let (_, n2) = redact_counting(&out);
        assert_eq!(n2, 0, "re-scrub must count 0");
        // Clean text counts 0.
        assert_eq!(redact_counting("hello world, nothing secret").1, 0);
    }

    #[test]
    fn redact_rows_parse_and_sum() {
        let dir = std::env::temp_dir().join(format!("tkn_redact_meter_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("redact.jsonl");
        // Real-shaped timestamps: the reader now range-checks them, and toy
        // values like 100 are indistinguishable from a truncated splice.
        std::fs::write(
            &p,
            "{\"ts\":1784355200,\"n\":3}\n{\"ts\":1784355300,\"n\":2}\nnot json\n",
        )
        .unwrap();
        let rows = read_redact_rows(&p);
        let flat: Vec<(u64, u64)> = rows.iter().map(|(t, n, _)| (*t, *n)).collect();
        assert_eq!(flat, vec![(1784355200, 3), (1784355300, 2)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ledger must name the RULE, not just a daily total.
    ///
    /// A total is what it recorded before, and a rule that starts firing
    /// wrongly reads as an anonymous spike. Measured case: the entropy sweep
    /// was masking Rust test names (`web::tests::a11y_preserves_…`) on every
    /// `cargo test` run, and the ledger's answer was `n: 111`.
    #[test]
    fn the_ledger_names_which_rule_masked_what() {
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("tkn_redact_by_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("redact.jsonl");
        let prev = std::env::var_os("TOKENADE_REDACT_LEDGER");
        std::env::set_var("TOKENADE_REDACT_LEDGER", &p);

        // A GitHub token and a bare high-entropy value: two DIFFERENT passes
        // (`providers` and the `entropy` sweep that runs LAST), so the row must
        // carry two names rather than one lump.
        //
        // Which pass claims which shape is NOT obvious, and guessing it wrong
        // is how this test was first written: `X-Api-Key: <blob>` looks like
        // the entropy sweep's job and is actually taken by a provider pattern.
        // The breakdown this test exists to check is what said so.
        let out = redact_for_model(
            "token: ghp_0123456789abcdefghijklmnopqrstuvwxyz\n\
             cfg: Kd8sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x\n",
        );
        assert!(
            !out.contains("ghp_0123456789"),
            "provider secret leaked: {out}"
        );
        assert!(!out.contains("Kd8sB2mZ"), "entropy secret leaked: {out}");

        let rows = read_redact_rows(&p);
        let by = &rows.last().expect("a row was written").2;
        assert!(!by.is_empty(), "row carries no breakdown: {rows:?}");
        let total: u64 = by.values().sum();
        assert_eq!(total, rows.last().unwrap().1, "breakdown must sum to `n`");
        assert!(
            by.contains_key("providers"),
            "the provider pass masked the token but is not named: {by:?}"
        );
        assert!(
            by.contains_key("entropy"),
            "two passes fired but only one is named — the breakdown does not \
             discriminate: {by:?}"
        );

        match prev {
            Some(v) => std::env::set_var("TOKENADE_REDACT_LEDGER", v),
            None => std::env::remove_var("TOKENADE_REDACT_LEDGER"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A row with no breakdown reads exactly as it always did — the field is
    /// omitted, not written as an empty object, so old readers see no change.
    #[test]
    fn a_row_without_a_breakdown_is_byte_identical_to_the_old_shape() {
        assert_eq!(
            row_json(1784355200, 3, &RuleCounts::new()),
            "{\"ts\":1784355200,\"n\":3}\n"
        );
        let mut by = RuleCounts::new();
        by.insert("entropy".into(), 2);
        by.insert("providers".into(), 1);
        assert_eq!(
            row_json(1784355200, 3, &by),
            "{\"ts\":1784355200,\"n\":3,\"by\":{\"entropy\":2,\"providers\":1}}\n"
        );
    }

    /// A torn row that stayed valid JSON must not be trusted for its date.
    ///
    /// Verbatim from a dogfood box: `{"ts":11784355200,"n":0}` — a spliced
    /// leading digit puts it in the year 2343, where `redactions_since` counts
    /// it in every window forever and `redactions_by_day` opens a 2343 bucket.
    #[test]
    fn a_spliced_timestamp_is_undated_not_counted_as_future() {
        let dir = std::env::temp_dir().join(format!("tkn_redact_ts_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("redact.jsonl");
        std::fs::write(
            &p,
            "{\"ts\":11784355200,\"n\":7}\n{\"ts\":1784355200,\"n\":5}\n{\"ts\":1,\"n\":4}\n",
        )
        .unwrap();
        let rows = read_redact_rows(&p);
        let flat: Vec<(u64, u64)> = rows.iter().map(|(t, n, _)| (*t, *n)).collect();
        assert_eq!(
            flat,
            vec![(0, 7), (1784355200, 5), (0, 4)],
            "out-of-range timestamps must become undated while keeping their count"
        );
        // The counts survive — only the dates were unusable.
        assert_eq!(rows.iter().map(|(_, n, _)| n).sum::<u64>(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
#[cfg(test)]
mod line_redactor_tests {
    use super::{redact, LineRedactor};

    const KEY: &str = "-----BEGIN RSA PRIVATE KEY-----\n\
                       MIIEowIBAAKCAQEA3Zx8kQ9wVsecretmaterialhere0123456789\n\
                       AAAAB3NzaC1yc2EAAAADAQABAAABgQDmoreKeyBytes9876543210\n\
                       -----END RSA PRIVATE KEY-----";

    fn stream(text: &str) -> String {
        let mut r = LineRedactor::new();
        let mut out = String::new();
        for line in text.lines() {
            if let Some(l) = r.line(line) {
                out.push_str(&l);
                out.push('\n');
            }
        }
        out
    }

    /// A key spans lines; a caller that streams sees one at a time. Whichever
    /// way the bytes arrive, the material must not come out the other side.
    #[test]
    fn a_key_split_across_lines_is_redacted_line_by_line_too() {
        let doc = format!("before\n{KEY}\nafter=ok\n");
        let streamed = stream(&doc);
        for material in [
            "MIIEowIBAAKCAQEA3Zx8kQ9wVsecretmaterialhere0123456789",
            "AAAAB3NzaC1yc2EAAAADAQABAAABgQDmoreKeyBytes9876543210",
        ] {
            assert!(
                !streamed.contains(material),
                "key material survived the streaming path:\n{streamed}"
            );
        }
        // The whole-text redactor is the reference: the streamed answer must
        // not be weaker than it.
        let whole = redact(&doc);
        assert!(!whole.contains("MIIEowIBAAKCAQEA3Zx8kQ9wVsecretmaterialhere0123456789"));
        assert!(streamed.contains("<redacted-private-key>"), "{streamed}");
        // And it must not eat the document around the key.
        assert!(streamed.contains("before"), "{streamed}");
        assert!(
            streamed.contains("after=<redacted>") || streamed.contains("after=ok"),
            "{streamed}"
        );
    }

    /// Re-running on an already-redacted file changes nothing — the help calls
    /// the command idempotent, and the block is recognised on the RAW line, so
    /// the marker left by a previous pass must not open a new block and eat
    /// the rest of the document.
    #[test]
    fn a_second_pass_over_a_redacted_file_keeps_it_whole() {
        let once = stream(&format!("head\n{KEY}\ntail line one\ntail line two\n"));
        let twice = stream(&once);
        assert_eq!(once, twice, "the second pass changed the file");
        assert!(
            twice.contains("tail line one") && twice.contains("tail line two"),
            "{twice}"
        );
    }

    /// A marker pair on ONE line closes there. Treating it as an opening would
    /// swallow every line after it.
    #[test]
    fn a_one_line_key_does_not_open_a_block() {
        let out = stream("-----BEGIN PRIVATE KEY----- abc -----END PRIVATE KEY-----\nkept\n");
        assert!(
            out.contains("kept"),
            "the rest of the document was eaten:\n{out}"
        );
    }
}
#[cfg(test)]
mod pem_bounds_tests {
    use super::redact;

    /// A key whose END marker was clipped away must still be redacted in full.
    /// Clipping happens in every compactor and runs BEFORE redaction, so this
    /// is the shape the rule actually meets — not the textbook one.
    #[test]
    fn a_private_key_without_its_end_marker_is_still_redacted() {
        let body = "MIIEowIBAAKCAQEAy8Dbv8prpJ/0kKhlGeJYozo2t60EG8L0561g13R29LvMR5hy";
        for text in [
            format!("-----BEGIN RSA PRIVATE KEY-----\n{body}\n"),
            format!("-----BEGIN RSA PRIVATE KEY-----\n{body}\n-----END RSA PRIVATE KEY-"),
            format!("\"note\": \"-----BEGIN RSA PRIVATE KEY-----\\n{body}\\n-----END RSA PRIVATE KEY-\""),
            format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{body}"),
        ] {
            let out = redact(&text);
            assert!(
                !out.contains(body),
                "key material survived:\n  in:  {text}\n  out: {out}"
            );
        }
    }

    /// And it must not eat the document. The rule consumes runs of 20+ base64
    /// characters, so ordinary prose after a header stops it at the first
    /// short word — over-redaction is the other way to lose a document.
    #[test]
    fn the_rule_stops_at_prose() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\n                    The deploy key used to live here but we rotated it in March.\n                    Ask the platform team if you need the new one.\n";
        let out = redact(text);
        assert!(
            out.contains("rotated it in March"),
            "prose was swallowed:\n{out}"
        );
        assert!(
            out.contains("Ask the platform team"),
            "prose was swallowed:\n{out}"
        );
    }

    /// A long base64 blob that is NOT introduced by a key header is left alone:
    /// data URIs, checksums and encoded payloads are content.
    #[test]
    fn a_base64_blob_that_is_not_a_key_is_untouched() {
        let blob = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk";
        let text = format!("image: data:image/png;base64,{blob}\n");
        assert!(
            redact(&text).contains(blob),
            "redacted ordinary base64 data"
        );
    }
}
#[cfg(test)]
mod tests {

    /// A package version is not a bank account number.
    ///
    /// The generic account rule matches 8-17 digits and fires only when an
    /// account word sits within 48 bytes. Both halves misfired at once on a
    /// real `pacman -Q`: `\b\d{8,17}\b` treats `.` as a boundary, so it
    /// matched the `20260526` inside the version `20260526.0-2`, and the
    /// context word came from the PACKAGE NAME on the next line,
    /// `accounts-qml-module`. One version in 1 533 came back redacted, from
    /// a listing whose whole purpose is its versions.
    ///
    /// The fixtures oppose on the only thing that separates them: whether
    /// the digits stand alone. Both carry the context word, so a test that
    /// only checked the gate would pass with the fix removed.
    #[test]
    fn a_dotted_version_is_not_an_account_number() {
        let listing = "abseil-cpp 20260526.0-2\naccounts-qml-module 0.7-8\n";
        let out = redact(listing);
        assert_eq!(
            out, listing,
            "a package version was redacted as an account number:\n{out}"
        );

        // Same context word, digits standing alone: still redacted.
        let real = "accounts payable\nAccount 20260526 was debited\n";
        let out = redact(real);
        assert!(
            !out.contains("20260526"),
            "a real account number survived, so the rule was gutted:\n{out}"
        );

        // Ending a sentence is not "continuing into a token".
        let dotted_end = "Account: 123456789012.\n";
        let out = redact(dotted_end);
        assert!(
            !out.contains("123456789012"),
            "a number ending a sentence was treated as part of a longer token:\n{out}"
        );

        // And a hyphen that continues into more characters is.
        let hyphenated = "account log\nbuild-20260526-rc1 finished\n";
        let out = redact(hyphenated);
        assert_eq!(
            out, hyphenated,
            "a build id was redacted as an account number:\n{out}"
        );

        // Joined on the LEFT only. The previous fixture is joined on both
        // sides, so it passes with the left half of the rule removed.
        let left_only = "account log\nrc-20260526 finished\n";
        let out = redact(left_only);
        assert_eq!(
            out, left_only,
            "a number joined to its prefix was taken for an account:\n{out}"
        );

        // The PROXIMITY half of the gate. A document that mentions accounts
        // somewhere opens the outer gate for the whole text, so a fixture
        // with no account word at all cannot show that the per-match
        // distance check is still there — it is the outer gate doing the
        // work. This one carries the word far away from the number.
        let far = format!(
            "accounts payable summary\n{}\nrun 2026052699 completed\n",
            "filler line with nothing of interest on it".repeat(3)
        );
        let out = redact(&far);
        assert!(
            out.contains("2026052699"),
            "a number 130 characters from the only account word was redacted:\n{out}"
        );

        // And with no account word anywhere, nothing fires at all.
        let no_context = "run 2026052699 completed in 4 minutes\n";
        let out = redact(no_context);
        assert_eq!(
            out, no_context,
            "a bare number was redacted with no account word in sight:\n{out}"
        );
    }

    /// The context gates read the ORIGINAL text while their rules run on a
    /// partly-masked copy. That is only sound because masking can never
    /// INTRODUCE a gate keyword — otherwise a keyword could appear in the copy,
    /// be absent from the original, and the gate would skip a rule that had
    /// something to mask. A missed redaction is the one failure this module
    /// cannot have, so the argument is asserted rather than trusted.
    ///
    /// Two ways masking could introduce one, both closed here:
    ///   1. the replacement text itself contains a keyword;
    ///   2. a keyword spans the seam where a span was cut out — impossible
    ///      while the replacement is delimited by `<` and `>`, which no keyword
    ///      contains.
    #[test]
    fn masking_can_never_introduce_a_context_keyword() {
        const MASK: &str = "<redacted>";
        let lower = MASK.to_ascii_lowercase();
        for (name, kws) in [
            ("ABA", ABA_KWS),
            ("NPI", NPI_KWS),
            ("NHS", NHS_KWS),
            ("ACCT", ACCT_KWS),
            ("CRYPTO", CRYPTO_KWS),
        ] {
            for k in kws {
                assert!(
                    !lower.contains(k),
                    "{name} keyword {k:?} is inside the replacement — gating on \
                     the original text would then be unsound"
                );
                assert!(
                    !k.contains('<') && !k.contains('>'),
                    "{name} keyword {k:?} contains a replacement delimiter, so it \
                     could span the seam left by a masked span"
                );
            }
        }
        assert!(
            MASK.starts_with('<') && MASK.ends_with('>'),
            "the seam argument depends on these delimiters"
        );
    }

    /// The provider pass must stay LINEAR and cheap in the size of its input.
    ///
    /// This is a regression guard with a real corpse behind it. The pass used
    /// to triage with a `regex::RegexSet`, on the reasoning that one automaton
    /// beats 112 — "identical by construction, not by heuristic", which was
    /// true of the OUTPUT and said nothing about the cost. Measured on 200 KB
    /// of base64-shaped text: **374 ms for the set scan against 2 ms for
    /// `is_match` on all 112 rules individually**, ~180x. It never showed up in
    /// the change's own measurement because that was taken on a command label
    /// of a few dozen bytes, where a set scan is free and the compilation it
    /// avoided was the whole cost.
    ///
    /// Content matters here: a repeating filler haystack understates the cost
    /// by ~450x versus pseudo-random base64, so this generates the latter.
    ///
    /// The bound is a RATIO against one representative rule scanning the same
    /// haystack, not a wall-clock number: an unoptimised `cargo test` build
    /// runs regexes ~100x slower than release, so any absolute millisecond
    /// budget is either useless in debug or vacuous in release. The ratio moves
    /// with neither.
    #[test]
    fn the_provider_pass_stays_linear_on_large_input() {
        const AL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let hay: String = (0..204_000)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                if i % 77 == 76 {
                    '\n'
                } else {
                    AL[(x % 64) as usize] as char
                }
            })
            .collect();
        // Compile everything first: this measures the SCAN, not the one-off
        // build that `redact::warm` moves off the critical path anyway.
        let ps = patterns();
        for i in 0..ps.raw.len() {
            let _ = ps.rule(i);
        }
        // Reference: the single most expensive rule in the set, on this exact
        // haystack, in this exact build.
        let reference = Regex::new(r"\b[5KL][1-9A-HJ-NP-Za-km-z]{50,51}\b").unwrap();
        // Both sides are timed BEST-OF-N, not once. The reference scan lasts a
        // few hundred nanoseconds, so a single sample of it is whatever the
        // scheduler was doing at that instant — and it sits in the
        // DENOMINATOR, which is the direction that makes interference look
        // like a regression. Measured 2026-08-18: run alone this test passes
        // on Linux and on the macOS VM, and it still failed at `130x` inside
        // the macOS release build, where 6757 other tests were saturating
        // every core. It took the whole build down with it. Twelve busy-loops
        // pinning the host did NOT reproduce it in 10 runs, which is the tell
        // that the noise is in the sub-microsecond sample and not in the load.
        // The minimum of N is the run that was interfered with least on each
        // side, which is the comparison the assertion below actually means.
        const REPS: usize = 9;
        let mut unit = u128::MAX;
        for _ in 0..REPS {
            let t = std::time::Instant::now();
            assert!(!reference.is_match(&hay));
            unit = unit.min(t.elapsed().as_nanos());
        }
        let unit = unit.max(1);

        let mut whole = u128::MAX;
        let mut out = String::new();
        for _ in 0..REPS {
            let t = std::time::Instant::now();
            out = apply_provider_patterns(&hay);
            whole = whole.min(t.elapsed().as_nanos());
        }

        assert_eq!(
            out.len(),
            hay.len(),
            "random base64 holds no provider secret"
        );
        let ratio = whole / unit;
        assert!(
            ratio < 100,
            "the provider pass costs {ratio}x one rule's scan of the same bytes. \
             Running all {} rules costs ~4x; the RegexSet triage it replaced cost \
             ~740x. Suspect a whole-set scan.",
            ps.raw.len()
        );
    }

    /// The warm-up is only worth its thread if it actually reaches the passes
    /// behind the cheap signature gates. If a pass stops matching the corpus —
    /// a rule tightened, a gate keyword renamed — the warm-up silently
    /// degrades into compiling two regex sets and nothing else, and the cost it
    /// was hiding quietly comes back onto the critical path with no test
    /// failing anywhere. So assert the corpus is still redacted line by line.
    #[test]
    fn warm_corpus_still_trips_every_gate() {
        // Per BLOCK, not per line: several gates are multi-line (the cookie
        // table needs its header, the k8s manifest needs `kind: Secret`), so a
        // line-by-line check would demand a redaction from a bare CSV header.
        for block in WARM_BLOCKS {
            assert_ne!(
                &redact(block),
                block,
                "no pass claims this warm-corpus block any more, so warming it \
                 is a no-op: {block:?}"
            );
        }
        // The opt-in pass is checked against its own function: `redact` would
        // skip it entirely with the env var unset, and mutating the process
        // environment from a test is how this suite has stalled before.
        assert_ne!(
            redact_optin_pii(WARM_BLOCK_OPTIN_PII),
            WARM_BLOCK_OPTIN_PII,
            "the opt-in PII pass no longer claims its warm block"
        );
    }

    /// `warm` must be a pure warm-up: it takes no arguments, writes nothing,
    /// and above all does not change what a later `redact` returns.
    #[test]
    fn warming_does_not_change_what_redact_returns() {
        let sample = "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\nplain line\n";
        let before = redact(sample);
        warm();
        assert_eq!(redact(sample), before);
    }

    /// The proof that the triage changed nothing.
    ///
    /// `redact` used to compile all 80 provider rules and run every one of them
    /// in order. It now compiles a rule only when the shared `RegexSet` reports
    /// that it matches. The argument is that an unreported rule cannot match, so
    /// running it would be a no-op — but "an argument" is not what one accepts
    /// in redaction code, so this re-runs the OLD implementation beside the new
    /// one and demands byte equality.
    #[cfg(test)]
    mod triage_is_equivalent {
        use super::super::{apply_provider_patterns, patterns, redact};
        use regex::Regex;

        /// The pre-2026-08-04 implementation, verbatim in behaviour.
        fn eager(s: &str) -> String {
            let ps = patterns();
            let mut out = s.to_string();
            for (pat, rep) in &ps.raw {
                let re = Regex::new(pat).expect("static");
                out = re.replace_all(&out, *rep).into_owned();
            }
            out
        }

        fn corpus() -> Vec<String> {
            let mut v: Vec<String> = vec![
                String::new(),
                "echo hi".into(),
                "cargo test --workspace".into(),
                "let token = line.split(':').nth(1);".into(),
                "PGPASSWORD=hunter2 psql -h db".into(),
                "export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123".into(),
                "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc.def".into(),
                "aws_access_key_id = AKIAIOSFODNN7EXAMPLE".into(),
                "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----"
                    .into(),
                "curl -H 'X-Api-Key: sk-proj-abcdefghijklmnopqrstuvwxyz012345'".into(),
                concat!("slack xo", "xb-123456789012-1234567890123-abcdefghijklmnopqrstuvwx").into(),
                "card 4111 1111 1111 1111 cvv 123".into(),
                concat!("stripe sk_", "live_abcdefghijklmnopqrstuvwx").into(),
                "google AIzaSyA1234567890abcdefghijklmnopqrstuv".into(),
                "npm //registry.npmjs.org/:_authToken=npm_abcdefghijklmnopqrstuvwxyz0123".into(),
                "twilio SK0123456789abcdef0123456789abcdef".into(),
                "sendgrid SG.abcdefghijklmnopqrstuv.abcdefghijklmnopqrstuvwxyz0123456789ab".into(),
                "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQDabcdefghij user@host".into(),
                "un paragraphe ordinaire, sans le moindre secret dedans".into(),
                "{\"password\": \"p4ssw0rd\", \"user\": \"bob\"}".into(),
            ];
            // Already-redacted text: the idempotence assumption the triage
            // leans on, exercised explicitly.
            let redacted: Vec<String> = v.iter().map(|s| redact(s)).collect();
            v.extend(redacted);
            // Concatenations, so several rules fire in one pass and ORDER matters.
            let joined = v.join("\n");
            v.push(joined);
            v
        }

        #[test]
        fn same_output_as_the_eager_loop() {
            for input in corpus() {
                let (a, b) = (apply_provider_patterns(&input), eager(&input));
                assert_eq!(
                    a.as_bytes(),
                    b.as_bytes(),
                    "triage diverged on {input:?}\n  triage: {:?}\n  eager : {:?}",
                    a.escape_debug().to_string(),
                    b.escape_debug().to_string()
                );
            }
        }

        /// Guard on the guard: if the corpus stopped containing anything the
        /// rules fire on, the test above would pass vacuously.
        #[test]
        fn the_corpus_actually_exercises_the_rules() {
            let hits = corpus()
                .iter()
                .filter(|s| apply_provider_patterns(s) != **s)
                .count();
            assert!(
                hits >= 10,
                "only {hits} corpus entries are redacted at all; the equivalence \
                 test would prove little"
            );
        }
    }

    /// Field report 2026-08-03, and the most expensive kind of false positive:
    /// the redactor masked the right-hand side of an ordinary Rust binding, the
    /// agent patched a line it had never seen whole, its replacement reported
    /// zero matches, and the fix was silently never applied.
    #[cfg(test)]
    mod source_code_is_not_a_secret {
        use super::super::redact;

        #[test]
        fn a_binding_whose_name_looks_secret_survives() {
            for line in [
                "let token = line.split(':').nth(1).unwrap_or_default();",
                "let secret = compute_secret(cfg);",
                "let password = prompt.read_line()?;",
                "let pass = matches.get_flag(\"pass\");",
                "let auth_token = header.strip_prefix(\"Bearer \");",
            ] {
                assert_eq!(
                    redact(line),
                    line,
                    "an expression was masked; an agent editing this line would \
                     patch text it never saw"
                );
            }
        }

        /// The half that must NOT be given up: the shapes a credential actually
        /// travels in.
        #[test]
        fn real_credential_shapes_are_still_masked() {
            for (input, must_not_contain) in [
                ("PGPASSWORD=hunter2 psql -h db", "hunter2"),
                (
                    "export token=ghp_abcdefghijklmnopqrstuvwxyz0123",
                    "ghp_abcdefghijklmnopqrstuvwxyz0123",
                ),
                ("secret=s3cr3tvalue", "s3cr3tvalue"),
                (
                    "let token = \"ghp_hardcodedliteral0123456789\";",
                    "ghp_hardcodedliteral0123456789",
                ),
            ] {
                let out = redact(input);
                assert!(
                    !out.contains(must_not_contain),
                    "{input:?} leaked its value: {out:?}"
                );
            }
        }
    }
    /// A secret keyword followed by a SPACE is still a secret keyword.
    ///
    /// `aws configure set aws_secret_access_key <VALUE>` carried the keyword
    /// right next to the value and travelled in clear, because every rule
    /// keyed on `=` or `:`. The command line is where credentials are typed.
    #[test]
    fn a_credential_passed_as_a_command_argument_is_masked() {
        for line in [
            "aws configure set aws_secret_access_key wJalrXUtnFEMI/K7MDENG/bPxRfiCY",
            "aws configure set aws_session_token FQoGZXIvYXdzEBYaDN0examplelong",
            "docker login -u me --password hunter2SuperSecretLong",
            "app --client-secret 9f8e7d6c5b4a32100abc",
        ] {
            let out = redact(line);
            assert!(out.contains("<redacted>"), "not masked: {out}");
        }
    }

    /// And the other direction, which is what keeps the rule usable: prose,
    /// documentation placeholders and shell references are not secrets.
    #[test]
    fn documentation_and_shell_references_are_left_readable() {
        for line in [
            "Run `aws configure set aws_secret_access_key` to store it.",
            "set aws_secret_access_key to your key",
            "--password is required for this command",
            "the --token flag takes a value",
            "--password <YOUR_PASSWORD>",
            "--token $GITHUB_TOKEN",
        ] {
            let out = redact(line);
            assert!(!out.contains("<redacted>"), "over-masked: {line} -> {out}");
        }
    }

    /// A call or a path on the right of the separator means the line is code.
    #[test]
    fn an_assignment_whose_value_is_a_call_is_left_alone() {
        for line in [
            "let user_password = form.get(\"pw\");",
            "let secret = derive(seed);",
            "let token = iter.next();",
            "api_key = build_key(cfg);",
            "self.auth_token = Token::from(raw);",
            "primary_key=True)",
            "pub struct Cfg { password: String }",
            "    secret: Vec,",
        ] {
            assert_eq!(&super::redact(line), line, "redaction ate code: {line}");
        }
    }

    /// And the credentials those same rules exist for still lose their values.
    /// No credential format contains `(`, `)` or `::`, so the veto costs
    /// nothing here.
    #[test]
    fn a_keyword_assignment_with_a_real_value_is_still_masked() {
        for (line, kept) in [
            ("password=hunter2", "password="),
            ("api_key = abc123def456ghi", "api_key = "),
            ("PGPASSWORD=s3cr3t", "PGPASSWORD="),
            ("token=eyJhbGciOiJIUzI1NiJ9.payload.sig", "token="),
            ("  password: hunter2", "password: "),
        ] {
            let out = super::redact(line);
            assert!(out.contains(kept), "the NAME stays readable: {out}");
            assert!(out.contains("<redacted>"), "the VALUE is masked: {out}");
        }
    }

    /// Reading code is what an agent does most, and it EDITS against what it
    /// read. A redaction that eats the right-hand side of an assignment hands
    /// it a line that never existed.
    #[test]
    fn an_assignment_to_a_secret_shaped_identifier_is_left_alone() {
        for line in [
            "let kind_token = value_kind_token(template_obj.get(k).unwrap_or(&Value::Null));",
            "x_secret = compute(1);",
            "let refresh_token = self.session.refresh();",
            "let signing_key = keys.first().cloned();",
        ] {
            assert_eq!(&super::redact(line), line, "redaction ate code: {line}");
        }
    }

    /// The other side of the same rule: an env var still loses its value.
    #[test]
    fn a_screaming_snake_env_assignment_is_still_masked() {
        for (line, kept) in [
            (
                "GITHUB_TOKEN=ghp_16C7e42F292c6912E7710c838347Ae178B4a",
                "GITHUB_TOKEN=",
            ),
            ("export API_KEY=abc123def456", "API_KEY="),
            ("DATABASE_PASSWORD=hunter2", "DATABASE_PASSWORD="),
            ("MY_APP_SECRET=s3cr3t", "MY_APP_SECRET="),
        ] {
            let out = super::redact(line);
            assert!(out.contains(kept), "the NAME stays readable: {out}");
            assert!(out.contains("<redacted>"), "the VALUE is masked: {out}");
        }
    }

    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn redacts_bearer_token() {
        let out =
            redact(r#"curl -H "Authorization: Bearer eyJhbGciOiJI.eyJzdWIiOi.signaturedata""#);
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
        assert!(!out.contains("eyJhbGciOiJI"));
    }

    /// Round 16 — `Bearer <x>` / `Basic <x>` used to redact the next word
    /// unconditionally, so `swagger.io/…/basic-structure/` was delivered
    /// titled `# Basic <redacted>`. Documentation prose must survive; a real
    /// credential must not.
    #[test]
    fn bare_auth_schemes_redact_credentials_not_documentation_prose() {
        for prose in [
            "Basic Structure",
            "Basic Usage",
            "Basic Concepts",
            "Basic Authentication",
            "Pass a Bearer token in the header",
        ] {
            assert_eq!(redact(prose), prose, "prose must survive: {prose}");
        }
        for cred in [
            "Basic YWRtaW46czNjcmV0",
            "Basic dXNlcjpwYXNzd29yZDE=",
            "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc",
        ] {
            let out = redact(cred);
            assert!(
                out.contains("<redacted>"),
                "credential must be masked: {cred} -> {out}"
            );
        }
        // The header form is masked whatever the value looks like: that is
        // where a credential actually travels, so shape is not consulted.
        let hdr = redact("Authorization: Basic Structure");
        assert!(
            hdr.contains("<redacted>"),
            "header form stays unconditional: {hdr}"
        );
    }

    #[test]
    fn redacts_basic_auth() {
        let out = redact("curl -u admin:s3cret -H 'Authorization: Basic YWRtaW46czNjcmV0'");
        assert!(out.contains("Basic <redacted>"));
    }

    #[test]
    fn redacts_aws_access_key() {
        let out = redact("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn redacts_github_pat() {
        let out = redact("export GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(!out.contains("ghp_abcdef"));
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn redacts_stripe_live_key() {
        let out = redact("STRIPE_KEY=sk_live_51Hxyz123abc456def789ghi");
        assert!(!out.contains("sk_live_51Hxyz"));
    }

    #[test]
    fn redacts_quoted_env_assignments() {
        // A quoted env-var assignment must redact: the bare-only value
        // matcher stopped at the opening quote, so quoted secrets escaped.
        // Both single- and double-quoted values must redact.
        let single = redact("MY_API_KEY='sk_fake_quotedSecretValue' node -e foo");
        assert!(
            !single.contains("sk_fake_quotedSecretValue"),
            "single-quote leak: {single}"
        );
        assert!(single.contains("<redacted>") && single.contains("node -e foo"));

        let double = redact("export TOKEN=\"super-secret-value-123\"");
        assert!(
            !double.contains("super-secret-value-123"),
            "double-quote leak: {double}"
        );

        let api = redact("curl 'https://x/y?api_key=\"abc123def\"'");
        assert!(!api.contains("abc123def"), "quoted api_key leak: {api}");

        let pwd = redact("PASSWORD='p@ss w0rd!'");
        assert!(
            !pwd.contains("p@ss w0rd!"),
            "quoted password (with space) leak: {pwd}"
        );

        // Bare (unquoted) values must still redact — no regression.
        let bare = redact("MY_API_KEY=ak_98_bareToken123");
        assert!(
            !bare.contains("ak_98_bareToken123"),
            "bare regression: {bare}"
        );
    }

    // --- Stripe test/restricted, GitLab, Twilio, SendGrid, Mailgun, Postman ---

    #[test]
    fn redacts_stripe_test_and_restricted_keys() {
        let out = redact("sk_test_51Hxyz123abc456def789ghi");
        assert!(!out.contains("51Hxyz"), "sk_test_ leaked: {out}");
        let out = redact("rk_live_xyzABC123def456GHI789jklMNO");
        assert!(!out.contains("xyzABC123"), "rk_live_ leaked: {out}");
        let out = redact("rk_test_xyzABC123def456GHI789jklMNO");
        assert!(!out.contains("xyzABC123"), "rk_test_ leaked: {out}");
    }

    #[test]
    fn redacts_gitlab_personal_access_token() {
        let out = redact("CI_JOB_TOKEN=glpat-abcd1234EFGH5678ijkl9012");
        assert!(!out.contains("abcd1234EFGH"), "glpat- leaked: {out}");
        let out = redact("glsoat-runner-token-12345_abc_67890");
        assert!(!out.contains("runner-token"), "glsoat- leaked: {out}");
        let out = redact("glrt-abc-def-ghi-jkl-mno-pqr-stu");
        assert!(!out.contains("abc-def-ghi"), "glrt- leaked: {out}");
    }

    #[test]
    fn redacts_twilio_sid_and_auth_token() {
        // Twilio Account SID: AC + 32 lowercase hex.
        let out = redact("TWILIO_ACCOUNT=ACabcdef1234567890abcdef1234567890");
        assert!(!out.contains("ACabcdef1234"), "AC SID leaked: {out}");
        // API key SID: SK + 32 lowercase hex.
        let out = redact("TWILIO_KEY=SKabcdef1234567890abcdef1234567890");
        assert!(!out.contains("SKabcdef1234"), "SK SID leaked: {out}");
    }

    #[test]
    fn twilio_pattern_does_not_false_positive_on_common_substrings() {
        // "ACK", "SKIP" etc. must NOT be redacted — the word boundary
        // + length-32 hex anchor prevents this.
        let out = redact("ACK received\nSKIP this line\nACCEPT or reject");
        assert!(out.contains("ACK received"), "false positive on ACK: {out}");
        assert!(
            out.contains("SKIP this line"),
            "false positive on SKIP: {out}"
        );
        assert!(out.contains("ACCEPT"), "false positive on ACCEPT: {out}");
    }

    #[test]
    fn redacts_sendgrid_api_key() {
        // SG.<22>.<43> — strict format from SendGrid spec.
        let key = "SG.abcdefghijklmnopqrstuv.0123456789abcdefghij0123456789abcdefghij012345";
        let out = redact(&format!("SENDGRID_KEY={key}"));
        assert!(!out.contains("abcdefghijklmnop"), "SendGrid leaked: {out}");
    }

    #[test]
    fn redacts_mailgun_key() {
        let out = redact("MAILGUN_KEY=key-abcdef1234567890abcdef1234567890");
        assert!(!out.contains("abcdef1234"), "Mailgun leaked: {out}");
    }

    // --- URL userinfo coverage ---

    #[test]
    fn redacts_url_userinfo_password_segment() {
        // DSN schemes (postgres/mysql/redis/mongodb/amqp/ftp) redact the WHOLE
        // authority — the infra host is itself sensitive — keeping only the
        // scheme + path so the URI stays grep-able. (Non-DSN schemes keep the
        // user + host; see `redacts_url_userinfo_https_form`.)
        let out = redact("postgres://alice:supersecret@db.example.com:5432/app");
        assert!(!out.contains("supersecret"), "userinfo leaked: {out}");
        assert!(!out.contains("db.example.com"), "DSN host leaked: {out}");
        assert!(
            out.contains("postgres://") && out.contains("/app"),
            "scheme/path lost: {out}"
        );
    }

    #[test]
    fn redacts_url_userinfo_https_form() {
        let out = redact("https://bot:ghp_abcdef0123456789abcdef0123456789xyz@github.com/repo");
        assert!(!out.contains("ghp_abcdef"), "https userinfo leaked: {out}");
        assert!(out.contains("bot"), "user lost: {out}");
    }

    #[test]
    fn url_userinfo_pattern_does_not_match_plain_email() {
        // The URL-userinfo rule (`scheme://user:pwd@`) must NOT fire on a
        // plain email: no `://`, no user:pwd structure. The PII pass (I) now
        // masks the local part but keeps the `@domain` grep-able — the
        // userinfo `:<redacted>@` mangling must never appear.
        let out = redact("Contact noreply@example.com for support.");
        assert!(
            !out.contains(":<redacted>@"),
            "userinfo rule wrongly fired: {out}"
        );
        assert!(out.contains("@example.com"), "email domain lost: {out}");
    }

    #[test]
    fn redacts_postman_api_key_and_access_token() {
        // PMAK-<24 hex>-<34 hex>
        let pmak = "PMAK-abcdef1234567890abcdef12-0123456789abcdef0123456789abcdef0123";
        let out = redact(&format!("POSTMAN={pmak}"));
        assert!(!out.contains("abcdef12345"), "PMAK leaked: {out}");
        // PMAT-<40 hex>
        let pmat = "PMAT-abcdef0123456789abcdef0123456789abcdef0123";
        let out = redact(&format!("POSTMAN={pmat}"));
        assert!(!out.contains("abcdef012"), "PMAT leaked: {out}");
    }

    #[test]
    fn redacts_jwt_in_url() {
        let out = redact("https://api.example.com/?token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("SflKxwRJSMeKKF2QT4fwpMeJf36"));
    }

    #[test]
    fn redacts_api_key_query_param() {
        let out = redact("curl 'https://api.example.com?api_key=sk-prod-abc123def456'");
        assert!(!out.contains("sk-prod-abc123"));
        assert!(out.contains("<redacted>"));
    }

    // --- LLM / cloud provider key shape tests (no env-var prefix) ---

    #[test]
    fn redacts_bare_anthropic_key() {
        // sk-ant-… with 90+ chars of body (real Anthropic format).
        let key = "sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let out = redact(&format!("debug: client_key = {key}"));
        assert!(out.contains("<redacted>"), "got: {out}");
        assert!(!out.contains("api03-aaa"));
    }

    #[test]
    fn redacts_bare_openai_key() {
        // OpenAI sk-… (40+ chars) without sk-ant- prefix.
        let key = "sk-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let out = redact(&format!("OPENAI={key} done"));
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn redacts_google_api_key() {
        // AIza + exactly 35 trailing chars (real format).
        let body = "a".repeat(35);
        let key = format!("AIza{body}");
        let out = redact(&format!("apiKey: '{key}'"));
        assert!(out.contains("<redacted>"), "got: {out}");
        assert!(!out.contains(&body));
    }

    #[test]
    fn redacts_huggingface_token() {
        let key = "hf_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // hf_ + 34
        let out = redact(&format!("login {key}"));
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("hf_aaaa"));
    }

    #[test]
    fn redacts_npm_publish_token() {
        let key = "npm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // npm_ + 36
        let out = redact(&format!("//registry.npmjs.org/:_authToken={key}"));
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("npm_aaaa"));
    }

    #[test]
    fn redacts_digitalocean_token() {
        // dop_v1_ + 64 lowercase hex chars.
        let hex = "0123456789abcdef".repeat(4);
        let key = format!("dop_v1_{hex}");
        let out = redact(&format!("doctl auth init -t {key}"));
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("dop_v1_0123"));
    }

    #[test]
    fn redacts_sentry_cli_token() {
        let key = "sntrys_eyJpYXQiOiAxNzAwAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let out = redact(&format!("export SENTRY_AUTH_TOKEN={key}"));
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("sntrys_eyJpYXQ"));
    }

    #[test]
    fn redacts_github_refresh_and_user_tokens() {
        // ghr_ (refresh) and ghu_ (user-to-server) — newly added.
        let r = redact("ghr_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(r.contains("<redacted>"));
        let u = redact("ghu_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(u.contains("<redacted>"));
    }

    #[test]
    fn short_sk_prefix_is_not_a_false_positive() {
        // `sk-foo` (too short) shouldn't trigger the OpenAI rule —
        // we require 40+ chars. Reduces false positives on legitimate
        // strings like `package:sk-bin/cmd`.
        let out = redact("alias sk-foo='./script'");
        assert!(!out.contains("<redacted>"), "false positive: {out}");
        // And the AIza pattern requires exactly 35 chars after the
        // prefix; a too-short string passes through.
        let out = redact("AIzaShort");
        assert!(!out.contains("<redacted>"));
    }

    #[test]
    fn paired_flag_redaction() {
        let r = redact_argv(&argv(&["mysql", "-u", "root", "--password", "s3cret123!"]));
        // --password value redacted (always-secret long flag)
        assert_eq!(r[r.len() - 1], "<redacted>");
        // -u user for mysql IS in SHORT_PAIRED_FLAG_SECRET_PROGS now, so `root` is
        // also redacted (defense-in-depth: -u <user> can hint at production-account names).
        assert_eq!(r[2], "<redacted>");
    }

    #[test]
    fn dash_p_not_redacted_for_cargo() {
        // Regression: `cargo test -p tokenade-core` MUST log the
        // package name. The previous unconditional `-p` redaction
        // turned every cargo test in gain.jsonl into
        // `cargo test -p <redacted>`, hurting log debugging.
        let r = redact_argv(&argv(&["cargo", "test", "-p", "tokenade-core"]));
        assert_eq!(
            r[3], "tokenade-core",
            "cargo package name must NOT be redacted"
        );
    }

    #[test]
    fn dash_p_not_redacted_for_pnpm_yarn_git() {
        for prog in ["pnpm", "yarn", "git"] {
            let r = redact_argv(&argv(&[prog, "build", "-p", "my-pkg"]));
            assert_eq!(
                r[3], "my-pkg",
                "{prog}: -p <project/branch> must NOT be redacted"
            );
        }
    }

    #[test]
    fn dash_p_redacted_when_sshpass_is_nested_in_wrap() {
        // Regression: bash cmd `tokenade wrap 'sshpass -p PWD
        // ssh ...'` after split_whitespace puts `tokenade` at argv[0]
        // and `sshpass` (potentially with surrounding quotes from the
        // shell literal) deeper in the stream. The argv-aware
        // redactor must flip its dangerous-prog state once it sees
        // `sshpass` anywhere — not just at argv[0]. Without this,
        // the VM password leaked to debug.log.
        let r = redact_argv(&argv(&[
            "/home/me/.local/bin/tokenade",
            "wrap",
            "'sshpass",
            "-p",
            "'\\''SuperSecret123'\\''",
            "ssh",
            "-T",
            "user@host",
        ]));
        // The 4th element (the password value) must be redacted.
        assert_eq!(
            r[4], "<redacted>",
            "sshpass -p VALUE inside `tokenade wrap '...'` must be redacted; got {r:?}"
        );
    }

    #[test]
    fn dash_p_redacted_when_sshpass_after_bash_dash_c() {
        // Same class: `bash -c "sshpass -p PWD ssh..."`. Argv[0] is
        // `bash`, sshpass appears later.
        let r = redact_argv(&argv(&[
            "bash", "-c", "sshpass", "-p", "pwd123", "ssh", "host",
        ]));
        assert_eq!(r[4], "<redacted>");
    }

    #[test]
    fn dash_p_still_redacted_for_mysql_redis() {
        // mysql -ppwd / redis-cli -p PASSWORD — still must redact.
        let r = redact_argv(&argv(&["mysql", "-p", "actual-password"]));
        assert_eq!(r[2], "<redacted>");
        let r = redact_argv(&argv(&["redis-cli", "-p", "shouldredact"]));
        assert_eq!(r[2], "<redacted>");
        // Path-prefixed: /usr/bin/mysql still detected.
        let r = redact_argv(&argv(&["/usr/local/bin/mysql", "-p", "secret"]));
        assert_eq!(r[2], "<redacted>");
    }

    #[test]
    fn redacts_cookie_request_header() {
        // Whole `Cookie:` header value is sensitive → fully redacted.
        let out = redact("Cookie: a=b; c=d; sessionid=SECRETVALUE");
        assert!(out.contains("Cookie: <redacted>"), "got: {out}");
        assert!(!out.contains("SECRETVALUE"), "cookie value leaked: {out}");
        assert!(!out.contains("a=b"), "cookie pairs leaked: {out}");
    }

    #[test]
    fn redacts_set_cookie_value_only_keeping_name_and_attrs() {
        // Only the VALUE is redacted; name + attributes stay visible.
        let out = redact("Set-Cookie: sid=SUPERSECRET; Path=/; HttpOnly; Secure");
        assert!(out.contains("Set-Cookie: sid=<redacted>"), "got: {out}");
        assert!(
            !out.contains("SUPERSECRET"),
            "set-cookie value leaked: {out}"
        );
        assert!(out.contains("Path=/"), "Path attr lost: {out}");
        assert!(out.contains("HttpOnly"), "HttpOnly attr lost: {out}");
        assert!(out.contains("Secure"), "Secure attr lost: {out}");
    }

    #[test]
    fn cookie_rule_does_not_swallow_set_cookie_attributes() {
        // The broad `Cookie:` rule must NOT match the `Cookie:` substring
        // inside `Set-Cookie:` (which would erase Path/HttpOnly).
        let out = redact("Set-Cookie: token=abc123; Domain=example.com; SameSite=Lax");
        assert!(!out.contains("abc123"), "value leaked: {out}");
        assert!(out.contains("Domain=example.com"), "Domain lost: {out}");
        assert!(out.contains("SameSite=Lax"), "SameSite lost: {out}");
        // Name preserved, not wholesale `Set-Cookie: <redacted>`.
        assert!(out.contains("token=<redacted>"), "name lost: {out}");
    }

    #[test]
    fn cookie_header_case_insensitive_and_curl_form() {
        let out = redact("curl -H 'cookie: auth=zzz'");
        assert!(!out.contains("auth=zzz"), "lowercase cookie leaked: {out}");
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn cookie_redaction_is_idempotent() {
        let c1 = redact("Cookie: a=b; c=d");
        assert_eq!(redact(&c1), c1, "Cookie re-redact changed: {c1}");
        let s1 = redact("Set-Cookie: sid=x; Path=/; HttpOnly");
        assert_eq!(redact(&s1), s1, "Set-Cookie re-redact changed: {s1}");
    }

    #[test]
    fn redact_scrubs_cookie_tsv_table_unconditionally() {
        // The always-run chokepoint must redact a raw DevTools cookie TSV
        // even when no compactor runs (small input → passthrough).
        let tsv = "Name\tValue\tDomain\tPath\tExpires\tHttpOnly\tSecure\tSameSite\n\
                   sessionid\tabc123deadbeefSECRET\t.ex.com\t/\t2027\ttrue\ttrue\tLax\n";
        let out = redact(tsv);
        assert!(
            !out.contains("abc123deadbeefSECRET"),
            "cookie value leaked: {out}"
        );
        assert!(out.contains("<redacted"), "no marker: {out}");
        assert!(out.contains(".ex.com"), "domain lost: {out}");
        assert!(out.contains("sessionid"), "name lost: {out}");
    }

    #[test]
    fn redact_scrubs_cookie_json_unconditionally() {
        let json = r#"[{"name":"sid","value":"SECRETJSON","domain":".ex.com","httpOnly":true}]"#;
        let out = redact(json);
        assert!(
            !out.contains("SECRETJSON"),
            "cookie json value leaked: {out}"
        );
        assert!(out.contains("<redacted"), "no marker: {out}");
        assert!(out.contains(".ex.com"), "domain lost: {out}");
    }

    #[test]
    fn redact_leaves_generic_metrics_table_intact() {
        // FALSE-POSITIVE GUARD: two-column Name/Value metrics table (no cookie
        // attribute) must pass through untouched.
        let out = redact("Name\tValue\nrequests\t4210\n");
        assert!(
            out.contains("4210"),
            "metrics value wrongly redacted: {out}"
        );
        assert!(!out.contains("<redacted"), "unexpected redaction: {out}");
    }

    #[test]
    fn redact_leaves_bare_name_value_json_intact() {
        let out = redact(r#"[{"name":"x","value":"keepme"}]"#);
        assert!(
            out.contains("keepme"),
            "generic json value wrongly redacted: {out}"
        );
        assert!(!out.contains("<redacted"), "unexpected redaction: {out}");
    }

    #[test]
    fn idempotent_on_already_redacted() {
        let once = redact("Bearer secrettoken");
        let twice = redact(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn passes_through_benign_text() {
        let bare = "git status";
        assert_eq!(redact(bare), bare);
    }

    #[test]
    fn handles_empty_and_unicode() {
        assert_eq!(redact(""), "");
        let unicode = "Vérification du token: ghp_abcdefghijklmnopqrstuvwxyz0123";
        let r = redact(unicode);
        assert!(r.starts_with("Vérification"));
        assert!(r.contains("<redacted>"));
    }

    // === A — additional provider prefixes ===
    #[test]
    fn a_provider_prefixes_detect() {
        for (label, key) in [
            ("sk-proj", "sk-proj-abcDEF123456789_ghijklmnop-qrstuvwx"),
            ("whsec", "whsec_abcDEF1234567890ghijKLMN"),
            ("shopify", "shpat_abcdef1234567890abcdef1234567890"),
            ("slack-app", "xapp-1-A012-345678-abcdefghij"),
            (
                "do-oauth",
                &format!("doo_v1_{}", "0123456789abcdef".repeat(4)),
            ),
            (
                "google-oauth",
                "ya29.a0AfB_abcdefghijklmnopqrstuvwxyz012345",
            ),
            ("google-refresh", "1//0abcdefghijklmnopqrstuvwxyz"),
            ("telegram", "123456789:AAabcdefghijklmnopqrstuvwxyz0123456"),
            ("newrelic", "NRAK-ABCDEFGHIJKLMNOPQRSTUVWXY01"),
            ("notion", &format!("secret_{}", "a".repeat(43))),
            ("linear", "lin_api_abcDEF1234567890ghijklmn"),
            ("doppler", "dp.pt.abcDEF1234567890ghijklmnop"),
            (
                "age",
                &format!("AGE-SECRET-KEY-1{}", "ABCDEF0123456789".repeat(2)),
            ),
            ("gitlab-deploy", "gldt-abcDEF1234567890ghijklmn"),
        ] {
            let out = redact(&format!("cred={key}"));
            assert!(out.contains("<redacted>"), "{label} not redacted: {out}");
            assert!(!out.contains(key), "{label} leaked: {out}");
        }
        // Slack incoming-webhook URL keeps the base, redacts the path.
        let wh = redact("url=https://hooks.slack.com/services/T000/B000/XXXXsecretXXXX");
        assert!(!wh.contains("XXXXsecretXXXX"), "slack webhook leaked: {wh}");
        assert!(wh.contains("hooks.slack.com/services/"), "base lost: {wh}");
    }

    #[test]
    fn a_prefixes_no_false_positive_on_short() {
        // A too-short lookalike must pass through.
        let out = redact("run shpat_x and lin_api_y and NRAK-short");
        assert!(!out.contains("<redacted>"), "false positive: {out}");
    }

    // === B — AWS secret access key by proximity ===
    #[test]
    fn b_aws_secret_by_proximity_detects_and_guards() {
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"; // 40 chars
                                                                 // Detection: keyed forms + CLI flag.
        for line in [
            format!("aws_secret_access_key = {secret}"),
            format!(r#""aws_secret_access_key": "{secret}""#),
            format!("aws --secret-access-key {secret}"),
        ] {
            let out = redact(&line);
            assert!(!out.contains(secret), "aws secret leaked: {out}");
            assert!(out.contains("<redacted>"), "no marker: {out}");
        }
        // FP guard: an isolated 40-char base64 blob (no aws_secret key) stays.
        let blob = format!("digest {secret} end");
        assert_eq!(
            redact(&blob),
            blob,
            "isolated 40-char blob wrongly redacted"
        );
    }

    // === C — connection-string DSN ===
    #[test]
    fn c_dsn_password_redacted_up_to_semicolon() {
        let out = redact("Server=db;Database=app;Password=Sup3rSecret;Trusted=false");
        assert!(!out.contains("Sup3rSecret"), "dsn pwd leaked: {out}");
        assert!(out.contains("Trusted=false"), "trailing key lost: {out}");
        assert!(out.contains("Database=app"), "leading key lost: {out}");
        let pwd = redact("odbc:Pwd=abc123;Server=x");
        assert!(!pwd.contains("abc123"), "Pwd= leaked: {pwd}");
        assert!(pwd.contains("Server=x"), "trailing lost: {pwd}");
    }

    // === D — registry auth files ===
    #[test]
    fn d_registry_auth_files() {
        let npm = redact("//registry.npmjs.org/:_authToken=npmSecretTokenValue123");
        assert!(
            !npm.contains("npmSecretTokenValue123"),
            "_authToken leaked: {npm}"
        );
        let npm_auth = redact("_auth=dXNlcjpwYXNzd29yZA==");
        assert!(
            !npm_auth.contains("dXNlcjpwYXNzd29yZA"),
            "_auth leaked: {npm_auth}"
        );
        let npm_pw = redact("//registry:_password=cGFzc3dvcmQ=");
        assert!(
            !npm_pw.contains("cGFzc3dvcmQ"),
            "_password leaked: {npm_pw}"
        );
        let netrc = redact("machine api.example.com login myuser password myNetrcPass");
        assert!(
            !netrc.contains("myNetrcPass"),
            "netrc password leaked: {netrc}"
        );
        assert!(!netrc.contains("myuser"), "netrc login leaked: {netrc}");
    }

    #[test]
    fn d_netrc_pattern_ignores_prose() {
        // "the password is wrong" must NOT fire (no machine/login anchor).
        let out = redact("Reminder: the password is stored in your vault.");
        assert!(!out.contains("<redacted>"), "prose password FP: {out}");
    }

    // === E — non-PEM private keys ===
    #[test]
    fn e_putty_and_openssh_private_keys() {
        let ppk = "PuTTY-User-Key-File-2: ssh-rsa\nEncryption: none\nComment: k\n\
                   Public-Lines: 1\nAAAApublic\nPrivate-Lines: 2\nSECRETLINE1\nSECRETLINE2\n\
                   Private-MAC: deadbeef\n";
        let out = redact(ppk);
        assert!(
            !out.contains("SECRETLINE1"),
            "ppk private lines leaked: {out}"
        );
        assert!(out.contains("Private-MAC"), "MAC line lost: {out}");
        // OpenSSH new-format is caught by the generic PEM rule.
        let ossh = "-----BEGIN OPENSSH PRIVATE KEY-----\nabcd\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact(ossh);
        assert!(
            out.contains("<redacted-private-key>"),
            "openssh not redacted: {out}"
        );
        assert!(!out.contains("abcd"), "openssh body leaked: {out}");
    }

    // === F — Kubernetes Secret ===
    #[test]
    fn f_k8s_secret_data_redacted_only_when_kind_secret() {
        let secret = "apiVersion: v1\nkind: Secret\nmetadata:\n  name: db\ndata:\n  \
                      password: c3VwZXJzZWNyZXR2YWx1ZTEyMw==\n";
        let out = redact(secret);
        assert!(
            !out.contains("c3VwZXJzZWNyZXR2YWx1ZTEyMw"),
            "k8s secret leaked: {out}"
        );
        assert!(out.contains("kind: Secret"), "kind lost: {out}");
        assert!(out.contains("password:"), "key lost: {out}");
        // FP guard: same base64 in a ConfigMap (no kind: Secret) survives.
        let cm = "apiVersion: v1\nkind: ConfigMap\ndata:\n  \
                  blob: c3VwZXJzZWNyZXR2YWx1ZTEyMw==\n";
        let out = redact(cm);
        assert!(
            out.contains("c3VwZXJzZWNyZXR2YWx1ZTEyMw"),
            "configmap wrongly redacted: {out}"
        );
    }

    // === G — GCP service-account JSON ===
    #[test]
    fn g_gcp_service_account_keys() {
        let json = r#"{"private_key_id":"abc123def456","client_secret":"GOCSPX-secretvalue","client_id":"12345.apps"}"#;
        let out = redact(json);
        assert!(
            !out.contains("abc123def456"),
            "private_key_id leaked: {out}"
        );
        assert!(
            !out.contains("GOCSPX-secretvalue"),
            "client_secret leaked: {out}"
        );
        assert!(
            out.contains("client_id"),
            "non-secret client_id lost: {out}"
        );
    }

    // === H — PAN + CVV ===
    #[test]
    fn h_pan_luhn_detect_and_fp_guard() {
        // Visa test number (valid Luhn).
        let out = redact("card 4111 1111 1111 1111 charged");
        assert!(!out.contains("4111 1111 1111 1111"), "PAN leaked: {out}");
        assert!(out.contains("1111"), "last4 lost: {out}");
        assert!(
            out.contains("****-****-****-1111"),
            "mask format wrong: {out}"
        );
        // Mastercard, no separators.
        let mc = redact("pan=5555555555554444");
        assert!(!mc.contains("5555555555554444"), "MC leaked: {mc}");
        // FP guard: a 16-digit non-Luhn ID must survive.
        let id = redact("order 1234567890123456 shipped");
        assert!(
            id.contains("1234567890123456"),
            "non-Luhn ID wrongly masked: {id}"
        );
        // FP guard, from a REAL mistake: a timestamp followed by two token
        // counts is 16 digits across single spaces, passes the IIN gate on 26
        // and happens to pass Luhn. It was served to an agent masked. Numbers
        // in ordinary output must come back as numbers.
        let tabular = redact("a11y 07-20 20:26 3179168 2954167 snapshot cli");
        assert!(
            tabular.contains("3179168") && tabular.contains("2954167"),
            "plain numeric output was masked as a card: {tabular}"
        );
        // …and the grouping guard must not cost us a card written normally.
        for real in [
            "4111 1111 1111 1111", // Visa, spaces
            "4111-1111-1111-1111", // Visa, dashes
            "3782 822463 10005",   // Amex 4-6-5
        ] {
            let out = redact(&format!("pay with {real} now"));
            assert!(!out.contains(real), "separated PAN leaked: {out}");
        }
        // Mixed separators are not how anyone writes a card, and accepting
        // them is what let arbitrary numeric text in.
        assert!(card_shaped("4111 1111 1111 1111"));
        assert!(!card_shaped("4111-1111 1111-1111"));
        assert!(!card_shaped("26 3179168 2954167"));
        // FP guard: a long timestamp/id run (20 digits) survives.
        let ts = redact("seq 12345678901234567890 end");
        assert!(
            ts.contains("12345678901234567890"),
            "20-digit run masked: {ts}"
        );
    }

    #[test]
    fn h_cvv_by_proximity() {
        let out = redact("CVV: 123 exp 12/26");
        assert!(!out.contains("123"), "cvv leaked: {out}");
        assert!(out.contains("CVV"), "cvv label lost: {out}");
        // Short input (<13 chars) must still redact CVV — the PAN-only length
        // pre-filter must not gate the CVV scan.
        let short = redact("cvv=789");
        assert!(!short.contains("789"), "short cvv leaked: {short}");
    }

    // NB: opt-out env vars (TOKENADE_NO_REDACT_{PAN,PII,ENTROPY}) are verified
    // via the release-binary smokes, not here — `std::env::set_var` is
    // process-global and would race the parallel detection tests above.

    // === I — email/phone (opt-in) + structured PII/PHI (default on) ===
    #[test]
    fn i_email_phone_opt_in_pass() {
        // Email + phone live behind the opt-in pass; exercise it directly
        // (env-independent, no test-race).
        let email = redact_optin_pii("from jane.doe@corp.example for review");
        assert!(!email.contains("jane.doe"), "email local leaked: {email}");
        assert!(
            email.contains("@corp.example"),
            "email domain lost: {email}"
        );
        let phone = redact_optin_pii("call +14155552671 now");
        assert!(!phone.contains("+14155552671"), "phone leaked: {phone}");
    }

    #[test]
    fn i_structured_iban_ssn_default_on() {
        // IBAN + SSN are now DEFAULT ON — they must be masked by `redact()`
        // with no env at all.
        let iban = redact("IBAN DE89370400440532013000 ok");
        assert!(
            !iban.contains("DE89370400440532013000"),
            "IBAN leaked: {iban}"
        );
        let ssn = redact("ssn 123-45-6789 filed");
        assert!(!ssn.contains("123-45-6789"), "SSN leaked: {ssn}");
        // A bad IBAN check digit must be KEPT (checksum is the gate).
        let bad = redact("IBAN DE00370400440532013000 nope");
        assert!(
            bad.contains("DE00370400440532013000"),
            "bad IBAN wrongly masked: {bad}"
        );
    }

    #[test]
    fn i_structured_checksum_and_context_gates() {
        // Valid ABA routing WITH context → masked.
        let aba = redact("routing number 021000021 for wire");
        assert!(!aba.contains("021000021"), "valid ABA leaked: {aba}");
        // Same digits, no context keyword → KEPT (avoid FP on random IDs).
        let aba_noctx = redact("order id 021000021 shipped");
        assert!(
            aba_noctx.contains("021000021"),
            "ABA masked without context: {aba_noctx}"
        );
        // Invalid-checksum routing WITH context → KEPT.
        let aba_bad = redact("routing number 021000022 for wire");
        assert!(
            aba_bad.contains("021000022"),
            "bad-checksum ABA masked: {aba_bad}"
        );
        // Valid NPI (Luhn) with context → masked.
        let npi = redact("provider NPI 1234567893 billed");
        assert!(!npi.contains("1234567893"), "valid NPI leaked: {npi}");
        // Random 9-digit with no context → KEPT.
        let rnd = redact("ticket 987654321 closed");
        assert!(rnd.contains("987654321"), "random digits masked: {rnd}");
    }

    #[test]
    fn i_ip_mac_are_opt_in() {
        // DEFAULT: IP + MAC are NOT masked (ubiquitous in normal output).
        let ip = redact("client 192.168.1.42 connected");
        assert!(ip.contains("192.168.1.42"), "IPv4 masked by default: {ip}");
        let mac = redact("device 00:1b:44:11:3a:b7 seen");
        assert!(
            mac.contains("00:1b:44:11:3a:b7"),
            "MAC masked by default: {mac}"
        );
        // OPT-IN pass masks them.
        let ip_on = redact_optin_pii("client 192.168.1.42 connected");
        assert!(
            !ip_on.contains("192.168.1.42"),
            "IPv4 not masked on opt-in: {ip_on}"
        );
        let mac_on = redact_optin_pii("device 00:1b:44:11:3a:b7 seen");
        assert!(
            !mac_on.contains("00:1b:44:11:3a:b7"),
            "MAC not masked on opt-in: {mac_on}"
        );
        // Invalid octet (>255) is never an IP.
        let notip = redact_optin_pii("version 300.1.2.3 build");
        assert!(notip.contains("300.1.2.3"), "non-IP masked: {notip}");
    }

    #[test]
    fn i_crypto_context_gated() {
        let hex64 = "0x1111111111111111111111111111111111111111111111111111111111111111";
        // ETH privkey WITH context → masked.
        let with = redact(&format!("private_key={hex64}"));
        assert!(!with.contains(hex64), "ctx crypto leaked: {with}");
        // Same 64-hex as a git blob (no keyword) → KEPT.
        let blob = redact(&format!("blob {hex64} tree"));
        assert!(
            blob.contains(hex64),
            "64-hex masked without context: {blob}"
        );
        // Bitcoin WIF is distinctive → masked even without context.
        let wif = redact("5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ");
        assert!(
            !wif.contains("5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"),
            "WIF leaked: {wif}"
        );
    }

    // PII is off by default: a plain email must pass through `redact()` intact.
    #[test]
    fn i_pii_off_by_default_keeps_email() {
        let out = redact("from jane.doe@corp.example for review");
        assert!(
            out.contains("jane.doe@corp.example"),
            "email wrongly masked by default: {out}"
        );
    }

    #[test]
    fn i_pii_diff_marker_guard() {
        // Diff-added numeric line `+14155552671` at col 0 must NOT be read as
        // a phone (no separator before `+`).
        let diff = redact_optin_pii("+14155552671\n");
        assert!(
            diff.contains("+14155552671"),
            "diff marker wrongly masked: {diff}"
        );
    }

    // === J — entropy sweep ===
    #[test]
    fn j_entropy_detects_high_entropy_value() {
        let secret = "Kd8sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x"; // 43 mixed chars
        let out = redact(&format!("credential = {secret}"));
        assert!(!out.contains(secret), "entropy secret leaked: {out}");
        assert!(out.contains("<redacted>"), "no marker: {out}");
    }

    /// A string that passes mod-97 is an IBAN only if its country exists and
    /// it has that country's length: a Docker overlay short id does not.
    #[test]
    fn a_mod97_string_of_no_iban_country_is_not_an_iban() {
        fn with_check(cc: &str, body: &str) -> String {
            let digits: String = format!("{body}{cc}00")
                .chars()
                .map(|c| c.to_digit(36).unwrap().to_string())
                .collect();
            let rem = digits
                .bytes()
                .fold(0u32, |r, d| (r * 10 + (d - b'0') as u32) % 97);
            format!("{cc}{:02}{body}", 98 - rem)
        }
        let id = with_check("AI", "QZ3XKRA4BJ5M7N8P9R2S4T");
        assert_eq!(id.len(), 26);
        let line = format!("overlay on /x type overlay (rw,lowerdir=/var/lib/docker/overlay2/l/{id}:/var/lib/docker/overlay2/l/Q2)");
        assert_eq!(redact(&line), line);
        // A real country, but not ITS length (Germany is 22): not an IBAN.
        let de = with_check("DE", "QZ3XKRA4BJ5M7N8P9R2S4T");
        assert_eq!(redact(&format!("x {de} y")), format!("x {de} y"));
        // Same body under a real country of that length: still masked.
        let tr = with_check("TR", "QZ3XKRA4BJ5M7N8P9R2S4T");
        assert!(
            redact(&format!("x {tr} y")).contains("<redacted>"),
            "a Turkish-shaped IBAN escaped"
        );
        assert!(redact("IBAN DE89370400440532013000").contains("<redacted>"));
    }

    /// Every row of `IBAN_LENGTHS`: a checksum-valid string of exactly that
    /// country's length is masked, one character longer is not.
    #[test]
    fn every_iban_country_masks_at_its_own_length_only() {
        fn with_check(cc: &str, body: &str) -> String {
            let digits: String = format!("{body}{cc}00")
                .chars()
                .map(|c| c.to_digit(36).unwrap().to_string())
                .collect();
            let rem = digits
                .bytes()
                .fold(0u32, |r, d| (r * 10 + (d - b'0') as u32) % 97);
            format!("{cc}{:02}{body}", 98 - rem)
        }
        for &(cc, len) in crate::redact::pii::IBAN_LENGTHS {
            let exact = with_check(cc, &"7".repeat(len - 4));
            assert!(
                redact(&format!("x {exact} y")).contains("<redacted>"),
                "{cc} at {len} escaped"
            );
            let longer = with_check(cc, &"7".repeat(len - 3));
            assert!(
                !redact(&format!("x {longer} y")).contains("<redacted>"),
                "{cc} at {} was masked",
                len + 1
            );
        }
    }

    #[test]
    fn the_shell_working_directory_is_not_a_password() {
        let dump = "HOME=/home/me\nPWD=/home/me/project\nSHELL=/bin/bash\n";
        assert_eq!(redact(dump), dump);
        assert_eq!(redact("PWD=C:\\Users\\me"), "PWD=C:\\Users\\me");
        assert_eq!(redact("PWD=~/src"), "PWD=~/src");
        // A password still masks: in a DSN, or when the value is no path.
        for leak in [
            "Server=db;Uid=sa;PWD=/s3cr3t-Pa55;",
            "PWD=hunter2hunter2",
            "Pwd=/home/x",
        ] {
            assert!(redact(leak).contains("<redacted>"), "{leak} was not masked");
        }
    }

    #[test]
    fn j_entropy_keeps_a_following_flag_and_a_jvm_option() {
        for line in [
            "chrome --type=crashpad-handler --annotation=channel= --annotation=lsb-release=Archlinux --annotation=plat=Linux",
            "java -XX:+UseG1GC -XX:G1HeapRegionSizeForLargeHeaps=33554432 -Dfile.encoding=UTF-8",
        ] {
            assert_eq!(redact(line), line);
        }
        // The value after a JVM option name is still examined.
        let secret = "Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7";
        let out = redact(&format!("java -XX:FlightRecorderOptions={secret}"));
        assert!(
            out.starts_with("java -XX:FlightRecorderOptions=") && !out.contains(secret),
            "{out}"
        );
        assert!(!redact(&format!("key={secret} --next")).contains(secret));
    }

    #[test]
    fn a_package_named_like_a_slack_prefix_is_not_a_token() {
        for line in [
            "xapp-symbolic-icons 1.1.0-1",
            "xoxb-legacy-bridge-plugin 0.4.2-1",
            "xoxe-refresh-documentation-site",
        ] {
            assert_eq!(redact(line), line);
        }
        // Real shapes still mask.
        let app = format!(
            "xapp-1-A0{}-{}-{}",
            "123ABCD",
            "1234567890123",
            "a1b2c3d4".repeat(8)
        );
        let bot = format!(
            "xoxb-{}-{}-{}",
            "1234567890", "0987654321", "AbCdEfGhIjKlMnOpQrStUvWx"
        );
        let refresh = format!("xoxe-1-{}", "My0xLTEtQTEyMzQ1Njc4OTAtMTIzNDU2Nzg5MA");
        for tok in [app, bot, refresh] {
            assert!(
                !redact(&format!("token {tok} end")).contains(&tok),
                "a Slack token escaped"
            );
        }
    }

    #[test]
    fn a_flag_valued_basic_is_not_an_auth_scheme() {
        let ps = "/opt/app/electron --type=zygote --password-store=basic --log-file=/home/me/.config/App/logs/app_log.txt";
        assert_eq!(redact(ps), ps);
        let next_flag = "curl --auth basic --header-file=/home/me/.config/headers_for_api.txt";
        assert_eq!(redact(next_flag), next_flag);
        // A real bare scheme still masks.
        let cred = "Authorization header was Basic dXNlcjpwYXNzd29yZDEyMzQ1Njc4OQ==";
        assert!(
            redact(cred).contains("Basic <redacted>"),
            "{}",
            redact(cred)
        );
    }

    #[test]
    fn j_entropy_keeps_a_unit_name_built_on_a_hash() {
        let h = "4f1c2a9e8b7d6c5f4e3d2c1b0a9f8e7d6c5b4a39281706f5e4d3c2b1a0f9e8d7";
        for line in [
            format!("Oct 03 09:12:01 ai systemd[1]: docker-{h}.scope: Deactivated successfully."),
            format!("Oct 03 09:12:01 ai systemd[1]: var-lib-docker-overlay2-{h}-merged.mount: Deactivated successfully."),
        ] {
            assert_eq!(redact(&line), line);
        }
        // Mixed case, or a segment that is neither a word nor hex: masked.
        for token in [
            "xoxb-1234567890-0987654321-AbCdEfGhIjKlMnOpQrStUvWx",
            "abc-a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7",
            "correct3-horse4-battery5-staple6-zebra7",
        ] {
            assert!(
                !redact(&format!("value: {token}")).contains(token),
                "{token} escaped"
            );
        }
    }

    #[test]
    fn j_entropy_keeps_a_cargo_build_directive() {
        for line in [
            "cargo:rustc-env=TOKENADE_BUILD_FINGERPRINT=3f9a2b7c1d8e4f6a0b5c9d2e7f1a3b8c",
            "cargo:warning=libgit2_sys_vendored_build_requested_by_features",
            "cargo:rerun-if-env-changed=TOKENADE_EMBEDDED_MODEL_DIRECTORY_OVERRIDE",
        ] {
            assert_eq!(redact(line), line);
        }
        // The value after the directive is still examined.
        let secret = "Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7";
        let out = redact(&format!("cargo:rustc-env=API_SECRET_VALUE={secret}"));
        assert!(
            out.starts_with("cargo:rustc-env=API_SECRET_VALUE=") && !out.contains(secret),
            "{out}"
        );
        // A value with no `KEY=` of its own is examined as the directive's.
        assert!(!redact(&format!("cargo:rustc-link-arg={secret}")).contains(secret));
        // An unknown directive is not spared.
        assert!(!redact(&format!("cargo:{secret}")).contains(secret));
    }

    #[test]
    fn j_entropy_keeps_a_bare_host_port_path() {
        for line in [
            "postgres on localhost:5432/mydatabase_production_replica_01",
            "listening on db.internal.example:8080/api/v1/internal_health_check",
        ] {
            assert_eq!(redact(line), line);
        }
        // Not glued to a host: a credential shape still masks.
        let secret = "4000/Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7";
        assert!(!redact(&format!("value: {secret}")).contains(secret));
        assert!(!redact(&format!("1234:{secret}")).contains(secret));
        assert!(!redact(&format!("port:{secret}")).contains(secret));
    }

    #[test]
    fn j_entropy_keeps_the_last_segment_of_a_colon_namespace() {
        for id in [
            "urn:li:fsd_profile:ACoAABcdEfGhIjKlMnOpQrStUvWxYz0123456789",
            "\"entityUrn\": \"urn:li:fsd_profile:ACoAABcdEfGhIjKlMnOpQrStUvWxYz0123456789\"",
            "arn:aws:secretsmanager:us-east-1:123456789012:secret:ProdDbCredentials-AbCdEf0123456789",
        ] {
            assert_eq!(redact(id), id);
        }
        // One colon is an assignment: the same value still masks.
        let secret = "Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7";
        assert!(!redact(&format!("value: {secret}")).contains(secret));
        assert!(!redact(&format!("\"value\":\"{secret}\"")).contains(secret));
        assert!(!redact(&format!("https://h.example/x?v:{secret}")).contains(secret));
        // A keyword with a space or an `=` still introduces a credential.
        assert!(!redact(&format!("app:secret: {secret}")).contains(secret));
        assert!(!redact(&format!("secret:{secret}")).contains(secret));
    }

    #[test]
    fn j_entropy_keeps_a_value_its_key_calls_a_name() {
        // PowerShell list view: the key says the value is a name.
        for name in [
            "Microsoft-Windows-Subsystem-Linux",
            "IIS-ClientCertificateMappingAuthentication",
            "MicrosoftWindowsPowerShellV2Root",
        ] {
            let line = redact(&format!("FeatureName : {name}\nState       : Disabled"));
            assert!(line.contains(name), "a feature name was masked: {line}");
        }
        assert!(
            redact("DisplayName=Windows-Defender-ApplicationGuard-Service")
                .contains("ApplicationGuard")
        );
        // A key that is not a name keeps the mask on the same shape.
        let secret = "Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7";
        assert!(!redact(&format!("Value : {secret}")).contains(secret));
        assert!(!redact(&format!("FeatureNames2 : {secret}")).contains(secret));
    }

    #[test]
    fn j_entropy_keeps_a_qualified_identifier_path() {
        // `::` is a path separator, not an assignment. The failing-test name
        // is the ONE actionable token in a `cargo test` run, and it was being
        // masked: 35 chars, three character classes (`_` is a symbol, `a11y`
        // supplies digits), entropy above the floor.
        let line = redact("test web::tests::a11y_preserves_refs_for_interaction ... FAILED");
        assert!(
            line.contains("a11y_preserves_refs_for_interaction"),
            "test name masked — a failure becomes unidentifiable: {line}"
        );
        // Rust paths in a panic location survive too.
        let panic = redact("thread 'web::tests::a11y_preserves_refs_for_interaction' panicked");
        assert!(
            panic.contains("a11y_preserves_refs_for_interaction"),
            "{panic}"
        );
        // …and NO secret coverage is lost: one colon still redacts.
        let secret = "Kd8sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x";
        let hdr = redact(&format!("X-Api-Key: {secret}"));
        assert!(!hdr.contains(secret), "single-colon secret leaked: {hdr}");
    }

    #[test]
    fn j_entropy_keeps_an_absolute_path_after_a_bare_connector() {
        // Found live, by the per-rule breakdown this ledger just gained: a
        // shell error naming a missing file came back with the FILE NAME
        // replaced by `<redacted>`, which is the entire content of that
        // message. `/` is in the candidate class (base64 needs it), and a long
        // dotless path clears length, class-count and entropy on its own.
        let raw = "sh: line 12: /tmp/claude-1000/-home-username--studio-local-projects-tokenade/ce341c20-7962-4a3d-a758-19245e8468f6/scratchpad/rl: No such file";
        let line = redact(raw);
        assert_eq!(
            line.len(),
            raw.len(),
            "the path IS the message — masking it empties the line: {line}"
        );

        // A secret KEYWORD still wins over the path reading: there the key
        // says what the value is, and a base64 blob may open with `/`.
        let secret = "/Ab3sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x/xy";
        let kw = redact(&format!("aws_secret_access_key: {secret}"));
        assert!(
            !kw.contains("Ab3sB2mZ"),
            "keyword-introduced secret leaked: {kw}"
        );

        // And a non-path high-entropy value after the SAME bare connector is
        // still masked — the exemption is the leading `/`, not the connector.
        let blob = "Kd8sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x";
        let bare = redact(&format!("X-Thing: {blob}"));
        assert!(
            !bare.contains("Kd8sB2mZ"),
            "bare-connector secret leaked: {bare}"
        );
    }

    #[test]
    fn j_entropy_keeps_git_sha_uuid_and_lockfile_hash() {
        // git log --oneline SHAs (short + full) must survive.
        let gitlog = redact("513b66b5 build: drop llama.cpp from default builds");
        assert!(gitlog.contains("513b66b5"), "short SHA masked: {gitlog}");
        let full = redact("commit: 513b66b5aa11223344556677889900aabbccddee");
        assert!(
            full.contains("513b66b5aa11223344556677889900aabbccddee"),
            "full SHA masked: {full}"
        );
        // UUID survives.
        let uuid = redact("id: 550e8400-e29b-41d4-a716-446655440000");
        assert!(
            uuid.contains("550e8400-e29b-41d4-a716-446655440000"),
            "UUID masked: {uuid}"
        );
        // npm lockfile SRI hash survives.
        let sri = redact(
            "integrity: sha512-abcDEF123456789ghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOP==",
        );
        assert!(sri.contains("sha512-"), "SRI hash masked: {sri}");
        // data: image base64 survives.
        let img = redact("src: data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAADIAAAAyCAYAAAA");
        assert!(img.contains("iVBORw0KGgo"), "data-uri masked: {img}");
    }

    #[test]
    fn new_passes_are_idempotent() {
        for input in [
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "Server=db;Password=Sup3rSecret;Trusted=false",
            "card 4111 1111 1111 1111",
            "from jane.doe@corp.example",
            "credential = Kd8sB2mZ9qXfL4pR7wYc1nJ6tV0hA3eU5oM2iG8bN4x",
            "apiVersion: v1\nkind: Secret\ndata:\n  p: c3VwZXJzZWNyZXR2YWx1ZTEyMw==\n",
        ] {
            let once = redact(input);
            let twice = redact(&once);
            assert_eq!(once, twice, "not idempotent for {input:?}");
        }
    }

    // =======================================================================
    // ADVERSARIAL — PO principle: by default, redaction must NEVER degrade the
    // normal output of a command. Two corpora:
    //   (A) anti-false-positive: real command output → `redact(x) == x`
    //   (B) detection: real secrets/PII still masked; email/IP opt-in only.
    // =======================================================================

    /// Assert `redact` leaves a real, secret-free command output byte-for-byte
    /// intact under the DEFAULT config (no env vars).
    fn assert_no_default_redaction(label: &str, sample: &str) {
        let out = redact(sample);
        assert_eq!(
            out, sample,
            "{label}: default redaction altered clean output\n--- got:\n{out}"
        );
        assert!(
            !out.contains("<redacted>"),
            "{label}: unexpected <redacted> marker"
        );
    }

    #[test]
    fn adversarial_a_real_command_output_untouched() {
        // git log --oneline — short + full SHAs, author email (email is opt-in).
        assert_no_default_redaction(
            "git log",
            "513b66b5 fix: bug\n87ada9bf docs: update readme\nec0e0bd8 feat: add reporter\ncommit 513b66b5aa11223344556677889900aabbccddee\nAuthor: Jane Doe <jane.doe@corp.com>",
        );
        // ls -la — permissions, sizes, dates.
        assert_no_default_redaction(
            "ls -la",
            "drwxr-xr-x  2 user group  4096 Jul  9 10:30 src\n-rw-r--r--  1 user group 12345678 Jul  9 10:31 Cargo.lock",
        );
        // ps aux / docker ps — PIDs, container hex id, IP:port, published ports.
        assert_no_default_redaction(
            "ps/docker",
            "USER  PID %CPU  COMMAND\nroot 1234 0.0 /usr/bin/tokenade\nCONTAINER ID  IMAGE          PORTS\na1b2c3d4e5f6  nginx:latest   0.0.0.0:8080->80/tcp",
        );
        // netstat -tlnp — IPv4/IPv6 addresses + ports (IP now opt-in → kept).
        assert_no_default_redaction(
            "netstat",
            "tcp  0 0 127.0.0.1:5432 0.0.0.0:* LISTEN 1234/postgres\ntcp6 0 0 ::1:6379 :::* LISTEN 5678/redis",
        );
        // package.json — versions, author email.
        assert_no_default_redaction(
            "package.json",
            r#"{"name":"app","version":"1.2.3","dependencies":{"serde":"^1.0.196"},"author":"Jane <jane@corp.com>"}"#,
        );
        // Cargo.lock — `checksum = "<64-hex>"` (2 char-classes → not a secret).
        assert_no_default_redaction(
            "Cargo.lock",
            "[[package]]\nname = \"serde\"\nversion = \"1.0.196\"\nchecksum = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"",
        );
        // npm ci / cargo build — deprecation, SRI integrity, 4-part version.
        assert_no_default_redaction(
            "npm ci",
            "added 142 packages in 3s\nnpm WARN deprecated foo@1.2.3\ncompiling libc v1.2.3.4\nintegrity sha512-abcDEF0123456789ghijklmnopqrstuvwxyzABCDEF0123456789ghijkl==",
        );
        // source file — version const, UUID, git blob 64-hex, data-URI base64.
        assert_no_default_redaction(
            "source file",
            "const VERSION: &str = \"1.2.3\";\nlet id = \"550e8400-e29b-41d4-a716-446655440000\";\nlet blob = \"4a5b6c7d8e9f00112233445566778899aabbccddeeff00112233445566778899\";\nlet img = \"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAA\";",
        );
        // normal API JSON — numeric ids, ISO timestamps, short base64 cursor/etag.
        assert_no_default_redaction(
            "api json",
            r#"{"id":123456,"user_id":789,"created_at":"2026-07-09T10:30:00Z","cursor":"eyJvZmZzZXQiOjEwfQ==","etag":"a1b2c3"}"#,
        );
    }

    #[test]
    fn adversarial_a_edge_cases_not_masked_by_default() {
        // 4-part version string — must NOT match IP any more (IP is opt-in).
        assert_no_default_redaction("4-part version", "built libc v1.2.3.4 ok");
        // SHA-256 hex (64) with no crypto keyword — not a private key.
        assert_no_default_redaction(
            "sha256",
            "digest 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 verified",
        );
        // UUID.
        assert_no_default_redaction("uuid", "trace 550e8400-e29b-41d4-a716-446655440000 done");
        // ULID (Crockford base32, 26 chars).
        assert_no_default_redaction("ulid", "event 01ARZ3NDEKTSV4RRFFQ69G5FAV logged");
        // SRI base64 hash.
        assert_no_default_redaction(
            "sri",
            "sha384-oqVuAfXRKap7fdgcCY5uykM6+R9GqQ8K/uxy9rx7HNQlGYl1kPzQho1wx4JwY8wC",
        );
        // JWT-LIKE but not a real token: a lone base64url segment (no 3-part
        // `x.y.z` structure) is not a credential → kept. (A FULL 3-segment JWT
        // `eyJ.eyJ.sig` IS masked by design — it is a bearer credential.)
        assert_no_default_redaction("jwt-like fragment", "header eyJhbGciOiJIUzI1NiJ9 decoded");
        // 9-digit id, not a valid NPI (no Luhn) and no context.
        assert_no_default_redaction("random 9-digit", "ticket 987654321 closed");
        // IBAN with an invalid check digit → kept (checksum is the gate).
        assert_no_default_redaction("bad iban", "iban DE00370400440532013000 rejected");
    }

    #[test]
    fn adversarial_b_real_secrets_still_masked_by_default() {
        let cases: &[(&str, &str)] = &[
            ("valid iban", "settle DE89370400440532013000 now"),
            ("ssn", "ssn 123-45-6789 on file"),
            ("aba+ctx", "ABA routing number 021000021"),
            ("npi+ctx", "provider NPI 1234567893 billed"),
            (
                "aws secret",
                "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            ),
            (
                "groq",
                "GROQ=gsk_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL",
            ),
            (
                "github pat",
                "GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            ),
            (
                "crypto+ctx",
                "private_key=0x1111111111111111111111111111111111111111111111111111111111111111",
            ),
        ];
        for (label, input) in cases {
            let out = redact(input);
            assert!(
                out.contains("<redacted>"),
                "{label}: not masked by default: {out}"
            );
        }
        // cookie DevTools TSV export — value column masked, name/domain kept.
        let cookie =
            redact("name\tvalue\tdomain\tpath\nsid\tAbc123SecretCookieValue987\t.corp.com\t/\n");
        assert!(
            !cookie.contains("Abc123SecretCookieValue987"),
            "cookie value leaked: {cookie}"
        );
    }

    #[test]
    fn adversarial_b_email_and_ip_masked_only_on_opt_in() {
        // DEFAULT keeps them (proven env-free via `redact`).
        let plain = redact("mail bob@corp.com from 10.1.2.3");
        assert!(
            plain.contains("bob@corp.com") && plain.contains("10.1.2.3"),
            "opt-in PII masked by default: {plain}"
        );
        // OPT-IN pass masks both (env-free via direct call, no test-race).
        let masked = redact_optin_pii("mail bob@corp.com from 10.1.2.3");
        assert!(
            !masked.contains("bob@corp.com"),
            "email not masked on opt-in: {masked}"
        );
        assert!(
            !masked.contains("10.1.2.3"),
            "IP not masked on opt-in: {masked}"
        );
    }
}
#[cfg(test)]
mod credential_shape_tests_2026_08_04 {
    use super::redact;

    /// Redis is the one credentialed DSN with NO username — its standard URL is
    /// `redis://:password@host`. Both the DSN rule and the generic userinfo rule
    /// required a non-empty user, so the password travelled in clear while
    /// postgres:// and mongodb+srv:// beside it were redacted.
    #[test]
    fn an_empty_username_still_means_a_password() {
        let out = redact("redis://:verysecretpassword@10.0.0.5:6379/0");
        assert!(
            !out.contains("verysecretpassword"),
            "password survived: {out}"
        );
        assert!(
            out.contains("redis://"),
            "scheme must stay grep-able: {out}"
        );
        // Same hole on the generic rule, for any scheme.
        let out = redact("https://:hunter2token99@internal.example.com/api");
        assert!(!out.contains("hunter2token99"), "password survived: {out}");
        // A credential-less URL is still untouched — the rule keys on `:`…`@`.
        let clean = "redis://cache.internal:6379/0";
        assert_eq!(redact(clean), clean, "no credential, no redaction");
    }

    /// Anchored on shape, so it cannot fire on prose or on ordinary identifiers.
    #[test]
    fn vendor_shapes_are_matched_narrowly() {
        let sg = "SG.Ab1Cd2Ef3Gh4Ij5Kl6Mn7O.Ex4mpleSendGr1dKeyNotRealEx4mpleSendGr1dKey";
        assert!(
            !redact(sg).contains("leSendGr1dKey"),
            "sendgrid key survived"
        );
        // Wrong widths are NOT a SendGrid key and must not be touched.
        let near = "SG.short.alsoshort";
        assert_eq!(redact(near), near, "narrow match: {near}");

        let sas = "https://a.blob.core.windows.net/c?sv=2021-06-08&sig=AbCdEf%2FGhIjKlMnOpQrStUvWxYz0123456789%3D";
        assert!(
            !redact(sas).contains("AbCdEf%2FGhIjKl"),
            "SAS signature survived"
        );
        // `sig` is a common word — a short value must stay.
        let prose = "let sig = compute(sig, key); // sig=abc";
        assert_eq!(redact(prose), prose, "must not eat ordinary `sig`: {prose}");
    }
}
#[cfg(test)]
mod keyword_prefilter_tests {
    use super::*;

    /// The prefilter is only sound while EVERY rule needs one of the literals.
    /// Checked against the compiled table itself, so a rule added without one
    /// fails here rather than silently escaping redaction.
    #[test]
    fn every_keyword_rule_has_a_literal() {
        for (re, _mask) in keyword_assignment_rules() {
            let pat = re.as_str().to_ascii_lowercase();
            assert!(
                KEYWORD_LITERALS.iter().any(|k| pat.contains(k)),
                "this rule can match without any prefilter literal, so the \
                 prefilter would skip it: {}",
                re.as_str()
            );
        }
    }

    /// Skipping must be invisible: identical output on both sides of the gate.
    #[test]
    fn the_prefilter_never_changes_the_answer() {
        // NOTE the spacing: `password = x` with spaces is deliberately NOT
        // masked (it reads as code — see the split rule above), so the fixtures
        // must use the shapes configuration actually writes. My first draft got
        // this wrong and the test caught it, which is the point of it.
        let hits = [
            "password=hunter2hunter2",
            "MYSQL_PWD=s3cr3tvalue123",
            "REDISCLI_AUTH=abcdef123456",
            "api_key='quoted-secret-value'",
        ];
        for h in hits {
            assert!(may_hold_keyword_assignment(h), "gate must open for: {h}");
            assert_ne!(redact_keyword_assignments(h), h, "should have masked: {h}");
        }
        // …and closed for text no rule could touch.
        for miss in [
            "echo hi",
            "ls -la /tmp",
            "cargo build --release",
            "git status",
        ] {
            assert!(
                !may_hold_keyword_assignment(miss),
                "gate must stay shut: {miss}"
            );
            assert_eq!(
                redact_keyword_assignments(miss),
                miss,
                "unchanged either way: {miss}"
            );
        }
    }
}
#[cfg(test)]
mod entropy_veto_tests {
    use super::redact;

    /// The entropy sweep claims any ≥32-char token after a `:`/`=`. Measured
    /// 2026-08-28 over 1 993 third-party source files, these are the shapes it
    /// was taking with no secret anywhere in sight.
    #[test]
    fn a_called_name_is_not_a_credential() {
        for (src, keep) in [
            (
                "unified_batch_id=_is_base64_encoded_unified_file_id(job.unified_object_id),",
                "_is_base64_encoded_unified_file_id",
            ),
            (
                "# google_text_moderation_obj = _ENTERPRISE_GoogleTextModerationHandler()",
                "_ENTERPRISE_GoogleTextModerationHandler",
            ),
        ] {
            let out = redact(src);
            assert!(
                out.contains(keep),
                "a call site was masked: {src:?} -> {out:?}"
            );
        }
    }

    /// A name mapped to itself is a feature flag, not a secret. One header
    /// config file supplied five of the twelve entropy hits in the sample.
    #[test]
    fn a_name_mapped_to_itself_is_not_a_credential() {
        for (src, keep) in [
            (
                "\"token-efficient-tools-2025-02-19\": \"token-efficient-tools-2025-02-19\",",
                "\"token-efficient-tools-2025-02-19\",",
            ),
            (
                "\"fine-grained-tool-streaming-2025-05-14\": \"fine-grained-tool-streaming-2025-05-14\",",
                "\"fine-grained-tool-streaming-2025-05-14\",",
            ),
        ] {
            let out = redact(src);
            assert!(
                out.contains(keep),
                "a self-mapped name was masked: {src:?} -> {out:?}"
            );
        }
    }

    /// And the sweep still does its job. Every veto above is a hole if this
    /// stops being true.
    #[test]
    fn entropy_still_masks_a_real_blob() {
        for src in [
            "api_token: Ab3dEf6hIj9lMn2pQr5tUv8xYz1aB2c3D4e5F6g7",
            "SECRET=dGhpcyBpcyBhIHZlcnkgbG9uZyBiYXNlNjQgYmxvYiBoZXJl",
            "credential = Ab3dEf6hIj9lMn2pQr5tUv8xYz1aB2c3D4e5",
        ] {
            let out = redact(src);
            assert!(
                out.contains("redacted"),
                "a real blob went through: {src:?} -> {out:?}"
            );
        }
    }

    /// PURE HEX is deliberately NOT masked, and the trade is pinned here so it
    /// stays a decision rather than an accident. `entropy_is_secret` refuses a
    /// value carrying fewer than three character classes, and its comment says
    /// why: a git SHA is exactly that shape, and git output is a primary
    /// compaction target. The cost is real — a 40-char hex credential goes
    /// through — and it is the cost that was chosen.
    #[test]
    fn pure_hex_is_a_deliberate_hole_not_an_oversight() {
        let out = redact("credential = 9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c");
        assert!(
            !out.contains("redacted"),
            "pure hex started being masked — check what it does to `git log` \
             before keeping this: {out:?}"
        );
    }
}
#[cfg(test)]
mod placeholder_veto_tests {
    use super::redact;

    /// A variable reference or a fill-me-in marker is what documentation and
    /// shell snippets are made of. Measured 2026-08-28 over 1 993 third-party
    /// source files: the provider rule family masked 183 values on code holding
    /// no secret, and these are its shapes.
    #[test]
    fn a_placeholder_is_not_a_credential() {
        for (src, keep) in [
            ("export APORIO_API_KEY=<your key>", "<your key>"),
            (
                "-H \"X-APORIA-API-KEY: $APORIO_API_KEY\"",
                "$APORIO_API_KEY",
            ),
            (
                "print(f\"    export OPENAI_API_KEY={master_key}\")",
                "{master_key}",
            ),
            ("api_key = $OPENAI_API_KEY", "$OPENAI_API_KEY"),
            ("api_key: {{ vault_openai_key }}", "{{ vault_openai_key }}"),
        ] {
            let out = redact(src);
            assert!(
                out.contains(keep),
                "a placeholder was masked: {src:?} -> {out:?}"
            );
        }
    }

    /// The veto lives in ONE predicate used by both passes. With it in the
    /// provider pass alone, `export OPENAI_API_KEY={master_key}` simply moved
    /// from one tally to the other and the agent still read `<redacted>` — the
    /// corpus said so: `providers` fell by 13 while `keyword` rose by 3.
    #[test]
    fn both_passes_share_the_veto() {
        // Reaches the KEYWORD family (`api_key<space>=<space>` with a quoted or
        // bare value), not the provider table.
        let out = redact("api_key = {master_key}");
        assert!(out.contains("{master_key}"), "{out:?}");
        // Reaches the PROVIDER table (an `export` of a named vendor variable).
        let out = redact("export OPENAI_API_KEY=$MASTER_KEY");
        assert!(out.contains("$MASTER_KEY"), "{out:?}");
    }

    /// And the pass still masks. The provider loop was rewritten from a
    /// template `replace_all` to a closure that expands the same template, so
    /// the kept prefix must still be kept and the value must still go.
    #[test]
    fn the_provider_pass_still_masks_and_still_keeps_its_prefix() {
        // The fixtures are SHORT on purpose. A 33-character token after an
        // `=` is masked by the entropy sweep whatever the provider table does,
        // so a longer one proves nothing about this pass — the mutation that
        // vetoes every provider match stayed GREEN against exactly that,
        // reading (a): redundant given the other passes. Both values below are
        // under the 32-character entropy floor and sit in no assignment shape,
        // so only the provider table can reach them.
        let out = redact("the deploy used AKIAIOSFODNN7EXAMPLE last week");
        assert!(out.contains("redacted"), "a real key went through: {out:?}");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "{out:?}");

        // And the replacement still keeps the prefix it is written to keep —
        // the loop was rewritten from a template `replace_all` to a closure
        // that expands the same template.
        let out = redact("export OPENAI_API_KEY=sk-proj-Ab3dEf6hIj9lMn2pQr5tUv8xYz1a");
        assert!(out.contains("redacted"), "{out:?}");
        assert!(
            out.contains("OPENAI_API_KEY"),
            "the replacement dropped the prefix it is supposed to keep: {out:?}"
        );
    }
}
#[cfg(test)]
mod xml_element_tests {
    use super::redact;

    fn secret() -> String {
        format!("{}{}", "AbCdEf0123456789", "ZyXwVu9876543210")
    }

    /// XML has no assignment: the name is a TAG and the value is the text
    /// between the tags, so every assignment rule in this module walked past
    /// a Maven `settings.xml` or a .NET `app.config`.
    #[test]
    fn a_credential_in_an_xml_element_is_masked() {
        let s = secret();
        for src in [
            format!("<apiKey>{s}</apiKey>"),
            format!("    <password>{s}</password>"),
            format!("<client-secret>{s}</client-secret>"),
            format!("<sec:accessKey>{s}</sec:accessKey>"),
        ] {
            let out = redact(&src);
            assert!(!out.contains(&s), "an XML credential leaked: {out}");
            assert!(
                out.contains("</"),
                "the closing tag went with the value: {out}"
            );
        }
    }

    /// The tag name may BE the keyword. Requiring a letter before it (the
    /// first shape of this rule) meant `<apiKey>` and `<password>` — the two
    /// commonest tags there are — never matched, while `<client-secret>` did.
    #[test]
    fn the_keyword_may_open_the_tag_name() {
        let s = secret();
        for src in [
            format!("<apiKey>{s}</apiKey>"),
            format!("<password>{s}</password>"),
        ] {
            assert!(!redact(&src).contains(&s), "leaked: {src}");
        }
    }

    /// The closing-tag gate. Without it a Java generic is a credential.
    #[test]
    fn a_generic_type_parameter_is_not_an_element() {
        for src in [
            "List<Token> tokens = new ArrayList<>();",
            "Map<String, AuthToken> m = new HashMap<>();",
            "the <token> element holds it",
        ] {
            assert_eq!(redact(src), src, "a generic was masked: {src:?}");
        }
    }

    /// A placeholder is not a credential — same predicate as the provider pass.
    #[test]
    fn an_xml_placeholder_is_left_alone() {
        for src in ["<password>${env.PW}</password>", "<Token>{value}</Token>"] {
            assert_eq!(redact(src), src, "a placeholder was masked: {src:?}");
        }
    }

    /// Masking twice changes nothing: the value class stops at `<`, and after
    /// one pass the value IS a `<`.
    #[test]
    fn the_pass_is_idempotent() {
        let once = redact(&format!("<password>{}</password>", secret()));
        assert_eq!(redact(&once), once);
    }
}
#[cfg(test)]
mod authorization_quote_tests {
    use super::redact;

    /// The leak this rule was inverted on: with the header written as a
    /// STRING, the scheme alternative could not match, the generic one ate
    /// `"Bearer` and the token itself was served in clear.
    #[test]
    fn a_quoted_header_masks_the_token_and_not_the_scheme() {
        let tok = format!("st_{}_{}", "test", "AbCdEf0123456789");
        for src in [
            format!("Authorization: \"Bearer {tok}\","),
            format!("authorization: 'Token {tok}'"),
            format!("headers: {{ Authorization: \"Bearer {tok}\" }},"),
        ] {
            let out = redact(&src);
            assert!(!out.contains(&tok), "the token survived: {out}");
            assert!(
                out.to_lowercase().contains("bearer") || out.contains("Token"),
                "the scheme was masked instead of the token: {out}"
            );
        }
    }

    /// The closing quote belongs to the source, not to the secret: the line
    /// the agent reads back must still be valid.
    #[test]
    fn the_quotes_survive_the_mask() {
        let src = format!("Authorization: \"Bearer {}\",", "AbCdEf0123456789xyz");
        let out = redact(&src);
        assert!(
            out.ends_with("\","),
            "the closing quote went with the value: {out}"
        );
        assert_eq!(out.matches('"').count(), 2, "quote count changed: {out}");
        // A quoted header carrying NO scheme word goes through the GENERIC
        // alternative, which needs its own opening quote — and needs it to
        // MASK AT ALL, not merely to keep the quotes: with the value class
        // stopping at a quote, an alternative that cannot consume the opening
        // one matches nothing and the credential is served in clear. Measured
        // by mutation: without it this line comes back verbatim.
        let bare_tok = "AbCdEf0123456789xyz";
        let out = redact(&format!("Authorization: \"{bare_tok}\""));
        assert!(
            !out.contains(bare_tok),
            "a scheme-less quoted header leaked: {out}"
        );
        assert_eq!(
            out.matches('"').count(),
            2,
            "a scheme-less quoted header lost its quotes: {out}"
        );
    }

    /// An interpolation is not a credential, and now that the rule captures
    /// the VALUE the placeholder veto can see it.
    #[test]
    fn an_interpolated_header_is_left_alone() {
        for src in [
            "Authorization: `Bearer ${apiKey}`,",
            "Authorization: \"Bearer {env:GITHUB_TOKEN}\",",
        ] {
            assert_eq!(redact(src), src, "an interpolation was masked: {src:?}");
        }
    }

    /// The unquoted form, and idempotence, both unchanged.
    #[test]
    fn the_bare_header_still_behaves() {
        let out = redact(&format!("Authorization: Bearer {}", "abc123def456ghi789"));
        assert_eq!(out, "Authorization: Bearer <redacted>");
        assert_eq!(
            redact("Authorization: <redacted>"),
            "Authorization: <redacted>"
        );
    }
}
#[cfg(test)]
mod quote_veto_tests {
    use super::{redact, unquoted};

    /// The quoted form is the MAJORITY form: JSON, Python and TypeScript all
    /// write it. Every veto read the quotes as part of the value, so none of
    /// them could match.
    #[test]
    fn a_quoted_value_takes_the_same_vetos_as_a_bare_one() {
        for src in [
            r#"{"api_key": "null", "model": "gpt-4o"}"#,
            r#"{"token": "{env:OPENCODE_CONSOLE_TOKEN}"}"#,
            r#"{"api_key": "self.api_key"}"#,
        ] {
            assert_eq!(redact(src), src, "a quoted non-secret was masked: {src:?}");
        }
    }

    /// The security half: quotes do not make a credential into a non-secret.
    #[test]
    fn a_quoted_credential_is_still_masked() {
        let key = format!("sk-{}-{}", "live", "AbCdEf0123456789AbCdEf0123");
        let src = format!(r#"{{"api_key": "{key}"}}"#);
        assert!(
            redact(&src).contains("redacted"),
            "a quoted credential went through: {src}"
        );
    }

    /// Only a MATCHING pair is a quote pair. An unbalanced quote is part of
    /// the value, and stripping it would change what the veto reads.
    #[test]
    fn only_a_matching_pair_is_stripped() {
        assert_eq!(unquoted("\"null\""), "null");
        assert_eq!(unquoted("'null',"), "null");
        assert_eq!(unquoted("\"null'"), "\"null'");
        assert_eq!(unquoted("null"), "null");
        assert_eq!(unquoted("\""), "\"");
    }
}
#[cfg(test)]
mod entropy_shape_veto_tests {
    use super::{redact, value_is_port_then_path};

    /// A documented `curl` example: the endpoint is the whole content of the
    /// line, and the bare `[:=]` alternative was matching the PORT colon.
    #[test]
    fn a_url_port_and_path_is_not_a_credential() {
        for src in [
            "curl -X GET 'http://localhost:4000/utils/supported_openai_params'",
            "curl -X POST \"http://localhost:4000/guardrails/test_custom_code\"",
        ] {
            assert_eq!(redact(src), src, "a URL path was masked: {src:?}");
        }
    }

    /// The two conditions that keep it narrow, each pinned on its own.
    #[test]
    fn the_port_veto_needs_a_scheme_and_a_bare_connector() {
        // No `://` anywhere: nothing says this colon is a port separator.
        let no_scheme = format!("port:4000/{}", "AbCdEfGhIjKlMnOpQrStUvWxYz012345");
        assert!(
            redact(&no_scheme).contains("redacted"),
            "masked nothing without a scheme: {no_scheme}"
        );
        // A secret KEYWORD introduced the value: the key says what it is.
        // `credential` is in the ENTROPY keyword list and in none of the
        // keyword-assignment rules, so this line reaches the entropy pass
        // with nothing else already masking it — an `api_key:` fixture here
        // stays green whatever this veto does, because the keyword pass has
        // already taken it.
        let keyed = format!(
            "http://h/ credential: 4000/{}",
            "AbCdEfGhIjKlMnOpQrStUvWxYz012345"
        );
        assert!(
            redact(&keyed).contains("redacted"),
            "a keyword-introduced value survived: {keyed}"
        );
        // And the predicate itself: a port is 1-5 digits then a slash.
        assert!(value_is_port_then_path("4000/utils/x"));
        assert!(!value_is_port_then_path("400000/utils/x"));
        assert!(!value_is_port_then_path("utils/4000"));
        assert!(!value_is_port_then_path("4000/a+b"));
    }

    /// A CamelCase type name clears the entropy floor: `_` is the third
    /// character class and the token is long enough to look random.
    #[test]
    fn an_identifier_after_a_bare_connector_is_not_a_credential() {
        for src in [
            "response_model=LiteLLM_ManagedVectorStoreListResponse,",
            "    vector_store: LiteLLM_ManagedVectorStoresTable",
        ] {
            assert_eq!(redact(src), src, "an identifier was masked: {src:?}");
        }
    }

    /// The gate: where a secret keyword introduces the value, the mask stays.
    #[test]
    fn a_keyword_connector_still_masks_an_identifier_shaped_value() {
        let src = "api_key: Some_Very_Long_Identifier_Shaped_Value_Here";
        assert!(
            redact(src).contains("redacted"),
            "a keyword-introduced identifier survived: {src}"
        );
    }

    /// The security half: neither veto may pass a real credential.
    #[test]
    fn a_real_credential_after_a_bare_connector_is_still_masked() {
        let blob = format!("{}{}", "aG9tZS1zZWNyZXQtdmFsdWU", "9zZWNyZXQtdmFsdWUx23");
        let src = format!("http://h/ x = {blob}");
        assert!(
            redact(&src).contains("redacted"),
            "a credential went through: {src}"
        );
    }
}
#[cfg(test)]
mod file_path_veto_tests {
    use super::{redact, value_is_file_path};

    /// The controlled pair. One digit, in a directory name, is the whole
    /// difference: `value_is_identifier_path` refuses any value carrying a
    /// digit, and `e2e` carries one.
    #[test]
    fn a_digit_in_a_directory_name_no_longer_masks_the_path() {
        let with = "read: crates/tokenade-cli/tests/e2e/wiring.rs";
        let without = "read: crates/tokenade-cli/tests/ee/wiring.rs";
        assert_eq!(redact(with), with, "the path with a digit was masked");
        assert_eq!(redact(without), without);
    }

    /// The shape this was found on: the harness's own notice. Masked, the
    /// agent is told a file changed and not which one.
    #[test]
    fn the_harness_notice_keeps_its_path() {
        let s = "[This command modified 1 file you've previously read: \
                 crates/tokenade-cli/tests/e2e/wiring.rs. Call Read before editing.]";
        assert_eq!(redact(s), s);
    }

    /// A credential is still a credential. The last one is the case the whole
    /// rule turns on: base64 CAN contain `/`, so the slash alone would have
    /// been a hole; it cannot contain `.`, so the extension closes it.
    #[test]
    fn credentials_after_the_same_connector_are_still_masked() {
        for s in [
            "token: xoxb-2914839023-2913847-QWERTYuiop1234567890abcd",
            "key: sk-proj-abcdefghijklmnopqrstuvwxyz0123456789ABCD",
            "aws: AKIAIOSFODNN7EXAMPLE/wJalrXUtnFEMI+K7MDENGbPxRfiCYEXAMPLEKEY",
            "auth: dGhpcy9pcy9hL2Jhc2U2NC9zZWNyZXQvdmFsdWUvaGVyZQ==",
        ] {
            assert_ne!(redact(s), s, "a credential went through: {s}");
        }
    }

    /// The extension lives OUTSIDE the captured value — the entropy pass's
    /// value class has no `.` — so the veto is handed the text that follows.
    #[test]
    fn the_extension_is_read_from_what_follows_the_value() {
        assert!(value_is_file_path(
            "a/b/c/deeply/nested/name",
            ".rs. Call Read"
        ));
        assert!(value_is_file_path("a/b/c/deeply/nested/name", ".fr.mdx"));
        // No extension after it: a directory path, not a file, and not vetoed.
        assert!(!value_is_file_path("a/b/c/deeply/nested/name", " and more"));
        // An extension that is really a longer word is not an extension.
        assert!(!value_is_file_path(
            "a/b/c/deeply/nested/name",
            ".alongsuffix"
        ));
        // No slash at all: never a path.
        assert!(!value_is_file_path("QWERTYuiop1234567890abcd", ".rs"));
        // A URL is handled by its own veto, not this one.
        assert!(!value_is_file_path("https://host/a/b", ".rs"));
    }

    /// A `cookie` module in a symbol path is not a Cookie HEADER, and the
    /// header rule eats everything after itself to end of line.
    #[test]
    fn a_cookie_module_in_a_path_keeps_the_rest_of_its_line() {
        let kept = redact("impl (5): FetchOpts, reqwest::cookie::CookieStore, FetchError\n");
        assert!(
            kept.contains("CookieStore") && kept.contains("FetchError"),
            "a `::cookie::` path took the line with it:\n{kept}"
        );
        // The header itself still loses its value, from every spelling that a
        // real one arrives in.
        for src in [
            "Cookie: session=abc; theme=dark",
            "> Cookie: session=abc",
            "cookie: session=abc",
            "  \"cookie\": \"session=abc\"",
        ] {
            let out = redact(&format!("{src}\n"));
            assert!(
                !out.contains("session=abc"),
                "a real cookie header leaked from {src:?}:\n{out}"
            );
        }
    }

    /// The JSON form is bounded by the closing quote: masking it must not take
    /// the fields after it, and must leave the document parseable.
    #[test]
    fn a_json_cookie_field_loses_only_its_own_value() {
        let out = redact("{\"cookie\":\"session=abc\",\"accept\":\"*/*\",\"n\":1}\n");
        assert!(!out.contains("session=abc"), "the cookie leaked:\n{out}");
        assert!(
            out.contains("\"accept\":\"*/*\"") && out.contains("\"n\":1"),
            "an end-of-line rule would have eaten the rest of the object:\n{out}"
        );
        assert!(
            out.trim().ends_with('}') && out.matches('"').count().is_multiple_of(2),
            "the document must still parse:\n{out}"
        );
    }

    /// A hyphen inside a path segment does not make the value a credential.
    #[test]
    fn the_dpkg_legend_survives_its_own_hyphens() {
        let legend = "Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend";
        let out = redact(&format!("| Status={legend}\n"));
        assert!(
            out.contains(legend),
            "the legend that explains every row below it was masked:\n{out}"
        );
        // The no-digit requirement is what keeps that safe: the same shape with
        // digits in it is still a credential as far as this pass is concerned.
        let keyish = "Ab3/Cd4/Ef5-Gh6/Ij7-Kl8/Mn9/Op0-Qr1/St2";
        let out2 = redact(&format!("| Status={keyish}\n"));
        assert!(
            !out2.contains(keyish),
            "a digit-bearing value must still be masked:\n{out2}"
        );
    }

    /// A canonical UUID after a bare connector is an identifier. After a
    /// Only the canonical shape. Anything that merely carries hex and hyphens
    /// A Hive-partitioned object key is a path, and every segment of it
    /// carries a `=`. base64 padding is the opposite shape and stays refused.
    #[test]
    fn a_partitioned_object_key_is_a_path_and_base64_padding_is_not() {
        assert!(value_is_file_path(
            "data/year=2026/month=08/day=15/part-00042-a1b2c3d4",
            ".snappy.parquet"
        ));
        // Every extension below is <= 6 bytes on purpose: `.parquet` is seven,
        // and the length check would refuse these lines before the `=` was
        // ever looked at, passing the test for the wrong reason.
        //
        // Trailing `=` is base64 padding, not a partition.
        assert!(!value_is_file_path("abc/defghijklmnopqrstuv==", ".gz"));
        // `=` adjacent to a separator is not `k=v` either — on EITHER side, so
        // both neighbours have to be tested.
        assert!(!value_is_file_path("abc/defghijklmn=/opqrstuv", ".gz"));
        assert!(!value_is_file_path("abc/defghijklmn=-opqrstuv", ".gz"));
        assert!(!value_is_file_path("abc/defghijklmn-=opqrstuv", ".gz"));
        // A leading `=` has no left neighbour at all. The index bound is what
        // keeps that from indexing below zero.
        assert!(!value_is_file_path("=abc/defghijklmnopqrstuv", ".gz"));
        // A SINGLE trailing `=` has no right neighbour, and it is the only
        // fixture that reaches the upper bound: with two of them the first
        // already answers, and `any` stops there.
        assert!(!value_is_file_path("abc/defghijklmnopqrstu=", ".gz"));
    }

    /// End to end, on the shape `aws s3 ls --recursive` actually prints: the
    /// key is the only thing the line says, and it used to be the only thing
    /// the line lost.
    #[test]
    fn a_recursive_object_listing_keeps_its_keys() {
        let key = "data/year=2026/month=08/day=15/part-00042-a1b2c3d4e5f6.snappy.parquet";
        let out = redact(&format!("2026-08-15 14:32:07    1234567 {key}\n"));
        assert!(out.contains(key), "object key was redacted: {out}");
    }
}
#[cfg(test)]
mod identifier_veto_tests {
    use super::{redact, value_is_identifier_path};

    /// The largest remaining class of masked lines on secret-free source: a
    /// variable being passed through, not a credential.
    #[test]
    fn an_identifier_being_passed_through_is_not_a_credential() {
        for (src, keep) in [
            ("api_key=self.api_key,", "self.api_key"),
            ("api_key=litellm_params.api_key,", "litellm_params.api_key"),
            (
                "api_key: os.environ/AZURE_AI_API_KEY",
                "os.environ/AZURE_AI_API_KEY",
            ),
            (
                "api_key=LITELLM_INTERNAL_JOBS_SERVICE_ACCOUNT_NAME,",
                "LITELLM_INTERNAL_JOBS_SERVICE_ACCOUNT_NAME",
            ),
        ] {
            let out = redact(src);
            assert!(
                out.contains(keep),
                "an identifier was masked: {src:?} -> {out:?}"
            );
        }
    }

    /// The two conditions that keep it narrow, each pinned on its own.
    #[test]
    fn a_separator_and_no_digit_are_both_required() {
        // No separator: a passphrase is still a passphrase.
        assert!(!value_is_identifier_path("correcthorsebatterystaple"));
        assert!(redact("password: correcthorsebatterystaple").contains("redacted"));
        // A digit anywhere: every prefixed vendor token, every hex, every
        // base64 blob keeps its digits and keeps its mask.
        assert!(!value_is_identifier_path("sk_live_51H8xYzAbCdEf"));
        assert!(!value_is_identifier_path("api_key_9f8e7d6c5b4a3928"));
        // A segment must open on a letter or `_`, so a dotted NUMBER is not an
        // identifier path.
        assert!(!value_is_identifier_path("10.0.0.1"));
        assert!(value_is_identifier_path("self.api_key"));
    }

    /// The predicate is deliberately NOT shared with the provider pass, and
    /// that limit was measured: applied there it unmasked `hf_…`, `npm_…` and a
    /// Sentry token, which carry `_` as a prefix separator and can be all
    /// letters. The provider table matches a KNOWN prefix; the keyword rules
    /// match a generic `api_key =`.
    #[test]
    fn the_provider_table_does_not_take_this_veto() {
        for src in [
            "hf_AbCdEfGhIjKlMnOpQrStUvWxYzAbCdEfGh",
            "npm_AbCdEfGhIjKlMnOpQrStUvWxYzAbCdEfGhIj",
        ] {
            // Non-vacuity: the predicate really would accept these.
            assert!(
                value_is_identifier_path(src),
                "{src:?} no longer has the shape this test is about"
            );
            assert!(
                redact(src).contains("redacted"),
                "a vendor token went through: {src:?}"
            );
        }
    }
}

#[cfg(test)]
mod aws_key_boundaries {
    use super::*;

    /// A Docker overlay layer id is the AWS key alphabet, 26 characters long;
    /// `ANVA` + 16 inside one was redacted and `mount` delivered a path that
    /// does not exist. The key is still redacted wherever it stands alone.
    #[test]
    fn a_key_shaped_run_inside_a_longer_id_is_not_a_key() {
        let layer = format!("MK{}ROL4", "ANVAUC73HU7VFAPK6GNS");
        // The key-shaped run can also END the id: only the left bound tells.
        let tail = format!("QZ7KD2{}", "ASIAPX4N2KQ7HWJ3BRTV");
        let line = format!(
            "lowerdir=/var/lib/docker/overlay2/l/{layer}:/var/lib/docker/overlay2/l/{tail}:/x"
        );
        assert_eq!(redact(&line), line);

        let key = format!("AKIA{}", "H4KLNPQRSTUV2WXY");
        for shown in [
            format!("AWS_ACCESS_KEY_ID={key}"),
            format!("\"{key}\""),
            key.clone(),
            format!("id: {key}\nnext"),
            format!("key_{key}_old"),
        ] {
            let out = redact(&shown);
            assert!(!out.contains(&key), "{shown:?} -> {out:?}");
            assert!(out.contains("<redacted>"), "{out:?}");
        }
    }

    /// A path or slug made of WORDS after a bare `:` is not a credential:
    /// a row of tokenade's own value ranking, `auto:aligned-block/S=/tmp/
    /// claude-1000/-home-username--studio-local-proje…`, was masked whole.
    #[test]
    fn j_entropy_keeps_a_path_made_of_words() {
        for line in [
            "- auto:aligned-block/S=/tmp/claude-1000/-home-username--studio-local-proje",
            "rows: docs/fixtures/web-corpus/2026-10-03/claude-1000-home-username",
        ] {
            assert_eq!(redact(line), line);
        }
        // Random tokens of the same alphabet still mask: mixed case, or
        // digits interleaved inside a segment.
        for secret in [
            "Ab3k-9QzP_x7Lm-Wq2r/Zt8yUv-K4nBc9",
            "a8f9k2m3/q7w1e5r9-t2y6u4i8-o0p3x5z1",
        ] {
            assert!(
                !redact(&format!("value: {secret}")).contains(secret),
                "{secret}"
            );
        }
    }

    /// A field that NAMES an identifier holds one: 935 of 3 163 masks in this
    /// machine's stash were `member_id`, `PathSegment`, `conversation_id`… —
    /// the very value the agent must quote back to act on the record.
    #[test]
    fn j_entropy_keeps_a_field_named_as_an_identifier() {
        let v = "ACoAAB8xYz12AbCdEfGhIjKlMnOpQrStUvWx";
        for key in [
            "member_id",
            "profilePathSegment",
            "conversationId",
            "thread-uuid",
        ] {
            let line = format!("{{\"{key}\":\"{v}\"}}");
            assert_eq!(redact(&line), line);
        }
        // A key that also says it guards access, or names no identifier.
        for key in ["session_id", "api_key_id", "csrfTokenId", "value"] {
            let line = format!("{{\"{key}\":\"{v}\"}}");
            assert!(!redact(&line).contains(v), "{key}");
        }
    }

    /// A value followed by an extension is a file name: a multi-file listing
    /// header came back as `== <redacted>.txt`.
    #[test]
    fn j_entropy_keeps_a_file_name() {
        for line in [
            "== BREATHE_NORTHWIND_dossier-presse_2026-09-07_v3-final.txt",
            "file=Very_Long_Report_Name_With_Words_And_Version_2026-09-10.pdf",
        ] {
            assert_eq!(redact(line), line);
        }
        // A JWT's next segment is no extension: still masked.
        let jwt_head = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let line = format!("value: {jwt_head}.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.sig");
        assert!(!redact(&line).contains(jwt_head), "{}", redact(&line));
        // Nor is the second half of an `<id>.<secret>` pair.
        let id = "Ab3kZ9QzPx7LmWq2rZt8yUvK4nBc9Xy1";
        let line = format!("value: {id}.Xy9Zq8Wp7Vo6Un5Tm4Rk3");
        assert!(!redact(&line).contains(id), "{}", redact(&line));
    }

    /// A value followed by `: ` is a key: `grep -n` over a list of named
    /// fields masked every field name after its line number.
    #[test]
    fn j_entropy_keeps_a_name_that_is_itself_a_key() {
        let line = "105:scored_tweets_model_weight_video_playback50: The probability (for a video)";
        assert_eq!(redact(line), line);
        // A base64 value followed by `: ` still masks.
        let b64 = "Ab3k/9QzPx7Lm+Wq2rZt8yUvK4nBc9Xy1Q==";
        assert!(!redact(&format!("12:{b64}: x")).contains(b64));
    }

    /// An RFC 2047 subject is quoted-printable TEXT, not a token.
    #[test]
    fn j_entropy_keeps_a_quoted_printable_subject() {
        let line = "Subject: =?utf-8?q?R=C3=A9union_budget_Q3_=E2=80=94_d=C3=A9cision?=";
        assert_eq!(redact(line), line);
        // Introduced by a secret keyword, the same shape still masks — and not
        // only by `password=`, which another layer masks on its own.
        let val = "Zq9=C3xY7=A9Lm2Pk8Wd4Rt6Vb1Nh5Jg3Fs0";
        for key in ["password=", "token: ", "auth=", "credential: "] {
            let out = redact(&format!("{key}{val}"));
            assert!(!out.contains(val), "{out}");
        }
    }
}
