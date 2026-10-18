//! **A cap is a RULE, not a number** — and its evaluation.
//!
//! gemini-cli's `read_file` caps 2,000 LINES *and* 2,000 characters PER
//! line: the same 2 MB payload passes almost whole as 500 long lines
//! and loses 95 % as 200,000 short ones. No scalar expresses that, so any
//! scalar chosen for gemini-cli is wrong for most inputs.
//!
//! Hence the separation this file keeps: `delivered_chars` MEASURES on the
//! real payload, `worst_case_chars` announces a bound. The caller who no longer
//! has the bytes gets the bound — labelled as such, never disguised
//! as a measurement.

// No `use super::*;`: a cap rule is evaluated on a payload, without
// knowing anything about the matrix or the agents populating it.

/// How much of a payload survives an agent's truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The agent truncates NOTHING on this path. Delivered == the whole
    /// payload, however large. This is a real, measured state (aider's
    /// `cmd_run` asks for confirmation and then adds the output whole) — not a
    /// "we don't know" placeholder. See [`Provenance`] for the unknown case.
    Unbounded,
    /// A flat character cap.
    Chars(u64),
    /// A flat token cap, counted by the agent's own tokenizer.
    Tokens(u64),
    /// First `max_lines` lines, each individually cut to `max_line_chars`.
    /// **The effective size depends on the data** — that is the whole point of
    /// keeping this shape instead of collapsing it.
    Lines {
        max_lines: u64,
        max_line_chars: Option<u64>,
    },
    /// gemini-cli's shell truncation, and the reason [`Ctx`] exists.
    ///
    /// Read out of `saveTruncatedContent` in the installed package:
    /// 1. below the threshold the payload is delivered ENTIRE — a cliff, not a
    ///    ramp;
    /// 2. above it, if the payload has FEWER than `total_lines` lines, every
    ///    over-long line is first re-wrapped at `wrap_width` so that there is
    ///    something to cut. A 400 KB single-line payload is not exempt; it is
    ///    turned into ~3 300 lines and then cut like any other;
    /// 3. what survives is the FIRST `total_lines / 5` lines and the LAST
    ///    `total_lines - total_lines / 5` — head and tail, not a prefix.
    ///
    /// And `threshold_chars` here is only the static ceiling. The effective
    /// threshold is `min(ceiling, 4 * (model context - prompt already used))`,
    /// which is why [`Limit::delivered_chars`] takes a [`Ctx`]: on a 1M-context
    /// model with a nearly-empty prompt the vendor's own test expects
    /// **124 000**, not the 4 000 000 ceiling. Treating the ceiling as the cap
    /// would over-state this channel by ~32x, and by far more as context fills.
    HeadTail {
        threshold_chars: u64,
        total_lines: u64,
        wrap_width: Option<u64>,
    },
    /// The agent REFUSES rather than truncates: past the cap the model receives
    /// an error (and sometimes a path to a spilled file), not a shortened
    /// payload. Claude Code's `MaxFileReadTokenExceededError` — "File content
    /// (N tokens) exceeds maximum allowed tokens (M). Use offset and limit…".
    ///
    /// Kept distinct from `Tokens` because the two have OPPOSITE accounting
    /// consequences. Under a truncating cap, the counterfactual is that the
    /// user paid the cap. Under a refusing one, they paid ~nothing and then
    /// spent several turns doing offset/limit reads instead — so crediting the
    /// cap here would invent tokens that were never going to be spent.
    RefusedAbove { tokens: u64 },
    /// The LAST `max_lines` lines survive, not the first. Antigravity's command
    /// output: a 9 000-line payload came back as `<truncated 8801 lines>`
    /// followed by the final 199. Distinct from `Lines` because the size of the
    /// tail of a payload is not the size of its head.
    TailLines { max_lines: u64 },
    /// Delivered whole below `threshold_chars`; above it, a fixed CHARACTER
    /// budget survives, split head and tail. Distinct from `HeadTail`, which
    /// budgets LINES: grok keeps 10 000 characters at each end regardless of
    /// how the payload is broken into lines.
    HeadTailChars {
        threshold_chars: u64,
        keep_chars: u64,
    },
    /// Several rules apply at once and the TIGHTEST wins. qwen-code caps its
    /// search results at 25 000 characters AND 1 000 lines simultaneously;
    /// which one bites depends entirely on the payload, so picking either as
    /// "the" cap would be right only for the inputs that happen to favour it.
    Tightest(&'static [Limit]),
}

/// The marker `saveTruncatedContent` splices between head and tail.
/// Counted because it is really delivered; the several hundred characters of
/// surrounding instructions are not, because they are the agent's own prose and
/// exist whether or not tokenade is installed.
pub(crate) const HEAD_TAIL_MARKER_CHARS: u64 = "\n... [CONTENT TRUNCATED] ...\n".len() as u64;

/// Session state that a cap may depend on.
///
/// Some limits are not a property of the agent alone: gemini-cli's shell
/// threshold is a function of how much context is still free, so the same
/// command truncates differently on turn 2 and on turn 200. Callers that know
/// the session pass it; callers that do not pass [`Ctx::UNKNOWN`] and get the
/// agent's static ceiling, which is an upper bound and is documented as one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ctx {
    /// The model's whole context window, in tokens.
    pub model_token_limit: Option<u64>,
    /// Tokens the prompt already occupies.
    pub prompt_tokens_used: Option<u64>,
}

impl Ctx {
    /// Nothing known about the session. Yields the static ceiling.
    pub const UNKNOWN: Ctx = Ctx {
        model_token_limit: None,
        prompt_tokens_used: None,
    };

    pub fn new(model_token_limit: u64, prompt_tokens_used: u64) -> Ctx {
        Ctx {
            model_token_limit: Some(model_token_limit),
            prompt_tokens_used: Some(prompt_tokens_used),
        }
    }

    /// Free context expressed in characters, using the 1 token ~= 4 chars ratio
    /// the vendor's own code uses for this computation. Deliberately their
    /// ratio and not our estimator: the point is to reproduce THEIR threshold,
    /// not to be more accurate than they are.
    pub fn remaining_chars(&self) -> Option<u64> {
        let limit = self.model_token_limit?;
        let used = self.prompt_tokens_used.unwrap_or(0);
        Some(limit.saturating_sub(used).saturating_mul(4))
    }
}

impl Limit {
    /// What this agent would really put in front of its model, for THIS text.
    ///
    /// Counted in `char`s, not bytes: every cap in the table below is written
    /// against a JS `String.length` or a Python `len(str)`, both of which count
    /// code units of text and not UTF-8 bytes. Using `.len()` here would
    /// under-report every non-ASCII payload — the byte-as-char class this
    /// codebase has already been bitten by more than once.
    pub fn delivered_chars(&self, payload: &str, ctx: Ctx) -> u64 {
        let total = payload.chars().count() as u64;
        match *self {
            Limit::Unbounded => total,
            Limit::Chars(c) => total.min(c),
            // A token cap cannot be applied to chars without a tokenizer; use
            // the shared estimator rather than inventing a local ratio.
            Limit::Tokens(t) => {
                let est = crate::tokenizer::estimate(payload) as u64;
                if est <= t {
                    total
                } else {
                    // Scale back proportionally: the fraction of the text that
                    // fits in `t` tokens.
                    total.saturating_mul(t) / est.max(1)
                }
            }
            Limit::Lines {
                max_lines,
                max_line_chars,
            } => {
                let mut kept: u64 = 0;
                for (i, line) in payload.lines().enumerate() {
                    if i as u64 >= max_lines {
                        break;
                    }
                    let len = line.chars().count() as u64;
                    kept += match max_line_chars {
                        Some(m) => len.min(m),
                        None => len,
                    };
                    kept += 1; // the newline the agent keeps between lines
                }
                kept.min(total)
            }
            Limit::HeadTailChars {
                threshold_chars,
                keep_chars,
            } => {
                if total <= threshold_chars {
                    total
                } else {
                    keep_chars.min(total) + HEAD_TAIL_MARKER_CHARS
                }
            }
            Limit::TailLines { max_lines } => {
                let lens: Vec<u64> = payload
                    .split('\n')
                    .map(|l| l.chars().count() as u64)
                    .collect();
                let n = lens.len() as u64;
                let from = n.saturating_sub(max_lines) as usize;
                lens[from..].iter().map(|l| l + 1).sum::<u64>().min(total)
            }
            Limit::Tightest(rules) => rules
                .iter()
                .map(|r| r.delivered_chars(payload, ctx))
                .min()
                .unwrap_or(total),
            Limit::RefusedAbove { tokens } => {
                if crate::tokenizer::estimate(payload) as u64 <= tokens {
                    total
                } else {
                    0
                }
            }
            Limit::HeadTail {
                threshold_chars,
                total_lines,
                wrap_width,
            } => {
                let effective = threshold_chars.min(ctx.remaining_chars().unwrap_or(u64::MAX));
                if total <= effective {
                    return total;
                }
                // Line lengths in chars. `split('\n')` and not `lines()`: the
                // agent splits on '\n' exactly, so a trailing newline yields a
                // final empty line for it too, and the counts must match.
                let mut lens: Vec<u64> = payload
                    .split('\n')
                    .map(|l| l.chars().count() as u64)
                    .collect();
                // Too few lines to cut? Re-wrap first, exactly as the agent
                // does — this is what stops a one-line 400 KB blob from
                // slipping through a line-based rule untouched.
                if let Some(w) = wrap_width {
                    if w > 0 && (lens.len() as u64) <= total_lines {
                        let mut wrapped: Vec<u64> = Vec::with_capacity(lens.len());
                        for len in &lens {
                            if *len > w {
                                let mut rest = *len;
                                while rest > w {
                                    wrapped.push(w);
                                    rest -= w;
                                }
                                wrapped.push(rest);
                            } else {
                                wrapped.push(*len);
                            }
                        }
                        lens = wrapped;
                    }
                }
                let head = total_lines / 5;
                let tail = total_lines.saturating_sub(head);
                let n = lens.len() as u64;
                let sum_range = |from: u64, to: u64| -> u64 {
                    let (from, to) = (from.min(n) as usize, to.min(n) as usize);
                    // +1 per line for the '\n' the agent re-joins them with.
                    lens[from..to].iter().map(|l| l + 1).sum::<u64>()
                };
                let kept = if n <= total_lines {
                    // Above the byte threshold but under the line budget even
                    // after wrapping: head and tail overlap, so everything is
                    // kept and only the marker is added.
                    sum_range(0, n)
                } else {
                    sum_range(0, head) + sum_range(n - tail, n)
                };
                (kept + HEAD_TAIL_MARKER_CHARS).min(total + HEAD_TAIL_MARKER_CHARS)
            }
        }
    }

    /// An upper bound for callers that no longer hold the payload.
    ///
    /// Deliberately separate from [`Limit::delivered_chars`] and deliberately
    /// named "worst case": a per-line rule has no scalar equivalent, so this
    /// returns the most the agent COULD deliver, and a caller reading the name
    /// knows it is not a measurement. `None` means genuinely unbounded.
    pub fn worst_case_chars(&self) -> Option<u64> {
        match *self {
            Limit::Unbounded => None,
            Limit::Chars(c) => Some(c),
            Limit::Tokens(t) => Some(t.saturating_mul(4)),
            Limit::Lines {
                max_lines,
                max_line_chars,
            } => max_line_chars.map(|m| max_lines.saturating_mul(m.saturating_add(1))),
            // No constant bound exists, and saying so is the point. Below the
            // threshold the payload is delivered ENTIRE, so the bound is at
            // least the threshold. Above it, the surviving head and tail are
            // whole LINES of unbounded length — the re-wrap that would bound
            // them only runs when the payload has too FEW lines. So a payload
            // of 1 000 very long lines is delivered almost whole, at any size.
            // Returning the threshold here would look like a cap and be one of
            // the ways a scalar table lies; `None` forces the caller to say
            // what it wants instead of inheriting a wrong number.
            Limit::HeadTail { .. } => None,
            // N whole lines of unbounded length: no constant bounds it.
            Limit::TailLines { .. } => None,
            // Below the threshold everything arrives, so that is the ceiling.
            Limit::HeadTailChars {
                threshold_chars, ..
            } => Some(threshold_chars),
            // Below the cap everything is delivered, above it nothing is. The
            // largest delivery is therefore the cap itself.
            Limit::RefusedAbove { tokens } => Some(tokens.saturating_mul(4)),
            // The tightest KNOWN bound; if no rule states one, neither does
            // the combination.
            Limit::Tightest(rules) => rules.iter().filter_map(|r| r.worst_case_chars()).min(),
        }
    }

    /// True when the rule is a single number that does NOT depend on the
    /// payload. Only these can meaningfully be compared with a scalar table:
    /// for a per-line or head/tail rule, `worst_case_chars` is an upper bound
    /// on a completely different quantity than a conservative ingestion
    /// estimate, and comparing them would manufacture a disagreement out of
    /// two answers to two different questions.
    pub fn is_flat(&self) -> bool {
        matches!(
            self,
            Limit::Chars(_) | Limit::Tokens(_) | Limit::RefusedAbove { .. }
        )
    }

    /// True when this path performs no truncation at all.
    pub fn is_unbounded(&self) -> bool {
        matches!(self, Limit::Unbounded)
    }
}
