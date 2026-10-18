// The tests of `spec`.
//
// Spliced by `include!` from `spec.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {

    /// A strip that leaves the output enormous has not compacted it, and
    /// claiming it blocks the only thing that would.
    ///
    /// `Shape::Strip` drops the prefixes a spec names and returns the rest.
    /// It used to return `Some` unconditionally, so `readelf -a` — 161 038
    /// tokens, of which the spec removes two section-header prefixes to
    /// leave 132 053 — was CLAIMED, and the generic fold that returns 5 483
    /// on the same bytes never ran. The existing safety net compares a
    /// compaction against doing nothing (18% is positive, so it passed) and
    /// never against the fallback.
    ///
    /// The two fixtures differ ONLY in size: same noise prefix, same
    /// proportion stripped. Either alone would pass with the rule removed
    /// (the small one) or with the shape gutted (the large one).
    #[test]
    fn a_strip_declines_when_what_it_leaves_is_still_huge() {
        let spec = Spec {
            key: "probe_strip",
            detect: Detect::NonEmpty,
            shape: Shape::Strip,
            head: 0,
            label: "",
            unit: "probe row",
            strip: &["  NOISE"],
        };
        let body = |rows: usize| -> String {
            (0..rows)
                .map(|i| {
                    if i % 5 == 0 {
                        format!("  NOISE line {i} that the spec is written to drop\n")
                    } else {
                        format!("keeper {i}: a line of ordinary content that survives the strip\n")
                    }
                })
                .collect()
        };

        let small = body(40);
        let out = apply(&spec, &small).expect("a small strip still claims its output");
        assert!(
            !out.contains("NOISE"),
            "the strip did not run on the small fixture"
        );

        let large = body(20_000);
        assert!(
            crate::tokenizer::estimate(&archetypes::strip_noise(&large, spec.strip, spec.unit))
                > crate::disclose::threshold_tokens(),
            "the large fixture is not actually large after stripping — it proves nothing"
        );
        assert!(
            apply(&spec, &large).is_none(),
            "a strip leaving {} tokens claimed the output, blocking the generic fold",
            crate::tokenizer::estimate(&large)
        );

        // Third fixture, and the one that makes the rule mean what it says:
        // a HUGE input the strip cuts down to almost nothing. The bar is
        // what is LEFT, not what arrived — a spec that legitimately removes
        // 98% has compacted the output and must still claim it. Without
        // this case, measuring the input instead of the remainder passes.
        let mostly_noise: String = (0..20_000)
            .map(|i| {
                if i % 500 == 0 {
                    format!("keeper {i}: the handful of lines this spec exists to surface\n")
                } else {
                    format!("  NOISE line {i} that the spec is written to drop\n")
                }
            })
            .collect();
        assert!(
            crate::tokenizer::estimate(&mostly_noise) > crate::disclose::threshold_tokens(),
            "the mostly-noise fixture is not large on input — it proves nothing"
        );
        let out = apply(&spec, &mostly_noise)
            .expect("a strip that removed almost everything must still claim its output");
        assert!(!out.contains("NOISE"), "the strip did not run");
    }
    use super::*;

    /// Inputs that exercise every branch of every shape.
    fn battery() -> Vec<String> {
        vec![
            String::new(),
            "   \n".into(),
            "single line".into(),
            // table-shaped (header + dashed rule + rows)
            {
                let mut s = String::from("NAME   STATUS   AGE\n");
                for i in 0..120 {
                    s.push_str(&format!("svc{i}   Running  {i}h\n"));
                }
                s
            },
            // key:value records
            {
                let mut s = String::new();
                for i in 0..120 {
                    s.push_str(&format!("key{i}: value {i}\nname: thing {i}\n\n"));
                }
                s
            },
            // long flat output
            (0..200)
                .map(|i| format!("plain row {i} of output"))
                .collect::<Vec<_>>()
                .join("\n"),
            // output carrying common noise prefixes (for Strip)
            (0..80)
                .map(|i| format!("(see the transcript file for row {i})"))
                .collect::<Vec<_>>()
                .join("\n"),
            // header + dashed rule: a table by one detector's reckoning and
            // not by another's, which is where the two ladders part ways.
            {
                let mut s = String::from("KEY        VALUE\n---------  ---------\n");
                for i in 0..90 {
                    s.push_str(&format!("net.k{i}    {i}\n"));
                }
                s
            },
            // `key = value` assignments (sysctl, .ini, kernel params)
            (0..300)
                .map(|i| format!("net.ipv4.conf.k{i} = {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            // path:line:text — what a search tool emits
            (0..40)
                .map(|i| format!("/path/to/file{i}.rs:{i}:  some matched text here"))
                .collect::<Vec<_>>()
                .join("\n"),
            // a commit followed by a diff
            format!(
                "commit {}\nAuthor: A <a@b.c>\n\ndiff --git a/x b/x\n{}",
                "a".repeat(40),
                (0..150)
                    .map(|i| format!("+added line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ]
    }

    /// Behavioural snapshot (Phase A parity vs. the original stubs was proven
    /// in commit 6f54e16a, before the stub files were deleted; the byte
    /// equivalence is guaranteed by construction since `apply` calls the same
    /// archetype functions). This guards `apply` against future regressions and
    /// confirms the table is well-formed.
    #[test]
    fn specs_well_formed_and_apply_per_shape() {
        let specs = crate::cmd::spec_table::BUILTIN_SPECS;
        assert!(specs.len() > 2000, "table populated: {}", specs.len());
        let inputs = battery();
        for shape in [Shape::TableHeadTail, Shape::TableKvHeadTail, Shape::Strip] {
            let spec = specs
                .iter()
                .find(|s| s.shape == shape)
                .expect("each shape present in the table");
            // Empty input is always a pass-through (None).
            assert_eq!(apply(spec, "  \n"), None, "{shape:?} empty → None");
            // No input ever panics, and a non-empty run yields a value for
            // shapes that fold unconditionally (Strip) or on large input.
            for inp in &inputs {
                let _ = apply(spec, inp);
            }
        }
        // Hyphen alias mirrors build.rs: a `_` key is reachable hyphenated.
        let big = "plain row of output\n".repeat(200);
        if let Some(s) = specs.iter().find(|s| s.key.contains('_')) {
            assert_eq!(
                dispatch(&s.key.replace('_', "-"), &big).is_some(),
                dispatch(s.key, &big).is_some(),
                "hyphen and underscore forms resolve to the same spec",
            );
        }
    }

    /// An input crafted to FIRE a spec's detect gate (so the ladder + params,
    /// not just the gate, get exercised), padded long enough to fold.
    /// One triggering input PER alternative the gate accepts, not one for the
    /// whole gate.
    ///
    /// `trigger_input` fired `xs[0]` and `ds[0]` only, so every needle after the
    /// first went unprobed: a row whose gate accepts something its stub does not
    /// passed parity untouched. That is not hypothetical — `nmap`'s row reads
    /// `ContainsAny(&["Nmap scan report for", "Host is up", "PORT     STATE"])`
    /// while its stub reads `"Starting Nmap" || ("Nmap scan report" && "PORT")`,
    /// and the harness never built an input carrying only `"Host is up"`.
    fn trigger_inputs(spec: &Spec) -> Vec<String> {
        fn alternatives(d: &Detect) -> Vec<Detect> {
            match d {
                Detect::ContainsAny(xs) => xs
                    .iter()
                    .map(|x| Detect::ContainsAny(std::slice::from_ref(x)))
                    .collect(),
                Detect::StartsWithAny(ps) => ps
                    .iter()
                    .map(|p| Detect::StartsWithAny(std::slice::from_ref(p)))
                    .collect(),
                Detect::LineStartsWithAny(ps) => ps
                    .iter()
                    .map(|p| Detect::LineStartsWithAny(std::slice::from_ref(p)))
                    .collect(),
                // Each branch of an `Any` is its own way in; an `All` is one.
                Detect::Any(ds) => ds.iter().flat_map(alternatives).collect(),
                other => vec![*other],
            }
        }
        alternatives(&spec.detect)
            .into_iter()
            .map(|d| trigger_input_for(&d))
            .collect()
    }

    fn trigger_input_for(detect: &Detect) -> String {
        fn fire(d: &Detect, pad: &str) -> String {
            match d {
                Detect::ContainsAny(xs) => format!("line with {} here\n{pad}", xs[0]),
                Detect::StartsWithAny(ps) => format!("{}rest of first line\n{pad}", ps[0]),
                Detect::LineStartsWithAny(ps) => format!("noise\n{}rest\n{pad}", ps[0]),
                Detect::Any(ds) => fire(&ds[0], pad),
                // Satisfy every conjunct: concatenate each one's trigger.
                Detect::All(ds) => {
                    let body: String = ds.iter().map(|d| fire(d, "")).collect();
                    format!("{body}{pad}")
                }
                // Long enough to clear the gate, and shaped like a table so the
                // ladder itself gets exercised too.
                Detect::LineLongerThan(n) => format!("{}\n{pad}", "x".repeat(n + 1)),
                Detect::TableOrLong | Detect::NonEmpty => format!("NAME STATUS AGE\n{pad}"),
            }
        }
        let pad: String = (0..120).map(|i| format!("row {i} data\n")).collect();
        fire(detect, &pad)
    }

    /// Do the ten pinned divergences survive REALISTIC input?
    ///
    /// `every_row_with_a_live_stub_reproduces_it` feeds every row the same
    /// generic battery, so seven of the ten differ only on a Kubernetes-shaped
    /// `NAME STATUS AGE` table handed to `id`, `uptime`, `which`, … — output
    /// those commands cannot produce. A divergence reachable only by impossible
    /// input is a different (and much smaller) problem than one reachable by a
    /// real run, and the decision about the ten should not confuse the two.
    #[test]
    fn the_pinned_divergences_on_realistic_output() {
        // `Shape::Strip` declines when what it leaves would still be stashed,
        // so this comparison reads `disclose::threshold_tokens()` — a
        // process-global that tests in `disclose` set to 20 while they run.
        // They hold `ENV_LOCK` for it; this reader did not take that lock, and
        // a lock only protects the parties that take it: the two raced, and
        // three of the pinned rows flipped in two full-workspace runs out of
        // five (`nmap` stopped diverging, `npm_audit` and `npm_install`
        // started). Take the lock, and read the rule at its stated threshold
        // rather than at whatever the ambient environment happens to say.
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ambient: Vec<(&str, Option<String>)> =
            ["TOKENADE_DISCLOSE_THRESHOLD", "TOKENADE_PRESSURE"]
                .into_iter()
                .map(|k| (k, std::env::var(k).ok()))
                .collect();
        for (k, _) in &ambient {
            std::env::remove_var(k);
        }
        let restore_ambient = || {
            for (k, v) in &ambient {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        };

        let samples: &[(&str, &str)] = &[
            ("id", "uid=1000(user) gid=1000(user) groups=1000(user),27(sudo),998(docker)\n"),
            ("uptime", " 14:22:01 up 12 days,  3:41,  2 users,  load average: 0.42, 0.31, 0.28\n"),
            ("which", "/usr/bin/git\n"),
            ("type", "git is /usr/bin/git\n"),
            ("wall", "Broadcast message from root@host (pts/0) (Tue Aug  5 14:22:01 2026):\n\nrebooting now\n"),
            ("write", "Message from user@host on pts/1 at 14:22 ...\nhello\n"),
            ("shutdown", "Shutdown scheduled for Tue 2026-08-05 15:00:00 CEST, use 'shutdown -c' to cancel.\n"),
            ("ls", "total 24\ndrwxr-xr-x 2 user user 4096 Aug  5 14:22 src\n-rw-r--r-- 1 user user  220 Aug  5 14:22 Cargo.toml\n"),
            ("route", "Kernel IP routing table\nDestination     Gateway         Genmask         Flags Metric Ref    Use Iface\n0.0.0.0         192.168.1.1     0.0.0.0         UG    100    0        0 eth0\n"),
            ("nmap", "Starting Nmap 7.94 ( https://nmap.org ) at 2026-08-05 14:22 CEST\nNmap scan report for 192.168.1.10\nHost is up (0.0012s latency).\nPORT     STATE SERVICE\n22/tcp   open  ssh\n"),
        ];
        let by_key: std::collections::HashMap<&str, &Spec> = crate::cmd::spec_table::BUILTIN_SPECS
            .iter()
            .map(|s| (s.key, s))
            .collect();
        let mut real: Vec<&str> = Vec::new();
        for (key, sample) in samples {
            let Some(spec) = by_key.get(key) else {
                continue;
            };
            if apply(spec, sample) != crate::cmd::dispatch_auto(key, sample) {
                real.push(key);
            }
        }
        // MEASURED 2026-08-05: exactly ONE of the ten survives realistic input.
        //
        // `nmap`'s stub is a BESPOKE parser — it counts the `Starting Nmap`
        // lines it drops and keeps `Not shown:` abbreviated — and this module's
        // own doc says such stubs "keep their own module and are never
        // represented here". It was migrated anyway, to `Shape::Strip` with
        // prefixes `["Starting Nmap ", "Initiating "]`, which cannot express any
        // of that (and strips a trailing space the stub does not). Since
        // `spec::dispatch` runs before `dispatch_auto`, the approximation WINS.
        //
        // The fix is to drop the row so the real parser is reached again — not
        // applied here: `spec_table.rs` is generated and says "do not edit by
        // hand", and a hand-deletion would come back on the next regeneration.
        // Owner decision, with the row's own module doc as the argument.
        restore_ambient();
        assert_eq!(
            real,
            vec!["nmap"],
            "the set of divergences reachable by REAL command output changed"
        );
    }

    /// Every row that still HAS a stub must reproduce it.
    ///
    /// The module doc claimed this was proven by `builtin_spec_parity_with_stubs`
    /// — a test that does not exist, and never did. The only other guard,
    /// `candidate_specs_match_live_stubs`, returns on an empty `CANDIDATE_KEYS`
    /// (a documented no-op since the Phase-C stubs were deleted). So the 3292-row
    /// `BUILTIN_SPECS` table, which governs built-in command compaction, was
    /// guarded by nothing while saying otherwise.
    ///
    /// `AUTO_DISPATCH_KEYS` (build.rs, from the directory contents) is what makes
    /// this selectable: a row whose stub is gone has nothing to compare against
    /// and is skipped — visibly, via the count this asserts.
    #[test]
    fn every_row_with_a_live_stub_reproduces_it() {
        // `Shape::Strip` declines when what it leaves would still be stashed,
        // so this comparison reads `disclose::threshold_tokens()` — a
        // process-global that tests in `disclose` set to 20 while they run.
        // They hold `ENV_LOCK` for it; this reader did not take that lock, and
        // a lock only protects the parties that take it: the two raced, and
        // three of the pinned rows flipped in two full-workspace runs out of
        // five (`nmap` stopped diverging, `npm_audit` and `npm_install`
        // started). Take the lock, and read the rule at its stated threshold
        // rather than at whatever the ambient environment happens to say.
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ambient: Vec<(&str, Option<String>)> =
            ["TOKENADE_DISCLOSE_THRESHOLD", "TOKENADE_PRESSURE"]
                .into_iter()
                .map(|k| (k, std::env::var(k).ok()))
                .collect();
        for (k, _) in &ambient {
            std::env::remove_var(k);
        }
        let restore_ambient = || {
            for (k, v) in &ambient {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        };

        let live: std::collections::HashSet<&str> =
            crate::cmd::AUTO_DISPATCH_KEYS.iter().copied().collect();
        let mut compared = 0usize;
        let mut diverging: Vec<&str> = Vec::new();
        for spec in crate::cmd::spec_table::BUILTIN_SPECS {
            if !live.contains(spec.key) {
                continue;
            }
            compared += 1;
            let mut inputs = battery();
            inputs.extend(trigger_inputs(spec));
            for i in &inputs {
                let (a, b) = (apply(spec, i), crate::cmd::dispatch_auto(spec.key, i));
                if a != b {
                    if std::env::var("TOKENADE_DUMP_PARITY").is_ok() {
                        let dir = match (a.is_some(), b.is_some()) {
                            (true, false) => "ROW-FIRES-STUB-DOESNT",
                            (false, true) => "STUB-FIRES-ROW-DOESNT",
                            _ => "BOTH-FIRE-DIFFERENT-OUTPUT",
                        };
                        let head = i
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(70)
                            .collect::<String>();
                        eprintln!("PARITY {} {} | 1st line: {:?}", dir, spec.key, head);
                    }
                    diverging.push(spec.key);
                    break;
                }
            }
        }
        assert!(compared > 0, "no row had a live stub — the guard is inert");
        // MEASURED 2026-08-05, not accepted: these ten rows do NOT reproduce the
        // stub they replaced, and the row is what production runs (`spec::dispatch`
        // is tried before `dispatch_auto`). Seven produce DIFFERENT OUTPUT where
        // both fire; three (`ls`, `nmap`, `route`) fire where the stub declines,
        // so output that used to pass through is now compacted.
        //
        // Pinned rather than silently fixed: which side is correct is a product
        // decision, and rewriting a row to match a stub that may itself be the
        // stale one would be a guess. `TOKENADE_DUMP_PARITY=1` prints the
        // direction for each.
        //
        // `assert_eq!` and not "is a subset": a row that starts diverging fails
        // here, and so does one that gets FIXED — which keeps this list honest
        // instead of letting it rot into a permanent exemption.
        //
        // `get_childitem` (2026-10-05): by design. Its compactor now reads a
        // RECURSIVE listing only and declines a single table, which the row
        // (tried after it, `cmd::dispatch_named`) keeps handling.
        const KNOWN_DIVERGENT: &[&str] = &[
            "get_childitem", "id", "ls", "route", "shutdown", "type", "uptime", "wall", "which", "write", "nmap",
        ];
        restore_ambient();
        assert_eq!(
            diverging, KNOWN_DIVERGENT,
            "the set of rows diverging from their stub changed ({compared} rows compared)"
        );
    }

    /// Phase-C gate: every CANDIDATE spec (whose stub file still exists) must be
    /// byte-identical to that live stub via `dispatch_auto`, on both the generic
    /// battery and a detect-triggering input. Lets the candidate stubs be
    /// deleted. Becomes a no-op once `CANDIDATE_KEYS` is emptied post-deletion.
    #[test]
    fn candidate_specs_match_live_stubs() {
        let cand: std::collections::HashSet<&str> = crate::cmd::spec_table::CANDIDATE_KEYS
            .iter()
            .copied()
            .collect();
        if cand.is_empty() {
            return;
        }
        let mut mismatches = Vec::new();
        for spec in crate::cmd::spec_table::BUILTIN_SPECS {
            if !cand.contains(spec.key) {
                continue;
            }
            let mut inputs = battery();
            inputs.extend(trigger_inputs(spec));
            for inp in &inputs {
                if apply(spec, inp) != crate::cmd::dispatch_auto(spec.key, inp) {
                    mismatches.push(spec.key);
                    break;
                }
            }
        }
        if std::env::var("TOKENADE_DUMP_PARITY").is_ok() {
            let _ = std::fs::write("/tmp/claude-1000/diverge_c.txt", mismatches.join("\n"));
        }
        assert!(
            mismatches.is_empty(),
            "{} candidate specs diverge from their stub (first few: {:?})",
            mismatches.len(),
            &mismatches[..mismatches.len().min(15)]
        );
    }

    /// A real `node app.js` crash: three frames of the program, seven of the
    /// runtime, whose locations sit at the END of their lines.
    #[test]
    fn a_node_crash_keeps_the_program_frames_and_counts_the_runtime_ones() {
        let raw = "Error: ENOENT: no such file or directory, open './missing-budget.json'\n    at Object.readFileSync (node:fs:441:20)\n    at loadBudget (app.js:2:47)\n    at main (app.js:3:85)\n    at Module._compile (node:internal/modules/cjs/loader:1706:14)\n    at wrapModuleLoad (node:internal/modules/cjs/loader:237:24) {\n  code: 'ENOENT',\n}\n\nNode.js v22.21.1\n";
        let out = dispatch("node", raw).expect("node row claims a JS stack");
        assert_eq!(
            out,
            "Error: ENOENT: no such file or directory, open './missing-budget.json'\n    at loadBudget (app.js:2:47)\n    at main (app.js:3:85)\n    [3 runtime frame(s) not shown] {\n  code: 'ENOENT',\n}\n\nNode.js v22.21.1\n"
        );
    }
}
