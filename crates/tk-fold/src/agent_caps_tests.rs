// The tests of `crate::agent_caps`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `agent_caps`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cap_states_where_it_came_from() {
        for (agent, action, cap) in TABLE {
            let d = cap.provenance.describe();
            assert!(
                !d.trim().is_empty() && d.len() > 6,
                "{agent}/{} has a nameless provenance: {d:?}",
                action.as_str()
            );
            if let Provenance::Package { file, symbol } = cap.provenance {
                assert!(
                    !file.is_empty() && !symbol.is_empty(),
                    "{agent}/{} claims to be measured but names no file/symbol",
                    action.as_str()
                );
            }
        }
    }

    #[test]
    fn a_per_line_cap_depends_on_the_data() {
        // The exact case a scalar cannot express: same size, opposite outcomes.
        let l = Limit::Lines {
            max_lines: 3,
            max_line_chars: Some(10),
        };
        // Few long lines: the LINE cap bites, the line-count does not.
        let wide = "x".repeat(100) + "\n" + &"y".repeat(100);
        assert_eq!(
            l.delivered_chars(&wide, Ctx::UNKNOWN),
            22,
            "2 lines x (10 + newline)"
        );
        // Many short lines: the line-COUNT bites instead.
        let tall = "abc\n".repeat(50);
        assert_eq!(
            l.delivered_chars(&tall, Ctx::UNKNOWN),
            12,
            "3 lines x (3 + newline)"
        );
        // A scalar would have had to pick one of 22 or 12 and be wrong about
        // the other, for payloads of 201 and 200 chars respectively.
    }

    #[test]
    fn the_threshold_rule_is_a_cliff_not_a_ramp() {
        let l = Limit::HeadTail {
            threshold_chars: 100,
            total_lines: 10,
            wrap_width: None,
        };
        let under = "ab\n".repeat(33); // 99 chars
        assert_eq!(under.chars().count(), 99);
        assert_eq!(
            l.delivered_chars(&under, Ctx::UNKNOWN),
            99,
            "under the threshold NOTHING is cut"
        );
        // 60 lines of 2: over the threshold, and over the 10-line budget.
        let over = "ab\n".repeat(60);
        // split('\n') on a trailing newline yields 61 slots; head=2, tail=8.
        let got = l.delivered_chars(&over, Ctx::UNKNOWN);
        // head = 10/5 = 2 slots, tail = 8 slots (the last of which is the empty
        // one a trailing '\n' leaves behind), each re-joined with a newline.
        assert_eq!(
            got,
            6 + 22 + HEAD_TAIL_MARKER_CHARS,
            "head 2 + tail 8 + marker"
        );
        assert!(got < 180, "and that is far less than the 180 chars given");
    }

    #[test]
    fn a_full_context_shrinks_the_threshold_below_the_ceiling() {
        // The vendor computes min(ceiling, 4 * free tokens). Same agent, same
        // payload, opposite outcomes depending only on SESSION state.
        let l = Limit::HeadTail {
            threshold_chars: 4_000_000,
            total_lines: 10,
            wrap_width: Some(120),
        };
        let payload = "line\n".repeat(4_000); // 20 000 chars, 4 000 lines
        assert_eq!(
            l.delivered_chars(&payload, Ctx::UNKNOWN),
            20_000,
            "against the static ceiling alone, nothing is truncated"
        );
        // 1M-token model with 995 000 already used: 4 * 5 000 = 20 000 free...
        assert_eq!(Ctx::new(1_000_000, 995_000).remaining_chars(), Some(20_000));
        // ...so a 20 000-char payload is exactly AT the threshold, still whole.
        assert_eq!(
            l.delivered_chars(&payload, Ctx::new(1_000_000, 995_000)),
            20_000
        );
        // One more turn of context and the SAME payload is cut to ~10 lines.
        let squeezed = l.delivered_chars(&payload, Ctx::new(1_000_000, 999_000));
        assert!(
            squeezed < 100,
            "a fuller context truncates the same bytes: {squeezed}"
        );
    }

    #[test]
    fn a_single_huge_line_is_re_wrapped_before_being_cut() {
        // The trap a naive line rule falls into: one line, so "1 line <= 1000
        // lines, nothing to truncate" — while the agent wraps it at 120 first.
        let l = Limit::HeadTail {
            threshold_chars: 1_000,
            total_lines: 10,
            wrap_width: Some(120),
        };
        let one_line = "z".repeat(100_000);
        let got = l.delivered_chars(&one_line, Ctx::UNKNOWN);
        assert!(
            got < 2_000,
            "100 KB on ONE line must still be cut, got {got}"
        );
        // Without the re-wrap the whole 100 000 would have been delivered.
        let no_wrap = Limit::HeadTail {
            threshold_chars: 1_000,
            total_lines: 10,
            wrap_width: None,
        };
        assert!(no_wrap.delivered_chars(&one_line, Ctx::UNKNOWN) > 100_000 - 1);
    }

    #[test]
    fn head_and_tail_are_kept_not_a_prefix() {
        // `saveTruncatedContent` keeps total/5 from the START and the rest from
        // the END. A prefix-only model would report the same SIZE and the wrong
        // CONTENT, so the shape is asserted through an asymmetric payload.
        let l = Limit::HeadTail {
            threshold_chars: 10,
            total_lines: 10,
            wrap_width: None,
        };
        // 2 head lines of 1 char, 8 tail lines of 50 chars.
        let mut p = String::new();
        for _ in 0..2 {
            p.push_str("a\n");
        }
        for _ in 0..100 {
            p.push_str("m\n");
        }
        for _ in 0..8 {
            p.push_str(&("t".repeat(50) + "\n"));
        }
        let got = l.delivered_chars(&p, Ctx::UNKNOWN);
        // head = 10/5 = 2 lines ("a"), tail = 8 lines. split('\n') leaves a
        // trailing empty slot, so the tail is 7 't' lines + the empty one.
        let expected = 2 * 2 + 7 * 51 + 1 + HEAD_TAIL_MARKER_CHARS;
        assert_eq!(got, expected, "head 2 + tail 8, not a 10-line prefix");
        // A prefix model would have kept 10 x 2 = 20 chars. It does not.
        assert!(got > 300, "the long TAIL survives, so this is not a prefix");
    }

    #[test]
    fn chars_are_counted_as_chars_not_bytes() {
        // Three-byte characters. A byte-based count would report 3x here and
        // under-report every non-ASCII payload's real delivery.
        let l = Limit::Chars(10);
        assert_eq!(l.delivered_chars("日本語", Ctx::UNKNOWN), 3);
        let per_line = Limit::Lines {
            max_lines: 1,
            max_line_chars: Some(2),
        };
        assert_eq!(
            per_line.delivered_chars("日本語", Ctx::UNKNOWN),
            3,
            "2 chars + newline"
        );
    }

    #[test]
    fn unbounded_delivers_everything() {
        let payload = "z".repeat(5_000_000);
        assert_eq!(
            Limit::Unbounded.delivered_chars(&payload, Ctx::UNKNOWN),
            5_000_000
        );
        assert!(Limit::Unbounded.worst_case_chars().is_none());
        // aider is the measured case, and it must not be mistaken for unknown.
        let c = cap("aider", Action::Command).expect("aider command cap");
        assert!(c.limit.is_unbounded());
        assert!(c.provenance.is_measured(), "aider's unbounded is MEASURED");
    }

    #[test]
    fn worst_case_is_only_an_upper_bound() {
        let l = Limit::Lines {
            max_lines: 2_000,
            max_line_chars: Some(2_000),
        };
        let bound = l.worst_case_chars().unwrap();
        let real = l.delivered_chars(&"short\n".repeat(2_000), Ctx::UNKNOWN);
        assert!(
            real < bound,
            "the bound ({bound}) must never be below the measurement ({real})"
        );
    }

    #[test]
    fn a_guess_never_reads_as_a_measurement() {
        // `is_measured` must actually discriminate, so build one of each.
        let guess = peer(Limit::Chars(1), "borrowed from a peer of the same lineage");
        assert!(!guess.provenance.is_measured());
        let real = pkg(Limit::Chars(1), "some/file.js", "SOME_CONST");
        assert!(real.provenance.is_measured());

        // And the property worth holding: the table carries NO guesses today.
        // Every agent in it was read out of an installed package or measured in
        // a live session. When that stops being true, this fails and the new
        // row has to justify itself instead of blending in.
        let guesses: Vec<&str> = TABLE
            .iter()
            .filter(|(_, _, c)| !c.provenance.is_measured())
            .map(|(a, _, _)| *a)
            .collect();
        assert!(
            guesses.is_empty(),
            "these rows are guesses and must say so: {guesses:?}"
        );
        // Anti-vacuity: an empty table would satisfy the line above.
        assert!(agents().len() >= 10, "and the table is not empty");
    }

    #[test]
    fn a_refusing_cap_delivers_nothing_not_the_cap() {
        // The distinction that decides whether a credit is honest. A truncating
        // cap means the user WOULD have paid the cap; a refusing one means they
        // paid nothing and re-read in slices instead.
        let l = Limit::RefusedAbove { tokens: 10 };
        assert_eq!(
            l.delivered_chars("tiny", Ctx::UNKNOWN),
            4,
            "under: delivered"
        );
        let big = "word ".repeat(10_000);
        assert_eq!(
            l.delivered_chars(&big, Ctx::UNKNOWN),
            0,
            "over: the model gets an ERROR, not a shortened payload"
        );
        // And it must not be confused with a truncating cap of the same size.
        assert!(Limit::Tokens(10).delivered_chars(&big, Ctx::UNKNOWN) > 0);
        // claude-code is the measured case.
        let c = cap("claude-code", Action::ReadFile).expect("claude-code read");
        assert_eq!(c.limit, Limit::RefusedAbove { tokens: 25_000 });
        assert!(c.provenance.is_measured());
    }

    #[test]
    fn the_tightest_of_two_caps_wins_and_which_one_depends_on_the_data() {
        let l = Limit::Tightest(&[
            Limit::Chars(25_000),
            Limit::Lines {
                max_lines: 1_000,
                max_line_chars: None,
            },
        ]);
        // Many short lines: the LINE cap bites first (1 000 x "ab\n" = 3 000).
        let tall = "ab\n".repeat(20_000);
        assert_eq!(l.delivered_chars(&tall, Ctx::UNKNOWN), 3_000);
        // Few long lines: the CHAR cap bites first instead.
        let wide = ("z".repeat(5_000) + "\n").repeat(20);
        assert_eq!(l.delivered_chars(&wide, Ctx::UNKNOWN), 25_000);
        // Neither value could have stood in for the other.
        let q = cap("qwen", Action::ToolResult).expect("qwen tool result");
        assert!(
            q.provenance.is_measured(),
            "installed and read, not inherited"
        );
    }

    #[test]
    fn a_refusing_channels_credit_is_labelled_an_approximation() {
        // The decision is to credit the cap; the requirement is that the
        // product never present that number as a measurement.
        let b = credit_basis("claude-code", Action::ReadFile);
        assert!(b.is_approximate(), "a refusing channel cannot be measured");
        if let CreditBasis::Approximated { because } = b {
            assert!(because.len() > 40, "the reason travels with the label");
        }
        // A truncating channel IS exact arithmetic and must not be labelled.
        assert!(!credit_basis("claude-code", Action::Command).is_approximate());
        assert!(!credit_basis("gemini-cli", Action::Command).is_approximate());
        // An agent the table does not know is not silently called approximate.
        assert!(!credit_basis("does-not-exist", Action::ReadFile).is_approximate());
    }

    #[test]
    fn a_tail_rule_keeps_the_end_and_that_is_not_the_same_size() {
        let l = Limit::TailLines { max_lines: 3 };
        // Asymmetric payload: short head, long tail. A head-keeping rule would
        // return a completely different number for the same line budget.
        let mut p = String::new();
        for _ in 0..50 {
            p.push_str("s\n");
        }
        for _ in 0..3 {
            p.push_str(&("L".repeat(40) + "\n"));
        }
        let tail = l.delivered_chars(&p, Ctx::UNKNOWN);
        let head = Limit::Lines {
            max_lines: 3,
            max_line_chars: None,
        }
        .delivered_chars(&p, Ctx::UNKNOWN);
        assert_eq!(head, 6, "3 head lines of 1 char + newlines");
        // Exact, not "bigger than": a first assertion here said only
        // `tail > head * 10`, and a mutation that ignored `max_lines` entirely
        // still satisfied it — the whole payload is also bigger than the head.
        // The last 3 slots are two 40-char lines and the empty slot a trailing
        // newline leaves, each re-joined with a newline.
        assert_eq!(tail, 41 + 41 + 1, "the last three slots, and only those");
        assert!(tail > head * 10, "and the tail dwarfs the head here");
        assert_eq!(p.chars().count(), 223, "so 'kept everything' would be 223");
        // antigravity is the measured case: 200 lines, from the end.
        let c = cap("antigravity", Action::Command).expect("antigravity command");
        assert_eq!(c.limit, Limit::TailLines { max_lines: 200 });
    }

    #[test]
    fn a_character_budget_ignores_how_the_payload_is_split_into_lines() {
        // The property that made grok's shape conclusive: two payloads with the
        // SAME total size but different line lengths must deliver the same
        // number of characters. A line-based rule would not.
        let l = Limit::HeadTailChars {
            threshold_chars: 40_000,
            keep_chars: 20_000,
        };
        let short_lines = "0000000\n".repeat(12_500); // 100 000 chars, 8/line
        let long_lines = ("x".repeat(99) + "\n").repeat(1_000); // 100 000, 100/line
        assert_eq!(short_lines.chars().count(), long_lines.chars().count());
        let a = l.delivered_chars(&short_lines, Ctx::UNKNOWN);
        let b = l.delivered_chars(&long_lines, Ctx::UNKNOWN);
        assert_eq!(a, b, "a CHARACTER budget cannot depend on line shape");
        assert_eq!(a, 20_000 + HEAD_TAIL_MARKER_CHARS);
        // And a line-based rule on the same pair does NOT agree — which is what
        // the second live run was for.
        let by_lines = Limit::HeadTail {
            threshold_chars: 40_000,
            total_lines: 200,
            wrap_width: None,
        };
        assert_ne!(
            by_lines.delivered_chars(&short_lines, Ctx::UNKNOWN),
            by_lines.delivered_chars(&long_lines, Ctx::UNKNOWN),
            "anti-vacuity: the two payloads must be able to tell the rules apart"
        );
        // Under the threshold nothing is cut.
        assert_eq!(l.delivered_chars("small", Ctx::UNKNOWN), 5);
    }

    #[test]
    fn windsurf_read_needs_both_halves_of_its_rule() {
        // The devin CLI states a total budget AND a per-line one in the same
        // sentence, and each is the binding one on a different payload shape.
        // Two payloads, each chosen so that exactly one half decides.
        let c = cap("windsurf", Action::ReadFile).expect("windsurf read");

        // Five 5 000-char lines: the per-line rule cuts each to 2 000, so
        // 5 x 2001 = 10 005 arrives — well under the 20 000 total.
        let few_long = ("x".repeat(5_000) + "\n").repeat(5);
        assert_eq!(
            c.limit.delivered_chars(&few_long, Ctx::UNKNOWN),
            10_005,
            "the per-line half must bite when the lines are long"
        );

        // Four hundred 100-char lines: no line is over 2 000, so only the
        // 20 000-char total can cut the 40 400 on offer.
        let many_short = ("y".repeat(99) + "\n").repeat(400);
        assert_eq!(
            c.limit.delivered_chars(&many_short, Ctx::UNKNOWN),
            20_000,
            "the total half must bite when the lines are short"
        );

        // Anti-vacuity: the two payloads genuinely separate the halves, so a
        // rule holding only one of them fails one of the assertions above.
        assert_ne!(few_long.chars().count(), 10_005);
        assert!(many_short.chars().count() > 20_000);
    }

    #[test]
    fn an_unknown_agent_inherits_nobody_elses_number() {
        assert!(cap("does-not-exist", Action::ReadFile).is_none());
        assert!(delivered_chars("does-not-exist", Action::Command, "x", Ctx::UNKNOWN).is_none());
    }

    #[test]
    fn gemini_read_of_a_real_shaped_file_is_computed_not_assumed() {
        // 3 000 lines of 3 000 chars: BOTH caps bite at once.
        let payload = ("q".repeat(3_000) + "\n").repeat(3_000);
        let got = delivered_chars("gemini-cli", Action::ReadFile, &payload, Ctx::UNKNOWN).unwrap();
        assert_eq!(got, 2_000 * 2_001, "2 000 lines, each cut to 2 000 + \\n");
    }
}
#[cfg(test)]
mod the_host_cut_is_marked_nowhere {
    use super::{cap, host_cut_advisory, Action, Limit};

    #[test]
    fn an_over_cap_command_result_is_named() {
        let note = host_cut_advisory("claude-code", Action::Command, &"x".repeat(40_000))
            .expect("40 000 chars is over the 30 000 cap");
        assert!(note.contains("30000"), "{note}");
        assert!(note.contains("10000"), "{note}");
    }

    #[test]
    fn a_result_that_fits_says_nothing() {
        assert!(host_cut_advisory("claude-code", Action::Command, &"x".repeat(1_000)).is_none());
    }

    #[test]
    fn empty_output_says_nothing() {
        assert!(host_cut_advisory("claude-code", Action::Command, "").is_none());
    }

    /// An agent with no measured cap on this channel must stay silent rather
    /// than borrow a peer's number.
    #[test]
    fn an_unmeasured_agent_says_nothing() {
        assert!(host_cut_advisory("not-an-agent", Action::Command, &"x".repeat(40_000)).is_none());
    }

    /// The figure comes from `TABLE`, never from a local constant, so the note
    /// and the credit accounting cannot drift apart.
    #[test]
    fn the_number_comes_from_the_table() {
        let c = cap("claude-code", Action::Command).expect("measured Command cap");
        assert_eq!(c.limit, Limit::Chars(30_000));
    }
}
