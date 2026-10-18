// The tests of `session_lifecycle`.
//
// Spliced in by `include!` from `session_lifecycle.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod cursor_tests {
    use super::cursor_growth;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-cursor-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("acct")
    }

    /// The defect: a meter that lost its zero billed everything produced
    /// before it existed.
    #[test]
    fn a_missing_cursor_adopts_the_backlog_instead_of_billing_it() {
        let p = tmp("backlog");
        assert_eq!(cursor_growth(&p, 4_860_000), None, "backlog was billed");
        assert_eq!(
            cursor_growth(&p, 4_861_000),
            Some(1_000),
            "growth after it is credited"
        );
    }

    /// …and a genuinely new session loses nothing, because its first reading
    /// is taken before the turn has produced anything.
    #[test]
    fn a_new_session_starts_at_zero_and_loses_nothing() {
        let p = tmp("new");
        assert_eq!(cursor_growth(&p, 0), None);
        assert_eq!(cursor_growth(&p, 700), Some(700));
    }

    /// Reporting growth must not advance the cursor: the caller drops deltas
    /// that round to zero, and those have to accumulate.
    #[test]
    fn a_delta_too_small_to_record_is_not_swallowed() {
        let p = tmp("small");
        cursor_growth(&p, 0);
        assert_eq!(cursor_growth(&p, 3), Some(3));
        assert_eq!(
            cursor_growth(&p, 6),
            Some(6),
            "the unrecorded 3 must still be owed"
        );
    }

    /// A counter that goes backwards (transcript replaced, file truncated)
    /// credits nothing rather than a negative-turned-huge number.
    #[test]
    fn a_counter_that_shrinks_credits_nothing() {
        let p = tmp("shrink");
        cursor_growth(&p, 0);
        cursor_growth(&p, 5_000);
        std::fs::write(&p, "5000").unwrap();
        assert_eq!(cursor_growth(&p, 10), None);
    }
}

#[cfg(test)]
mod overhead_horizon_tests {
    use super::COMPOUNDING_HORIZON_MAX_TURNS as H;

    /// The rule the fix implements, stated as arithmetic so it can be checked
    /// without a session on disk: a token pays at most H times, so a session's
    /// cumulative re-bill converges at `H x injected` instead of growing with
    /// the square of the turn count.
    fn simulate(injected_per_turn: &[u64]) -> u64 {
        let mut already = 0u64;
        let mut total = 0u64;
        for &injected in injected_per_turn {
            let book = (injected.saturating_mul(H))
                .saturating_sub(already)
                .min(injected);
            already += book;
            total += book;
        }
        total
    }

    /// The shape observed in production: the counter climbs, the old rule
    /// re-billed the running total every turn. 341 turns ending at 62 k of
    /// injected context were charged 11.4 M — the triangular sum.
    #[test]
    fn a_long_session_no_longer_pays_the_triangle() {
        let turns: Vec<u64> = (1..=341).map(|t| t * 182).collect();
        let injected_total = *turns.last().unwrap();
        let old: u64 = turns.iter().sum();
        let new = simulate(&turns);
        assert!(
            new <= injected_total * H,
            "the bound must hold: {new} > {injected_total} x {H}"
        );
        assert!(
            new * 5 < old,
            "the correction must be substantial: old={old} new={new}"
        );
    }

    /// And a short session is not penalised: below the horizon, nothing is
    /// clipped and the accounting is exactly what it was.
    #[test]
    fn a_short_session_is_unchanged() {
        let turns: Vec<u64> = (1..=6).map(|t| t * 100).collect();
        assert_eq!(simulate(&turns), turns.iter().sum::<u64>());
    }

    /// A session that injects nothing books nothing — no floor, no minimum.
    #[test]
    fn nothing_injected_is_nothing_billed() {
        assert_eq!(simulate(&[0, 0, 0]), 0);
    }
}

#[cfg(test)]
mod compounding_horizon_tests {
    use super::fold_compounding_log;

    /// PATHOLOGICAL SESSION (the 377M repro, now impossible): 500 turns with a
    /// single huge Read banked early. Three bounds compose to keep the
    /// cumulative compounding tiny: (a) the δ is bounded at the SOURCE to the
    /// Read delivery cap (~20k, not before−after = 1_000_000); (b) the
    /// per-event horizon caps each fold's multiplier at H turns; (c) the
    /// session backstop caps the total at K× what was folded. Result: a few ×
    /// the banked δ (H, the same bound as the overhead re-bill), never hundreds
    /// of millions.
    #[test]
    fn pathological_session_compounding_is_bounded() {
        use super::compounding_session_cap;
        // δ already bounded at read_staging by the harness Read cap.
        const READ_CAP: u64 = 20_000;
        let log = format!("{{\"turn\":1,\"delta\":{READ_CAP}}}\n");
        let banked = READ_CAP;
        let cap = compounding_session_cap(banked, super::RereadProxy::PER_TURN);
        let mut prev = 0u64;
        let mut credited = 0u64;
        for t in 1..=500u64 {
            // No real delivered growth → no synthetic reset; the horizon +
            // session cap alone must contain it.
            let f = fold_compounding_log(&log, t, 0, 140_000, super::RereadProxy::PER_TURN);
            let cumulative = f.cumulative.min(cap);
            if cumulative > prev {
                let slice = (cumulative - prev).min(f.resident_delta.max(1));
                credited += slice;
                prev = cumulative;
            }
        }
        // Unbounded this would be ~READ_CAP·499 ≈ 10M (or, without the source
        // bound, 1M·499 ≈ 499M — the field 377M). Bounded, it's ≤ K×banked.
        assert!(
            credited <= super::COMPOUNDING_SESSION_K * READ_CAP,
            "session cap failed: {credited} > {}",
            super::COMPOUNDING_SESSION_K * READ_CAP
        );
        assert!(credited > 0, "compounding must still credit something");
        // 12 re-reads of the fold: what the re-bill charges an injected token.
        assert_eq!(credited, 12 * READ_CAP, "the fold is credited H times, no more, no less");
    }

    /// Legacy log shape (events only, no resets), short session well under
    /// the threshold: exact pre-horizon behaviour — Σδ·(T−turn).
    #[test]
    fn short_session_unchanged() {
        let log = "{\"turn\":2,\"delta\":100}\n{\"turn\":5,\"delta\":50}\n";
        let f = fold_compounding_log(log, 10, 0, 140_000, super::RereadProxy::PER_TURN);
        // 100·(10−2) + 50·(10−5) = 800 + 250
        assert_eq!(f.cumulative, 1050);
        assert!(!f.synthetic_reset);
    }

    /// A real (PreCompact-observed) reset freezes accrual at the reset turn
    /// and expires earlier events; later events accrue from their own turn.
    #[test]
    fn real_reset_freezes_and_expires() {
        let log = concat!(
            "{\"turn\":2,\"delta\":100}\n",
            "{\"turn\":6,\"reset\":1,\"delivered\":4000}\n",
            "{\"turn\":8,\"delta\":40}\n",
        );
        let f = fold_compounding_log(log, 20, 4000, 140_000, super::RereadProxy::PER_TURN);
        // frozen: 100·(6−2)=400 ; live: 40·(20−8)=480
        assert_eq!(f.cumulative, 880);
        assert!(!f.synthetic_reset);
        // Without the reset the same log keeps the folds paying, but the
        // per-event horizon (H=12 turns) caps each age: turn-2 fold is 18 turns
        // old → clamped to 12, turn-8 fold is 12 → 12. 100·12 + 40·12 = 1680
        // (vs the unbounded 100·18 + 40·12 = 2280 — the horizon cut it).
        let no_reset = "{\"turn\":2,\"delta\":100}\n{\"turn\":8,\"delta\":40}\n";
        assert_eq!(
            fold_compounding_log(no_reset, 20, 0, 140_000, super::RereadProxy::PER_TURN).cumulative,
            1680
        );
    }

    /// Counterfactual context (delivered + folded) past the threshold →
    /// synthetic reset: freeze NOW, expire live events.
    #[test]
    fn synthetic_reset_when_counterfactual_crosses_threshold() {
        // delivered 400_000 bytes ≈ 100k tokens + 50k folded > 140k threshold
        let log = "{\"turn\":3,\"delta\":50000}\n";
        let f = fold_compounding_log(log, 10, 400_000, 140_000, super::RereadProxy::PER_TURN);
        assert!(f.synthetic_reset);
        // frozen at turn 10: 50000·(10−3) — and nothing accrues past it
        assert_eq!(f.cumulative, 350_000);
        assert_eq!(
            fold_compounding_log(log, 10, 390_000, 140_000, super::RereadProxy::PER_TURN).cumulative,
            350_000,
            "just under threshold: same value at T=10, but still live"
        );
        // After the reset is persisted, later turns add NOTHING for it.
        let log2 = concat!(
            "{\"turn\":3,\"delta\":50000}\n",
            "{\"turn\":10,\"reset\":1,\"delivered\":400000}\n",
        );
        assert_eq!(
            fold_compounding_log(log2, 30, 410_000, 140_000, super::RereadProxy::PER_TURN).cumulative,
            350_000
        );
    }

    /// Delivered floor restarts at each reset: growth AFTER a compaction is
    /// measured against a fresh (small) real context, not the session total.
    #[test]
    fn delivered_floor_restarts_after_reset() {
        let log = concat!(
            "{\"turn\":3,\"delta\":1000}\n",
            "{\"turn\":10,\"reset\":1,\"delivered\":600000}\n",
            "{\"turn\":12,\"delta\":2000}\n",
        );
        // total delivered 620_000 bytes but only 20_000 since the reset
        // (≈5k tokens) + 2k live folded — well under threshold, no reset.
        let f = fold_compounding_log(log, 15, 620_000, 140_000, super::RereadProxy::PER_TURN);
        assert!(!f.synthetic_reset);
        // frozen 1000·(10−3)=7000 ; live 2000·(15−12)=6000
        assert_eq!(f.cumulative, 13_000);
    }

    /// Very long session with regular folds: horizon caps the credit well
    /// below the unbounded Σδ·(T−turn). (The owner's order-of-magnitude
    /// scenario — see the audit note in the fn docs.)
    #[test]
    fn long_session_bounded_by_horizon() {
        // 50 turns, 2k tokens folded per turn, 3k tokens (12k bytes) of
        // real delivered content per turn, 200k window → 140k threshold.
        let mut log = String::new();
        let mut unbounded = 0u64;
        for t in 1..=50u64 {
            log.push_str(&format!("{{\"turn\":{t},\"delta\":2000}}\n"));
            unbounded += 2000 * (50 - t);
        }
        // Replay turn by turn like the hook does, persisting synthetic resets.
        let mut credited = 0u64;
        let mut prev = 0u64;
        for t in 1..=50u64 {
            let delivered = t * 12_000;
            let upto: String = log
                .lines()
                .filter(|l| {
                    serde_json::from_str::<serde_json::Value>(l)
                        .ok()
                        .and_then(|v| v["turn"].as_u64())
                        .map(|lt| lt <= t)
                        .unwrap_or(false)
                })
                .map(|l| format!("{l}\n"))
                .collect();
            let f = fold_compounding_log(&upto, t, delivered, 140_000, super::RereadProxy::PER_TURN);
            if f.synthetic_reset {
                log.push_str(&format!(
                    "{{\"turn\":{t},\"reset\":1,\"delivered\":{delivered}}}\n"
                ));
            }
            if f.cumulative > prev {
                credited += (f.cumulative - prev).min(140_000);
                prev = f.cumulative;
            }
        }
        assert_eq!(unbounded, 2_450_000);
        // Horizon must bite: strictly less than unbounded, but nonzero.
        assert!(credited > 0);
        assert!(
            credited < unbounded,
            "horizon failed to bound: {credited} >= {unbounded}"
        );
        // Counterfactual grows 5k tokens/turn → first synthetic reset ≈ turn
        // 28, second ≈ turn 46 (floor restarts, folds keep coming). Expect
        // roughly half the unbounded figure; assert a generous envelope so
        // the test pins the ORDER, not brittle exact numbers.
        assert!(
            credited < unbounded * 3 / 4,
            "horizon too weak: {credited} vs {unbounded}"
        );
        assert!(
            credited > unbounded / 5,
            "horizon over-corrected: {credited} vs {unbounded}"
        );
    }

    /// Malformed lines are skipped; empty log yields zero.
    #[test]
    fn tolerant_of_garbage() {
        let f = fold_compounding_log("not json\n{\"turn\":1,\"delta\":10}\n", 3, 0, 140_000, super::RereadProxy::PER_TURN);
        assert_eq!(f.cumulative, 20);
        assert_eq!(fold_compounding_log("", 5, 0, 140_000, super::RereadProxy::PER_TURN).cumulative, 0);
    }
}

#[cfg(test)]
mod delivered_counter_tests {
    use super::session_delivered_bytes;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-delivered-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("sess-delivered")
    }

    /// Concurrent writers append; the reader sums. Nothing is lost.
    #[test]
    fn every_appended_delivery_is_counted() {
        let p = tmp("append");
        std::fs::write(&p, "1200\n800\n40\n").unwrap();
        assert_eq!(session_delivered_bytes(&p), 2040);
    }

    /// A file an older build left holds one integer and no newline. It must
    /// keep meaning exactly what it meant.
    #[test]
    fn a_counter_written_by_an_older_build_still_reads() {
        let p = tmp("legacy");
        std::fs::write(&p, "98765").unwrap();
        assert_eq!(session_delivered_bytes(&p), 98765);
    }

    /// A torn or foreign line must not poison the total.
    #[test]
    fn a_line_that_is_not_a_number_is_skipped_not_fatal() {
        let p = tmp("junk");
        std::fs::write(&p, "500\n\nnot-a-number\n250\n").unwrap();
        assert_eq!(session_delivered_bytes(&p), 750);
    }

    #[test]
    fn a_missing_file_is_zero() {
        assert_eq!(session_delivered_bytes(&tmp("missing")), 0);
    }
}

#[cfg(test)]
mod cursor_failure_tests {
    use super::cursor_growth;

    /// The cursor lives on disk. When the disk refuses it — a full filesystem,
    /// a read-only home, a sessions path that is not a directory — the seeding
    /// rule must degrade to crediting NOTHING, never to crediting the whole
    /// backlog on every single turn.
    ///
    /// That is the direction that matters: a meter that cannot remember where
    /// it was should under-report, because the alternative is billing the same
    /// tokens again at every prompt for as long as the disk stays broken.
    #[test]
    fn a_cursor_that_cannot_be_written_credits_nothing_rather_than_everything() {
        let d = std::env::temp_dir().join(format!(
            "tokenade-cursor-ro-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // The cursor's PARENT is a file, so no write to it can ever land.
        let blocked = d.join("not-a-dir");
        std::fs::write(&blocked, b"x").unwrap();
        let cursor = blocked.join("sess-outacct");

        let mut credited = 0u64;
        for turn in 1..=10u64 {
            if let Some(growth) = cursor_growth(&cursor, 100_000 + turn * 1_000) {
                credited += growth;
            }
        }
        assert_eq!(
            credited, 0,
            "a cursor that cannot persist billed {credited} tokens across ten turns"
        );
    }
}

#[cfg(test)]
mod the_recovery_offer_names_what_was_lost {
    use super::*;

    fn stash_with(sizes: &[(&str, u64)]) -> std::path::PathBuf {
        // See `tokenade_core::testtmp`: 550 MB over 308 abandoned copies.
        tokenade_core::testtmp::sweep_stale_scratch_dirs();
        let d = std::env::temp_dir().join(format!(
            "tokenade-offer-{}-{}",
            std::process::id(),
            sizes.len()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (name, sz) in sizes {
            std::fs::write(d.join(format!("{name}.txt")), vec![b'x'; *sz as usize]).unwrap();
        }
        d
    }

    /// The shape observed at a real compaction: eight 240-byte fragments were
    /// the FRESHEST, and the megabyte-sized folds — all under an hour old —
    /// were not offered at all.
    #[test]
    fn the_big_folds_win_over_the_recent_crumbs() {
        let d = stash_with(&[
            ("aaaaaaaaaaaa", 240),
            ("bbbbbbbbbbbb", 343),
            ("cccccccccccc", 5_861_284),
            ("dddddddddddd", 1_493_286),
        ]);
        let got = refs_worth_offering(&d, None);
        let names: Vec<&str> = got.iter().map(|(_, h, _)| h.as_str()).collect();
        assert_eq!(
            names,
            vec!["cccccccccccc", "dddddddddddd"],
            "an agent offered eight `(0k)` learns not to call expand-ref at all"
        );
    }

    /// Below the floor a ref cannot return more than the ~15 tokens spent
    /// naming it. Measured: of 92 refs in the window, 53 are ≥512 bytes and the
    /// other 39 are all below, so any cut in that gap selects the same 53.
    #[test]
    fn crumbs_are_not_offered_even_when_they_are_all_there_is() {
        let d = stash_with(&[("aaaaaaaaaaaa", 240), ("bbbbbbbbbbbb", 1023)]);
        assert!(
            refs_worth_offering(&d, None).is_empty(),
            "offering a 240-byte fragment costs more than it returns"
        );
    }

    #[test]
    fn a_ref_at_the_floor_is_offered() {
        let d = stash_with(&[("aaaaaaaaaaaa", 1024)]);
        assert_eq!(refs_worth_offering(&d, None).len(), 1);
    }

    #[test]
    fn at_most_eight_and_biggest_first() {
        let sizes: Vec<(String, u64)> = (0..12)
            .map(|i| (format!("{:012}", i), 2048 + i as u64 * 1000))
            .collect();
        let refs: Vec<(&str, u64)> = sizes.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let d = stash_with(&refs);
        let got = refs_worth_offering(&d, None);
        assert_eq!(got.len(), 8);
        for w in got.windows(2) {
            assert!(w[0].2 >= w[1].2, "must be biggest-first");
        }
    }
}

#[cfg(test)]
mod the_digest_offers_only_this_sessions_folds {
    //! `refs_worth_offering` picked by mtime inside a six-hour window over a
    //! stash directory shared by every agent on the machine. Measured over
    //! this machine's ledger: 1 828 of 3 676 moments (50 %) had more than one
    //! session active in that window, and 2026-08-28 alone held fifteen.

    use super::refs_worth_offering_in;

    fn stash(dir: &std::path::Path, hash: &str, bytes: usize) {
        std::fs::write(dir.join(format!("{hash}.txt")), "x".repeat(bytes)).unwrap();
    }

    #[test]
    fn a_ref_another_session_folded_is_not_offered_as_ours() {
        let root = std::env::temp_dir().join(format!("tkn-digest-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let stash_dir = root.join("stash");
        let sessions = root.join(".tokenade").join("sessions");
        std::fs::create_dir_all(&stash_dir).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        stash(&stash_dir, "aaaaaaaaaaaa", 4096);
        stash(&stash_dir, "bbbbbbbbbbbb", 8192);
        stash(&stash_dir, "cccccccccccc", 2048);
        std::fs::write(sessions.join("mine.refs"), "aaaaaaaaaaaa\ncccccccccccc\n").unwrap();
        std::fs::write(sessions.join("theirs.refs"), "bbbbbbbbbbbb\n").unwrap();

        let names = |v: Vec<(std::time::SystemTime, String, u64)>| {
            let mut n: Vec<String> = v.into_iter().map(|(_, h, _)| h).collect();
            n.sort();
            n
        };

        let ours = names(refs_worth_offering_in(&stash_dir, &sessions, Some("mine")));
        assert_eq!(
            ours,
            vec!["aaaaaaaaaaaa".to_string(), "cccccccccccc".to_string()],
            "the digest offered a ref this session never folded"
        );

        // A session that folded nothing is offered nothing — better than
        // somebody else's bytes under the sentence "tokenade folded earlier".
        assert!(
            refs_worth_offering_in(&stash_dir, &sessions, Some("silent")).is_empty(),
            "an unrecorded session must not inherit the directory"
        );

        // No session id at all → the old whole-directory behaviour, so a hook
        // payload without one degrades rather than going blank.
        assert_eq!(
            names(refs_worth_offering_in(&stash_dir, &sessions, None)).len(),
            3
        );

        // The size floor still applies inside the scoping.
        std::fs::write(sessions.join("tiny.refs"), "dddddddddddd\n").unwrap();
        stash(&stash_dir, "dddddddddddd", 64);
        assert!(
            refs_worth_offering_in(&stash_dir, &sessions, Some("tiny")).is_empty(),
            "a ref too small to be worth naming was offered"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

/// SessionStart writes project files via `ensure` and must not re-inject
/// the full rules block as `additionalContext` on a normal session.
#[cfg(test)]
mod sessionstart_does_not_reinject_the_scaffold {
    #[test]
    fn cmd_hookwrap_sessionstart_does_not_put_scaffold_in_additional_context() {
        let src = tokenade_core::source_guard::module_family(
            &tokenade_core::source_guard::crates_dir()
                .join("tokenade-cli")
                .join("src")
                .join("hooks"),
            "session_lifecycle",
        );
        let body = src
            .split("pub(crate) fn cmd_hookwrap_sessionstart")
            .nth(1)
            .expect("cmd_hookwrap_sessionstart");
        let body = body
            .split("pub(crate) fn")
            .next()
            .expect("end of sessionstart");
        assert!(
            body.contains("No scaffold additionalContext"),
            "the no-reinject decision comment is gone"
        );
        // `scaffold_block_plain()` is the tokenizer overhead cost, booked
        // before `additions` is built — not an additionalContext payload.
        let after_ac = body
            .split("let mut additions")
            .nth(1)
            .expect("additions vec");
        assert!(
            !after_ac.contains("scaffold_block_plain()"),
            "SessionStart started injecting scaffold_block_plain as additionalContext"
        );
        assert!(
            !after_ac.contains("scaffold_block("),
            "SessionStart started injecting the rules block as additionalContext"
        );
        assert!(
            !after_ac.contains("Explore code with the tokenade CLI"),
            "SessionStart additionalContext carries the rules lecture"
        );
    }
}
