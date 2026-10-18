//! **Was the recovery path used, and what did it return?**
//!
//! `expand-ref` is the intended fallback. Taking it proves that it works, not
//! that something is wrong. What is measured here is what happens AFTER the call:
//! was the stored output still there, did the agent find what it was looking
//! for, and in what SEQUENCE — an isolated call does not say the same thing as a
//! chain of three.

use super::*;

/// **What a recovery RETURNS, and how long the fold had been waiting.**
///
/// A throwaway script once counted the empty-handed recoveries: 41 out of 436.
/// This measurement belongs in the instrument — but above all, a failure count
/// does not answer the only question that decides whether there is a DEFECT:
/// was a not-found hash missing after the store's sweep, which is the
/// announced rule, or BEFORE it, which means a fold promised bytes it had not
/// written?
///
/// The gap is measured between the timestamp of the banner that printed the
/// hash and that of the recovery that asked for it. The threshold is not written
/// here: it is read from `disclose::CACHE_TTL_SECS`, the constant the sweep applies.
#[derive(Default)]
pub struct RecoveryOutcomes {
    pub served: usize,
    pub not_found: usize,
    pub zero_match: usize,
    pub refused: usize,
    /// Hash not found although the banner that printed it is LESS than the TTL
    /// old: the store should have had it. This is the only one of these counts
    /// that blames the product.
    pub not_found_inside_ttl: usize,
    /// Hash not found and banner older than the TTL: swept, as announced.
    pub not_found_after_ttl: usize,
    /// Hash not found whose banner was not read, has no timestamp, or came
    /// AFTER the call (the first banner read is then not the one that was
    /// used): undecidable, counted separately rather than filed on the
    /// convenient side.
    pub not_found_undatable: usize,
    /// Hash not found that is a fixture from our own sources: a staged
    /// failure, not a missed fold.
    pub not_found_fixture: usize,
    /// Hash whose banner names a store ELSEWHERE than the one `expand-ref`
    /// reads — a lab session that redirected `HOME` to a temporary
    /// directory. Not found by construction: the store queried is not the one
    /// that wrote.
    pub not_found_redirected_store: usize,
    /// `(project, hash, age in hours)` of the not-found hashes INSIDE the TTL —
    /// a bare count would not let anyone go and look.
    pub inside_ttl_samples: Vec<(String, String, i64)>,
}
pub(super) fn classify_recovery(text: &str) -> Option<&'static str> {
    if text.contains("not found in dedup cache or stash") || text.contains("not found (may have") {
        return Some("not_found");
    }
    if text.contains("0 matching line") {
        return Some("zero_match");
    }
    if text.contains("too large to dump whole") {
        return Some("refused");
    }
    if text.trim().is_empty() {
        return None;
    }
    Some("served")
}

pub fn recovery_outcomes(root: &Path) -> RecoveryOutcomes {
    recovery_outcomes_with(root, &fixture_hashes())
}

/// The same measurement, with the set of fixtures SUPPLIED. The canary passes its
/// own: the real survey reads our sources, and the hashes the canary stages are
/// in there too — written in this file. An instrument that read itself then
/// filed its four failures as "fixtures" and no longer validated anything.
pub fn recovery_outcomes_with(
    root: &Path,
    fixtures: &std::collections::BTreeSet<String>,
) -> RecoveryOutcomes {
    let mut out = RecoveryOutcomes::default();
    // hash → timestamp of the BANNER that printed it (the first one read). Only a
    // banner counts: a mention in prose does not prove that a fold took place.
    let announced: std::cell::RefCell<BTreeMap<String, String>> = Default::default();
    // **The join is the IDENTIFIER, not proximity.** "The first block
    // received after the command" counted 483 served and ONE single not-found where
    // the raw transcript text carries the failure message 146 times: between
    // the call and its result sits the assistant's prose, which got
    // classified as "served". The transcript writes `tool_use_id` opposite
    // `id`; that is the only exact join.
    let waiting: std::cell::RefCell<BTreeMap<String, (String, String)>> = Default::default();
    // Hashes whose banner named a store that is not the one
    // `expand-ref` reads.
    let redirected: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
    let real_stash = tokenade_core::disclose::stash_dir();
    let mut served = 0usize;
    let mut not_found = 0usize;
    let mut zero = 0usize;
    let mut refused = 0usize;
    for_each_block(
        root,
        |project, _f, ts, block| match block {
            Block::Bash { id, cmd } => {
                let (Some(id), Some(hash)) = (id, expand_ref_call(cmd)) else {
                    return;
                };
                waiting.borrow_mut().insert(id, (hash, ts.to_string()));
            }
            Block::Received { id, text, .. } => {
                if let Some((hash, at)) = id.and_then(|i| waiting.borrow_mut().remove(&i)) {
                    match classify_recovery(&text) {
                        Some("served") => served += 1,
                        Some("zero_match") => zero += 1,
                        Some("refused") => refused += 1,
                        Some("not_found") => {
                            not_found += 1;
                            let age = announced.borrow().get(&hash).and_then(|b| age_secs(b, &at));
                            match age {
                                // A fixture from our sources fails by
                                // construction: the store never had those
                                // bytes.
                                _ if fixtures.contains(&hash) => out.not_found_fixture += 1,
                                _ if redirected.borrow().contains(&hash) => {
                                    out.not_found_redirected_store += 1
                                }
                                // A NEGATIVE gap is not an age: the banner
                                // read first is not the one the call
                                // targeted.
                                Some(a) if a < 0 => out.not_found_undatable += 1,
                                Some(a) if a <= disclose_ttl() => {
                                    out.not_found_inside_ttl += 1;
                                    out.inside_ttl_samples.push((
                                        project.to_string(),
                                        hash.clone(),
                                        a / 3600,
                                    ));
                                }
                                Some(_) => out.not_found_after_ttl += 1,
                                None => out.not_found_undatable += 1,
                            }
                        }
                        _ => {}
                    }
                }
                let mut hashes = Vec::new();
                folds_in(&text, &mut hashes);
                let mut a = announced.borrow_mut();
                for (h, fam) in hashes {
                    if fam == "(no banner)" {
                        continue;
                    }
                    if names_foreign_stash(&text, &h, &real_stash) {
                        redirected.borrow_mut().insert(h.clone());
                    }
                    a.entry(h).or_insert_with(|| ts.to_string());
                }
            }
        },
        // A call whose result never came back (interrupted session) is
        // filed nowhere: it does not outlive the file.
        || waiting.borrow_mut().clear(),
    );
    out.served = served;
    out.not_found = not_found;
    out.zero_match = zero;
    out.refused = refused;
    out
}

/// Does the banner name, for this hash, a store file located ELSEWHERE than
/// in the real store? `[stashed: <path>/<hash>.txt · …]` spells it out, and a
/// session that redirects `HOME` to a temporary directory is recognised there
/// without guessing anything.
pub(super) fn names_foreign_stash(text: &str, hash: &str, real: &Path) -> bool {
    let needle = format!("stash/{hash}");
    let real = real.to_string_lossy().to_string();
    let mut from = 0usize;
    while let Some(i) = text[from..].find(&needle) {
        let at = from + i;
        from = at + needle.len();
        let start = text[..at].rfind(char::is_whitespace).map_or(0, |k| k + 1);
        let dir = format!("{}stash", &text[start..at]);
        if !dir.ends_with(&real) {
            return true;
        }
    }
    false
}
pub(super) fn disclose_ttl() -> i64 {
    tokenade_core::disclose::CACHE_TTL_SECS as i64
}

/// The gap in seconds between two transcript timestamps, or `None` if either
/// is missing — an undated event is not filed.
pub(super) fn age_secs(from: &str, to: &str) -> Option<i64> {
    let a = tokenade_core::claudecode_transcript::iso8601_to_epoch_ms(from)? as i64;
    let b = tokenade_core::claudecode_transcript::iso8601_to_epoch_ms(to)? as i64;
    Some((b - a) / 1000)
}

/// **Outcomes CANARY**: a tree where each outcome occurs once, and where the
/// same failure ("hash not found") shows up TWICE — once on a fold from an
/// hour ago, once on a fold from thirty days ago. An instrument that mixed
/// them up would not see the difference between "swept, as announced" and
/// "the fold never wrote those bytes".
pub fn canary_outcomes(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-issue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let ev = |ts: &str, body: serde_json::Value| {
        serde_json::json!({"timestamp": ts, "message": {"content": [body]}}).to_string()
    };
    let call = |ts: &str, id: &str, cmd: &str| {
        ev(
            ts,
            serde_json::json!({"type": "tool_use", "id": id, "name": "Bash",
                               "input": {"command": cmd}}),
        )
    };
    let result = |ts: &str, id: &str, text: &str| {
        ev(
            ts,
            serde_json::json!({"type": "tool_result", "tool_use_id": id, "content": text}),
        )
    };
    // The prose the assistant writes BETWEEN the call and its result: that is
    // what passed itself off as the answer when the join was "the next
    // block".
    let prose = |ts: &str, text: &str| ev(ts, serde_json::json!({"type": "text", "text": text}));
    let banner =
        |h: &str| format!("body\n[tokenade:auto 9 tokens folded away — `tokenade expand-ref {h}`]");
    let lines = [
        // A fresh fold, asked for an hour later: not found ⇒ the store
        // should have had it. The prose sits between the call and its answer.
        result("2026-09-01T10:00:00.000Z", "t0", &banner("aaaaaaaaaaaa")),
        call(
            "2026-09-01T11:00:00.000Z",
            "t1",
            "tokenade expand-ref aaaaaaaaaaaa",
        ),
        prose(
            "2026-09-01T11:00:00.500Z",
            "unfolding the hash announced above",
        ),
        result(
            "2026-09-01T11:00:01.000Z",
            "t1",
            "expand-ref: hash 'aaaaaaaaaaaa' not found in dedup cache or stash",
        ),
        // A fold from thirty days ago: swept, as announced.
        result("2026-08-01T10:00:00.000Z", "t2", &banner("bbbbbbbbbbbb")),
        call(
            "2026-08-31T10:00:00.000Z",
            "t3",
            "tokenade expand-ref bbbbbbbbbbbb",
        ),
        result(
            "2026-08-31T10:00:01.000Z",
            "t3",
            "expand-ref: hash 'bbbbbbbbbbbb' not found in dedup cache or stash",
        ),
        // Served.
        result("2026-09-02T10:00:00.000Z", "t4", &banner("cccccccccccc")),
        call(
            "2026-09-02T10:01:00.000Z",
            "t5",
            "tokenade expand-ref cccccccccccc",
        ),
        result("2026-09-02T10:01:01.000Z", "t5", "the whole bytes"),
        // Refused for size.
        call(
            "2026-09-02T10:02:00.000Z",
            "t6",
            "tokenade expand-ref cccccccccccc",
        ),
        result(
            "2026-09-02T10:02:01.000Z",
            "t6",
            "this blob is ~900000 tokens — too large to dump whole. ask instead",
        ),
        // Empty.
        call(
            "2026-09-02T10:03:00.000Z",
            "t7",
            "tokenade expand-ref cccccccccccc --grep zz",
        ),
        result(
            "2026-09-02T10:03:01.000Z",
            "t7",
            "[tokenade:expand-ref] 0 matching line(s) of 4 total",
        ),
        // A call whose answer never comes back: it is filed nowhere.
        call(
            "2026-09-02T10:04:00.000Z",
            "t8",
            "tokenade expand-ref cccccccccccc",
        ),
        // A fixture from our own sources: it fails by construction.
        call(
            "2026-09-02T10:05:00.000Z",
            "t9",
            "tokenade expand-ref deadbeefdead",
        ),
        result(
            "2026-09-02T10:05:01.000Z",
            "t9",
            "expand-ref: hash 'deadbeefdead' not found in dedup cache or stash",
        ),
        // A REDIRECTED store: the banner names a file outside the store that
        // `expand-ref` reads. Not found by construction.
        result(
            "2026-09-02T11:00:00.000Z",
            "tc",
            "body\n[stashed: /tmp/lab/stashhome/.cache/tokenade/stash/dddddddddddd.txt · \
             ~9 tokens · `tokenade expand-ref dddddddddddd`]",
        ),
        call(
            "2026-09-02T11:01:00.000Z",
            "td",
            "tokenade expand-ref dddddddddddd",
        ),
        result(
            "2026-09-02T11:01:01.000Z",
            "td",
            "expand-ref: hash 'dddddddddddd' not found in dedup cache or stash",
        ),
        // A banner read AFTER the call: the gap is negative, so it is not
        // an age, and the case is not filed on the blaming side.
        call(
            "2026-09-03T10:00:00.000Z",
            "ta",
            "tokenade expand-ref eeeeeeeeeeee",
        ),
        result(
            "2026-09-03T10:00:01.000Z",
            "ta",
            "expand-ref: hash 'eeeeeeeeeeee' not found in dedup cache or stash",
        ),
        result("2026-09-04T10:00:00.000Z", "tb", &banner("eeeeeeeeeeee")),
        // A call whose banner lives in ANOTHER file, read before this one
        // because files are walked in path order, not date order: the gap
        // is then negative. This is the shape in which the case shows up
        // for real.
        call(
            "2026-09-01T09:00:00.000Z",
            "tf",
            "tokenade expand-ref ffffffffffff",
        ),
        result(
            "2026-09-01T09:00:01.000Z",
            "tf",
            "expand-ref: hash 'ffffffffffff' not found in dedup cache or stash",
        ),
    ]
    .join("\n");
    let early = dir.join("a");
    let elsewhere = result("2026-09-10T10:00:00.000Z", "tg", &banner("ffffffffffff"));
    if std::fs::write(proj.join("s.jsonl"), lines).is_err()
        || std::fs::create_dir_all(&early).is_err()
        || std::fs::write(early.join("z.jsonl"), elsewhere).is_err()
    {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    backdate(&early.join("z.jsonl"));
    let o = recovery_outcomes_with(&dir, &["deadbeefdead".to_string()].into_iter().collect());
    let _ = std::fs::remove_dir_all(&dir);
    let ok = o.served == 1
        && o.refused == 1
        && o.zero_match == 1
        && o.not_found == 6
        && o.not_found_inside_ttl == 1
        && o.not_found_after_ttl == 1
        && o.not_found_fixture == 1
        && o.not_found_redirected_store == 1
        && o.not_found_undatable == 2;
    if ok {
        if verbose {
            println!(
                "outcomes canary: 1 served, 1 refused, 1 empty, 6 not found (1 inside the TTL, \
                 1 after, 1 fixture, 1 redirected store, 2 undecidable) — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — served {}, refused {}, empty {}, not found {} \
         (inside {}, after {}, fixture {}, redirected {}, undecidable {})",
        o.served,
        o.refused,
        o.zero_match,
        o.not_found,
        o.not_found_inside_ttl,
        o.not_found_after_ttl,
        o.not_found_fixture,
        o.not_found_redirected_store,
        o.not_found_undatable
    );
    1
}

/// **When the agent comes back TWICE to the same fold.**
///
/// `mine` counts 213 `stashed` folds of which 60 were asked for — but **107 calls**
/// for those 60 folds. Almost two calls per recovered fold: so the first answer
/// was not enough, one time in two. A recovery count does not say that;
/// the SEQUENCE of calls on the same hash does.
///
/// Each call is reduced to its shape — what the agent asked for — and the
/// successive pairs are counted. `grep → bare` says a pattern returned nothing
/// and the agent asked for everything; `prompt → prompt` says one question
/// led to the next, which is the intended behaviour.
#[derive(Default)]
pub struct Sequences {
    /// Number of calls per hash: `[1 call, 2 calls, 3+]`.
    pub once: usize,
    pub twice: usize,
    pub more: usize,
    /// Pairs `(shape, next shape)` → occurrences.
    pub pairs: BTreeMap<(String, String), usize>,
    /// What the FIRST call of a pair returned, when the second repeats
    /// the same shape. An agent that reruns an identical command does so because
    /// the first answer was not enough for it — and the reason decides whether
    /// there is a defect: "served" means it re-read, "refused" means that
    /// our message did not stop the repetition.
    pub repeat_after: BTreeMap<String, usize>,
    /// How many blocks separate two IDENTICAL calls. An immediate repeat
    /// is a defect — the answer was still in view; a very distant repeat
    /// is the expected behaviour, the answer having left the context.
    pub repeat_gap: BTreeMap<&'static str, usize>,
    /// Repeats whose FIRST answer carried a banner. Beware of the
    /// misreading: `maybe_stash` stores what was SHOWN, so a
    /// recovery legitimately returns text that already contains the banners
    /// of the original output. This count proves nothing on its own.
    pub repeat_on_folded_answer: usize,
    /// The count that WOULD blame the product: the answer announces a
    /// NEW fold — "tokens folded away" under a hash different from the one
    /// just asked for. That would be the recovery path folded onto
    /// itself.
    pub repeat_on_refolded_answer: usize,
    /// The commands of an IMMEDIATE repeat, with their project: a count does
    /// not say what the agent was redoing.
    pub immediate_samples: Vec<(String, String)>,
    /// **The real repeat: the WHOLE command is the same.** Counting by
    /// SHAPE filed two successive `--prompt` calls among the repeats, whereas
    /// asking a second question about the same document is the intended
    /// behaviour — conversation, not repetition. Only a command
    /// replayed word for word says the first answer was of no use.
    pub identical: usize,
    pub identical_immediate: usize,
}

/// Does the answer announce a NEW fold, that is, a fold of the answer
/// itself? The criterion: the fold banner's sentence, under a hash that is not
/// the one just asked for. The banners of the ORIGINAL output are part
/// of what was stored and prove nothing.
pub(super) fn announces_new_fold(text: &str, asked: &str) -> bool {
    if !text.contains("tokens folded away") {
        return false;
    }
    let mut hashes = Vec::new();
    folds_in(text, &mut hashes);
    hashes
        .iter()
        .any(|(h, fam)| h != asked && fam != "(no banner)")
}

/// **A command sample comes out of a FOREIGN transcript.**
///
/// The surveys of this axis print commands taken from the sessions of
/// ALL the projects on the machine, and a command contains whatever it contains
/// — e.g. a password passed on a command line, readable in clear in the
/// instrument's output, hence one copy-paste away from a file tracked by
/// git. Every sample goes through the product's redaction before being
/// shown.
pub(super) fn sample(text: &str, max: usize) -> String {
    tokenade_core::redact::redact(text)
        .chars()
        .take(max)
        .collect()
}

/// **Is this command a recovery call, and which one?**
///
/// Two traps, both measured:
///
/// * the text of a command can be OUR OWN banner. On a cold
///   resume, tokenade elides old commands and the transcript keeps
///   `[tokenade:elided 90 tokens on cold resume — recover: tokenade expand-ref
///   <hash>]` IN PLACE of the command. Read naively, this banner becomes a
///   call — it has all the words of one;
/// * `expand-ref <hash>` can appear in any command that
///   quotes the line (a `grep` over transcripts, a help `echo`). A call
///   names the program.
pub(super) fn expand_ref_call(cmd: &str) -> Option<String> {
    if cmd.trim_start().starts_with("[tokenade:") {
        return None;
    }
    let re = crate::transcripts::call_pattern();
    re.captures(cmd).map(|c| c[1].to_string())
}
pub(super) fn call_pattern() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?:^|[\s;&|(])(?:[^\s;&|()]*/)?tokenade\s+expand-ref\s+([0-9a-f]{8,})")
            .expect("recovery call pattern")
    })
}

/// The shape of a recovery call, as the command writes it.
pub(super) fn call_shape(cmd: &str) -> &'static str {
    if cmd.contains("--grep") {
        "grep"
    } else if cmd.contains("--prompt") {
        "prompt"
    } else if cmd.contains("--raw") {
        "raw"
    } else {
        "bare"
    }
}

pub fn recovery_sequences(root: &Path) -> Sequences {
    let mut out = Sequences::default();
    // hash → sequence of calls: (shape, what the result returned).
    #[allow(clippy::type_complexity)]
    let per_hash: std::cell::RefCell<
        BTreeMap<String, Vec<(&'static str, String, usize, bool, bool, String)>>,
    > = Default::default();
    // The project of the current transcript, to name where a sample comes from.
    let project: std::cell::RefCell<String> = Default::default();
    // The rank of the current block in the transcript: the distance between two
    // calls is counted in blocks, not seconds (a user pause
    // is not a distance).
    let seen: std::cell::RefCell<usize> = Default::default();
    // call id → (hash, rank in the sequence), to carry the outcome over to the
    // right call: two calls can go out before a result comes back.
    let waiting: std::cell::RefCell<BTreeMap<String, (String, usize)>> = Default::default();
    for_each_block(
        root,
        |p, _f, _ts, block| match block {
            Block::Bash { id, cmd } => {
                *project.borrow_mut() = p.to_string();
                *seen.borrow_mut() += 1;
                if let Some(hash) = expand_ref_call(cmd) {
                    let mut h = per_hash.borrow_mut();
                    let v = h.entry(hash.clone()).or_default();
                    v.push((
                        call_shape(cmd),
                        "no answer".to_string(),
                        *seen.borrow(),
                        false,
                        false,
                        sample(cmd, 90),
                    ));
                    if let Some(id) = id {
                        waiting.borrow_mut().insert(id, (hash, v.len() - 1));
                    }
                }
            }
            Block::Received { id, text, kind, .. } => {
                *seen.borrow_mut() += 1;
                if kind != Recv::Result {
                    return;
                }
                if let Some((hash, at)) = id.and_then(|i| waiting.borrow_mut().remove(&i)) {
                    if let Some(slot) = per_hash
                        .borrow_mut()
                        .get_mut(&hash)
                        .and_then(|v| v.get_mut(at))
                    {
                        slot.1 = classify_recovery(&text).unwrap_or("empty").to_string();
                        slot.3 = carries_fold(&text);
                        slot.4 = announces_new_fold(&text, &hash);
                    }
                }
            }
        },
        // A sequence of calls does not span two sessions: the same hash
        // asked for in two sessions is two stories.
        || {
            let mut h = per_hash.borrow_mut();
            for shapes in h.values() {
                match shapes.len() {
                    0 => {}
                    1 => out.once += 1,
                    2 => out.twice += 1,
                    _ => out.more += 1,
                }
                for w in shapes.windows(2) {
                    *out.pairs
                        .entry((w[0].0.to_string(), w[1].0.to_string()))
                        .or_default() += 1;
                    let gap_blocks = w[1].2.saturating_sub(w[0].2);
                    if w[0].5 == w[1].5 {
                        out.identical += 1;
                        if gap_blocks <= 4 {
                            out.identical_immediate += 1;
                            if out.immediate_samples.len() < 12 {
                                out.immediate_samples
                                    .push((project.borrow().clone(), w[1].5.clone()));
                            }
                        }
                    }
                    if w[0].0 == w[1].0 {
                        *out.repeat_after.entry(w[0].1.clone()).or_default() += 1;
                        if w[0].3 {
                            out.repeat_on_folded_answer += 1;
                        }
                        if w[0].4 {
                            out.repeat_on_refolded_answer += 1;
                        }
                        let gap = w[1].2.saturating_sub(w[0].2);
                        let bucket = if gap <= 4 {
                            "immediate (≤4 blocks)"
                        } else if gap <= 50 {
                            "near (≤50)"
                        } else {
                            "far (>50)"
                        };
                        *out.repeat_gap.entry(bucket).or_default() += 1;
                    }
                }
            }
            h.clear();
            waiting.borrow_mut().clear();
            *seen.borrow_mut() = 0;
        },
    );
    out
}

/// **Sequences CANARY**: one hash asked for once, another twice
/// (`grep` then `bare`), a third three times — and a fourth asked for
/// once in EACH of two transcripts, which must not count as a sequence
/// of two: those are two stories.
pub fn canary_sequences(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-suite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let call = |cmd: &str| {
        serde_json::json!({"message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "x", "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let res = |text: &str| {
        serde_json::json!({"message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "r1", "content": text}]}})
        .to_string()
    };
    let call_id = |id: &str, cmd: &str| {
        serde_json::json!({"message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let a = [
        // Two IDENTICAL calls, and what the first had returned: a refusal
        // because of size. That is the pair the measurement must name.
        call_id("r1", "tokenade expand-ref eeeeeeeeeeee"),
        res("this blob is ~900000 tokens — too large to dump whole. ask instead"),
        call_id("r2", "tokenade expand-ref eeeeeeeeeeee"),
        // A SERVED answer that contains the banner of the original output —
        // `maybe_stash` stores what was SHOWN. It is not a new
        // fold, and the measurement must not blame it.
        call_id("r3", "tokenade expand-ref ffffffffffff"),
        res("the stored bytes\n[tokenade:ls 9 tokens folded away — `tokenade expand-ref ffffffffffff`]"),
        call_id("r4", "tokenade expand-ref ffffffffffff"),
        call("tokenade expand-ref aaaaaaaaaaaa"),
        call("tokenade expand-ref bbbbbbbbbbbb --grep 'x'"),
        call("tokenade expand-ref bbbbbbbbbbbb"),
        call("tokenade expand-ref cccccccccccc --prompt 'q'"),
        call("tokenade expand-ref cccccccccccc --prompt 'q2'"),
        call("tokenade expand-ref cccccccccccc --raw"),
        call("tokenade expand-ref dddddddddddd"),
    ]
    .join("\n");
    let b = call("tokenade expand-ref dddddddddddd");
    if std::fs::write(proj.join("s.jsonl"), a).is_err()
        || std::fs::write(proj.join("t.jsonl"), b).is_err()
    {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcripts");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    backdate(&proj.join("t.jsonl"));
    let q = recovery_sequences(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let pair = |a: &str, b: &str| {
        q.pairs
            .get(&(a.to_string(), b.to_string()))
            .copied()
            .unwrap_or(0)
    };
    let ok = q.once == 3
        && q.twice == 3
        && q.repeat_on_refolded_answer == 0
        && q.more == 1
        && q.repeat_after.get("refused").copied().unwrap_or(0) == 1
        && pair("grep", "bare") == 1
        && pair("prompt", "prompt") == 1
        && pair("prompt", "raw") == 1;
    if ok {
        if verbose {
            println!(
                "sequences canary: 3 hashes once, 3 twice, 1 three times, 1 repeat \
                 after refusal, 0 new fold — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"3 / 2 / 1, grep→bare 1, prompt→prompt 1, \
         prompt→raw 1, repeat after refusal 1\", got {} / {} / {} and {}, {}, {}",
        q.once,
        q.twice,
        q.more,
        pair("grep", "bare"),
        pair("prompt", "prompt"),
        pair("prompt", "raw")
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A sample never leaks a secret out of a foreign transcript.**
    ///
    /// The orphan list used to print a command line carrying a password,
    /// taken from the sessions of another project on the machine, one
    /// copy-paste away from a file tracked by git.
    #[test]
    fn a_sample_is_redacted() {
        let cmd = "sshpass -p 'Xq7pTr2w' ssh -T server";
        let got = sample(cmd, 110);
        assert!(
            !got.contains("Xq7pTr2w"),
            "the password goes through the sample: {got}"
        );
        assert!(
            got.contains("sshpass"),
            "redaction must not erase the command: {got}"
        );
    }

    /// Truncation stays: a sample is a line, not a transcript.
    #[test]
    fn a_sample_is_short() {
        let long = "echo ".to_string() + &"x".repeat(500);
        assert_eq!(sample(&long, 90).chars().count(), 90);
    }
}
