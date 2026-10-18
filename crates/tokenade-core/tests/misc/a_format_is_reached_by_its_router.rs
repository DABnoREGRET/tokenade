//! **Is a format REACHED by the route that must reach it?**
//!
//! These assertions lived in the format modules themselves, under a
//! `#[cfg(test)] mod wiring`. They were not at fault there — an end-to-end
//! test is allowed to reach the router — but they address `filter` and
//! `auto`, which stay in `tokenade-core` when the formats move to
//! `tk-formats`: a test cannot name a crate that names its own.
//!
//! So they are here, whole, and each keeps the comment saying WHY the route
//! matters more than the function. It is the same lesson the extraction of
//! `tk-fold` already cost: the layer guard only reads the production half,
//! so TESTS had always gone upward freely, and the compiler does not make
//! that distinction.

mod pem_routes {
    /// The fold has to be REACHED, and by every command route.
    ///
    /// It lives in `filter::shrink_only_post_passes`, which runs after routing,
    /// so the route a transcript happens to be claimed by cannot change the
    /// answer. That mattered here: on a live `openssl s_client -showcerts` the
    /// dedicated `openssl` compactor and the generic fallback BOTH returned
    /// 0.0%, and the text was in fact claimed by `auto:command-chain` under
    /// `cat`. A guard on one route would have proved nothing about the others.
    fn handshake(certs: usize) -> String {
        let mut s = String::from("CONNECTED(00000003)\ndepth=2 CN = Test Root\n");
        for n in 0..certs {
            s.push_str(&format!("{n} s:CN = leaf{n}\n   i:CN = Test CA\n"));
            s.push_str("-----BEGIN CERTIFICATE-----\n");
            for k in 0..24 {
                s.push_str(&format!("{:A<64}\n", format!("c{n}l{k}")));
            }
            s.push_str("-----END CERTIFICATE-----\n");
        }
        s.push_str("subject=CN = leaf0\nissuer=CN = Test CA\nVerify return code: 0 (ok)\n");
        s
    }

    #[test]
    fn every_command_route_folds_the_bodies_and_keeps_the_fields() {
        let text = handshake(4);
        for prog in ["cat", "openssl", "curl", "kubectl"] {
            let argv = vec![prog.to_string(), "x".to_string()];
            let out = tokenade_core::filter::filter_command(&argv, &text).output;
            assert_eq!(
                out.matches("elided by tokenade").count(),
                4,
                "{prog}: not every block folded: {out}"
            );
            assert!(
                !out.contains("c2l7AAAA"),
                "{prog}: a body line survived: {out}"
            );
            for field in [
                "subject=CN = leaf0",
                "issuer=CN = Test CA",
                "Verify return code",
            ] {
                assert!(out.contains(field), "{prog}: lost {field:?}: {out}");
            }
            assert!(
                out.len() * 2 < text.len(),
                "{prog}: {} of {} is not a saving",
                out.len(),
                text.len()
            );
        }
    }

    /// A user who asked for the bytes still gets the bytes. The pass sits
    /// behind `explicit_raw_intent`, so a verbose flag takes it out of the way
    /// — `openssl s_client -showcerts -v` hands back the certificate whole.
    #[test]
    fn a_verbose_flag_hands_the_certificate_back_whole() {
        let text = handshake(1);
        let argv = vec![
            "openssl".to_string(),
            "s_client".to_string(),
            "-v".to_string(),
        ];
        assert!(
            tokenade_core::filter::explicit_raw_intent(&argv),
            "the premise of this test is gone: -v no longer reads as raw intent"
        );
        let out = tokenade_core::filter::filter_command(&argv, &text).output;
        assert!(
            out.contains("c0l7AAAA"),
            "a raw request lost the body anyway: {out}"
        );
        assert!(
            !out.contains("elided by tokenade"),
            "folded a raw request: {out}"
        );
    }

    /// A compactor can destroy what this pass needs before it ever sees it.
    ///
    /// The fold needs a block that OPENS and CLOSES; a routed answer that
    /// trimmed either delimiter leaves nothing to parse. MEASURED 2026-09-06 on
    /// a git-log-shaped transcript carrying an armored signature: the fold turns
    /// 1 115 bytes into 408, `auto:git-log` claims the text and delivers 986
    /// with the signature WHOLE. The pass therefore also runs on the raw text as
    /// a competing candidate.
    ///
    /// The COMMAND here is `cat`, which is how the measurement was taken and
    /// how such a transcript is usually met — a saved log, a paste, a file. Not
    /// `git log --show-signature`: that flag is in the `show-` family that
    /// `explicit_raw_intent` reads as a request for the bytes, so it skips
    /// every post-pass, and a test spelled that way would prove nothing about
    /// this one. (Checked while here: `git log --show-signature -20` on this
    /// repo is still compacted 89.8% by its own route, so the flag costs the
    /// post-passes and not the fold.)
    #[test]
    fn a_signature_survives_a_compactor_that_touched_it_first() {
        let body: Vec<String> = (0..14)
            .map(|n| format!("{:A<64}", format!("s{n}")))
            .collect();
        let text = format!(
            "commit 4f2a1b9c8d3e5f60718293a4b5c6d7e8f9012345\n\
             gpg: Good signature from \"Dev <d@x.test>\"\n\
             Author: Dev <d@x.test>\n\
             Date:   Sat Sep 6 14:02:11 2026 +0200\n\n    fix: the thing\n\n\
             -----BEGIN PGP SIGNATURE-----\n\n{}\n=Xy9z\n-----END PGP SIGNATURE-----\n",
            body.join("\n")
        );
        let argv = vec!["cat".to_string(), "signed.log".to_string()];
        let routed = tokenade_core::filter::filter_command(&argv, &text);
        let out = routed.output;
        assert!(
            routed.filter.contains("git-log"),
            "this fixture no longer reaches a compactor that touches the block, \
             so it cannot show the raw candidate doing anything: {}",
            routed.filter
        );
        assert!(
            out.contains("elided by tokenade"),
            "the signature was delivered whole: {out}"
        );
        assert!(!out.contains("s7AAAA"), "a signature line survived: {out}");
        assert!(
            out.contains("fix: the thing"),
            "the commit subject was lost: {out}"
        );
    }

    /// And the competing candidate must LOSE when routing did better. Folding
    /// the raw keeps every line the compactor dropped, so where a compactor
    /// earned its answer the folded raw is the big one and must not be chosen.
    ///
    /// Tested on `shrink_only_post_passes` DIRECTLY, and on text no other pass
    /// can rescue. Through `filter_command` with a git-log fixture the mutation
    /// "let the raw candidate always win" SURVIVES: `fold_repetitive_runs` runs
    /// after it, sees four hundred lines of one shape, and folds the oversized
    /// answer back down — so the guard is invisible from there. Distinct prose
    /// gives the repetition fold nothing to work with, and the guard becomes
    /// the only thing standing between the caller and the whole input.
    #[test]
    fn folding_the_raw_does_not_beat_a_compactor_that_earned_it() {
        // Distinct prose: nothing here repeats, so no later pass can shrink an
        // oversized answer and hide the guard being gone.
        let words = [
            "harbour", "kettle", "quarry", "lantern", "thistle", "morrow", "gable", "cinder",
            "plover", "wharf", "bramble", "sextant", "fathom", "willow",
        ];
        let mut text = String::new();
        for n in 0..300usize {
            let a = words[n % words.len()];
            let b = words[(n * 7 + 3) % words.len()];
            let c = words[(n * 13 + 5) % words.len()];
            text.push_str(&format!("{n} the {a} was {b} beside a {c} of {n}\n"));
        }
        let body: Vec<String> = (0..14)
            .map(|n| format!("{:A<64}", format!("s{n}")))
            .collect();
        text.push_str(&format!(
            "-----BEGIN PGP SIGNATURE-----\n\n{}\n-----END PGP SIGNATURE-----\n",
            body.join("\n")
        ));
        // A routed answer a compactor genuinely earned.
        let routed = "300 lines of notes, one signature\n";
        let argv = vec!["cat".to_string(), "notes.log".to_string()];
        let out = tokenade_core::filter::shrink_only_post_passes(&argv, &text, routed);
        let delivered = out.as_deref().unwrap_or(routed);
        assert!(
            delivered.len() <= routed.len(),
            "a post-pass delivered MORE than routing had already achieved: {} against {}",
            delivered.len(),
            routed.len()
        );
        // Anti-vacuity: the raw really does hold a foldable block, so the
        // candidate this guard rejects was a real one.
        assert_eq!(
            tokenade_core::pem::fold_pem_blocks(&text).1,
            1,
            "no block to reject"
        );
    }

    /// Anti-vacuity: output with no PEM in it must come back untouched by this
    /// pass, or "folds everything" would pass the test above.
    #[test]
    fn a_transcript_with_no_pem_is_not_touched_by_this_pass() {
        let text = "subject=CN = leaf0\nissuer=CN = Test CA\nVerify return code: 0 (ok)\n";
        let argv = vec!["openssl".to_string(), "x".to_string()];
        let out = tokenade_core::filter::filter_command(&argv, text).output;
        assert!(
            !out.contains("elided by tokenade"),
            "invented an elision: {out}"
        );
    }
}

mod ssh_keys_routes {
    /// Reached by every command route, like the PEM fold beside it.
    fn known_hosts(n: usize) -> String {
        (0..n)
            .map(|i| format!("host{i}.example.com ssh-ed25519 k{i}{}\n", "A".repeat(370)))
            .collect()
    }

    #[test]
    fn every_command_route_folds_the_keys_and_keeps_the_hosts() {
        let text = known_hosts(40);
        for prog in ["cat", "ssh-keyscan", "grep"] {
            let argv = vec![prog.to_string(), "known_hosts".to_string()];
            let out = tokenade_core::filter::filter_command(&argv, &text).output;
            assert!(
                out.contains("chars elided by tokenade"),
                "{prog}: no key was folded: {out}"
            );
            assert!(
                !out.contains("AAAAAAAAAAAAAAAA"),
                "{prog}: a key body survived: {out}"
            );
            assert!(
                out.contains("host0.example.com"),
                "{prog}: lost the first host"
            );
        }
    }

    #[test]
    fn a_transcript_with_no_keys_is_not_touched_by_this_pass() {
        let text = "Host github.com\n  User git\n  IdentityFile ~/.ssh/id_ed25519\n";
        let argv = vec!["cat".to_string(), "config".to_string()];
        let out = tokenade_core::filter::filter_command(&argv, text).output;
        assert!(
            !out.contains("elided by tokenade"),
            "invented an elision: {out}"
        );
    }
}

/// The router and the compactor must agree on what an environment dump is —
/// a shell script is not one.
#[test]
fn a_shell_script_is_not_routed_to_the_env_dump_compactor() {
    const SCRIPT: &str = "#!/usr/bin/env bash\nset -euo pipefail\nROOT_DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nLOG_FILE=/tmp/run.log\nMODE=fast\nAPI_SECRET=\"$(cat /run/secrets/api)\"\necho \"$ROOT_DIR\"\n";
    assert_ne!(
        format!("{:?}", tokenade_core::auto::detect(SCRIPT)),
        "EnvDump",
        "the router must agree with the compactor"
    );
}

/// And the reverse: a `.env` without a shebang is indeed routed there.
#[test]
fn a_dotenv_file_without_a_shebang_still_detects() {
    let dotenv =
        "DATABASE_URL=postgres://localhost/db\nREDIS_URL=redis://localhost\nNODE_ENV=production\n";
    assert_eq!(
        format!("{:?}", tokenade_core::auto::detect(dotenv)),
        "EnvDump"
    );
}

/// The BODY of a JUnit failure stays as the file spells it. Unescaping it
/// made `auto::guarded`'s error-preservation pass miss its own lines — it
/// compares by substring — so it re-attached them all and the answer carried
/// the same errors twice.
#[test]
fn the_junit_summary_does_not_come_back_with_its_errors_attached_twice() {
    let mut s = String::from("<testsuites tests=\"30\" failures=\"30\" errors=\"0\">\n<testsuite name=\"a.ts\" tests=\"30\" failures=\"30\" errors=\"0\">\n");
    for i in 0..30 {
        s.push_str(&format!(
            "<testcase classname=\"a.ts\" name=\"case {i}\">\n<failure message=\"Cannot read properties of undefined (reading &apos;0&apos;)\" type=\"TypeError\">\nTypeError: Cannot read properties of undefined (reading &apos;0&apos;)\n at run a.ts:{}:5\n</failure>\n</testcase>\n",
            10 + i
        ));
    }
    s.push_str("</testsuite>\n</testsuites>\n");
    let (_k, out, _sav) = tokenade_core::auto::compact(&s);
    assert!(
        !out.contains("errors preserved from elided lines"),
        "the error appendix duplicated the summary:\n{out}"
    );
    // The message is still readable, unescaped, where a person reads it.
    assert!(out.contains("(reading '0')"), "{out}");
}

/// The end-to-end consequence of NDJSON's "all counters at zero" floor: the
/// document comes back WHOLE. The fixture is the one of the test left in the
/// module — detected as NDJSON, parsed by no line.
#[test]
fn the_ndjson_all_zero_floor_hands_the_document_back_through_the_router() {
    let doc: String = (0..40)
        .map(|i| format!("{{ligne de prose ordinaire numero {i} sans guillemets}}\n"))
        .collect();
    let doc = doc.as_str();
    let (_kind, delivered, _s) = tokenade_core::auto::compact(doc);
    assert_eq!(delivered, doc, "the floor must hand the input back");
}

/// A PowerShell `Format-Table` table is recognised BY the router, not only by
/// its detector: that is the half missing when a format works and is never
/// reached.
#[test]
fn a_powershell_dash_table_is_reached_by_the_router() {
    use std::fmt::Write as _;
    let mut t = String::from(
        "Handles  NPM(K)    PM(K)      WS(K)   CPU(s)     Id  SI ProcessName\n\
         -------  ------    -----      -----   ------     --  -- -----------\n",
    );
    for i in 0..120 {
        writeln!(
            t,
            "    {:3}      12     45678      9012     1.23  {:5}   1 proc{}",
            100 + i,
            1000 + i,
            i
        )
        .unwrap();
    }
    assert_eq!(
        tokenade_core::auto::detect(&t),
        tokenade_core::auto::Kind::DashTable
    );
}
