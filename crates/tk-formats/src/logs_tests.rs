// The tests of `crate::logs`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `logs`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines differing only by a UUID must fold to ONE template — including
    /// the five in eight that open with a DIGIT.
    ///
    /// The uuid check sat AFTER the number pass, which claims a token's
    /// leading digits, so `550e8400-e29b-41d4-a716-446655440000` templated as
    /// `<N>e<N>-e29b-<N>d<N>-a<N>` and two uuids agreed only when their digits
    /// and letters happened to fall in the same places. MEASURED over 300 log
    /// lines differing only by a real uuid: **9.3% saved before, 97.9% after**.
    /// An all-letter uuid always worked, which is why a fixture built from one
    /// proves nothing here.
    #[test]
    fn lines_differing_only_by_a_uuid_fold_together() {
        let templates: std::collections::HashSet<String> = [
            "550e8400-e29b-41d4-a716-446655440000", // digit-leading
            "00000000-0000-0000-0000-000000000000", // all digits
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", // all letters
            "9f2c1ab5-7d34-4410-a8f0-b2c6d1e5f709",
            "1e6f0a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b",
            "F47AC10B-58CC-4372-A567-0E02B2C3D479", // upper case
        ]
        .iter()
        .map(|u| template_of(&format!("2026-09-07 12:00:00 INFO req {u} ok")))
        .collect();
        assert_eq!(
            templates.len(),
            1,
            "uuids did not fold to one template: {templates:?}"
        );
        let only = templates.into_iter().next().unwrap();
        assert!(only.contains("<UUID>"), "no uuid slot: {only}");
    }

    /// A hash is ONE slot whether it opens with a digit or a letter: a real
    /// `docker volume ls` read `57faf2…` as `<N><HEX>`.
    #[test]
    fn a_digit_leading_hash_is_one_hex_slot() {
        let templates: std::collections::HashSet<String> = ["57faf2d2def6", "da136e78c9ad", "0a5b9c1d2e3f4a5b", "105759f3070a"]
            .iter()
            .map(|h| template_of(&format!("run-docker-netns-{h}.mount: Deactivated successfully.")))
            .collect();
        assert_eq!(templates.len(), 1, "{templates:?}");
        // A plain number, a number with its unit, an id INSIDE a word: not hashes.
        for (line, want) in [("took 12345678 ms", "took <N> ms"), ("size 20260930kb", "size <N>kb")] {
            assert_eq!(template_of(line), want);
        }
        assert!(!template_of("build x12345678ab").contains("<HEX>"));
    }

    /// Narrow: these merely resemble one.
    #[test]
    fn what_is_not_a_uuid_is_not_slotted() {
        for not_uuid in [
            "x 550e8400-e29b-41d4-a716-44665544000 y",   // one digit short
            "x 550e8400-e29b-41d4-a716z446655440000 y",  // wrong separator
            "x 550e8400e29b41d4a716446655440000 y",      // no separators at all
            "x zzzzzzzz-e29b-41d4-a716-446655440000 y",  // not hex
        ] {
            let t = template_of(not_uuid);
            assert!(!t.contains("<UUID>"), "read as a uuid: {not_uuid} -> {t}");
        }
    }

    /// Lines differing only by a MAC address must fold to ONE template.
    ///
    /// A MAC is a declared format, and without a rule for it the surrounding
    /// passes tore it apart: the number pass ate each group's leading digits
    /// and left the hex LETTERS in the template, so `00:1a:2b:3c:4d:5e` became
    /// `<N>:<N>a:<N>b:<N>c:<N>d:<N>e` and no two addresses agreed — while an
    /// all-letter `aa:bb:cc:dd:ee:ff` matched nothing and was kept whole.
    /// MEASURED over 300 lines differing only by the address: **50.3% saved
    /// before, 98.0% after**, against ~98% for the same lines differing only
    /// by an integer, a uuid, a path or an IPv4.
    #[test]
    fn addresses_that_differ_only_by_a_mac_fold_together() {
        let one = template_of("nic 00:1a:2b:3c:4d:5e up");
        for addr in [
            "00:1a:2b:3c:4d:5f", // digit-leading, the case the number pass ate
            "aa:bb:cc:dd:ee:ff", // all letters, which matched nothing before
            "AA:BB:CC:DD:EE:FF", // upper case
            "00-1a-2b-3c-4d-5f", // dash-separated, as Windows writes them
            "ff:ff:ff:ff:ff:ff",
        ] {
            assert_eq!(
                template_of(&format!("nic {addr} up")),
                one,
                "{addr} did not fold to the same template"
            );
        }
        assert!(one.contains("<MAC>"), "no MAC slot in the template: {one}");
    }

    /// And the rule is narrow: everything here merely LOOKS like one.
    #[test]
    fn what_is_not_a_mac_is_not_slotted() {
        for not_mac in [
            "time 12:00:00 x",                        // three groups, a clock
            "t 12:00:00.123 x",                       // and with a fraction
            "g 00:1a:2b:3c:4d x",                     // five groups
            "g 00:1a:2b:3c:4d:5e:6f x",               // seven — not the first six
            "g 000:1a:2b:3c:4d:5e x",                 // groups of the wrong width
            // Not hex — but the FIRST character must still be hex, or the
            // branch is never entered and the arm proves nothing about the
            // rest of the positions. Measured: with `zz:…` the check could be
            // deleted and nothing failed.
            "g 0z:1a:2b:3c:4d:5e x",
            "g 00:1z:2b:3c:4d:5e x",
            "g 00:1a:2b:3c:4d:5z x",
            "g 00:1a-2b:3c:4d:5e x",                  // separators disagree
            "g 00.1a.2b.3c.4d.5e x",                  // a separator we do not take
            "u 550e8400-e29b-41d4-a716-446655440000 x", // a uuid keeps its own slot
        ] {
            let t = template_of(not_mac);
            assert!(!t.contains("<MAC>"), "read as a MAC: {not_mac} -> {t}");
        }
        // The uuid must still be recognised as a uuid, not merely "not a MAC".
        assert!(template_of("u 550e8400-e29b-41d4-a716-446655440000 x").contains("<UUID>"));
    }

    /// A multibyte character inside the timestamp window must not kill the
    /// process, and must not be read as a timestamp either.
    ///
    /// The guard checked five separators and the FIRST digit, then jumped
    /// `i += 19` and sliced — so a character like `é` in the seconds field
    /// left the index inside it. MEASURED on 1.1.9: a log holding
    /// `2026-09-07 12:00:0é0 INFO service=api …` killed `tokenade read`.
    /// Found by injecting a multibyte character at EVERY offset of ordinary
    /// log shapes, which is how a whole family of these is reachable.
    #[test]
    fn a_multibyte_char_in_a_timestamp_is_not_a_timestamp() {
        // Every position the old guard left unchecked, with a 2-, 3- and
        // 4-byte character each. Any one of them could land the jump inside
        // a character, so the arm is the whole set rather than one example.
        for pos in [0usize, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
            for mb in ["é", "界", "🚀"] {
                let stamp = "2026-09-07 12:00:00";
                let broken = format!("{}{mb}{} INFO service=api", &stamp[..pos], &stamp[pos + 1..]);
                let t = template_of(&broken);
                assert!(
                    !t.contains("<TS>"),
                    "pos={pos} mb={mb}: read as a timestamp: {t}"
                );
            }
        }
        // A non-digit that is plain ASCII is not a timestamp either — the
        // positions must be DIGITS, not merely one byte wide.
        assert!(!template_of("2026-09-07 12:00:0a INFO x").contains("<TS>"));

        // And the separators still have to be separators. Each of these is
        // wrong in exactly ONE position, so no single dropped check passes
        // them all.
        for broken in [
            "2026x09-07 12:00:00 INFO x",
            "2026-09x07 12:00:00 INFO x",
            "2026-09-07x12:00:00 INFO x",
            "2026-09-07 12x00:00 INFO x",
            "2026-09-07 12:00x00 INFO x",
        ] {
            assert!(
                !template_of(broken).contains("<TS>"),
                "separator not checked: {broken}"
            );
        }
    }

    /// And the guard did not stop recognising real ones.
    #[test]
    fn a_real_timestamp_is_still_folded() {
        for stamp in [
            "2026-09-07 12:00:00",
            "2026-09-07T12:00:00",
            "1999-01-31T23:59:59",
        ] {
            let t = template_of(&format!("{stamp} INFO service=api"));
            assert!(t.contains("<TS>"), "lost a real timestamp: {stamp} -> {t}");
        }
        // With a fraction and a zone, which the walker consumes after the 19.
        assert!(template_of("2026-09-07T12:00:00.123Z INFO x").contains("<TS>"));
    }

    use std::fmt::Write;

    #[test]
    fn path_token_end_matches_real_paths_only() {
        let end = |s: &str| path_token_end(s.as_bytes(), 0);
        // real paths with a dir + extension match.
        assert_eq!(end("src/foo.rs rest"), Some("src/foo.rs".len()));
        assert!(end("a/b/c/deep.tar.gz").is_some());
        // not paths: no slash, no extension, too short.
        assert!(end("hello world").is_none());
        assert!(end("a/b").is_none()); // too short + no .ext
        assert!(end("12-34-56").is_none()); // no slash
        assert!(end("/etc/").is_none()); // no .ext after last slash
    }

    #[test]
    fn curl_folds_tls_handshake() {
        let curl = "*   Trying 1.2.3.4:443...\n\
* Connected to api.example.com (1.2.3.4) port 443\n\
* ALPN: offers h2,http/1.1\n\
* TLSv1.3 (OUT), TLS handshake, Client hello (1):\n\
* TLSv1.3 (IN), TLS handshake, Server hello (2):\n\
* TLSv1.3 (IN), TLS handshake, Encrypted Extensions (8):\n\
* TLSv1.3 (IN), TLS handshake, Certificate (11):\n\
* SSL connection using TLSv1.3 / TLS_AES_256_GCM_SHA384\n\
* Server certificate:\n\
*  subject: CN=api.example.com\n\
*  start date: Jan 1 00:00:00 2024 GMT\n\
*  expire date: Jan 1 00:00:00 2025 GMT\n\
*  issuer: CN=Let's Encrypt\n\
> GET /api/v1/users HTTP/2\n\
> Host: api.example.com\n\
> user-agent: curl/8.4.0\n\
< HTTP/2 200\n\
< content-type: application/json\n\
{\"users\":[]}\n\
* Connection #0 to host api.example.com left intact\n";
        let out = compact_curl_verbose(curl);
        // Headers and body must survive.
        assert!(out.contains("GET /api/v1/users HTTP/2"));
        assert!(out.contains("HTTP/2 200"));
        assert!(out.contains("content-type: application/json"));
        assert!(out.contains("{\"users\":[]}"));
        // First Trying/Connected lines preserved.
        assert!(out.contains("*   Trying"));
        assert!(out.contains("* Connected to"));
        // TLS folded.
        assert!(out.contains("TLS-handshake lines elided"));
        // Cert chain folded.
        assert!(out.contains("cert-chain lines elided"));
        // Trailing connection-left-intact dropped.
        assert!(!out.contains("left intact"));
    }

    #[test]
    fn curl_savings_real_shape() {
        // 50 TLS lines + 10 cert lines + a small request/response.
        let mut t =
            String::from("*   Trying 1.2.3.4:443...\n* Connected to a.b (1.2.3.4) port 443\n");
        for i in 0..50 {
            let _ = writeln!(
                t,
                "* TLSv1.3 (IN), TLS handshake, Encrypted Extensions ({i}):"
            );
        }
        for _ in 0..10 {
            t.push_str("*  subjectAltName: DNS:a.b\n");
        }
        t.push_str("> GET / HTTP/2\n< HTTP/2 200\n< x: y\n{}\n");
        let (out, sav) = compact_curl_verbose_with_savings(&t);
        assert!(out.contains("HTTP/2 200"));
        assert!(sav.percent() > 60.0, "got {:?}", sav);
    }

    #[test]
    fn logs_fold_repeated() {
        let mut t = String::new();
        for i in 0..100 {
            let _ = writeln!(
                t,
                "2024-01-01T00:00:{:02}Z ERROR timeout connecting to db",
                i % 60
            );
        }
        let out = compact_generic_logs(&t);
        // Option-C header: count, distinct slots, sample.
        assert!(out.contains("[×100 lines"), "got: {out}");
        // The timestamp slot varies; the rest of the line is invariant
        // so the placeholder appears AND a distinct-count is shown.
        assert!(out.contains("<TS>"), "TS placeholder missing: {out}");
        assert!(out.contains("distinct TS=60"), "missing TS=60: {out}");
        assert!(out.contains("ERROR timeout connecting to db"));
        assert!(out.contains("e.g. TS="));
        // Single rendered fold line for the whole run.
        assert!(out.lines().count() == 1, "got\n{out}");
    }

    #[test]
    fn folded_run_inlines_constant_slot() {
        // 10 lines from the SAME IP — the N slot has only 1 distinct
        // value across the run, so it must be inlined (no `<N>`
        // placeholder, no N= in the distinct count) and the model can
        // tell "10.0.0.5 connected 10 times" rather than "10 IPs".
        let mut t = String::new();
        for i in 0..10 {
            let _ = writeln!(t, "2026-05-29T13:42:{:02}Z INFO conn from 10.0.0.5 ok", i);
        }
        let out = compact_generic_logs(&t);
        assert!(out.contains("from 10.0.0.5 ok"), "N must be inlined: {out}");
        assert!(
            !out.contains("from <N>"),
            "N placeholder must be gone: {out}"
        );
        assert!(out.contains("distinct TS=10"), "{out}");
        assert!(
            !out.contains("N="),
            "N=… annotation should be absent: {out}"
        );
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn folded_run_keeps_placeholder_for_varying_slot() {
        // 10 lines from 10 different IPs — N stays as <N>, distinct=10.
        let mut t = String::new();
        for i in 0..10 {
            let _ = writeln!(t, "2026-05-29T13:42:{:02}Z INFO conn from 10.0.0.{i} ok", i);
        }
        let out = compact_generic_logs(&t);
        assert!(
            out.contains("from <N> ok"),
            "N must stay a placeholder: {out}"
        );
        // 10 different ts + 10 different IPs.
        assert!(out.contains("distinct TS=10 (2026-05-29T13:42:00Z … 2026-05-29T13:42:09Z),N=10"), "{out}");
        assert!(out.contains("e.g. TS="));
        assert!(out.contains("N=10.0.0.0"), "sample value missing: {out}");
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn fully_identical_lines_collapse_with_dedicated_marker() {
        let t = "static line\nstatic line\nstatic line\n";
        let out = compact_generic_logs(t);
        assert!(out.contains("[×3 identical lines]"), "{out}");
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn logs_preserve_distinct_messages() {
        let t = "2024-01-01T00:00:00Z INFO start\n\
                 2024-01-01T00:00:01Z ERROR boom\n\
                 2024-01-01T00:00:02Z INFO stop\n";
        let out = compact_generic_logs(t);
        assert!(out.contains("start"));
        assert!(out.contains("boom"));
        assert!(out.contains("stop"));
        assert!(!out.contains('×')); // all distinct templates
    }

    #[test]
    fn no_panic_on_empty_or_garbage() {
        let _ = compact_curl_verbose("");
        let _ = compact_curl_verbose("\x00\u{ff} garbage \n binary");
        let _ = compact_generic_logs("");
    }

    /// A trailing `.` or `-` is NOT part of the number. `f300.txt` folded to
    /// `f<N>txt`: the dot disappeared into the slot, so an agent reconstructing
    /// the name from the template gets `f300txt` and can conclude the file it
    /// asked about does not exist. Same false-absence failure that lossy
    /// truncation causes, reached through the templater instead.
    ///
    /// The separators still belong to the number when a DIGIT follows —
    /// versions, dotted quads and dates must keep folding as one token.
    #[test]
    fn a_separator_joins_the_number_only_when_a_digit_follows_it() {
        let mut src = String::new();
        for i in 100..400 {
            src.push_str(&format!("checking f{i}.txt now\n"));
        }
        let out = compact_generic_logs(&src);
        assert!(
            out.contains("f<N>.txt") || out.contains("f<N1>.txt"),
            "the extension lost its dot — an agent cannot rebuild the name: {out}"
        );

        // …and the versions/dates/quads still collapse as ONE slot each.
        for (label, line) in [
            ("version", "pkg libfoo (1.0.7-1) installed"),
            ("date", "2026-08-25 event happened"),
            ("quad", "peer 192.168.0.1 connected"),
        ] {
            let mut many = String::new();
            for i in 0..300 {
                many.push_str(&line.replace("happened", &format!("happened {i}")));
                many.push('\n');
            }
            let folded = compact_generic_logs(&many);
            assert!(
                !folded.contains("<N>.<N>") && !folded.contains("<N1>.<N2>"),
                "{label}: the number was split into several slots: {folded}"
            );
        }
    }

    /// Two slots of the same KIND used to produce `distinct N=60000,N=97` —
    /// two counts and nothing saying which is which, on the one line the fold
    /// is read for. MEASURED on a real 60 000-line log.
    #[test]
    fn two_slots_of_one_kind_are_told_apart_in_the_line_and_in_the_counts() {
        // 2 000 lines: past the listing budget, so the SUMMARY is what speaks
        // (600 lines are now listed column by column, every value kept).
        let mut src = String::new();
        for i in 0..2000 {
            src.push_str(&format!(
                "2026-08-04 INFO worker={i} status=ok latency_ms={}\n",
                i % 97
            ));
        }
        let out = compact_generic_logs(&src);
        assert!(
            out.contains("worker=<N1>") && out.contains("latency_ms=<N2>"),
            "the template does not tell the two slots apart: {out}"
        );
        assert!(
            out.contains("N1=2000") && out.contains("N2=97"),
            "the counts are still ambiguous: {out}"
        );
        // The ordinals must map the SAME way in both halves, or the line lies.
        let tmpl_first = out.find("<N1>").unwrap();
        let tmpl_second = out.find("<N2>").unwrap();
        assert!(
            tmpl_first < tmpl_second,
            "ordinals are not left-to-right: {out}"
        );
    }

    /// Several slots varying together are listed column by column, in line
    /// order — `journalctl` clock `05:<N1>:<N2>`, four runs: before, one
    /// `e.g.` and three times gone. Two slots that always agree are one column.
    #[test]
    fn several_varying_slots_are_listed_column_by_column() {
        let mut src = String::new();
        for (m, s) in [(42, 55), (43, 3), (44, 55), (45, 7)] {
            src.push_str(&format!("oct. 03 05:{m}:{s:02} ai systemd[1]: Starting acme-cli: start the automations that are due...\n"));
        }
        for i in 0..12 {
            src.push_str(&format!("compiling module {i} into build/out{i}.o\n"));
        }
        let out = compact_generic_logs(&src);
        assert!(out.contains("[×4 lines · N1= 42…45 · N2= 55, 03, 55, 07]"), "{out}");
        assert!(out.contains("[×12 lines · N= 0…11]") || out.contains("[×12 lines · N1=N2= 0…11]"), "{out}");
    }

    /// Zero-padded numbers at one width are a range too: `000…199`.
    #[test]
    fn a_zero_padded_count_is_a_range() {
        let src: String = (0..200).map(|i| format!("  worker-{i:03}.service loaded active running Worker {i} daemon\n")).collect();
        let out = compact_generic_logs(&src);
        assert!(out.contains("N1= 000…199 · N2= 0…199"), "{out}");
    }

    /// Two indices that only TOGETHER name a line are a grid: counted, not
    /// listed.
    #[test]
    fn a_grid_of_two_indices_keeps_its_counts() {
        let mut src = String::new();
        for s in 0..30 {
            for t in 0..12 {
                src.push_str(&format!("ok suite_{s}::case_{t:02} verifies the widget contract\n"));
            }
        }
        let out = compact_generic_logs(&src);
        assert!(out.contains("distinct N1=30,N2=12"), "{out}");
    }

    /// A single varying slot keeps the shorter placeholder it has always
    /// printed — an ordinal there would be noise with nothing to disambiguate.
    #[test]
    fn a_lone_slot_keeps_its_bare_placeholder() {
        let mut src = String::new();
        for i in 0..300 {
            src.push_str(&format!("2026-08-04 INFO ip=10.0.0.{i} ok\n"));
        }
        let out = compact_generic_logs(&src);
        assert!(out.contains("ip=<N>"), "gained a needless ordinal: {out}");
        assert!(!out.contains("<N1>"), "gained a needless ordinal: {out}");
    }
}
#[cfg(test)]
mod scattered_repeats {
    use super::*;

    fn shapes() -> Vec<fn(usize) -> String> {
        vec![
            |i| format!("gcc -c -O2 -Iinclude -o build/u{i}.o src/u{i}.c"),
            |i| format!("  CC      src/u{i}.c"),
            |i| format!("ar rcs lib/u{i}.a build/u{i}.o"),
            |i| format!("[INFO] Building svc-{i} 2.0.0-SNAPSHOT"),
        ]
    }

    fn build(k: usize, interleaved: bool) -> String {
        let sh = shapes();
        let mut t = String::new();
        if interleaved {
            for i in 0..60 {
                for f in sh.iter().take(k) {
                    t.push_str(&f(i));
                    t.push('\n');
                }
            }
        } else {
            for f in sh.iter().take(k) {
                for i in 0..60 {
                    t.push_str(&f(i));
                    t.push('\n');
                }
            }
        }
        t
    }

    /// The run-based pass sees only ADJACENT repeats, and interleaving is the
    /// normal shape of build output. Measured 2026-08-26, same shapes and same
    /// 60 iterations: grouped folded to 0.03 at every k, interleaved folded to
    /// 0.036 at k=1 and **1.000 from k=2 onward** — not a weaker fold, none.
    #[test]
    fn interleaved_shapes_fold_like_grouped_ones() {
        for k in 2..=4 {
            let inter = build(k, true);
            let out = compact_generic_logs(&inter);
            // ×8, not ×10 (amended 2026-10-02): a run whose one value differs on
            // every line now LISTS its values — ` CC src/u<N>.c` carries its 60
            // unit numbers, 482 of 3 930 bytes at k=2. What this guards is
            // "interleaved folds at all" (it used to be 1.000), not the ratio.
            assert!(
                out.len() * 8 < inter.len(),
                "k={k} interleaved: {} of {} bytes",
                out.len(),
                inter.len()
            );
        }
    }

    /// …and the pass that already worked must keep winning. Regrouping costs
    /// the reader its chronology, so it is a candidate, not a replacement.
    #[test]
    fn a_log_the_run_pass_already_folds_is_left_in_order() {
        for k in 1..=4 {
            let grouped = build(k, false);
            let out = compact_generic_logs(&grouped);
            assert!(
                !out.contains("[tokenade:fold]"),
                "k={k}: regrouped a log the run pass had already folded"
            );
            assert!(out.len() * 10 < grouped.len(), "k={k}");
        }
    }

    /// The reordering is DECLARED. An agent has to be able to tell "these
    /// happened in this order" from "these were collected for you".
    #[test]
    fn gathering_lines_out_of_order_says_so() {
        let out = compact_generic_logs(&build(3, true));
        assert!(out.starts_with("[tokenade:fold] "), "{:.120}", out);
        assert!(out.contains("not as printed"), "{:.120}", out);
    }

    /// Prose repeats no shape. Regrouping it would reorder a document for
    /// nothing, so the distinct-template count has to stop it.
    #[test]
    fn prose_is_never_regrouped() {
        let words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta"];
        let mut t = String::new();
        for i in 0..300 {
            for j in 0..12 {
                t.push_str(words[(i * 7 + j * 3) % words.len()]);
                t.push(' ');
            }
            t.push('\n');
        }
        let out = compact_generic_logs(&t);
        assert!(!out.contains("[tokenade:fold]"), "prose was regrouped");
    }

    /// A short log is not worth reordering whatever it looks like.
    #[test]
    fn a_short_log_is_left_alone() {
        let t = "a 1\nb 2\na 3\nb 4\na 5\nb 6\n";
        assert!(!compact_generic_logs(t).contains("[tokenade:fold]"));
    }

    /// Regrouping must bring a diagnostic back TOGETHER, not scatter it. In
    /// interleaved output the run pass emitted an unrelated worker's line in
    /// the middle of a compiler error, because it preserved a position that
    /// only interleaving had produced.
    #[test]
    fn a_diagnostic_comes_back_contiguous() {
        let mut t = String::new();
        let diag = [
            "src/u9.c: In function 'run':",
            "src/u9.c:14:5: error: too few arguments to function 'go'",
            "   14 |     go();",
        ];
        for i in 0..60 {
            t.push_str(&format!("gcc -c -o build/u{i}.o src/u{i}.c\n"));
            t.push_str(&format!("  CC      src/u{i}.c\n"));
            if i == 30 {
                for d in diag {
                    t.push_str(d);
                    t.push('\n');
                }
            }
        }
        let out = compact_generic_logs(&t);
        let joined = diag.join("\n");
        assert!(
            out.contains(&joined),
            "the diagnostic was split apart:\n{out:.400}"
        );
    }
}
#[cfg(test)]
mod scattered_margin {
    use super::*;

    /// The margin is the ONE rule that decides whether the reader loses its
    /// chronology. Everything else in `regroup_scattered` is a cost early-out.
    ///
    /// This payload is exactly the awkward case: enough repetition that
    /// regrouping produces SOMETHING, not enough that it is worth reordering
    /// the output for. It must come back in the order it was printed.
    ///
    /// The fixture is chosen to land INSIDE the band the factor governs, not
    /// merely outside the fold: 18 structurally distinct shapes, three passes
    /// each, regroups to 1 663 bytes against the run pass's 2 348 — a ratio of
    /// 0.71. A one-times margin would take it; the two-times margin refuses.
    /// Probed 2026-08-26 across one to five passes, because a fixture that
    /// misses the band tests nothing: an earlier one sat at 1.00 and stayed
    /// green with the factor removed.
    #[test]
    fn a_regroup_that_barely_wins_is_refused() {
        let verbs = [
            "compiling",
            "linking",
            "resolving",
            "verifying",
            "packaging",
            "signing",
            "uploading",
            "indexing",
            "pruning",
            "vendoring",
            "hashing",
            "diffing",
            "restoring",
            "stamping",
            "shrinking",
            "auditing",
            "sealing",
            "probing",
        ];
        let mut t = String::new();
        for pass in 0..3 {
            for (k, v) in verbs.iter().enumerate() {
                t.push_str(&format!("{v} unit {k} of the workspace at pass {pass}\n"));
            }
        }
        let out = compact_generic_logs(&t);
        assert!(
            !out.contains("[tokenade:fold]"),
            "reordered the output for a fold that barely wins:\n{out:.300}"
        );
    }

    /// …and one that wins by a wide margin is taken. The shapes have to differ
    /// STRUCTURALLY, not by a number: `phase0`/`phase1` template to the same
    /// `phase<N>`, so the run pass folds them without help and the regroup is
    /// never reached. That first fixture measured nothing.
    #[test]
    fn a_regroup_that_wins_widely_is_taken() {
        let mut t = String::new();
        for i in 0..100 {
            t.push_str(&format!("compiling module {i} into build/out{i}.o\n"));
            t.push_str(&format!("  linking build/out{i}.o -> lib/mod{i}.a\n"));
            t.push_str(&format!("[done] module {i} in 0.4s\n"));
        }
        let out = compact_generic_logs(&t);
        assert!(
            out.contains("[tokenade:fold]"),
            "refused a fold that wins by 10x:\n{out:.300}"
        );
        assert!(out.len() * 10 < t.len(), "{} of {}", out.len(), t.len());
    }
}
#[cfg(test)]
mod scattered_source_guard {
    use super::*;

    /// Source code is two interleaved shapes like any build log — a comment and
    /// a statement — and regrouping it hands the agent a template where it
    /// asked for exact bytes. Caught by an EXISTING test on the `sed` path;
    /// pinned here at the level the rule actually lives, so moving the sed
    /// compactor cannot silently remove the protection.
    #[test]
    fn a_source_slice_is_never_regrouped() {
        let mut t = String::new();
        for i in 0..300 {
            t.push_str(&format!("// a comment that carries no meaning {i}\n"));
            t.push_str(&format!("let value_{i} = compute_something(i, {i});\n"));
        }
        let out = compact_generic_logs(&t);
        assert!(
            !out.contains("[tokenade:fold]"),
            "a source slice was regrouped"
        );
        assert!(
            out.contains("let value_7 = compute_something(i, 7);"),
            "the exact bytes the agent asked for are gone"
        );
    }

    /// …and a Python slice, whose two shapes are a docstring and a `def`.
    #[test]
    fn a_python_slice_is_never_regrouped_either() {
        let mut t = String::new();
        for i in 0..200 {
            t.push_str(&format!("def handler_{i}(request, context):\n"));
            t.push_str(&format!("    return dispatch(request, {i})\n"));
        }
        assert!(!compact_generic_logs(&t).contains("[tokenade:fold]"));
    }

    /// The build log the rule must NOT catch, so the guard cannot be satisfied
    /// by refusing everything.
    #[test]
    fn a_build_log_is_still_regrouped() {
        let mut t = String::new();
        for i in 0..120 {
            t.push_str(&format!(
                "gcc -c -O2 -Iinclude -o build/u{i}.o src/u{i}.c\n"
            ));
            t.push_str(&format!("ar rcs lib/u{i}.a build/u{i}.o\n"));
            t.push_str(&format!("[done] unit {i} in 0.4s\n"));
        }
        let out = compact_generic_logs(&t);
        assert!(out.contains("[tokenade:fold]"), "{out:.200}");
        assert!(out.len() * 10 < t.len(), "{} of {}", out.len(), t.len());
    }

    /// A number's right-alignment padding is not part of the sentence.
    ///
    /// `[   9%]` and `[  10%]` are one line said twice. Keeping the spaces
    /// literal put them in different templates, so a cmake build listed every
    /// module TWICE — once for its 1-digit percentages, once for its 2-and-3
    /// digit ones — and the banner counted those halves as separate shapes.
    #[test]
    fn a_numbers_padding_does_not_split_its_template() {
        let mods = ["core", "net", "ui", "codec", "math"];
        let mut t = String::new();
        for i in 0..300 {
            let pct = i * 100 / 300;
            let m = mods[i % mods.len()];
            t.push_str(&format!(
                "[ {pct:3}%] Building CXX object src/{m}/CMakeFiles/{m}.dir/unit{i:03}.cpp.o\n"
            ));
        }
        let out = compact_generic_logs(&t);
        assert!(out.contains("[tokenade:fold]"), "{out:.200}");
        let rows = out
            .lines()
            .filter(|l| l.contains("Building CXX object"))
            .count();
        assert_eq!(
            rows,
            mods.len(),
            "one row per module, not one per module per digit width:\n{out}"
        );

        // The other half of the rule: LEADING indentation is structure, not
        // padding. The number has to be the FIRST token for this to bite —
        // with anything before it the indent is never a candidate for removal,
        // which is how the first version of this half passed under a mutation
        // that stripped every indent. Two depths of the same sentence must
        // stay two templates, or the rule folds a tree or a stack trace flat.
        let mut nested = String::new();
        for i in 0..40 {
            nested.push_str(&format!("{i} files scanned in the outer module\n"));
            nested.push_str(&format!("    {i} files scanned in the outer module\n"));
        }
        let out = compact_generic_logs(&nested);
        let rows: Vec<&str> = out
            .lines()
            .filter(|l| l.contains("files scanned in the outer module"))
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "the two depths are two sentences, not one:\n{out}"
        );
        assert!(
            rows.iter().any(|l| l.starts_with("    "))
                && rows.iter().any(|l| !l.starts_with(' ')),
            "one row must keep its indent and one must not:\n{out}"
        );
    }
}

#[cfg(test)]
mod a_distinct_value_on_every_line {
    use super::compact_generic_logs;

    /// `coupé   orbit/<N>` ×6: the six cut identifiers ARE the answer;
    /// the fold kept only one, as an example.
    #[test]
    fn every_value_is_listed_in_order() {
        let ids = [161_093_874u64, 161_093_875, 170_224_001, 170_224_919, 181_000_442, 199_004_117];
        let text: String = ids.iter().map(|i| format!("coupé   orbit/{i}\n")).collect();
        let out = compact_generic_logs(&text);
        for i in ids {
            assert!(out.contains(&i.to_string()), "{i} lost:\n{out}");
        }
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(out.find("161093874").unwrap() < out.find("199004117").unwrap(), "order kept: {out}");
    }

    /// Distinct timestamps remain the noise the fold removes, and a
    /// value that repeats keeps the `distinct … e.g.` summary.
    #[test]
    fn a_timestamp_or_a_repetition_keeps_the_summary() {
        let ts: String = (0..30).map(|i| format!("2026-10-02T12:00:{i:02}Z worker heartbeat ok\n")).collect();
        let out = compact_generic_logs(&ts);
        assert!(out.contains("e.g.") && !out.contains("12:00:17"), "{out}");
        let rep: String = (0..30).map(|i| format!("request from 10.0.0.{}\n", i % 3)).collect();
        assert!(compact_generic_logs(&rep).contains("e.g."));
    }

    /// Consecutive integers are said as two numbers, without loss.
    #[test]
    fn consecutive_integers_are_said_as_a_range() {
        let text: String = (0..800).map(|i| format!("2026-09-04T10:00:00Z INFO svc handled req {i} ok\n")).collect();
        let out = compact_generic_logs(&text);
        assert!(out.contains("N= 0…799"), "{out}");
        let down: String = (0..5).rev().map(|i| format!("step {} done\n", 120 + i)).collect();
        assert!(compact_generic_logs(&down).contains("N= 124…120"), "{}", compact_generic_logs(&down));
        // Leading zeros at ONE width: both bounds carry it, every
        // value recomposes (amended 2026-10-03, `worker-{i:03}`). Two
        // mixed widths, or a step of two: the list, not the range.
        let padded: String = (8..12).map(|i| format!("frame {i:03} ok\n")).collect();
        assert!(compact_generic_logs(&padded).contains("N= 008…011"), "{}", compact_generic_logs(&padded));
        let mixed: String = ["frame 08 ok\n", "frame 9 ok\n", "frame 10 ok\n"].concat();
        assert!(!compact_generic_logs(&mixed).contains('…'), "{}", compact_generic_logs(&mixed));
        let even: String = (0..6).map(|i| format!("port {} open\n", 8000 + 2 * i)).collect();
        assert!(compact_generic_logs(&even).contains("8000, 8002"), "{}", compact_generic_logs(&even));
    }

    /// Past the budget, the summary comes back: 2,000 identifiers are not written
    /// on one line.
    #[test]
    fn past_the_budget_the_summary_comes_back() {
        let text: String = (0..2_000).map(|i| format!("processed item {}\n", 1_000_000 + 7 * i)).collect();
        let out = compact_generic_logs(&text);
        assert!(out.contains("e.g.") && out.len() < 400, "{} bytes", out.len());
    }
}

/// A hex request id that happens to be all digits keeps its line in the run.
#[test]
fn an_all_digit_hex_id_does_not_break_its_run() {
    let ids = ["f2a74de4", "92276658", "1a61dbe2", "57124242", "9474031b", "38703800", "5675f6ad", "c6c91b92"];
    let log: String = ids
        .iter()
        .enumerate()
        .map(|(i, id)| format!("2026-10-03T12:00:0{i}.000Z INFO request id={id} status=200\n"))
        .collect();
    let out = compact_generic_logs(&log);
    assert!(out.contains("[×8 lines"), "{out}");
    assert!(!out.contains("id=92276658"), "printed alone: {out}");
    // Without a hex neighbour, a long number stays a number.
    let nums: String = (0..8).map(|i| format!("2026-10-03T12:00:0{i}.000Z INFO total={} ok\n", 10_000_000 + i * 7)).collect();
    assert!(!compact_generic_logs(&nums).contains("HEX"));
    // A hex line of ANOTHER shape does not make those numbers hex.
    let mixed = format!("2026-10-03T11:59:59.000Z WARN cache key=deadbeef9 evicted\n{nums}");
    let out = compact_generic_logs(&mixed);
    assert!(out.contains("total=<N"), "{out}");
}

/// A slot with a handful of values names them with their counts; a slot with
/// many does not.
#[test]
fn a_few_values_are_named_with_their_counts() {
    let log: String = (0..40)
        .map(|i| {
            let status = [500, 500, 200, 404][i % 4];
            format!("2026-10-03T12:00:{:02}.000Z GET /item/{} status={status}\n", i, 1000 + i * 7)
        })
        .collect();
    let out = compact_generic_logs(&log);
    assert!(out.contains("=3 (500 ×20 · 200 ×10 · 404 ×10)"), "{out}");
    assert!(!out.contains("N1=40 ("), "{out}");
}

/// A log whose in-order fold is still too big to deliver is gathered by shape,
/// so no error is lost to the cut; small groups do not name their values.
#[test]
fn an_oversized_interleaved_log_is_gathered_by_shape() {
    let mut log = String::new();
    for i in 0..30_000 {
        let ts = format!("2026-10-03T12:{:02}:{:02}.{:03}Z", i / 3600 % 60, i / 60 % 60, i % 1000);
        match i % 50 {
            7 => log.push_str(&format!("{ts} ERROR worker-{} payment gateway timeout after 30s\n", i % 8)),
            13 | 31 => log.push_str(&format!("{ts} WARN  worker-{} slow query took {}ms\n", i % 8, 1000 + i % 977)),
            _ => log.push_str(&format!("{ts} INFO  worker-{} request id=a{:07x} status=200 dur={}ms\n", i % 8, i * 7919 % 0xfffffff, i % 900)),
        }
    }
    assert!(!tk_base::delivery::fits(&log));
    let out = compact_generic_logs(&log);
    assert!(out.contains("line order is BY SHAPE"), "{}", &out[..out.len().min(600)]);
    assert!(out.contains("payment gateway timeout after 30s  [×600 lines"), "{}", &out[..out.len().min(1200)]);
    // A small in-order group does not list its few workers.
    let small: String = (0..16).map(|i| format!("2026-10-03T12:00:{:02}.000Z INFO worker-{} ok id={}\n", i, i % 5, 1000 + i * 13)).collect();
    assert!(!compact_generic_logs(&small).contains(" ×"), "{}", compact_generic_logs(&small));
}

/// Gathered by shape, a stack trace stays one record: its frames are not
/// pooled with another trace's.
#[test]
fn a_stack_trace_is_gathered_whole() {
    let mut log = String::new();
    for i in 0..30_000 {
        let ts = format!("2026-10-03T12:{:02}:{:02}.{:03}Z", i / 3600 % 60, i / 60 % 60, i % 1000);
        if i % 300 == 150 {
            log.push_str(&format!("{ts} ERROR request failed\n"));
            if (i / 300) % 2 == 0 {
                log.push_str("java.lang.NullPointerException: discount is null\n\tat Svc.apply(Svc.java:142)\n\tat Ctl.checkout(Ctl.java:88)\n");
            } else {
                log.push_str("java.net.SocketTimeoutException: Read timed out\n\tat Pay.charge(Pay.java:61)\n\tat Ctl.checkout(Ctl.java:91)\n");
            }
        } else {
            log.push_str(&format!("{ts} INFO  GET /api/items/{} 200 {}ms\n", i % 999, i % 900));
        }
    }
    let out = compact_generic_logs(&log);
    assert!(out.contains("line order is BY SHAPE"), "{out}");
    assert!(out.contains("discount is null\n\tat Svc.apply(Svc.java:142)\n\tat Ctl.checkout(Ctl.java:88)\n"), "{out}");
    assert!(out.contains("Read timed out\n\tat Pay.charge(Pay.java:61)\n\tat Ctl.checkout(Ctl.java:91)"), "{out}");
}

/// Without timestamps, a Python traceback is one record: its indented frames
/// and the exception line that closes it.
#[test]
fn an_unstamped_traceback_is_gathered_whole() {
    let mut log = String::new();
    for i in 0..30_000 {
        if i % 300 == 150 {
            if (i / 300) % 2 == 0 {
                log.push_str("Traceback (most recent call last):\n  File \"views.py\", line 88, in checkout\n    total = apply_discount(cart)\nValueError: bad cart\n");
            } else {
                log.push_str("Traceback (most recent call last):\n  File \"views.py\", line 91, in checkout\n    charge(cart)\nValueError: bad cart\n");
            }
        } else {
            log.push_str(&format!("GET /api/items/{} 200 {}ms\n", i % 999, i % 900));
        }
    }
    let out = compact_generic_logs(&log);
    // The same exception closes both: each traceback keeps its own.
    assert!(out.contains("line 88, in checkout\n    total = apply_discount(cart)\nValueError: bad cart"), "{out}");
    assert!(out.contains("line 91, in checkout\n    charge(cart)\nValueError: bad cart"), "{out}");
}

#[cfg(test)]
mod name_order_ranges {
    use super::*;

    /// `git status` / `ls` sort names as STRINGS: `1, 10, 100, …`. A set that
    /// is exactly 1…N in that order is a range; anything else is listed.
    #[test]
    fn a_name_sorted_run_of_integers_is_a_range() {
        let mut names: Vec<String> = (1..=300).map(|i| i.to_string()).collect();
        names.sort();
        let v: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(range_of(&v).as_deref(), Some("1…300, in name order"));
        // Consecutive as a set but NOT in name order: the range would not
        // rebuild the lines.
        assert_eq!(range_of(&["3", "1", "2", "4"]), None);
        // In name order, with a gap: not a range.
        assert_eq!(range_of(&["1", "10", "2", "4", "5", "6", "7", "8", "9"]), None);
    }
}
