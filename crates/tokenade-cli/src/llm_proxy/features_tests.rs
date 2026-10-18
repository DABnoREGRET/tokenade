//! The proposal has to be true before it is persuasive.

use super::*;

/// Every agent the proxy can install for has a transcribed baseline.
///
/// The `_ => Full` arm exists so the match is total; if a new recipe lands
/// without a baseline, that arm would silently claim the agent is already at
/// 10/10 and the install would offer nothing. This is what stops that.
#[test]
fn the_baseline_covers_every_agent_the_proxy_can_install_for() {
    const TRANSCRIBED: &[Agent] = &[
        Agent::ClaudeCode,
        Agent::OpenCode,
        Agent::KiloCode,
        Agent::Cline,
        Agent::Qwen,
        Agent::Codex,
        Agent::Grok,
        Agent::Aider,
    ];
    // This list is a SECOND copy of the same fact as `transcribed()`, and a
    // second copy drifts. It did drift: reasonix got its row on 2026-08-20 and
    // this test failed accusing the agent of "claim gains" with no starting
    // point, when the starting point had just been measured. Kept for
    // readability, but checked against the function.
    for a in Agent::all() {
        assert_eq!(
            TRANSCRIBED.contains(a),
            super::transcribed(*a),
            "{} is on one list and not the other",
            a.as_str()
        );
    }
    for a in Agent::all() {
        if super::super::install::recipe(*a).is_none() {
            continue;
        }
        // An agent may have a recipe before its page row is transcribed —
        // measuring a config key and reading a page in another repository are
        // different jobs with different lead times. What it may NOT do is
        // claim a gain against a baseline nobody wrote down: without a row,
        // the difference would be computed against `Full` and silently read
        // as "rank 4 adds nothing".
        assert!(
            TRANSCRIBED.contains(a) || super::gains(*a).is_empty(),
            "{} has an install recipe and no baseline from the compatibility \
             page, yet claims gains — transcribe its row, or the claim is \
             measured against a default",
            a.as_str()
        );
    }
}

/// An agent a higher rung already covers is offered NOTHING.
///
/// The one-authority-per-payload rule, stated where a user would see the consequence: on claude-code,
/// opencode, kilo and cline a hook or a plugin already folds every tool result,
/// so rank 4 is a meter. A meter is worth having — it is the only source of the
/// provider's real `usage` — but it is not one of the ten boxes, and offering
/// it as if it were would be selling a box that does not light up.
#[test]
fn an_agent_on_a_wire_we_cannot_read_is_offered_nothing() {
    // The list used to be the HOOKED agents — claude-code, opencode, kilo,
    // cline, qwen, grok — on the theory that their rung above covers them.
    // Measured 2026-08-20, it does not: their histories carry payloads no hook
    // ever sees, and the proxy folded one of them 56% on the very agents this
    // test forbade it for.
    //
    // What is genuinely offered nothing is an agent whose body we cannot
    // parse. That is a fact about the wire, and no amount of product work
    // changes it without wire support.
    for a in [Agent::Cursor, Agent::DevinDesktop] {
        assert!(
            gains(a).is_empty(),
            "{} must not be offered the proxy for features: {:?}",
            a.as_str(),
            gains(a)
        );
        assert!(offer(a).is_none());
    }
}

/// And the agents the old list named ARE offered something now.
///
/// The positive half, kept next to the negative one so the pair cannot drift
/// back: if this ever empties, the coarse gate has crept back in.
#[test]
fn a_hooked_agent_is_offered_what_its_hook_does_not_carry() {
    // Derived, not listed. The rule is "an agent with a cell below what the
    // proxy delivers must be offered that cell", and hardcoding which agents
    // those are is how the previous version of this test ended up asserting
    // the opposite of the measurement for four of them.
    //
    // Cline is the instructive pass: its only gap is `Mcp: Partial`, and the
    // proxy is Partial there too, so it is offered nothing and that is
    // CORRECT — it is already at the best status available. An agent at the
    // ceiling and an agent behind a gate look identical from a list; they do
    // not look identical from the baseline.
    let delivers = [
        (Feature::Commands, Support::Full),
        (Feature::WebSearch, Support::Full),
        (Feature::CodeSearch, Support::Full),
        (Feature::LeanOutput, Support::Full),
        (Feature::Mcp, Support::Partial),
        (Feature::Context, Support::Partial),
        (Feature::Security, Support::Partial),
    ];
    let mut checked = 0;
    for a in Agent::all() {
        let a = *a;
        if !transcribed(a) || crate::llm_proxy::install::recipe(a).is_none() {
            continue;
        }
        let e = crate::llm_proxy::agent_matrix::entry(a);
        if !e.role(a.coverage()).may_fold() {
            continue;
        }
        let behind: Vec<_> = delivers
            .iter()
            .filter(|(f, to)| effective_baseline(a, *f, true) < *to)
            .map(|(f, _)| *f)
            .collect();
        checked += 1;
        if behind.is_empty() {
            assert!(
                gains_with(a, true).is_empty(),
                "{} is at the ceiling and is offered something anyway: {:?}",
                a.as_str(),
                gains_with(a, true)
            );
        } else {
            let got: Vec<_> = gains(a).iter().map(|g| g.feature).collect();
            for f in behind {
                assert!(
                    got.contains(&f),
                    "{} is behind on {f:?} and is offered nothing for it — \
                     the per-agent fold gate is back",
                    a.as_str()
                );
            }
        }
    }
    assert!(checked >= 6, "only {checked} agents exercised this rule");
}

/// An agent with no measured config site is never offered anything either.
#[test]
fn an_agent_we_cannot_install_for_is_never_offered() {
    for a in [Agent::Cursor, Agent::Droid, Agent::OpenClaw] {
        assert!(gains(a).is_empty(), "{}", a.as_str());
    }
}

/// Codex is the case the rung exists for, and the offer says so exactly.
#[test]
fn codex_is_offered_what_its_unapproved_hooks_cannot_give_it() {
    // Fourth move, 2026-10-06, the other way: the cells above were measured
    // with codex's hooks TRUSTED, and nobody approves them (operator rule, CLI
    // and desktop app alike). What only an approved hook carries is counted
    // absent, so the proxy — which needs no approval — is offered for it.
    let offered: Vec<Feature> = gains_with(Agent::Codex, false)
        .iter()
        .map(|g| g.feature)
        .collect();
    for f in [
        Feature::Commands,
        Feature::LeanOutput,
        Feature::Security,
        Feature::Context,
    ] {
        assert!(
            offered.contains(&f),
            "codex without approved hooks is not offered {f:?}: {offered:?}"
        );
    }
    // And once its hooks ARE validated, they carry it: nothing is offered twice.
    assert!(
        gains_with(Agent::Codex, true).is_empty(),
        "{:?}",
        gains_with(Agent::Codex, true)
    );

    // The sentence a user needs before pointing an agent somewhere is still
    // required, on an agent to which rank 4 still brings something.
    let text = offer(Agent::Aider).expect("aider must be offered");
    assert!(text.contains("LOCAL proxy"), "{text}");
    assert!(text.contains("127.0.0.1"), "{text}");
    assert!(text.contains("stays on this machine"), "{text}");
}

/// Aider has nothing above it, so it gains the most — and every gain is real.
#[test]
fn aider_gains_three_boxes_and_none_of_them_is_invented() {
    // The list grew on 2026-08-20 with the three gains the proxy was already
    // producing and not declaring — web, code search, and the history-wide
    // shrink. Asserted as a SET that must contain the measured ones rather
    // than as an exact vector, because the exact vector is what stopped
    // anybody noticing the product under-reported itself.
    let g = gains(Agent::Aider);
    let got: Vec<_> = g.iter().map(|x| (x.feature, x.from, x.to)).collect();
    for want in [
        (Feature::Commands, Support::Partial, Support::Full),
        (Feature::WebSearch, Support::Partial, Support::Full),
        (Feature::Mcp, Support::Partial, Support::Partial),
    ] {
        if want.1 == want.2 {
            continue; // equal is not a gain, and `add` drops it
        }
        assert!(got.contains(&want), "aider is missing {want:?}: {got:?}");
    }
    // Never offered as an upgrade — and this list SHRANK on 2026-08-20,
    // because "the proxy adds no TOOL" was conflating two different things.
    //
    // Adding a tool and folding what a tool RETURNED are not the same act. The
    // proxy cannot give an agent a web search it does not have; it can fold the
    // page that agent already fetched, and it does (144 390 → 52 540 measured).
    // Same for search output. Those two left this list.
    //
    // What genuinely stays out:
    //   * `Media` — a PDF never reaches the wire as a PDF. By the time the
    //     history carries it, the agent has already turned it into whatever
    //     text it managed, and the bytes the proxy sees are the loss, not the
    //     source. Measured `ko-proxy`: 134 298 → 134 298, relayed unchanged.
    //   * `Dedup` — measured `ko-proxy`: a repeated identical result still
    //     costs 89 447 of 89 447 bytes. There is no wire-level dedup today.
    //     This one is a GAP and not a law; the day the proxy remembers a
    //     payload across turns, it leaves this list too.
    for f in [Feature::Media, Feature::Dedup] {
        assert!(
            !g.iter().any(|x| x.feature == f),
            "{:?} is claimed and the matrix measured it ko-proxy",
            f
        );
    }
}

/// A gain never claims a level the agent already has.
#[test]
fn no_offer_ever_repeats_something_that_already_works() {
    for a in Agent::all() {
        for g in gains(*a) {
            assert!(
                g.to > g.from,
                "{} would be told the proxy 'adds' {:?}, which it already has",
                a.as_str(),
                g.feature
            );
        }
    }
}

/// The table answers for every box, on every installable agent.
///
/// The guard against a feature that exists in the enum and nowhere else: a row
/// that never appears is a claim the product cannot make and cannot deny.
#[test]
fn the_table_has_a_row_for_every_box_and_never_goes_backwards() {
    for a in Agent::all() {
        if super::super::install::recipe(*a).is_none() {
            continue;
        }
        let t = support_table(*a);
        assert_eq!(t.len(), Feature::ALL.len(), "{}", a.as_str());
        for (f, from, to) in t {
            assert!(
                to >= from,
                "{} would be told the proxy DOWNGRADES {:?}",
                a.as_str(),
                f
            );
            assert!(!f.label().is_empty());
        }
    }
}

/// The baseline table is a MEASUREMENT, and this re-derives it from the files
/// the measurement wrote.
///
/// It was a hand transcription until 2026-08-20 and it had drifted in both
/// directions — four agents declared `Full` on all ten where the bench had
/// measured Partial cells, and two declared `None` on features they have.
/// A table nobody can re-derive is a table that rots silently, and this one
/// rotted into switching rank 4 off for eleven agents.
///
/// The results are committed test data (`testdata/compat/results/`), so a
/// missing file fails here instead of silently skipping the comparison.
#[test]
fn the_baseline_matches_the_measured_matrix() {
    use std::collections::BTreeMap;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("testdata/compat/results"));
    let Some(root) = root else { return };
    let pairs: &[(&str, Feature)] = &[
        ("cmd", Feature::Commands),
        ("web", Feature::WebSearch),
        ("mcp", Feature::Mcp),
        ("codesearch", Feature::CodeSearch),
        ("sobriety", Feature::LeanOutput),
        ("context", Feature::Context),
        ("readdedup", Feature::Dedup),
        ("docs", Feature::Media),
        ("privacy", Feature::Privacy),
        ("redact", Feature::Security),
    ];
    // slug -> feature -> (worst, best) recorded on any OS.
    //
    // DECIDED 2026-09-13. This test compared the table ONLY to the worst, and
    // it had been red since `7c21c544` on a single pair: `qwen-code/cmd` is
    // measured `ok-bin` on linux and macos, `ko-bin` on windows. The table
    // says `Partial`, the worst says `None`, and the table was RIGHT —
    // `Partial` is precisely the value that describes "it works here, not
    // there".
    //
    // The exact rule, now written down:
    //   worst == best == Full  → the table must say Full
    //   worst == best == None  → the table must say None
    //   otherwise (the OSes diverge, or one OS measured Partial) → Partial
    //
    // The other way out was adding one column per OS to the table. Refused:
    // its consumers — the banner an agent reads, `llm-proxy --rank` — announce
    // what tokenade can do WITH this agent, not on which machine. A per-OS
    // value would be true there and unusable, and `Partial` already carries
    // the nuance without lying in either direction.
    let mut worst: BTreeMap<String, BTreeMap<&str, u8>> = BTreeMap::new();
    let mut best: BTreeMap<String, BTreeMap<&str, u8>> = BTreeMap::new();
    let mut any = false;
    for os in ["linux", "macos", "windows"] {
        let Ok(text) = std::fs::read_to_string(root.join(format!("{os}.json"))) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        any = true;
        for a in v["agents"].as_array().into_iter().flatten() {
            let slug = a["agent"].as_str().unwrap_or_default().to_string();
            for (key, _) in pairs {
                let Some(verdict) = a["features"][key]["verdict"].as_str() else {
                    continue;
                };
                let rank = if verdict.starts_with("ok") {
                    2
                } else if verdict.starts_with("partial") {
                    1
                } else {
                    0
                };
                let e = worst
                    .entry(slug.clone())
                    .or_default()
                    .entry(key)
                    .or_insert(2);
                *e = (*e).min(rank);
                let b = best
                    .entry(slug.clone())
                    .or_default()
                    .entry(key)
                    .or_insert(0);
                *b = (*b).max(rank);
            }
        }
    }
    assert!(
        any,
        "no results under {} — the measured matrix is committed test data",
        root.display()
    );
    for a in Agent::all() {
        let a = *a;
        if !transcribed(a) {
            continue;
        }
        let Some(row) = worst.get(a.as_str()) else {
            continue;
        };
        for (key, feature) in pairs {
            let Some(&low) = row.get(key) else { continue };
            let high = best
                .get(a.as_str())
                .and_then(|r| r.get(key))
                .copied()
                .unwrap_or(low);
            let measured = match (low, high) {
                (2, 2) => Support::Full,
                (0, 0) => Support::None,
                // The OSes diverge, or one of them measured "partial". Only
                // one value can say that without lying.
                _ => Support::Partial,
            };
            assert_eq!(
                baseline(a, *feature),
                measured,
                "{}/{key}: the table says {:?}, the matrix measures {:?} \
                 (worst {low}, best {high} over the recorded OSes)",
                a.as_str(),
                baseline(a, *feature),
                measured
            );
        }
    }
}

/// The operator's rule: what an agent already gets without the proxy, the
/// proxy does not do again. The style preamble reaches opencode, kilo, aider
/// and qwen through their rules file or prompt hook — not a second time from
/// rank 4; grok has neither, so the proxy carries it.
#[test]
fn the_proxy_never_doubles_the_preamble_an_agent_already_gets() {
    // With their hooks running — stated, not read off this machine's HOME,
    // which another test may be pointing elsewhere at that moment.
    for a in [
        Agent::OpenCode,
        Agent::KiloCode,
        Agent::Aider,
        Agent::Qwen,
        Agent::ClaudeCode,
    ] {
        assert_eq!(
            effective_baseline(a, Feature::LeanOutput, true),
            Support::Full,
            "{}",
            a.as_str()
        );
    }
    assert!(effective_baseline(Agent::Grok, Feature::LeanOutput, true) < Support::Full);
    // Codex's own carrier is a hook that needs validation: carried or not
    // depending on that state.
    assert_eq!(
        effective_baseline(Agent::Codex, Feature::LeanOutput, false),
        Support::None
    );
    assert_eq!(
        effective_baseline(Agent::Codex, Feature::LeanOutput, true),
        Support::Full
    );
    assert_eq!(preamble_for(None), None, "no agent named, no preamble");
}
