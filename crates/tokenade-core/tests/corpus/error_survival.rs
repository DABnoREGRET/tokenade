//! The one line that mattered must reach the agent.
//!
//! A build, a test run, a deploy: the log is long, its lines are all different,
//! and exactly one of them says what went wrong. The generic head+tail summary
//! kept the first thirty lines and the last thirty and dropped everything
//! between — so the agent was handed a clean tail and read the run as having
//! passed. The bytes stayed recoverable from the stash, which is no help: the
//! delivered text gave no reason to go looking.
//!
//! MEASURED before the fix, on 2 500-line logs of unique lines: a Python
//! traceback, a JUnit `[ERROR]`, a Docker build failure, a Go `--- FAIL` and an
//! `npm ERR!` were all lost, five out of five.

/// Distinct lines, so nothing survives by being the only thing that repeats —
/// that is what made the first version of this probe pass while the product
/// was broken.
fn noisy_log(lines: usize) -> Vec<String> {
    let words = [
        "deploy", "widget", "cluster", "shard", "index", "rebuild", "flush", "verify", "token",
        "cache", "lease", "probe", "socket", "buffer",
    ];
    (0..lines)
        .map(|i| {
            let w: Vec<&str> = (0..6)
                .map(|k| words[(i * 7 + k * 3) % words.len()])
                .collect();
            format!(
                "[2026-08-03T10:{:02}:{:02}Z] INFO {} id={}",
                i / 60 % 60,
                i % 60,
                w.join(" "),
                1_000_000 + i * 37
            )
        })
        .collect()
}

fn log_with(failure: &[&str]) -> String {
    let mut lines = noisy_log(2500);
    for (k, f) in failure.iter().enumerate() {
        lines.insert(1250 + k, (*f).to_string());
    }
    lines.join("\n")
}

#[test]
fn a_failure_buried_in_a_long_log_reaches_the_agent() {
    // Each case is the shape a real tool prints, and the needle is the part an
    // agent needs to act: the exception, the assertion, the failing test name.
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "python",
            &[
                "Traceback (most recent call last):",
                "  File \"/app/main.py\", line 88, in handler",
                "ValueError: expected a positive count, got -3",
            ],
            "expected a positive count, got -3",
        ),
        (
            "junit",
            &["[ERROR] WidgetTest.testCount:42 expected:<3> but was:<0>"],
            "expected:<3> but was:<0>",
        ),
        (
            "docker",
            &["ERROR: failed to solve: process \"/bin/sh -c npm ci\" did not complete successfully"],
            "did not complete successfully",
        ),
        ("go", &["--- FAIL: TestWidgetCount (0.00s)"], "FAIL: TestWidgetCount"),
        ("npm", &["npm ERR! code ELIFECYCLE"], "ELIFECYCLE"),
        (
            "rust",
            &["thread 'main' panicked at src/lib.rs:42:9:"],
            "panicked at src/lib.rs:42:9",
        ),
        (
            "make",
            &["make: *** [Makefile:17: build] Error 2"],
            "[Makefile:17: build] Error 2",
        ),
    ];
    let mut lost = Vec::new();
    for (name, failure, needle) in cases {
        let log = log_with(failure);
        let delivered = tokenade_core::disclose::maybe_stash(
            log.clone(),
            tokenade_core::disclose::SourceHint::Shell,
        );
        // The probe is worthless if the summary did not actually fold.
        assert!(
            delivered.len() < log.len() / 4,
            "{name}: the log was not folded, so this proves nothing"
        );
        if !delivered.contains(needle) {
            lost.push(*name);
        }
    }
    assert!(
        lost.is_empty(),
        "the failure was folded out of the delivered text for: {lost:?}"
    );
}

/// The rescue must stay a rescue. An all-error log would otherwise rebuild
/// itself inside the summary that exists to replace it.
#[test]
fn rescuing_errors_does_not_undo_the_fold() {
    let log: String = (0..2000)
        .map(|i| format!("[ERROR] widget {i} failed to start after {i} attempts"))
        .collect::<Vec<_>>()
        .join("\n");
    let delivered = tokenade_core::disclose::maybe_stash(
        log.clone(),
        tokenade_core::disclose::SourceHint::Shell,
    );
    assert!(
        delivered.len() < log.len() / 4,
        "an all-error log came back nearly whole ({} of {} bytes)",
        delivered.len(),
        log.len()
    );
}

/// The row that differs is the row that matters.
///
/// A status listing is uniform by nature: two hundred services running, one
/// failed; a hundred and fifty pods Running, one CrashLoopBackOff. The whole
/// reason to read the output is the exception, and a compactor that summarises
/// "mostly fine" while dropping it answers the opposite of the question.
#[test]
fn the_one_failing_row_survives_a_status_listing() {
    fn uniform(
        header: &str,
        row: impl Fn(usize) -> String,
        n: usize,
        odd: &str,
        at: usize,
    ) -> String {
        let mut lines = vec![header.to_string()];
        for i in 0..n {
            lines.push(row(i));
        }
        lines.insert(at, odd.to_string());
        lines.join("\n")
    }
    let cases: Vec<(&str, Vec<String>, String, &str)> = vec![
        (
            "systemctl",
            vec!["systemctl".into(), "list-units".into()],
            uniform(
                "  UNIT                LOAD   ACTIVE SUB     DESCRIPTION",
                |i| format!("  svc-{i}.service      loaded active running Service number {i}"),
                200,
                "● broker-7.service    loaded failed failed  Message broker shard 7",
                120,
            ),
            "broker-7.service",
        ),
        (
            "kubectl",
            vec!["kubectl".into(), "get".into(), "pods".into()],
            uniform(
                "NAME                    READY   STATUS             RESTARTS   AGE",
                |i| format!("api-{i:04}-abcde         1/1     Running            0          5d"),
                150,
                "worker-9f2b-xyz         0/1     CrashLoopBackOff   417        2d",
                77,
            ),
            "CrashLoopBackOff",
        ),
        (
            "docker",
            vec!["docker".into(), "ps".into()],
            uniform(
                "CONTAINER ID   IMAGE         STATUS                     NAMES",
                |i| format!("a1b2c3d4e{i:03}   svc/img:1.2   Up 3 days                  svc-{i}"),
                40,
                "deadbeef0001   svc/img:1.2   Exited (1) 4 minutes ago   svc-broken",
                21,
            ),
            "svc-broken",
        ),
        (
            "df",
            vec!["df".into(), "-h".into()],
            uniform(
                "Filesystem      Size  Used Avail Use% Mounted on",
                |i| {
                    format!(
                        "/dev/sd{}1       100G   40G   60G  40% /mnt/vol{i}",
                        (b'a' + (i % 26) as u8) as char
                    )
                },
                30,
                "/dev/sdz9       500G  500G     0G 100% /var/lib/data",
                15,
            ),
            "/var/lib/data",
        ),
    ];
    let mut lost = Vec::new();
    for (name, argv, output, needle) in &cases {
        let compacted = tokenade_core::filter::filter_command(argv, output).output;
        if !compacted.contains(*needle) {
            lost.push(format!("{name} (lost {needle:?})"));
        }
    }
    assert!(
        lost.is_empty(),
        "a status listing was summarised without the row that failed: {lost:?}"
    );
}

/// `ps` is the most aggressive compactor in the product — MEASURED at 19.6 %
/// unique-word retention on a real capture. That is the right trade for three
/// hundred idle processes and the wrong one if it takes the runaway with them.
#[test]
fn a_runaway_process_survives_the_ps_fold() {
    let mut lines = vec![
        "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND".to_string(),
    ];
    for i in 0..300 {
        lines.push(format!(
            "root     {:5}  0.0  0.1  12345  6789 ?        S    10:00   0:00 /usr/lib/daemon-{i}",
            1000 + i
        ));
    }
    lines.insert(
        150,
        "app       9999 98.7 41.2 9876543 4194304 ?     R    09:12 412:33 /opt/app/runaway-worker"
            .to_string(),
    );
    lines.insert(
        200,
        "app       8888  0.0  0.0      0     0 ?        Z    09:12   0:00 [stuck-child] <defunct>"
            .to_string(),
    );
    let out =
        tokenade_core::filter::filter_command(&["ps".into(), "aux".into()], &lines.join("\n"))
            .output;
    let mut lost = Vec::new();
    if !out.contains("runaway-worker") {
        lost.push("the process burning the CPU");
    }
    if !out.contains("defunct") && !out.contains("stuck-child") {
        lost.push("the zombie");
    }
    assert!(lost.is_empty(), "ps folded away: {lost:?}\n---\n{out}");
}

/// The same rule, on the listings that rank and cut.
#[test]
fn a_ranked_listing_keeps_what_stands_out() {
    // netstat: hundreds of ESTABLISHED, one socket leaking in CLOSE_WAIT.
    let mut ns = vec![
        "Proto Recv-Q Send-Q Local Address           Foreign Address         State".to_string(),
    ];
    for i in 0..300 {
        ns.push(format!(
            "tcp        0      0 10.0.0.5:8080           10.0.1.{}:5{:04}      ESTABLISHED",
            i % 250,
            i
        ));
    }
    ns.insert(
        140,
        "tcp      412      0 10.0.0.5:8080           10.0.9.9:44412          CLOSE_WAIT".into(),
    );
    ns.insert(
        200,
        "tcp        0      0 10.0.0.5:22             0.0.0.0:*               SYN_RECV".into(),
    );
    // top: one process pegged, one zombie counted in the header.
    let mut tp = vec![
        "top - 10:00:00 up 3 days,  2 users,  load average: 41.2, 38.7, 30.1".to_string(),
        "Tasks: 302 total,   2 running, 299 sleeping,   0 stopped,   1 zombie".to_string(),
        "  PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND".to_string(),
    ];
    for i in 0..300 {
        tp.push(format!(
            "{:5} root      20   0  123456  12345   6789 S   0.0   0.1   0:00.01 daemon-{i}",
            2000 + i
        ));
    }
    // Real `top -b` emits rows already sorted by %CPU descending, so the busy
    // process leads and the zombie — 0.0 % by definition — sits at the very
    // bottom. Ordering the fixture any other way would test a shape `top`
    // never produces.
    tp.insert(
        3,
        " 9999 app       20   0 9876543 4194304  1234 R  98.7  41.2 412:33.7 runaway".into(),
    );
    tp.push(
        " 8888 app       20   0       0      0      0 Z   0.0   0.0   0:00.00 stuck <defunct>"
            .into(),
    );
    // npm ls --all: one unmet peer dependency among many satisfied ones. The
    // direct dependencies are the answer and are listed whole, so the fold
    // happens in what they pull in.
    let mut np = vec!["myapp@1.0.0 /srv/app".to_string()];
    for i in 0..200 {
        np.push(format!("├─┬ pkg-{i}@1.2.3"));
        for j in 0..3 {
            np.push(format!("│ ├── sub-{i}-{j}@0.4.1"));
        }
    }
    np.insert(90, "├── UNMET PEER DEPENDENCY react@^18.0.0".into());
    for (name, argv, text, needle) in [
        (
            "netstat/close_wait",
            vec!["netstat", "-an"],
            ns.join("\n"),
            "CLOSE_WAIT",
        ),
        (
            "netstat/syn_recv",
            vec!["netstat", "-an"],
            ns.join("\n"),
            "SYN_RECV",
        ),
        (
            "top/runaway",
            vec!["top", "-b", "-n1"],
            tp.join("\n"),
            "runaway",
        ),
        (
            "top/zombie",
            vec!["top", "-b", "-n1"],
            tp.join("\n"),
            "defunct",
        ),
        ("npm/unmet", vec!["npm", "ls"], np.join("\n"), "UNMET PEER"),
    ] {
        let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = tokenade_core::filter::filter_command(&a, &text).output;
        assert!(
            out.len() < text.len() / 2,
            "{name}: nothing was folded, so this proves nothing"
        );
        assert!(out.contains(needle), "{name}: {needle:?} was folded away");
    }
}

/// The single most common thing an agent does: run the tests, one fails.
///
/// Every one of these folds thousands of lines into a dozen, and the dozen has
/// to carry the test's name, what it expected, and where. Verified rather than
/// assumed — this is the path where a dropped line costs the agent the whole
/// task.
#[test]
fn a_test_runner_keeps_the_failure_detail() {
    let mut cargo = vec![
        "   Compiling widget v0.1.0 (/srv/widget)".to_string(),
        "    Finished test profile [unoptimized + debuginfo] target(s) in 12.34s".to_string(),
        "     Running unittests src/lib.rs (target/debug/deps/widget-abc123)".to_string(),
        "".to_string(),
        "running 2000 tests".to_string(),
    ];
    for i in 0..2000 {
        cargo.push(format!("test module_{}::case_{} ... ok", i % 40, i));
    }
    cargo.insert(900, "test module_7::counts_the_widgets ... FAILED".into());
    cargo.extend([
        "".into(),
        "failures:".into(),
        "".into(),
        "---- module_7::counts_the_widgets stdout ----".into(),
        "thread 'module_7::counts_the_widgets' panicked at src/lib.rs:212:9:".into(),
        "assertion `left == right` failed: widget count drifted".into(),
        "  left: 41".into(),
        "  right: 42".into(),
        "".into(),
        "failures:".into(),
        "    module_7::counts_the_widgets".into(),
        "".into(),
        "test result: FAILED. 2000 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out".into(),
    ]);

    let mut pytest = vec![
        "============================= test session starts =============================="
            .to_string(),
        "platform linux -- Python 3.12.1, pytest-8.0.0".to_string(),
        "collected 1500 items".to_string(),
        "".to_string(),
    ];
    for i in 0..1500 {
        pytest.push(format!(
            "tests/test_mod{}.py::test_case_{} PASSED       [ {:2}%]",
            i % 30,
            i,
            i * 100 / 1500
        ));
    }
    pytest.insert(
        700,
        "tests/test_widget.py::test_count FAILED                                  [ 46%]".into(),
    );
    pytest.extend([
        "".into(),
        "=================================== FAILURES ===================================".into(),
        "_________________________________ test_count __________________________________".into(),
        "".into(),
        "    def test_count():".into(),
        ">       assert widget_count() == 42".into(),
        "E       assert 41 == 42".into(),
        "".into(),
        "tests/test_widget.py:88: AssertionError".into(),
        "=========================== 1 failed, 1500 passed in 42.11s ====================".into(),
    ]);

    let mut jest = vec!["PASS src/a.test.ts".to_string()];
    for i in 0..800 {
        jest.push(format!("PASS src/mod{}/case{}.test.ts", i % 20, i));
    }
    jest.insert(400, "FAIL src/widget/count.test.ts".into());
    jest.extend([
        "  ● widget › counts the widgets".into(),
        "".into(),
        "    expect(received).toBe(expected) // Object.is equality".into(),
        "    Expected: 42".into(),
        "    Received: 41".into(),
        "".into(),
        "      at Object.<anonymous> (src/widget/count.test.ts:88:23)".into(),
        "Tests:       1 failed, 800 passed, 801 total".into(),
    ]);

    let mut gotest = Vec::new();
    for i in 0..600 {
        gotest.push(format!("ok  \tgithub.com/org/app/pkg{}\t0.0{}s", i, i % 10));
    }
    gotest.insert(300, "--- FAIL: TestWidgetCount (0.01s)".into());
    gotest.insert(301, "    widget_test.go:88: expected 42, got 41".into());
    gotest.push("FAIL\tgithub.com/org/app/widget\t0.012s".into());

    for (name, argv, text, needles) in [
        (
            "cargo test",
            vec!["cargo", "test"],
            cargo.join("\n"),
            vec![
                "counts_the_widgets",
                "widget count drifted",
                "src/lib.rs:212",
            ],
        ),
        (
            "pytest",
            vec!["pytest"],
            pytest.join("\n"),
            vec!["test_count", "assert 41 == 42", "test_widget.py:88"],
        ),
        (
            "jest",
            vec!["npx", "jest"],
            jest.join("\n"),
            vec!["counts the widgets", "Expected: 42", "count.test.ts:88"],
        ),
        (
            "go test",
            vec!["go", "test", "./..."],
            gotest.join("\n"),
            vec!["TestWidgetCount", "expected 42, got 41"],
        ),
    ] {
        let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = tokenade_core::filter::filter_command(&a, &text).output;
        let lost: Vec<&str> = needles
            .iter()
            .copied()
            .filter(|n| !out.contains(n))
            .collect();
        assert!(
            out.lines().count() * 4 < text.lines().count(),
            "{name}: nothing was folded, so this proves nothing"
        );
        assert!(
            lost.is_empty(),
            "{name} folded away the failure detail: {lost:?}"
        );
    }
}

/// A plan and a provisioning run: the destructive action and the failed task
/// must survive the generic cap that runs after the per-format compactor.
#[test]
fn a_plan_keeps_what_it_will_destroy_and_a_run_keeps_what_failed() {
    // terraform plan: 200 creates and one DESTROY — the one line that decides
    // whether a human should be asked before applying.
    let mut tf = vec![
        "Terraform will perform the following actions:".to_string(),
        "".to_string(),
    ];
    for i in 0..200 {
        tf.push(format!("  # aws_instance.node[{i}] will be created"));
        tf.push("  + resource \"aws_instance\" \"node\" {".to_string());
        tf.push(format!("      + ami = \"ami-0abc{i:04}\""));
        tf.push("    }".into());
        tf.push("".into());
    }
    // Inserted as a WHOLE block on a block boundary — each resource above is
    // five lines starting at index 2, so anything else would split a block in
    // half and test a shape terraform never emits.
    let at = 2 + 5 * 80;
    for (k, l) in [
        "  # aws_db_instance.primary will be destroyed",
        "  - resource \"aws_db_instance\" \"primary\" {",
        "      - identifier = \"prod-primary\" -> null",
        "    }",
        "",
    ]
    .iter()
    .enumerate()
    {
        tf.insert(at + k, (*l).to_string());
    }
    tf.push("Plan: 200 to add, 0 to change, 1 to destroy.".into());

    // ansible: 300 ok, one failed task with its message.
    let mut ans =
        vec!["PLAY [deploy] ******************************************************".to_string()];
    for i in 0..300 {
        ans.push(format!(
            "TASK [role : step {i}] *********************************************"
        ));
        ans.push(format!("ok: [host-{}]", i % 12));
    }
    ans.insert(
        200,
        "TASK [role : migrate database] *************************************".into(),
    );
    ans.insert(
        201,
        "fatal: [host-3]: FAILED! => {\"msg\": \"could not connect to prod-primary: timeout\"}"
            .into(),
    );
    ans.push("PLAY RECAP *********************************************************".into());
    ans.push("host-3   : ok=199 changed=0 unreachable=0 failed=1".into());

    // pip: a resolution conflict at the end of a long install log.
    let mut pip = Vec::new();
    for i in 0..400 {
        pip.push(format!("Collecting pkg-{i}==1.2.{i}"));
    }
    pip.insert(
        250,
        "ERROR: pip's dependency resolver does not currently take into account all the packages"
            .into(),
    );
    pip.insert(
        251,
        "widget 2.0 requires urllib3<2, but you have urllib3 2.2.1 which is incompatible.".into(),
    );

    for (name, argv, text, needles) in [
        (
            "terraform",
            vec!["terraform", "plan"],
            tf.join("\n"),
            vec!["aws_db_instance.primary will be destroyed", "1 to destroy"],
        ),
        (
            "ansible",
            vec!["ansible-playbook", "site.yml"],
            ans.join("\n"),
            vec!["migrate database", "could not connect to prod-primary"],
        ),
        (
            "pip",
            vec!["pip", "install", "-r", "requirements.txt"],
            pip.join("\n"),
            vec!["urllib3 2.2.1 which is incompatible"],
        ),
    ] {
        let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = tokenade_core::filter::filter_command(&a, &text).output;
        let lost: Vec<&str> = needles
            .iter()
            .copied()
            .filter(|n| !out.contains(n))
            .collect();
        assert!(
            out.lines().count() * 2 < text.lines().count(),
            "{name}: nothing was folded, so this proves nothing"
        );
        assert!(lost.is_empty(), "{name} folded away: {lost:?}");
    }
}

/// A handful of zombies is a diagnosis; a hundred is a number.
///
/// Found by running the changed compactor on a real machine that happened to
/// carry 113 of them: promoting every unhealthy row turned a 30-line fold into
/// 132 lines all saying `<defunct>`. Evidence, then a count.
#[test]
fn a_machine_full_of_zombies_still_gets_a_short_answer() {
    let mut lines = vec![
        "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND".to_string(),
    ];
    for i in 0..400 {
        lines.push(format!(
            "root     {:5}  0.0  0.1  12345  6789 ?        S    10:00   0:00 /usr/lib/daemon-{i}",
            1000 + i
        ));
    }
    for i in 0..113 {
        lines.push(format!(
            "app      {:5}  0.0  0.0      0     0 ?        Z    09:12   0:00 [worker-{i}] <defunct>",
            7000 + i
        ));
    }
    let out =
        tokenade_core::filter::filter_command(&["ps".into(), "aux".into()], &lines.join("\n"))
            .output;
    // Two folds can reach this input and BOTH answer well: the `ps` promotion
    // caps the rows it shows and counts the rest, and the template fold
    // collapses the 113 identical zombie lines into one `[×113 lines]` row.
    // The invariant is the ANSWER, not which of them produced it — asserting
    // the wording of one made this test fail on output that was better than
    // what it demanded.
    assert!(
        out.contains("<defunct>"),
        "no zombie reached the reader at all:\n{out}"
    );
    assert!(
        out.lines().count() < 40,
        "{} lines delivered — the fold became the listing",
        out.lines().count()
    );
    assert!(
        out.contains("113") || out.contains("more"),
        "the reader cannot tell HOW MANY zombies there were:\n{out}"
    );
}

/// The definition among eight hundred call sites, and the one real change
/// among sixty regenerated files.
#[test]
fn a_search_and_a_diff_keep_the_line_that_answers() {
    // grep -rn: 800 boilerplate call sites and one line that is the definition.
    let mut g: Vec<String> = (0..800)
        .map(|i| {
            format!(
                "src/mod{}/file{i}.rs:{}:    let widget_count = compute(&self.items);",
                i % 40,
                i % 200 + 1
            )
        })
        .collect();
    g.insert(
        430,
        "src/core/registry.rs:88:pub fn widget_count(items: &[Item]) -> usize { items.len() }"
            .into(),
    );

    // git diff: sixty files of regenerated churn and one real logic change.
    let mut d: Vec<String> = Vec::new();
    for i in 0..60 {
        d.extend([
            format!("diff --git a/src/gen{i}.rs b/src/gen{i}.rs"),
            format!("index aaa{i}..bbb{i} 100644"),
            format!("--- a/src/gen{i}.rs"),
            format!("+++ b/src/gen{i}.rs"),
            "@@ -1,4 +1,4 @@".into(),
            " fn generated() {".into(),
            "-    let x = 1;".into(),
            "+    let x = 1; // regenerated".into(),
            " }".into(),
        ]);
    }
    d.extend([
        "diff --git a/src/auth.rs b/src/auth.rs".into(),
        "index ccc..ddd 100644".into(),
        "--- a/src/auth.rs".into(),
        "+++ b/src/auth.rs".into(),
        "@@ -42,7 +42,7 @@ pub fn verify(token: &str) -> bool {".into(),
        "     let claims = decode(token);".into(),
        "-    claims.exp > now() && claims.sig_ok".into(),
        "+    claims.exp > now()".into(),
        " }".into(),
    ]);

    for (name, argv, text, needles) in [
        (
            "grep",
            vec!["grep", "-rn", "widget_count", "."],
            g.join("\n"),
            vec!["registry.rs:88", "pub fn widget_count"],
        ),
        (
            "git diff",
            vec!["git", "diff"],
            d.join("\n"),
            vec!["claims.sig_ok", "src/auth.rs"],
        ),
    ] {
        let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = tokenade_core::filter::filter_command(&a, &text).output;
        let lost: Vec<&str> = needles
            .iter()
            .copied()
            .filter(|n| !out.contains(n))
            .collect();
        assert!(
            out.lines().count() * 2 < text.lines().count(),
            "{name}: nothing was folded, so this proves nothing"
        );
        assert!(lost.is_empty(), "{name} folded away: {lost:?}");
    }
}

#[test]
#[ignore = "measurement: what a compactor CLAIMS about an output cut mid-way"]
fn measure_truncated_input() {
    // Output cut mid-structure is ordinary: a pipe closed, a `head -c`, a
    // container log rotated. The question is whether a compactor states
    // something about it that is not true.
    let json_full = format!(
        "[{}]",
        (0..300)
            .map(|i| format!(r#"{{"id":{i},"state":"ok"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let json_cut = &json_full[..json_full.len() * 2 / 3];
    let csv_full: String = std::iter::once("id,name,state".to_string())
        .chain((0..300).map(|i| format!("{i},widget-{i},ok")))
        .collect::<Vec<_>>()
        .join("\n");
    let csv_cut = &csv_full[..csv_full.len() * 2 / 3];
    let yaml_cut =
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: app\ndata:\n  key: \"unterminat";
    for (name, text) in [
        ("json cut", json_cut.to_string()),
        ("csv cut", csv_cut.to_string()),
        ("yaml cut", yaml_cut.to_string()),
    ] {
        let (kind, out, _) = tokenade_core::auto::compact(&text);
        let claims_count =
            out.contains('×') || out.to_lowercase().contains("rows") || out.contains("items");
        println!(
            "  {:<9} kind={:?} {}→{} chars, makes a count claim = {}",
            name,
            kind,
            text.len(),
            out.len(),
            claims_count
        );
        for l in out
            .lines()
            .filter(|l| {
                l.contains('×')
                    || l.to_lowercase().contains("row")
                    || l.contains("more")
                    || l.contains("…")
            })
            .take(3)
        {
            println!("      claim| {}", &l[..l.len().min(120)]);
        }
        println!(
            "      real rows in the truncated input = {}",
            text.lines().count()
        );
    }
}

/// The opposite shape to the one this file was written for: NEAR-UNIFORM lines
/// with a single anomaly. The original fix targeted logs of distinct lines, and
/// a repetitive log kept its first sixty and last three — so the one line that
/// differed from all the others was exactly the one that fell in the middle.
///
/// MEASURED 2026-08-04 through `tokenade wrap`: 800 lines of
/// `job-N step i of 800 ok` around one `ERROR: … checksum mismatch` at position
/// 401, delivered with no trace of the error.
#[test]
fn a_lone_failure_among_uniform_lines_survives_the_fold() {
    let mut lines: Vec<String> = (1..=400)
        .map(|i| format!("job-7 step {i} of 800 ok"))
        .collect();
    lines.push("ERROR: job-7 step 401 checksum mismatch".to_string());
    lines.extend((402..=800).map(|i| format!("job-7 step {i} of 800 ok")));
    let text = lines.join("\n");
    let out = tokenade_core::archetypes::head_tail(&text, 60, 3, "probe", "line(s)")
        .expect("800 lines must fold");
    assert!(
        out.contains("checksum mismatch"),
        "the one line that differed was dropped:\n{out}"
    );
    // The count must stay honest once a line is lifted out of the elided range.
    assert!(
        out.contains("look like failures"),
        "a rescued line must be announced, not smuggled in: {out}"
    );
    // And the fold must still be a fold.
    assert!(
        out.lines().count() < 120,
        "no longer folding: {}",
        out.lines().count()
    );
}

/// An all-error log must not defeat the fold: the rescue is bounded.
#[test]
fn rescuing_is_bounded_so_a_failing_run_still_folds() {
    let text: String = (1..=800)
        .map(|i| format!("ERROR: shard {i} failed to open"))
        .collect::<Vec<_>>()
        .join("\n");
    let out = tokenade_core::archetypes::head_tail(&text, 30, 3, "probe", "line(s)")
        .expect("800 lines must fold");
    assert!(
        out.lines().count() < 60,
        "bounded rescue, got {} lines",
        out.lines().count()
    );
}

/// `line_cap` keeps a HEAD and nothing else — no tail to catch a late failure.
/// It backs 50+ compactors (grep, ack, ag, apt-cache …), so this was the widest
/// version of the same defect.
///
/// MEASURED 2026-08-04: 300 lines of `unit N loaded ok` capped at 60, with
/// `ERROR: unit 251 failed to load` in the elided range — gone, and the marker
/// said only "240 more elided".
#[test]
fn line_cap_keeps_a_failure_past_the_cap() {
    let mut lines: Vec<String> = (1..=300).map(|i| format!("unit {i} loaded ok")).collect();
    lines[250] = "ERROR: unit 251 failed to load".to_string();
    let out = tokenade_core::archetypes::line_cap(&lines.join("\n"), 60, "line(s)");
    assert!(
        out.contains("failed to load"),
        "a failure past the cap was dropped:\n{out}"
    );
    assert!(
        out.contains("look like failures"),
        "the rescue must be announced: {out}"
    );
    assert!(
        out.lines().count() < 100,
        "still a fold: {}",
        out.lines().count()
    );
}

/// Bounded, or a listing that is all failures stops folding at all.
#[test]
fn line_cap_rescue_is_bounded() {
    let lines: Vec<String> = (1..=300)
        .map(|i| format!("ERROR: unit {i} failed"))
        .collect();
    let out = tokenade_core::archetypes::line_cap(&lines.join("\n"), 30, "line(s)");
    assert!(
        out.lines().count() < 60,
        "bounded, got {}",
        out.lines().count()
    );
}

/// The record-shaped sibling: `kv_records` folds whole key/value records, so
/// the rescue has to lift a RECORD, not a line — a bare line spliced into a
/// key/value stream corrupts the very shape the fold exists to preserve.
#[test]
fn kv_records_keeps_a_failing_record_whole() {
    let mut recs: Vec<String> = (1..=40)
        .map(|i| format!("name: unit-{i}\nstate: active\nrestarts: 0"))
        .collect();
    recs[30] = "name: unit-31\nstate: failed\nerror: could not bind socket".to_string();
    let out = tokenade_core::archetypes::kv_records(&recs.join("\n\n"), 5, "probe", "record(s)")
        .expect("40 records must fold");
    assert!(
        out.contains("could not bind socket"),
        "failing record dropped:\n{out}"
    );
    // Whole record, not a stray line.
    assert!(out.contains("name: unit-31"), "record was cut open:\n{out}");
    assert!(
        out.contains("look like failures"),
        "rescue must be announced: {out}"
    );
}
