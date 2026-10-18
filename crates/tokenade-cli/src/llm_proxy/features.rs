//! What the product claims per agent, in the site's own vocabulary — and what
//! rank 4 adds to it.
//!
//! `tokenade install` has to be able to say, in words a user recognises, "on
//! this agent the proxy buys you secret redaction, which you cannot have any
//! other way". That sentence needs two things this crate did not have: the
//! feature NAMES the marketing page uses, and the per-agent baseline it shows.
//!
//! Both are declared here rather than derived, because both are claims made to
//! users and a claim has to be editable in one place. The baseline is
//! transcribed from the published matrix (`tokenade.net`, the compatibility
//! section) on 2026-08-18; the DELTA is derived from
//! [`super::agent_matrix`], which is measured. So the honest split is: the
//! baseline is what we say, the delta is what we know.
//!
//! **If the website and this file disagree, one of them is lying to a user.**
//! `the_baseline_covers_every_agent_the_proxy_can_install_for` is what stops
//! the drift going unnoticed on our side; the site has to import from here, or
//! be regenerated from it, for the other direction. Flagged rather than solved:
//! the page lives in another repository.

use tokenade_core::detect::Agent;

/// The ten features the compatibility page is divided into.
///
/// Order and wording follow the page exactly (`commands`, `websearch`, `mcp`,
/// `codesearch`, `lean`, `context`, `dedup`, `media`, `privacy`, `security`) so
/// a user reading the install prompt and the site sees the same words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Commands,
    WebSearch,
    Mcp,
    CodeSearch,
    LeanOutput,
    Context,
    Dedup,
    Media,
    Privacy,
    Security,
}

impl Feature {
    /// Every box on the page, in the page's order.
    ///
    /// Not decoration: `support_table` walks it, so a feature added to the enum
    /// and forgotten everywhere else shows up as a row with no answer rather
    /// than as nothing at all. It is also what makes the enum HONEST to the
    /// compiler — six of these ten are only ever matched on, never built, and
    /// `-D warnings` is right to call that dead until something enumerates
    /// them.
    pub const ALL: &'static [Feature] = &[
        Feature::Commands,
        Feature::WebSearch,
        Feature::Mcp,
        Feature::CodeSearch,
        Feature::LeanOutput,
        Feature::Context,
        Feature::Dedup,
        Feature::Media,
        Feature::Privacy,
        Feature::Security,
    ];

    /// The label the site prints.
    pub fn label(self) -> &'static str {
        match self {
            Self::Commands => "Command compaction",
            Self::WebSearch => "Web search",
            Self::Mcp => "MCP optimization",
            Self::CodeSearch => "Code search",
            Self::LeanOutput => "Lean output",
            Self::Context => "Context optimization",
            Self::Dedup => "Re-read dedup",
            Self::Media => "Documents & media",
            Self::Privacy => "Privacy",
            Self::Security => "Secret redaction",
        }
    }
}

/// How well one feature works on one agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Support {
    None,
    Partial,
    Full,
}

impl Support {
    /// The three points of the scale, low to high.
    ///
    /// Exists for the same reason [`Feature::ALL`] does. `None` stopped being
    /// constructed on 2026-08-20, when `baseline` was rebuilt from the measured
    /// matrix and no agent came back `None` on any box — and `-D warnings` is
    /// right that a variant nothing builds is dead. It is not: it is the bottom
    /// of an ORDERED scale that `to > from` compares against, and `describe`
    /// still renders it. Deleting it would change what "better" means.
    pub const ALL: &'static [Support] = &[Support::None, Support::Partial, Support::Full];

    /// The one-word form the support table prints.
    pub fn mark(self) -> &'static str {
        match self {
            Support::None => "—",
            Support::Partial => "partial",
            Support::Full => "yes",
        }
    }

    /// The legend for that table, built from the scale itself.
    ///
    /// Printed rather than hardcoded so a reader can tell `—` from a missing
    /// measurement, which are different answers and looked identical.
    pub fn legend() -> String {
        Support::ALL
            .iter()
            .map(|s| {
                let what = match s {
                    Support::None => "not supported",
                    Support::Partial => "partly",
                    Support::Full => "supported",
                };
                format!("{} = {what}", s.mark())
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// One feature the proxy would add or improve on this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gain {
    pub feature: Feature,
    pub from: Support,
    pub to: Support,
    /// Why, in one line, for the install prompt.
    pub because: &'static str,
}

/// The published baseline, transcribed 2026-08-18.
///
/// Only the agents rank 4 can install for are listed: a baseline for an agent
/// we cannot reach would be a fact nobody can act on, and keeping it in sync
/// would be work with no reader. `recipe()` is the gate, and a test pins the
/// two lists together.
fn baseline(agent: Agent, f: Feature) -> Support {
    use Feature::*;
    use Support::*;
    // MEASURED, not transcribed. Every row below is the WORST verdict that
    // agent's cell carries across Linux, macOS and Windows in
    // `testdata/compat/results/*.json`, and `the_baseline_matches_the_measured_matrix`
    // re-derives it from those files rather than trusting this table.
    //
    // It was a hand transcription off a page until 2026-08-20, and it was
    // wrong in both directions. It declared claude-code, opencode, kilo-code
    // and cline `Full` on all ten — so `gains()` computed no gain and the
    // product told those users the proxy adds nothing — while the bench had
    // measured `web` Partial on claude-code and `mcp`/`sobriety`/`context`
    // Partial on opencode and kilo. In the other direction it declared codex
    // and grok `None` on Dedup and Security, which they have measured Full.
    //
    // That table was the deepest of the three layers switching rank 4 off. The
    // other two were a per-agent fold gate in `agent_matrix::higher_rung` and
    // the `may_fold` check just below.
    match agent {
        // `WebSearch` moved Partial → Full on 2026-08-21: the `PreToolUse`
        // hook on WebFetch fetches and serves a scoped excerpt INSIDE the agent
        // (1 019 430 → 8 294 bytes). It was measured Partial because the bench
        // used a prompt with no lexical hook, the one shape the gate declines
        // on purpose.
        Agent::ClaudeCode => Full,
        // `LeanOutput` moved from Partial to Full on 2026-08-21: the style
        // preamble is now INSIDE the rules file these agents read every
        // session, so it reaches the model without the proxy having to add
        // it. Re-derived from the bench, not transcribed by hand.
        Agent::OpenCode | Agent::KiloCode => match f {
            Context => Partial,
            _ => Full,
        },
        Agent::Cline => Full,
        // Re-derived from the bench on 2026-09-12, and it is a DOWNGRADE:
        // grok and reasonix do not have the in-agent channel we credited them
        // with. Grok discovers its hooks somewhere other than where we wrote
        // them, its `PreToolUse` only has `allow`/`deny` (no `updatedInput`)
        // and its `PostToolUse` has its output ignored; reasonix says itself
        // in its docs that `updatedInput` is not applied. What they cannot do
        // internally, the proxy does — that is exactly what this table is for
        // quantifying, and declaring them Full erased the contribution of
        // rank 4.
        //
        // **Qwen went back up to Full the same day, by the same bench.** It
        // had been declared Partial that morning on a wrong measurement: its
        // hook entries were written under Gemini's names (`BeforeTool`), which
        // its host does not know — live file, dead entries. Written under
        // Claude Code's names (`PreToolUse` / `SessionStart` /
        // `UserPromptSubmit`), they carry: command rewritten at `PreToolUse`
        // (33 724 → 556 B), `WebFetch` fetched and folded inside the agent
        // (1 023 004 → 21 B with network), a DOCX read routed by the hook,
        // `SessionStart` for context, style preamble at `UserPromptSubmit`,
        // and a sub-agent's prompt prefixed. 20 cells out of 23 green
        // excluding network.
        //
        // `Commands` stays Partial, and for a PLATFORM reason: the test below
        // takes the WORST of the three passes, and on Windows these hosts
        // launch `run_shell_command` through `cmd.exe`, which does not read
        // the POSIX quoting of our rewrite. The hook therefore steps aside on
        // purpose there (`cmd_gemini_hook`) rather than breaking the command —
        // so no in-agent folding of commands on Windows, and rank 4 carries
        // it. That is exactly what this table must quantify.
        Agent::Qwen => match f {
            Commands => Partial,
            _ => Full,
        },
        Agent::Grok => match f {
            Commands | LeanOutput | Media | WebSearch => Partial,
            _ => Full,
        },
        Agent::Codex => Full,
        // Same for aider: `LeanOutput` Full since the rules file started
        // carrying the preamble — it has neither hook nor bridge.
        Agent::Aider => match f {
            CodeSearch | Dedup | Privacy | Security | LeanOutput => Full,
            _ => Partial,
        },
        // Not transcribed; `gains` returns before ever asking, and the guard
        // below keeps that true.
        _ => Full,
    }
}

/// Does this agent get feature `f` fully WITHOUT the proxy (its hooks, plugin
/// or rules file)? Then the proxy must not do it again — the operator's rule:
/// rank 4 fills what is missing, it never duplicates what is there.
pub fn carried_without_proxy(agent: Agent, f: Feature) -> bool {
    effective_baseline(agent, f, hooks_effective(agent)) == Support::Full
}

/// Do this agent's hooks RUN on this machine now? An agent with no
/// validation step runs the hooks `install` wrote. One with a validation step
/// is asked where it keeps its answer — operator rule, 2026-10-06: know, per
/// agent, whether hooks need validating, and behave differently either way.
pub fn hooks_effective(agent: Agent) -> bool {
    tokenade_core::platform::home_dir()
        .is_some_and(|home| super::hook_state::hooks_effective(agent, &home))
}

/// What the agent gets without the proxy, counting only what runs WITHOUT
/// anyone's approval. `baseline` is measured with every hook trusted; a hook
/// that needs a consent click (Codex: a `trusted_hash` per hook) is not
/// assumed to run — operator rule, 2026-10-06: nobody approves them, CLI or
/// desktop app. What only such a hook carries is counted as absent, so the
/// proxy is installed for it and carries it. Nothing doubles if the hooks DO
/// run: the proxy leaves a payload a hook folded (its marker) and never adds a
/// style note the request already carries.
fn effective_baseline(agent: Agent, f: Feature, hooks_run: bool) -> Support {
    use Feature::*;
    match f {
        // The rules file the install writes (`AGENTS.md`, `QWEN.md`…) needs no
        // approval and tells the model to read documents with `tokenade read`:
        // partial — the model has to follow it, no hook makes it.
        Media if !hooks_run => baseline(agent, f).min(Support::Partial),
        Commands | WebSearch | LeanOutput | Context | Dedup | Security if !hooks_run => {
            Support::None
        }
        _ => baseline(agent, f),
    }
}

/// The style preamble the proxy should put in front of this agent's requests:
/// only where nothing else carries it (`LeanOutput` short of Full), and only
/// when the operator set a style.
pub fn preamble_for(agent: Option<Agent>) -> Option<String> {
    agent.filter(|a| !carried_without_proxy(*a, Feature::LeanOutput))?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    Some(
        tokenade_core::style::load(&cwd)
            .resolve("")
            .preamble()
            .to_string(),
    )
    .filter(|p| !p.trim().is_empty())
}

/// Whether this agent's row on the compatibility page has been transcribed
/// into `baseline` above.
///
/// The list grew a recipe faster than it grew a transcription, and that is the
/// normal direction: measuring an agent's config key takes a session, reading
/// its row off a page in another repository takes access to that page. The
/// honest handling is to say nothing about what rank 4 ADDS on an agent whose
/// starting point we have not written down — an empty cell beats a cell
/// computed against `Full` by default, which would silently claim the proxy
/// adds nothing at all.
fn transcribed(agent: Agent) -> bool {
    matches!(
        agent,
        Agent::ClaudeCode
            | Agent::OpenCode
            | Agent::KiloCode
            | Agent::Cline
            | Agent::Qwen
            | Agent::Codex
            | Agent::Grok
            | Agent::Aider
    )
}

/// What rank 4 adds on this agent, in the site's vocabulary. Empty is the
/// common answer and the honest one.
///
/// Two gates, and the second is the one that usually decides:
///
/// 1. **Is it installable?** No measured config site, no proposal — guessing a
///    key is how a product claims coverage it never verified.
/// 2. **May the proxy rewrite at all?** One authority per payload. On
///    every agent where a hook or a plugin already folds tool results, rank 4
///    is a meter and not a compactor, so it adds no FEATURE — the exact cost
///    figures it brings are real, but they are not one of the ten boxes.
pub fn gains(agent: Agent) -> Vec<Gain> {
    gains_with(agent, hooks_effective(agent))
}

/// [`gains`] for a given hook state — validated (`true`) or not.
pub fn gains_with(agent: Agent, hooks_run: bool) -> Vec<Gain> {
    if super::install::recipe(agent).is_none() {
        return Vec::new();
    }
    // 3. **Do we know where this agent STARTS?** A gain is a difference, and
    //    a difference against an unknown is not a measurement.
    if !transcribed(agent) {
        return Vec::new();
    }
    let e = super::agent_matrix::entry(agent);
    if !e.role(agent.coverage()).may_fold() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut add = |feature: Feature, to: Support, because: &'static str| {
        let from = effective_baseline(agent, feature, hooks_run);
        if to > from {
            out.push(Gain {
                feature,
                from,
                to,
                because,
            });
        }
    };

    // Redaction runs on every tool result the proxy relays, whatever its
    // length, and needs no hook the user has to approve. Partial rather than
    // full on purpose: it covers what the agent SENDS as tool results, not the
    // prose a user types into their own prompt.
    add(
        Feature::Security,
        Support::Partial,
        "secrets in tool results are scrubbed on the way to the provider, \
         with no hook to approve",
    );
    // The same 935 compactors the shell path uses, applied to every tool result
    // in the outgoing history rather than only to what the shell saw.
    add(
        Feature::Commands,
        Support::Full,
        "the command compactors reach every tool result, not only what the \
         shell proxy saw",
    );
    // MCP results ARE tool results by the time they reach the wire, so they are
    // folded. The tool MANIFESTS are not, which is why this is partial.
    add(
        Feature::Mcp,
        Support::Partial,
        "verbose MCP results are compacted in the request; tool manifests are not",
    );
    // The three below were MEASURED and never declared, which is a different
    // failure from the fold gate above: the proxy did the work and the product
    // did not say so, so `support_table` showed no improvement and `offer`
    // printed nothing. Figures from `testdata/compat/results/*.json`, the same
    // fixtures on all three OS.
    //
    // A fetched page is a tool result like any other by the time it reaches
    // the wire — it does not need the fetch to be intercepted, which is what
    // the rank-1 channel needs and what several agents do not offer.
    add(
        Feature::WebSearch,
        Support::Full,
        "a fetched page is folded in the outgoing history (144 390 → 52 540 bytes \
         measured), with no web hook to install",
    );
    // The single largest fold on the roster: search output is the most
    // repetitive payload an agent produces.
    add(
        Feature::CodeSearch,
        Support::Full,
        "search results are folded in the outgoing history (233 238 → 4 872 bytes \
         measured)",
    );
    // Partial and not Full on purpose: the history shrinks because its tool
    // results shrink. Nothing here drops a turn or summarises the prose, which
    // is what full context management would mean.
    // The one gain here that is not a fold. Eleven agents have no prompt-time
    // hook at all, so `tokenade style` writes a preamble that nothing reads at
    // the moment it would change a decision. The proxy is the last place it
    // can be said, and it says it on every request rather than once per
    // session — see `wire::prepend_system_preamble`, which is idempotent by
    // content so a fifty-turn conversation carries one copy and not fifty.
    add(
        Feature::LeanOutput,
        Support::Full,
        "the style preamble is prepended to the outgoing system prompt, on \
         agents that expose no prompt-time hook to carry it",
    );
    // Exact repeats only — the hooks also catch a file re-read with a small
    // change, as a diff — hence partial.
    add(
        Feature::Dedup,
        Support::Partial,
        "a tool result identical to an earlier one in the conversation is sent \
         once; later copies go out as a pointer to it",
    );
    add(
        Feature::Context,
        Support::Partial,
        "the whole outgoing history shrinks with its tool results (358 410 → \
         193 641 bytes over a four-tool history)",
    );
    out
}

/// Where one agent stands on all ten boxes, before and after rank 4.
///
/// The whole point of keeping the site's vocabulary in the binary: an operator
/// can ask the product what it claims, instead of reading a web page and hoping
/// the two agree.
pub fn support_table(agent: Agent) -> Vec<(Feature, Support, Support)> {
    let hooks_run = hooks_effective(agent);
    let gains = gains_with(agent, hooks_run);
    Feature::ALL
        .iter()
        .map(|&f| {
            let from = effective_baseline(agent, f, hooks_run);
            let to = gains
                .iter()
                .find(|g| g.feature == f)
                .map(|g| g.to)
                .unwrap_or(from);
            (f, from, to)
        })
        .collect()
}

/// The line `install` prints for one agent. `None` when there is nothing to
/// offer, which is most of the time.
pub fn offer(agent: Agent) -> Option<String> {
    let gains = gains(agent);
    if gains.is_empty() {
        return None;
    }
    let mut s = format!("  {} — the local LLM proxy would add:\n", agent.as_str());
    for g in &gains {
        let from = match g.from {
            Support::None => "not supported",
            Support::Partial => "partial",
            Support::Full => "supported",
        };
        s.push_str(&format!(
            "      · {} ({from} today) — {}\n",
            g.feature.label(),
            g.because
        ));
    }
    // Said every time, because it is the question a user asks before pointing
    // an agent's base URL at anything: where does my code go?
    s.push_str(
        "    It is a LOCAL proxy: it binds 127.0.0.1, and every request stays on \
         this machine\n    before going to the provider you already use.\n",
    );
    Some(s)
}

#[cfg(test)]
#[path = "features_tests.rs"]
mod tests;
