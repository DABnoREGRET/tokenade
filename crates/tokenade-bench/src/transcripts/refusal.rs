//! **Did the agent stop using the tool?**
//!
//! The other end of the measurement. Coming back for more (`expand-ref`) is a
//! success of the fallback; complaining, hitting an execution error, or bypassing
//! the fold is the agent turning away. Those three do not leave a trace
//! joined by a hash: they are read in what the agent writes and in the
//! command it runs next.

use super::*;

/// **The only direct witness: what the agent SAYS right after a fold.**
///
/// Every measurement on this axis infers discomfort from a behaviour — a
/// replay, a trip back for more, an empty hand. Yet transcripts carry
/// the agent's own words: when a truncated output blocks it, it
/// writes so ("truncated", "I can't see the rest", "output was cut").
/// The measurement counts these phrases in the prose block that FOLLOWS a tool
/// result, and counts them on both sides — after a FOLDED result and after
/// an intact one. Without this control, a 2 % rate would look damning
/// when it may just be the background noise of the job.
#[derive(Default)]
pub struct Complaints {
    /// `(complaints, opportunities)` after a result carrying a fold.
    pub folded: (usize, usize),
    /// `(complaints, opportunities)` after an intact result — the control.
    pub plain: (usize, usize),
    /// The phrases found after a fold, with the project: a rate does not say
    /// what the agent is complaining about, and that is what we are after.
    pub folded_samples: Vec<(String, String)>,
}

impl Complaints {
    pub fn folded_pct(&self) -> f64 {
        Self::pct(self.folded)
    }
    pub fn plain_pct(&self) -> f64 {
        Self::pct(self.plain)
    }
    fn pct((n, d): (usize, usize)) -> f64 {
        if d == 0 {
            0.0
        } else {
            n as f64 * 100.0 / d as f64
        }
    }
}

/// The phrasings by which an agent says it is missing material.
/// The list is short and literal: each entry must appear in the
/// PROSE, where the agent speaks for itself, and not in a banner it copies.
pub(super) const COMPLAINT_PHRASES: &[&str] = &[
    "truncated",
    "tronqué",
    "tronquée",
    "cut off",
    "coupée au milieu",
    "incomplete output",
    "output was cut",
    "je ne vois pas la suite",
    "je ne vois pas le reste",
    "il me manque",
    "can't see the rest",
    "cannot see the rest",
    "only shows the first",
    "n'affiche que le début",
];
pub(super) fn complains(text: &str) -> Option<&'static str> {
    let low = text.to_lowercase();
    COMPLAINT_PHRASES.iter().copied().find(|p| low.contains(p))
}

pub fn complaint_rates(root: &Path) -> Complaints {
    let out = Complaints::default();
    // Did the last tool result received carry a fold? `None` as long as no
    // result is awaiting a reaction: the prose that opens a turn reacts to
    // nothing.
    // Both closures touch the same state: it goes through a cell.
    let awaiting: std::cell::RefCell<Option<bool>> = Default::default();
    let out = std::cell::RefCell::new(out);
    for_each_block(
        root,
        |project, _path, _ts, block| match block {
            Block::Bash { .. } => {}
            Block::Received { text, kind, .. } => {
                if kind == Recv::Other {
                    return;
                }
                if kind == Recv::AssistantProse {
                    let was = awaiting.borrow_mut().take();
                    if let Some(folded) = was {
                        let out = &mut *out.borrow_mut();
                        let slot = if folded {
                            &mut out.folded
                        } else {
                            &mut out.plain
                        };
                        slot.1 += 1;
                        if let Some(p) = complains(&text) {
                            slot.0 += 1;
                            if folded {
                                let line = sample(
                                    text.lines()
                                        .find(|l| l.to_lowercase().contains(p))
                                        .unwrap_or("")
                                        .trim(),
                                    120,
                                );
                                out.folded_samples.push((project.to_string(), line));
                            }
                        }
                    }
                    return;
                }
                *awaiting.borrow_mut() = Some(carries_fold(&text));
            }
        },
        // A reaction does not span two transcripts.
        || *awaiting.borrow_mut() = None,
    );
    out.into_inner()
}

/// **Complaints CANARY**: four opportunities whose answer is known — a
/// complaint after a fold, a silence after a fold, a complaint after an INTACT
/// result (the control, without which a rate compares to nothing), and prose
/// preceded by no result at all, which therefore reacts to nothing.
pub fn canary_complaints(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-dit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let one = |role: &str, b: serde_json::Value| {
        serde_json::json!({"message": {"role": role, "content": [b]}}).to_string()
    };
    let result = |text: &str| {
        one(
            "user",
            serde_json::json!({"type": "tool_result", "content": text}),
        )
    };
    let prose = |text: &str| {
        one(
            "assistant",
            serde_json::json!({"type": "text", "text": text}),
        )
    };
    // A system reminder: a `text` block, but inside a `user` message. It
    // reacts to nothing and must not consume the opportunity.
    let reminder = |text: &str| one("user", serde_json::json!({"type": "text", "text": text}));
    let lines = [
        prose("starting by listing the directory"),
        result("body\n[tokenade:auto 9 tokens folded away — `tokenade expand-ref aaaaaaaaaaaa`]"),
        reminder("<system-reminder>remember to re-read the file</system-reminder>"),
        prose("la sortie est tronquée, je vais chercher le reste"),
        result("body\n[tokenade:auto 9 tokens folded away — `tokenade expand-ref bbbbbbbbbbbb`]"),
        prose("everything is there, moving on"),
        result("an intact output, without a single fold"),
        prose("hmm, the output was cut, rerunning"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let c = complaint_rates(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let ok = c.folded == (1, 2) && c.plain == (1, 1) && c.folded_samples.len() == 1;
    if ok {
        if verbose {
            println!("complaints canary: folded 1/2, control 1/1 — instrument valid");
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"folded 1/2, control 1/1\", got folded {}/{}, \
         control {}/{}",
        c.folded.0, c.folded.1, c.plain.0, c.plain.1
    );
    1
}

/// **The other half of the cost: EXECUTION.**
///
/// Every measurement on this axis is about REACQUISITION — the agent
/// goes back for what the fold removed. That is one half.
/// The literature on context compression for agents splits the interaction
/// cost in two: *retrieval* and *execution*, and reports an ordering
/// — tool execution failures appear BEFORE the success rate
/// collapses ("What Does Context Compression Cost an Agent?", arXiv
/// 2608.16370, §2; the idea is borrowed, no code is).
///
/// Transposed to observed traces: **does the command that FOLLOWS a folded result
/// fail more often than the others?** The flag comes from the transcript
/// (`is_error`), not from reading the text.
#[derive(Default)]
pub struct ExecErrors {
    /// `(failures, commands)` run after a result carrying a fold.
    pub after_fold: (usize, usize),
    /// `(failures, commands)` after an intact result — the control.
    pub after_plain: (usize, usize),
    /// The same thing under the STRICT definition of a fold (a banner carrying a
    /// hash). Publishing both says whether the answer depends on what we call a
    /// fold — a measurement whose conclusion changes with the definition is
    /// not one.
    pub after_fold_strict: (usize, usize),
    pub after_plain_strict: (usize, usize),
}

impl ExecErrors {
    pub fn after_fold_pct(&self) -> f64 {
        Self::pct(self.after_fold)
    }
    pub fn after_plain_pct(&self) -> f64 {
        Self::pct(self.after_plain)
    }
    fn pct((n, d): (usize, usize)) -> f64 {
        Self::pct_of((n, d))
    }
    pub fn pct_of((n, d): (usize, usize)) -> f64 {
        if d == 0 {
            0.0
        } else {
            n as f64 * 100.0 / d as f64
        }
    }
}

pub fn exec_errors(root: &Path) -> ExecErrors {
    let out = std::cell::RefCell::new(ExecErrors::default());
    // Did the last result read carry a fold? `None` before the first one.
    let prev_folded: std::cell::RefCell<Option<bool>> = Default::default();
    let prev_strict: std::cell::RefCell<Option<bool>> = Default::default();
    // call id → was the result preceding it folded? (broad, strict)
    let waiting: std::cell::RefCell<BTreeMap<String, (bool, bool)>> = Default::default();
    for_each_block(
        root,
        |_p, _f, _ts, block| match block {
            Block::Bash { id, .. } => {
                if let (Some(id), Some(folded), Some(strict)) =
                    (id, *prev_folded.borrow(), *prev_strict.borrow())
                {
                    waiting.borrow_mut().insert(id, (folded, strict));
                }
            }
            Block::Received {
                id,
                text,
                kind,
                is_error,
            } => {
                if kind != Recv::Result {
                    return;
                }
                if let Some((folded, strict)) = id.and_then(|i| waiting.borrow_mut().remove(&i)) {
                    let out = &mut *out.borrow_mut();
                    for slot in [
                        if folded {
                            &mut out.after_fold
                        } else {
                            &mut out.after_plain
                        },
                        if strict {
                            &mut out.after_fold_strict
                        } else {
                            &mut out.after_plain_strict
                        },
                    ] {
                        slot.1 += 1;
                        if is_error {
                            slot.0 += 1;
                        }
                    }
                }
                *prev_folded.borrow_mut() = Some(carries_fold(&text));
                *prev_strict.borrow_mut() = Some(carries_fold_with_hash(&text));
            }
        },
        || {
            *prev_folded.borrow_mut() = None;
            *prev_strict.borrow_mut() = None;
            waiting.borrow_mut().clear();
        },
    );
    out.into_inner()
}

/// **Execution CANARY**: four commands whose fate is known — after
/// a fold, one that fails and one that succeeds; after an intact result, one
/// that fails; and one whose result has no flag at all, which must not
/// count as a failure.
pub fn canary_exec_errors(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-exec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let call = |id: &str| {
        serde_json::json!({"message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "x"}}]}})
        .to_string()
    };
    let res = |id: &str, text: &str, err: Option<bool>| {
        let mut b = serde_json::json!({"type": "tool_result", "tool_use_id": id,
                                       "content": text});
        if let Some(e) = err {
            b["is_error"] = serde_json::Value::Bool(e);
        }
        serde_json::json!({"message": {"role": "user", "content": [b]}}).to_string()
    };
    let folded = "body\n[tokenade:auto 9 tokens folded away — `tokenade expand-ref aaaaaaaaaaaa`]";
    let lines = [
        res("t0", folded, Some(false)),
        call("t1"),
        res("t1", "boom", Some(true)),
        // The previous result ("boom") is not folded: the next command
        // goes on the control side.
        call("t2"),
        res("t2", "failure after an intact output", Some(true)),
        res("t3", folded, Some(false)),
        call("t4"),
        res("t4", "all good", Some(false)),
        // A result WITHOUT a flag: absence is not failure.
        res("t5", folded, Some(false)),
        call("t6"),
        res("t6", "no flag here", None),
        // A banner WITHOUT a hash: folded in the broad sense, not in the strict
        // sense — it offers no way back.
        res("t7", "[tokenade:ls 9 line(s) collapsed]", Some(false)),
        call("t8"),
        res("t8", "nothing to report", Some(false)),
        // A MENTION outside a banner: neither one nor the other. Without it, the
        // strict rule could be replaced by "any mention at all".
        res(
            "t9",
            "see `tokenade expand-ref abcdefabcdef` above",
            Some(false),
        ),
        call("ta"),
        res("ta", "another failure", Some(true)),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    // A STILL FRESH transcript — the one the agent is currently writing. It
    // carries three failures after a fold; if it entered the corpus, the canary
    // would see it at once.
    let live = [
        res("v0", folded, Some(false)),
        call("v1"),
        res("v1", "boum", Some(true)),
        call("v2"),
        res("v2", "boum", Some(true)),
        call("v3"),
        res("v3", "boum", Some(true)),
    ]
    .join("\n");
    if std::fs::write(proj.join("vivant.jsonl"), &live).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary's live transcript");
        return 1;
    }
    // An OLD transcript that quotes the instrument's output: the session
    // where the measurement ran. Three more failures after a fold if it got in.
    let echo = format!(
        "{}\n{}",
        res(
            "e0",
            "canari exécution : après un pli 1/4 — instrument valide",
            Some(false)
        ),
        live
    );
    if std::fs::write(proj.join("echo.jsonl"), echo).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary's echo");
        return 1;
    }
    backdate(&proj.join("echo.jsonl"));
    let e = exec_errors(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    // Broad: the three commands that follow a banner (including the hashless one).
    // Strict: only the three that follow a banner CARRYING a hash.
    let ok = e.after_fold == (1, 4)
        && e.after_plain == (2, 2)
        && e.after_fold_strict == (1, 3)
        && e.after_plain_strict == (2, 3);
    if ok {
        if verbose {
            println!(
                "execution canary: after a fold 1/4 (strict 1/3), control 2/2 (strict 2/3) \
                 — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"after a fold 1/4 (strict 1/3), control 2/2 \
         (strict 2/3)\", got {}/{} (strict {}/{}) and {}/{} (strict {}/{})",
        e.after_fold.0,
        e.after_fold.1,
        e.after_fold_strict.0,
        e.after_fold_strict.1,
        e.after_plain.0,
        e.after_plain.1,
        e.after_plain_strict.0,
        e.after_plain_strict.1
    );
    1
}

/// **Does the agent bypass the fold AFTER having suffered from it?**
///
/// The corpus carries **2,329** commands prefixed with `TOKENADE_HOOK_DISABLED=1`
/// and **133** `tokenade raw`: there the agent explicitly asks that its output
/// not be folded. It is the only gesture in the corpus that EXPRESSES a refusal
/// of the fold — what remains is to know what it is refusing.
///
/// The decisive question is causal: **had the same command already been
/// run, earlier in the session, with a FOLDED result?** If so, the agent
/// redoes the work because the fold was not enough for it — exactly the
/// defect this axis is looking for. Otherwise, the bypass is a habit or a
/// precaution, and it says nothing about our folds.
#[derive(Default)]
pub struct Bypasses {
    /// Bypasses whose inner command had ALREADY run, folded.
    pub after_folded_twin: usize,
    /// Bypasses whose inner command had already run, NOT folded.
    pub after_plain_twin: usize,
    /// Bypasses of a command never run before in this session.
    pub first_time: usize,
    /// Samples of the first case — the only one that blames the fold.
    pub after_folded_samples: Vec<(String, String)>,
    /// Bytes DELIVERED by the bypassed outputs: what the fold was not allowed
    /// to look at. This is the figure the home page calls
    /// "LOST savings", measured here on real sessions rather than estimated.
    pub bypassed_bytes: usize,
    /// The same bytes, for the PRE-EMPTIVE bypasses only — command
    /// never run before, hence decided without any experience of the fold.
    pub preemptive_bytes: usize,
    /// **What the fold would have returned.** The bypassed bytes are in the
    /// transcript: we run them back through the product's compactor
    /// (`cmd::dispatch_named`, the same door as the hook) and sum both
    /// sides. Without this, "935 KB delivered without a fold" reads as a loss of
    /// 935 KB, whereas the fold might have removed nothing — a short
    /// output, or one of a shape no compactor knows.
    pub would_fold_from: usize,
    pub would_fold_to: usize,
    /// The bypassed programs, with the bytes delivered: `docker exec -it` or
    /// `ssh` would have gone through INTACT anyway — the hook lets
    /// interactive invocations through. Without this breakdown, "727 KB the fold
    /// would have removed" is an UPPER bound presented as a figure.
    pub by_program: BTreeMap<String, (usize, usize)>,
}

/// The first executable word of a command, leading environment assignments
/// ignored: `FOO=1 bar -x` gives `bar`.
pub(super) fn first_program(cmd: &str) -> &str {
    cmd.split_whitespace()
        .find(|t| !t.contains('=') || t.starts_with('-'))
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
}

/// The core of a bypass: the command the agent wanted to see intact.
/// `None` if it is not one.
pub(super) fn bypassed_command(cmd: &str) -> Option<&str> {
    let c = cmd.trim_start();
    for p in ["TOKENADE_HOOK_DISABLED=1 ", "TOKENADE_HOOK_DISABLED=true "] {
        if let Some(rest) = c.strip_prefix(p) {
            return Some(rest.trim_start());
        }
    }
    for p in ["tokenade raw ", "tokenade bypass ", "tokenade noproxy "] {
        if let Some(i) = c.find(p) {
            return Some(c[i + p.len()..].trim_start());
        }
    }
    None
}

pub fn bypasses(root: &Path) -> Bypasses {
    let out = std::cell::RefCell::new(Bypasses::default());
    // command already run in this session → did its result carry a fold?
    let history: std::cell::RefCell<BTreeMap<String, bool>> = Default::default();
    // id of the pending call → its command, to file the outcome against it.
    let waiting: std::cell::RefCell<BTreeMap<String, String>> = Default::default();
    let project: std::cell::RefCell<String> = Default::default();
    // id of a BYPASSED call → was it pre-emptive? Used to weigh its output.
    let bypassed: std::cell::RefCell<BTreeMap<String, bool>> = Default::default();
    let prog_of: std::cell::RefCell<BTreeMap<String, String>> = Default::default();
    for_each_block(
        root,
        |p, _f, _ts, block| match block {
            Block::Bash { id, cmd } => {
                *project.borrow_mut() = p.to_string();
                if let Some(inner) = bypassed_command(cmd) {
                    let preemptive = !history.borrow().contains_key(inner);
                    if let Some(id) = id {
                        prog_of
                            .borrow_mut()
                            .insert(id.clone(), first_program(inner).to_string());
                        bypassed.borrow_mut().insert(id, preemptive);
                    }
                    let out = &mut *out.borrow_mut();
                    match history.borrow().get(inner) {
                        Some(true) => {
                            out.after_folded_twin += 1;
                            if out.after_folded_samples.len() < 12 {
                                out.after_folded_samples
                                    .push((p.to_string(), sample(inner, 100)));
                            }
                        }
                        Some(false) => out.after_plain_twin += 1,
                        None => out.first_time += 1,
                    }
                    return;
                }
                // An ordinary command: we remember that it ran, and we
                // wait for its result to know whether it was folded.
                if let Some(id) = id {
                    waiting.borrow_mut().insert(id, cmd.to_string());
                }
                history.borrow_mut().entry(cmd.to_string()).or_insert(false);
            }
            Block::Received { id, text, kind, .. } => {
                if kind != Recv::Result {
                    return;
                }
                let at = id.clone().unwrap_or_default();
                if let Some(preemptive) = id.as_ref().and_then(|i| bypassed.borrow_mut().remove(i))
                {
                    let out = &mut *out.borrow_mut();
                    out.bypassed_bytes += text.len();
                    if preemptive {
                        out.preemptive_bytes += text.len();
                    }
                    if let Some(prog) = prog_of.borrow_mut().remove(&at) {
                        let e = out.by_program.entry(prog).or_insert((0, 0));
                        e.0 += 1;
                        e.1 += text.len();
                    }
                    if let Some(folded) = tokenade_core::cmd::dispatch(&text) {
                        out.would_fold_from += text.len();
                        out.would_fold_to += folded.len();
                    }
                    return;
                }
                if let Some(cmd) = id.and_then(|i| waiting.borrow_mut().remove(&i)) {
                    let folded = carries_fold(&text);
                    let mut h = history.borrow_mut();
                    let e = h.entry(cmd).or_insert(false);
                    *e = *e || folded;
                }
            }
        },
        // A command's history does not span two sessions.
        || {
            history.borrow_mut().clear();
            waiting.borrow_mut().clear();
            bypassed.borrow_mut().clear();
            prog_of.borrow_mut().clear();
        },
    );
    out.into_inner()
}

/// **Bypasses CANARY**: a command folded then bypassed (the only
/// case that blames the fold), a command intact then bypassed, a
/// bypass of a command never seen, and a `tokenade raw` — whose
/// shape differs from the environment prefix.
pub fn canary_bypasses(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-contour-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let call = |id: &str, cmd: &str| {
        serde_json::json!({"message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let res = |id: &str, text: &str| {
        serde_json::json!({"message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": text}]}})
        .to_string()
    };
    let lines = [
        call("c1", "git log --oneline"),
        res("c1", "body\n[tokenade:git-log 9 tokens folded away]"),
        call("c2", "TOKENADE_HOOK_DISABLED=1 git log --oneline"),
        res("c2", "twelve bytes"),
        call("c3", "ls -la"),
        res("c3", "bare output"),
        call("c4", "TOKENADE_HOOK_DISABLED=1 ls -la"),
        call("c5", "TOKENADE_HOOK_DISABLED=1 cat /etc/hosts"),
        call("c6", "tokenade raw git log --oneline"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let b = bypasses(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let ok = b.after_folded_twin == 2
        && b.after_plain_twin == 1
        && b.first_time == 1
        && b.bypassed_bytes == "twelve bytes".len()
        && b.preemptive_bytes == 0;
    if ok {
        if verbose {
            println!(
                "bypasses canary: 2 after a fold, 1 after a bare output, 1 never seen \
                 — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"2 / 1 / 1\", got {} / {} / {}",
        b.after_folded_twin, b.after_plain_twin, b.first_time
    );
    1
}
