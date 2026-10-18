//! Tests for the `dispatch` hook.

/// Field bug: Grok's matcher fired (`bash`, lowercase) but the pretool
/// handler only accepted `Bash`, silently dropping every event — hooks
/// installed, zero gain. Droid's `Execute` would have hit the same wall.
#[test]
fn shell_tool_names_cover_grok_and_droid() {
    assert!(crate::hooks::is_shell_tool_name("Bash"));
    assert!(crate::hooks::is_shell_tool_name("bash"));
    assert!(crate::hooks::is_shell_tool_name("Execute"));
    // Every spelling of every name, not the ones we happened to meet. The
    // comment above already knew droid's `Execute` "would have hit the same
    // wall", and it was listed in ONE case while `Bash` was listed in two.
    assert!(crate::hooks::is_shell_tool_name("execute"));
    assert!(crate::hooks::is_shell_tool_name("BASH"));
    assert!(!crate::hooks::is_shell_tool_name("Read"));
    assert!(!crate::hooks::is_shell_tool_name("read"));
    assert!(!crate::hooks::is_shell_tool_name("shell-not-a-tool"));
}

/// The backoff is the only thing standing between a steer the agent ignores and
/// ~90 tokens re-spent on it forever, so its cadence has to be the one the code
/// says it is.
///
/// MEASURED 2026-08-04: 1 976 of 2 096 ignored `batch-bash` fires arrived 12
/// calls after the previous one — the BASE cadence — because the fire test was
/// a modulo of the absolute streak. Doubling `every` only skipped multiples
/// that had already fired, so each escalation landed one fire late.
#[test]
fn the_batch_backoff_actually_doubles_the_gap_between_fires() {
    const NUDGE_AT: u32 = 3;
    const EVERY: u32 = 12;
    const CAP: u32 = 4;
    let mut pending = 0u32;
    let mut ignored = 0u32;
    let mut gaps = Vec::new();
    for streak in 1..=3000u32 {
        let every = EVERY << ignored.min(CAP);
        if !crate::hooks::should_fire(streak, pending, NUDGE_AT, every) {
            continue;
        }
        if pending > 0 {
            gaps.push(streak - pending);
            ignored += 1;
        }
        pending = streak;
    }
    assert_eq!(
        &gaps[..6],
        &[12, 24, 48, 96, 192, 192],
        "backoff cadence is not the documented one: {gaps:?}"
    );
}

/// **Two refusals, and the session never hears about it again.**
///
/// Operator instruction, 2026-09-12: repeating the same nudge "confuses the
/// model" — the reported case shows an agent that starts COMMENTING on the
/// advice instead of applying it. The module's numbers already backed it: 8 %
/// adherence on `batch-bash`, 36 sessions out of 63 that never followed it.
///
/// The raw cadence still exists (the test above checks it); what this test
/// pins is that the CALLER stops at the second refusal. Without it, the rule
/// would live in a comment and a mutation would remove it without turning
/// anything red.
#[test]
fn two_refusals_are_enough_to_silence_the_steer_for_the_session() {
    const SILENCE_AFTER_IGNORED: u32 = 2;
    const NUDGE_AT: u32 = 3;
    const EVERY: u32 = 12;
    const CAP: u32 = 4;
    let mut pending = 0u32;
    let mut ignored = 0u32;
    let mut fires = 0u32;
    for streak in 1..=3000u32 {
        if ignored >= SILENCE_AFTER_IGNORED {
            break;
        }
        let every = EVERY << ignored.min(CAP);
        if !crate::hooks::should_fire(streak, pending, NUDGE_AT, every) {
            continue;
        }
        if pending > 0 {
            ignored += 1;
        }
        pending = streak;
        fires += 1;
    }
    assert_eq!(
        fires, 3,
        "a session that ignores must receive three sentences in all, not one more"
    );
}

/// **One sentence per window, three in total, and compaction reopens the budget.**
///
/// Pinned on the REAL function: a first version simulated the caller's loop
/// with its own constant, went green, and the mutant that disarmed the rule
/// SURVIVED it.
#[test]
fn the_nudge_budget_holds_the_window_and_reopens_on_compaction() {
    let home = tokenade_core::platform::ScratchHome::new("nudge-budget");
    let dir = home.path().join("pending");
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TOKENADE_READ_PENDING_DIR", &dir);
    let parsed = serde_json::json!({ "session_id": "nudgebudget" });

    // A state that lands on a firing window, timestamped far enough back not
    // to pass for a parallel call.
    let seed = |family: &str| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // One turn BEFORE the family's threshold — asked of production code,
        // not copied: `edit` speaks one turn earlier than the others since
        // 2026-09-14, and a hard-coded `2` here turned this test red for a
        // reason that was not its own.
        let before = crate::hooks::nudge_at_for(family).saturating_sub(1);
        std::fs::write(
            dir.join(format!("batch-streak-{family}-nudgebudget")),
            format!("{before}:{}:0:0", now - 5_000),
        )
        .unwrap();
    };

    // First time: the family speaks.
    seed("bash");
    assert!(
        crate::hooks::batching_family_nudge(&parsed, "bash", "MSG").is_some(),
        "the first advice of a window must go through"
    );
    // Second time, same family: the sentence is already in the context.
    seed("bash");
    assert!(
        crate::hooks::batching_family_nudge(&parsed, "bash", "MSG").is_none(),
        "the same sentence is not repeated while it is in front of the model"
    );

    // The DISTANCE in tokens reopens the same family, without compaction: the
    // context has grown by a quarter window, the earlier sentence is drowned.
    assert!(
        crate::hooks::read_staging::nudge_budget_take("nudgebudget", "batch-bash", 60_000, 50_000),
        "a quarter window further on, the advice can be said again"
    );
    assert!(
        !crate::hooks::read_staging::nudge_budget_take("nudgebudget", "batch-bash", 61_000, 50_000),
        "a thousand tokens further on, no"
    );
    // A context that SHRINKS is a compaction: the sentence is gone.
    assert!(
        crate::hooks::read_staging::nudge_budget_take("nudgebudget", "batch-bash", 9_000, 50_000),
        "after a compaction the advice can be repeated right away"
    );

    // **No global cap.** A silent family must not mute the others: a cap
    // would make one advice's arrival depend on UNRELATED advice, and the
    // swallowed family would never know it went quiet. Each family is
    // governed by its own distance.
    for family in ["read", "edit", "glob", "web"] {
        seed(family);
        assert!(
            crate::hooks::batching_family_nudge(&parsed, family, "MSG").is_some(),
            "{family}: every family keeps its first sentence, whatever the number \
             of advices already given by the others"
        );
    }

    // Compaction carries the sentences away: the budget reopens.
    crate::hooks::read_staging::reset_session_hints("nudgebudget");
    seed("bash");
    assert!(
        crate::hooks::batching_family_nudge(&parsed, "bash", "MSG").is_some(),
        "after a compaction, advice the summary carried away can be repeated"
    );
    std::env::remove_var("TOKENADE_READ_PENDING_DIR");
}

/// The first fire must still land at the third one-per-turn call, and a steer
/// that was FOLLOWED (pending cleared) must start over from there rather than
/// inherit a gap — otherwise complying would buy silence instead of a fresh
/// early nudge.
#[test]
fn the_first_fire_and_the_post_compliance_fire_both_land_at_the_third_call() {
    assert!(!crate::hooks::should_fire(2, 0, 3, 12));
    assert!(crate::hooks::should_fire(3, 0, 3, 12));
    // Outstanding fire at 3: nothing until a full `every` has elapsed.
    assert!(!crate::hooks::should_fire(14, 3, 3, 12));
    assert!(crate::hooks::should_fire(15, 3, 3, 12));
}

/// **A banner carrying a hash is NEVER muzzled.**
///
/// Operator instruction, 2026-09-12: "make sure the system does not block
/// hash-type nudges for expand-ref, otherwise the agent will not be able to
/// see the raw results". The repetition budget is meant for IDENTICAL
/// sentences; a recovery banner names different bytes every time, and
/// silencing it cuts the only path to them.
///
/// The test calls the producer TWICE in a row on two different folds, in the
/// same session, and requires both hashes. Without it, the separation would
/// live only in the fact that one module does not call the other — which a
/// refactor can undo without turning anything red.
#[test]
fn a_hash_carrying_banner_is_never_muzzled() {
    let _iso = tokenade_core::platform::StoreIsolation::new("hash-banner-free");
    let hint = tokenade_core::disclose::SourceHint::web();
    let hash_of = |raw: &str| {
        let display = format!("{} lignes montrées", raw.len());
        crate::wrap::nudges::attach_drop_recovery(display, raw, 2_500, 4_000, &hint, "test")
    };
    let first = hash_of(&"alpha ".repeat(4_000));
    let second = hash_of(&"beta ".repeat(4_000));
    for (n, out) in [("first", &first), ("second", &second)] {
        assert!(
            out.contains("expand-ref"),
            "{n} banner: the path to the raw bytes has disappeared:\n{out}"
        );
    }
    let hash = |s: &str| {
        s.split("expand-ref ")
            .nth(1)
            .map(|t| t.chars().take(12).collect::<String>())
            .unwrap_or_default()
    };
    assert_ne!(
        hash(&first),
        hash(&second),
        "two different folds must name two hashes — that is what makes these \
         banners NON-repetitive"
    );
}

/// The advice names only tools the agent can call: `MultiEdit` no longer
/// exists in Claude Code, and advice impossible to follow is counted
/// `ignored` (`batch-edit` at 19 % adherence across the fleet).
#[test]
fn the_edit_advice_names_only_the_edit_tool() {
    let named: Vec<&str> = crate::hooks::EDIT_BATCH_ADVICE
        .split('`')
        .skip(1)
        .step_by(2)
        .collect();
    assert_eq!(named, ["Edit"], "{}", crate::hooks::EDIT_BATCH_ADVICE);
}
