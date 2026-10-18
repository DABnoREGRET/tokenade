//! **Does the agent redo the work?**
//!
//! Two forms of the same symptom, and neither goes through `expand-ref`: rerunning
//! the SAME command, or using `grep` to fetch what the folded output did not
//! show it. An agent that does either has not taken the recovery
//! path — it went around it without saying so.

use super::*;

/// **Does a fold make the agent RERUN the command?**
///
/// This is the silent failure once thought to leave no trace: the agent that
/// does not come back through `expand-ref` and simply redoes the work. It does
/// leave one, in fact — the same command, twice, a few events
/// apart.
///
/// **The figure alone says nothing; it needs the CONTROL.** The commands that
/// get folded are the big, chatty ones (`cargo test`, `git status`),
/// and those naturally get rerun more than the others. The measurement
/// therefore compares two populations in the same transcripts: the commands
/// whose result carried a fold, and all the others.
///
/// The window is SHORT — three events — because a later rerun is
/// a normal work iteration, not a shortfall.
#[derive(Default)]
pub struct Reruns {
    pub folded: (usize, usize),
    pub plain: (usize, usize),
    /// The FOLDED commands that were then rerun: a rate does not say which of
    /// the compactors sends the agent back to the command.
    pub folded_reruns: Vec<String>,
}

impl Reruns {
    fn rate(p: (usize, usize)) -> f64 {
        if p.1 == 0 {
            0.0
        } else {
            100.0 * p.0 as f64 / p.1 as f64
        }
    }
    pub fn folded_pct(&self) -> f64 {
        Self::rate(self.folded)
    }
    pub fn plain_pct(&self) -> f64 {
        Self::rate(self.plain)
    }
}

pub fn rerun_rates(root: &Path) -> Reruns {
    // Both closures touch the same state, so it goes through a cell
    // rather than two mutable borrows — `for_each_block` keeps both
    // of them alive.
    let out = std::cell::RefCell::new(Reruns::default());
    // (command, did its result carry a fold?)
    let seq: std::cell::RefCell<Vec<(String, bool)>> = Default::default();
    // The INDEX of the command whose result we are waiting for, not "the last one":
    // two commands can go out before a result comes back.
    let mut awaiting: Option<usize> = None;
    fn tally(seq: &mut Vec<(String, bool)>, out: &mut Reruns) {
        for i in 0..seq.len() {
            let again = seq[i + 1..seq.len().min(i + 4)]
                .iter()
                .any(|(c, _)| *c == seq[i].0);
            let slot = if seq[i].1 {
                &mut out.folded
            } else {
                &mut out.plain
            };
            slot.1 += 1;
            if again {
                slot.0 += 1;
                if seq[i].1 {
                    out.folded_reruns.push(seq[i].0.clone());
                }
            }
        }
        seq.clear();
    }
    for_each_block(
        root,
        |_project, _path, _ts, block| match block {
            Block::Bash { cmd, .. } => {
                let mut q = seq.borrow_mut();
                q.push((cmd.to_string(), false));
                awaiting = Some(q.len() - 1);
            }
            Block::Received { text, kind, .. } => {
                // **Only a RESULT settles a command.** A system reminder
                // arrives as a `text` block in a `user` message: counted as the
                // answer, it marked "not folded" a command that was.
                if kind != Recv::Result {
                    return;
                }
                if let Some(at) = awaiting.take() {
                    if carries_fold(&text) {
                        seq.borrow_mut()[at].1 = true;
                    }
                }
            }
        },
        // The end of a transcript settles its commands: a rerun does not span
        // two sessions.
        || tally(&mut seq.borrow_mut(), &mut out.borrow_mut()),
    );
    out.into_inner()
}

/// **Control CANARY**: a fabricated tree where the answer is known in advance.
///
/// Six runs for four commands: `A` (folded, then rerun — the
/// SECOND run of `A` is not folded and is not rerun), `B` (folded,
/// never rerun), `C` (bare, rerun) and `D` (bare, alone). Expected: folded
/// **1/2**, control **1/4**. The first version of this canary expected 1/3 —
/// the instrument was right and the expectation was wrong, which is exactly
/// what a control is there to uncover. An instrument that returns "0 everywhere"
/// and one that returns "everything rerun" are two broken instruments, in
/// opposite directions — and it is because TWO versions of this measurement
/// returned different numerators (3,843 versus 1,189 folded commands) that
/// this one does not ship without a control.
pub fn canary_reruns(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-rejeu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let call = |cmd: &str| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let result = |text: &str| {
        serde_json::json!({"message": {"role": "user", "content": [
            {"type": "tool_result", "content": text}]}})
        .to_string()
    };
    // A system reminder: a `text` block in a `user` message. Slipped between
    // a command and its result, it settled the command as "not folded".
    let reminder = serde_json::json!({"message": {"role": "user", "content": [
        {"type": "text", "text": "<system-reminder>note</system-reminder>"}]}})
    .to_string();
    let other = serde_json::json!({"message": {"content": [
        {"type": "tool_use", "name": "Read", "input": {"file_path": "A"}}]}})
    .to_string();
    let lines = [
        // A `Read` whose path is "A": if it counted as a command, it
        // would make `A` one more rerun and the control would lie.
        other,
        call("A"),
        reminder,
        result("[tokenade:ls 9 tokens folded away]"),
        call("A"),
        result("bare output"),
        call("B"),
        result("[tokenade:ls 9 tokens folded away]"),
        result("bare output"),
        call("C"),
        result("bare output"),
        call("C"),
        result("bare output"),
        call("D"),
        result("bare output"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let r = rerun_rates(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    // A folded+rerun, B folded+not rerun; on the control side the SECOND run of
    // A, both Cs and D — of which only the first C is rerun.
    let ok = r.folded == (1, 2) && r.plain == (1, 4);
    if ok {
        if verbose {
            println!("replay canary: folded 1/2, control 1/4 — instrument valid");
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"folded 1/2, control 1/4\", got folded {}/{}, \
         control {}/{}",
        r.folded.0, r.folded.1, r.plain.0, r.plain.1
    );
    1
}

/// **Was what the agent came back for already IN the view it had?**
///
/// The question the earlier measurements could not ask: they counted the
/// returns, not their OBJECT. When the recovery is written
/// `expand-ref <hash> --grep <pattern>`, the pattern says exactly what the agent
/// was looking for — and the block that announced that hash carries the folded
/// view it had in front of it. The two can be joined.
///
/// * `removed` — the pattern was NOT in the view: the fold had removed it, and the
///   return is the recovery path doing its job;
/// * `kept` — it WAS there: the agent went back for what it already had,
///   which is not a loss but a readability defect;
/// * `too_short` — a pattern shorter than four characters matches everywhere,
///   it decides nothing and is COUNTED rather than filed on one side;
/// * `no_view` — the banner is not in the files read.
///
/// The comparison is made on the view STRIPPED of its banners: otherwise a pattern
/// that appears in the banner's prose ("--grep", "pattern") would count as
/// kept.
#[derive(Default)]
pub struct GrepOutcomes {
    pub removed: usize,
    pub kept: usize,
    pub too_short: usize,
    pub no_view: usize,
    /// The searches whose result said "0 matching line", classified the
    /// same way. A pattern ABSENT from the view AND absent from the store is a TRUE
    /// negative — the agent was looking for a string never printed. A pattern PRESENT
    /// in the view but not found in the store would be a FALSE negative, which is
    /// "worse than no pointer at all".
    pub zero_true: usize,
    pub zero_false: usize,
    /// The patterns classified `kept`, with the family name of the fold that carried
    /// the view. A count alone does not say WHY the agent went back for what
    /// it had in front of it; the list does.
    pub kept_patterns: Vec<(String, String, String)>,
    /// The patterns classified `removed`: (fold family, pattern, fold hash). This is the list that says WHICH fold removes what
    /// the agent comes back for — the count alone points at no
    /// compactor to fix.
    pub removed_patterns: Vec<(String, String, String)>,
    /// The patterns PRESENT in the view to which `expand-ref --grep` answered
    /// "0 matching line".
    pub false_negatives: Vec<(String, String)>,
}

pub fn grep_outcomes(root: &Path) -> GrepOutcomes {
    let mut out = GrepOutcomes::default();
    let views: std::cell::RefCell<BTreeMap<String, String>> = Default::default();
    // **The join is the identifier**, as in `recovery_outcomes`: "the
    // next received block" catches the prose the assistant writes between the call
    // and its answer, and passes it off as the result.
    let waiting: std::cell::RefCell<BTreeMap<String, (String, String)>> = Default::default();
    let banner = regex::Regex::new(r"(?s)\[[^\]]*\]").expect("banner pattern");
    let quoted = regex::Regex::new(r#"expand-ref ([0-9a-f]{12}).*?--grep\s+(['"])(.+?)['"]"#)
        .expect("quoted --grep pattern");
    let bare = regex::Regex::new(r"expand-ref ([0-9a-f]{12}).*?--grep\s+(\S+)")
        .expect("bare --grep pattern");
    for_each_block(
        root,
        |_p, _f, _ts, block| match block {
            Block::Bash { id, cmd } => {
                let found = quoted
                    .captures(cmd)
                    .map(|c| (c[1].to_string(), c[3].to_string()))
                    .or_else(|| {
                        bare.captures(cmd)
                            .map(|c| (c[1].to_string(), c[2].to_string()))
                    });
                if let (Some(id), Some(f)) = (id, found) {
                    waiting.borrow_mut().insert(id, f);
                }
            }
            Block::Received { id, text, .. } => {
                if let Some((hash, pat)) = id.and_then(|i| waiting.borrow_mut().remove(&i)) {
                    let core = pat.trim_matches(['\'', '"']).to_string();
                    // A ZERO, not any count ending in 0: `30 matching line(s)` contains
                    // the needle as a substring and made 8 of 8 "false negatives"
                    // (2026-10-05) out of greps that had answered.
                    let zero = zero_matches(&text);
                    match views.borrow().get(&hash) {
                        _ if core.chars().count() < 4 => out.too_short += 1,
                        None => out.no_view += 1,
                        Some(view) => {
                            let body = banner.replace_all(view, " ");
                            // `--grep` takes a REGEX: `Test Files|Tests |FAIL` is never
                            // in the view as written, and a literal test classed every
                            // alternation as "removed" (2026-10-05: 23 of the 27
                            // non-stash cases). Literal only when it does not compile.
                            let present = match regex::Regex::new(&core) {
                                Ok(re) => body.lines().any(|l| re.is_match(l)),
                                Err(_) => body.contains(&core),
                            };
                            match (present, zero) {
                                (true, true) => {
                                    out.zero_false += 1;
                                    out.false_negatives.push((core.clone(), hash.clone()));
                                }
                                (false, true) => out.zero_true += 1,
                                (present, false) => {
                                    let fam = view
                                        .find(&hash)
                                        .map(|at| family_of(view, at))
                                        .unwrap_or_else(|| "?".into());
                                    if present {
                                        out.kept += 1;
                                        let line = sample(
                                            body.lines()
                                                .find(|l| l.contains(&core))
                                                .unwrap_or("")
                                                .trim(),
                                            90,
                                        );
                                        out.kept_patterns.push((fam, core.clone(), line));
                                    } else {
                                        out.removed += 1;
                                        out.removed_patterns.push((
                                            fam,
                                            core.clone(),
                                            hash.clone(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                // **Only the banner carries the view.** `folds_in` recognises any
                // mention of `expand-ref <hash>`, including those that are
                // not a fold: the context-resume reminder that lists the
                // biggest folds still recoverable, or the output of a previous
                // `expand-ref` — that is, the WHOLE bytes. Taken
                // as "the view", the latter passes off as "already in
                // view" exactly what the fold had removed. A mention outside
                // brackets is not a banner, and therefore opens no view.
                let mut hashes = Vec::new();
                folds_in(&text, &mut hashes);
                let mut v = views.borrow_mut();
                for (h, fam) in hashes {
                    if fam == "(no banner)" {
                        continue;
                    }
                    v.entry(h).or_insert_with(|| text.clone());
                }
            }
        },
        || waiting.borrow_mut().clear(),
    );
    out
}

/// **`--grep` join CANARY**: three searches whose answer is
/// known — one on text the fold kept, one on text it
/// removed, and one that returns "0 matching line" on a string never printed.
pub fn canary_greps(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-grep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let call = |id: &str, cmd: &str| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let result = |id: &str, text: &str| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": text}]}})
        .to_string()
    };
    // Received output that answers no call: a prose block. The
    // proximity join took it for the answer.
    let prose = |text: &str| {
        serde_json::json!({"message": {"content": [{"type": "text", "text": text}]}}).to_string()
    };
    let lines = [
        result("v0", "PATTERN-KEPT stays in the view\n[tokenade: 9 tokens folded away — `tokenade expand-ref aaaaaaaaaaaa`]"),
        call("g1", "tokenade expand-ref aaaaaaaaaaaa --grep 'PATTERN-KEPT'"),
        prose("checking what this hash contains"),
        result("g1", "PATTERN-KEPT stays in the view"),
        call("g2", "tokenade expand-ref aaaaaaaaaaaa --grep 'PATTERN-REMOVED'"),
        result("g2", "PATTERN-REMOVED shows up here"),
        call("g3", "tokenade expand-ref aaaaaaaaaaaa --grep 'PATTERN-NEVER'"),
        result("g3", "[tokenade:expand-ref] 0 matching line(s) of 4 total"),
        // A pattern that ONLY the banner carries: without stripping, the prose of
        // our own banner would pass itself off as kept content.
        result("v1", "nothing here\n[tokenade:auto PATTERN-BANNER 9 tokens folded away — `tokenade expand-ref bbbbbbbbbbbb`]"),
        call("g4", "tokenade expand-ref bbbbbbbbbbbb --grep 'PATTERN-BANNER'"),
        result("g4", "PATTERN-BANNER shows up here"),
        // Three characters decide nothing: counted separately, never filed.
        call("g5", "tokenade expand-ref aaaaaaaaaaaa --grep 'the'"),
        result("g5", "PATTERN-KEPT stays in the view"),
        // A mention OUTSIDE a banner — here a context-resume reminder, which
        // names the hash in prose — must not serve as a view: otherwise the pattern
        // it contains passes for content the fold had kept.
        result("v2", "Biggest folds still recoverable: `tokenade expand-ref cccccccccccc` PATTERN-REMINDER"),
        result("v3", "nothing here either\n[tokenade:auto 9 tokens folded away — `tokenade expand-ref cccccccccccc`]"),
        call("g6", "tokenade expand-ref cccccccccccc --grep 'PATTERN-REMINDER'"),
        result("g6", "PATTERN-REMINDER shows up here"),
        // `--grep` takes a REGEX: an alternation one branch of which is in the
        // view is KEPT. Read as a literal, it passed for removed.
        call("g7", "tokenade expand-ref aaaaaaaaaaaa --grep 'PATTERN-ABSENT|PATTERN-KEPT'"),
        result("g7", "PATTERN-KEPT stays in the view"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let g = grep_outcomes(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let ok =
        g.kept == 2 && g.removed == 3 && g.zero_true == 1 && g.zero_false == 0 && g.too_short == 1;
    if ok {
        if verbose {
            println!(
                "grep canary: 2 kept (one of them an alternation), 3 removed, 1 true negative, 1 too short — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"2 kept, 3 removed, 1 true negative, 0 false, \
         1 too short\", got kept {}, removed {}, true {}, false {}, short {}",
        g.kept, g.removed, g.zero_true, g.zero_false, g.too_short
    );
    1
}

/// `expand-ref --grep` answered "nothing": the count before `matching line`
/// is exactly 0.
fn zero_matches(text: &str) -> bool {
    text.match_indices("0 matching line")
        .any(|(at, _)| !text[..at].ends_with(|c: char| c.is_ascii_digit()))
}

#[cfg(test)]
mod zero_matches_tests {
    #[test]
    fn only_a_zero_count_is_a_miss() {
        assert!(super::zero_matches(
            "[tokenade:expand-ref] 0 matching line(s) of 62 held under this hash"
        ));
        assert!(!super::zero_matches(
            "[tokenade:expand-ref] 30 matching line(s) of 62 held under this hash"
        ));
        assert!(!super::zero_matches("100 matching lines"));
    }
}
