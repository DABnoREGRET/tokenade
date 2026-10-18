//! **Which fold, and how big.**
//!
//! A fold's banner names the FAMILY that produced it and carries a hash. The
//! two together answer "which of our folds sends the agent looking for what
//! was removed from it", and "was the stash it reopens big or
//! tiny". This is the census; what the agent DOES next is measured
//! in the neighbouring modules.

use super::*;

/// What a fold family cost: how many times it folded, and how many
/// times the agent came back for what it had removed.
#[derive(Default, Clone)]
pub struct FamilyTally {
    pub folds: usize,
    /// DISTINCT folds the agent came back for. It is the numerator of the
    /// rate: the same fold requested three times remains one fold that fell short.
    pub recovered: usize,
    /// Calls, all counted. The first table gave 200 % and 300 % because
    /// it counted this in the numerator — `expand-ref` is rerun with `--grep`
    /// then `--prompt` on the same hash, and a rate above 100 % is the
    /// sign that two different things are being divided.
    pub calls: usize,
}

#[derive(Default)]
pub struct Mining {
    pub files: usize,
    /// Fold banners carrying a hash, per family.
    pub by_family: BTreeMap<String, FamilyTally>,
    /// The same count, crossed with the PROJECT. A compactor can suit
    /// everywhere and slip on one domain — XML, a data repo, an
    /// in-house tool's output. The total drowns it; the cross-tab names it.
    pub by_family_project: BTreeMap<(String, String), FamilyTally>,
    /// Hashes requested by `expand-ref` that no banner read carries — a
    /// session whose fold fell outside the files walked, or a hash
    /// copied by hand. Counted rather than hushed: they say what the join
    /// does not cover.
    pub orphan_recoveries: usize,
    /// Occurrences of `expand-ref <hash>` in RECEIVED output that are
    /// NOT a fold banner: a test that prints the string, a
    /// `gain --history` that lists past commands, an error message.
    /// Removed from the denominator — counting them as folds inflated the
    /// denominator by 388 and made the instrument say that a quarter of the
    /// "bannerless" folds were requested.
    pub mentions_outside_banner: usize,
    /// Recoveries whose hash was indeed SEEN, but outside a fold banner:
    /// they are explained, they simply do not measure a fold.
    pub recoveries_of_a_mention: usize,
    /// The orphan calls themselves: `(project, command, does the local store
    /// hold this hash?)`. Forty cases can be read one by one; a count cannot.
    /// The third column settles the only remaining question: did the
    /// fold EXIST? A hash the store holds was a real fold whose
    /// banner lives in a session outside the corpus; a hash not found
    /// anywhere may never have been announced.
    pub orphan_samples: Vec<(String, String, bool)>,
    /// The other escape hatches, which join no fold.
    pub raw_calls: usize,
    pub hook_disabled: usize,
    /// Projects touched, to say whether the measurement only speaks about us.
    pub projects: BTreeMap<String, usize>,
}

impl Mining {
    pub fn folds(&self) -> usize {
        self.by_family.values().map(|t| t.folds).sum()
    }
    pub fn recovered(&self) -> usize {
        self.by_family.values().map(|t| t.recovered).sum()
    }
    pub fn rate(&self) -> f64 {
        let f = self.folds();
        if f == 0 {
            0.0
        } else {
            self.recovered() as f64 / f as f64
        }
    }
}

/// Walks the transcript tree and joins the folds to their recoveries.
pub fn mine(root: &Path) -> Mining {
    let mut m = Mining::default();
    // hash → family, fed by the banners READ; a recovery that does not
    // find its hash there is an orphan.
    // hash → (family, project where the banner was read)
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    // (hash, project, command) — the command is used to READ the orphans.
    let mut asked: Vec<(String, String, String)> = Vec::new();
    // **Announced is not FOLDED.** A hash quoted outside a banner — a
    // `gain --history` that lists past commands, a test that prints the
    // string — is not a fold and does not count in the denominator. But
    // it EXPLAINS the recovery that follows, so forgetting it inflated the
    // "orphans" by everything the denominator gate set aside.
    let mut announced_anywhere: std::collections::BTreeSet<String> = Default::default();
    let mut files = 0usize;
    for_each_block(
        root,
        |project, _path, _ts, block| match block {
            Block::Bash { cmd, .. } => {
                if cmd.contains("TOKENADE_HOOK_DISABLED=1") {
                    m.hook_disabled += 1;
                }
                if is_raw_call(cmd) {
                    m.raw_calls += 1;
                }
                if let Some(h) = expand_ref_call(cmd) {
                    asked.push((h, project.to_string(), sample(cmd, 110)));
                    *m.projects.entry(project.to_string()).or_default() += 1;
                }
            }
            Block::Received { text, .. } => {
                let mut hashes = Vec::new();
                folds_in(&text, &mut hashes);
                for (h, fam) in hashes {
                    announced_anywhere.insert(h.clone());
                    if fam == "(no banner)" {
                        m.mentions_outside_banner += 1;
                        continue;
                    }
                    seen.entry(h).or_insert((fam, project.to_string()));
                }
            }
        },
        || files += 1,
    );
    m.files = files;
    for (fam, project) in seen.values() {
        m.by_family.entry(fam.clone()).or_default().folds += 1;
        m.by_family_project
            .entry((fam.clone(), project.clone()))
            .or_default()
            .folds += 1;
    }
    let mut counted: std::collections::BTreeSet<&String> = Default::default();
    for (h, project, cmd) in &asked {
        match seen.get(h) {
            Some((fam, seen_in)) => {
                let t = m.by_family.entry(fam.clone()).or_default();
                t.calls += 1;
                let first = counted.insert(h);
                if first {
                    t.recovered += 1;
                }
                let c = m
                    .by_family_project
                    .entry((fam.clone(), seen_in.clone()))
                    .or_default();
                c.calls += 1;
                if first {
                    c.recovered += 1;
                }
            }
            None if announced_anywhere.contains(h) => m.recoveries_of_a_mention += 1,
            None => {
                m.orphan_recoveries += 1;
                m.orphan_samples
                    .push((project.clone(), cmd.clone(), stash_holds(h)));
            }
        }
    }
    m
}

/// **CANARY**: a KNOWN defect the instrument must see.
///
/// The fixtures reproduce the TWO real spellings of a banner carrying a
/// hash — the first version of this canary had invented them, the hash placed
/// outside the bracket, and it declared valid an attribution that gave
/// `tokenade:toml` 58.8 % returns on a fold that prints no hash.
///
/// Three things, and the three verdicts land TOGETHER:
///
/// * a generic fold whose hash is REQUESTED further on — the join must return
///   it, under the name of the banner that wrote it;
/// * a `[stashed: …]` footer nobody requests — it counts as a fold and
///   NOT as a recovery, otherwise the instrument would confuse "folded" and
///   "requested again";
/// * a recovery whose hash was never announced — it comes out as an
///   ORPHAN rather than being attributed at random.
pub fn canary(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-plis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("un-projet");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let result = |text: &str| {
        serde_json::json!({"message": {"content": [{"type": "text", "text": text}]}}).to_string()
    };
    let call = |cmd: &str| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let a = [
        result(
            "[tokenade:said-once] une phrase dite une fois\n\
             [tokenade: 42 tokens folded away — recover with `tokenade expand-ref aaaaaaaaaaaa`]",
        ),
        result(
            "[disclose:summary kind=md tokens=9 lines=3]\n\
             [stashed: /tmp/bbbbbbbbbbbb.md · ~9 tokens · `tokenade expand-ref bbbbbbbbbbbb --grep x`]",
        ),
        call("tokenade expand-ref aaaaaaaaaaaa"),
        call("tokenade expand-ref cccccccccccc --prompt \"que dit la section 3 ?\""),
        // **A command's text can be OUR banner.** On a cold
        // resume, tokenade elides old commands and the transcript keeps
        // the banner IN PLACE of the command. Read naively, it becomes one more
        // call — and one more orphan.
        call("[tokenade:elided 90 tokens on cold resume — recover: tokenade expand-ref dddddddddddd]"),
        // A command that QUOTES the line without being a call: a call names the
        // program.
        call("grep -n 'tokenade expand-ref eeeeeeeeeeee' docs/notes.md"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), a).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let m = mine(&dir);
    let gen = m
        .by_family
        .get("tokenade:generic")
        .cloned()
        .unwrap_or_default();
    let stashed = m.by_family.get("stashed").cloned().unwrap_or_default();
    let ok = gen.folds == 1
        && gen.recovered == 1
        && stashed.folds == 1
        && stashed.recovered == 0
        && m.orphan_recoveries == 1;
    let _ = std::fs::remove_dir_all(&dir);
    if ok {
        if verbose {
            println!("canary: 1 fold requested, 1 silent fold, 1 orphan — instrument valid");
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"generic 1/1, stashed 1/0, 1 orphan\", \
         got generic {}/{}, stashed {}/{}, {} orphan(s)",
        gen.folds, gen.recovered, stashed.folds, stashed.recovered, m.orphan_recoveries
    );
    1
}

/// **Is the most requested fold the one that keeps the least?**
///
/// The per-family table gives `stashed` **28 % requests**
/// versus 0.1 % for `elided`: putting a WHOLE output behind a pointer
/// sends the agent looking for it one time in four, whereas eliding lines
/// in the middle almost never does. The next question is a
/// tuning one: does this rate depend on the stashed SIZE? If small stashes are
/// requested much more often than big ones, the threshold is too low — the
/// round trip costs more than what the fold saved.
///
/// The size is written in the banner itself (`· ~N tokens ·`), so it can be
/// read without recomputing anything.
#[derive(Default)]
pub struct StashSizes {
    /// size bucket → (folds announced, folds requested, requested by a BARE
    /// call). A bare call asks again for EVERYTHING that was stashed: the fold saved
    /// nothing that time, it only shifted the spend by one turn.
    /// A `--grep` or a `--prompt` returns only a slice — that is where the
    /// mechanism wins.
    pub buckets: BTreeMap<&'static str, (usize, usize, usize)>,
}
pub(super) fn size_bucket(tokens: usize) -> &'static str {
    match tokens {
        0..=999 => "< 1k tokens",
        1_000..=4_999 => "1k – 5k",
        5_000..=19_999 => "5k – 20k",
        20_000..=99_999 => "20k – 100k",
        _ => "> 100k",
    }
}

/// The size a `[stashed: … · ~N tokens · …]` banner announces for this hash.
pub(super) fn stashed_size(text: &str, hash: &str) -> Option<usize> {
    let at = text.find(hash)?;
    let start = text[..at].rfind("[stashed:")?;
    // The `~N tokens` is written AFTER the path, so after the first
    // occurrence of the hash: the window goes from the opening bracket to its closing, not
    // from the bracket to the hash. Written that way at first, the measurement returned zero buckets
    // and the canary said so.
    let end = text[start..]
        .find(']')
        .map(|k| start + k)
        .unwrap_or(text.len());
    let seg = &text[start..end];
    let n = seg.split_once('~')?.1;
    let digits: String = n.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

pub fn stash_sizes(root: &Path) -> StashSizes {
    let mut out = StashSizes::default();
    // hash → bucket, for the `stashed` folds only whose size is written.
    let mut size_of: BTreeMap<String, &'static str> = BTreeMap::new();
    // hash → shape of the FIRST call that requested it.
    let mut asked: BTreeMap<String, &'static str> = BTreeMap::new();
    for_each_block(
        root,
        |_p, _f, _ts, block| match block {
            Block::Bash { cmd, .. } => {
                if let Some(h) = expand_ref_call(cmd) {
                    asked.entry(h).or_insert_with(|| call_shape(cmd));
                }
            }
            Block::Received { text, kind, .. } => {
                if kind != Recv::Result {
                    return;
                }
                let mut hashes = Vec::new();
                folds_in(&text, &mut hashes);
                for (h, fam) in hashes {
                    if fam != "stashed" {
                        continue;
                    }
                    if let Some(n) = stashed_size(&text, &h) {
                        size_of.entry(h).or_insert_with(|| size_bucket(n));
                    }
                }
            }
        },
        || {},
    );
    for (h, bucket) in &size_of {
        let e = out.buckets.entry(bucket).or_insert((0, 0, 0));
        e.0 += 1;
        if let Some(shape) = asked.get(h) {
            e.1 += 1;
            if *shape == "bare" || *shape == "raw" {
                e.2 += 1;
            }
        }
    }
    out
}

/// **Sizes CANARY**: four stashed folds whose bucket and fate are known
/// — a small one requested, a small one silent, a big one requested, and a banner from
/// ANOTHER family that also carries a `~N tokens` and must not get in.
pub fn canary_stash_sizes(verbose: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!("tokenade-canari-taille-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("p");
    if std::fs::create_dir_all(&proj).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary tree");
        return 1;
    }
    let res = |text: &str| {
        serde_json::json!({"message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "r", "content": text}]}})
        .to_string()
    };
    let call = |cmd: &str| {
        serde_json::json!({"message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "c", "name": "Bash", "input": {"command": cmd}}]}})
        .to_string()
    };
    let stashed = |h: &str, n: usize| {
        format!("corps\n[stashed: /tmp/{h}.txt · ~{n} tokens · `tokenade expand-ref {h}`]")
    };
    let lines = [
        res(&stashed("aaaaaaaaaaaa", 400)),
        call("tokenade expand-ref aaaaaaaaaaaa"),
        res(&stashed("bbbbbbbbbbbb", 800)),
        res(&stashed("cccccccccccc", 45000)),
        call("tokenade expand-ref cccccccccccc --prompt \"section 3 ?\""),
        // A fold from ANOTHER family, with a size in its banner: it does not
        // go here, otherwise the measurement mixes two mechanisms.
        res("x\n[tokenade:generic ~9000 tokens folded away — `tokenade expand-ref dddddddddddd`]"),
        call("tokenade expand-ref dddddddddddd"),
    ]
    .join("\n");
    if std::fs::write(proj.join("s.jsonl"), lines).is_err() {
        eprintln!("INSTRUMENT NOT VALIDATED — cannot write the canary transcript");
        return 1;
    }
    backdate(&proj.join("s.jsonl"));
    let z = stash_sizes(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let get = |b: &str| z.buckets.get(b).copied().unwrap_or((0, 0, 0));
    // The small one is requested BARE (everything asked again), the big one by `--prompt` (a
    // slice): the difference is the heart of the measurement.
    let ok =
        get("< 1k tokens") == (2, 1, 1) && get("20k – 100k") == (1, 1, 0) && z.buckets.len() == 2;
    if ok {
        if verbose {
            println!(
                "sizes canary: < 1k → 1/2 (1 bare), 20k–100k → 1/1 (0 bare) — instrument valid"
            );
        }
        return 0;
    }
    eprintln!(
        "INSTRUMENT NOT VALIDATED — expected \"< 1k → 1/2, 20k–100k → 1/1, two buckets\", \
         got {:?}",
        z.buckets
    );
    1
}
