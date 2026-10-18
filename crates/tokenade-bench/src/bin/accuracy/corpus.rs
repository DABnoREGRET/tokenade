//! **The tasks** — a question, an output, a known answer.
//!
//! Each task PLANTS its answer in a realistic but synthetic output:
//! the ground truth is therefore true by construction, not by annotation.
//! It is the same discipline as the adversarial fixtures, and it is what lets us
//! say "the answer survived" without a human re-reading.
//!
//! The questions are those an agent really asks. A question no
//! agent would ask would measure the compaction of a text nobody reads.

use super::*;

// ── corpus ──────────────────────────────────────────────────────────────────
//
// Each task embeds a known answer in a realistic-but-synthetic output, so the
// ground truth is verified by construction (the same discipline the `synth_*`
// adversarial fixtures use). Questions are the kind an agent genuinely asks.

pub(crate) fn corpus() -> Vec<Task> {
    let mut v = Vec::new();

    // 1. kubectl: spot the one broken pod in a wall of healthy ones.
    {
        let mut raw =
            String::from("NAME                        READY   STATUS             RESTARTS   AGE\n");
        for i in 0..40 {
            raw.push_str(&format!(
                "web-frontend-{i:02}-abcde         1/1     Running            0          5d\n"
            ));
        }
        raw.push_str("payments-7d9f-x2k4q         0/1     CrashLoopBackOff   147        2d\n");
        for i in 0..30 {
            raw.push_str(&format!(
                "cache-worker-{i:02}-fghij         1/1     Running            1          9d\n"
            ));
        }
        v.push(Task {
            // The real path an agent hits: the Bash hook compacts `kubectl get
            // pods` table output via the tabular compactor — a much stronger
            // test than describe-mode, since a row-collapsing tabular pass could
            // drop the single anomalous pod.
            id: "kubectl:crashloop",
            comp: Comp::Filter(&["kubectl", "get", "pods"]),
            question: "Which pod is not healthy, and what is its status?",
            raw,
            must_survive: &["payments-7d9f-x2k4q", "CrashLoopBackOff"],
        });
    }

    // 2. kubectl describe: the OOMKilled termination reason buried in events.
    {
        let raw = "Name:         analytics-86c\nNamespace:    prod\nContainers:\n  app:\n    State:          Waiting\n      Reason:       CrashLoopBackOff\n    Last State:     Terminated\n      Reason:       OOMKilled\n      Exit Code:    137\n      Started:      Mon, 09 Jun 2026 10:00:00 +0000\n      Finished:     Mon, 09 Jun 2026 10:03:11 +0000\nEvents:\n  Type     Reason     Age   Message\n  Normal   Scheduled  5m    Successfully assigned prod/analytics-86c\n  Normal   Pulled     5m    Container image already present\n  Warning  BackOff    1m    Back-off restarting failed container\n".to_string();
        v.push(Task {
            id: "kubectl:oomkilled",
            comp: Comp::Filter(&["kubectl", "describe", "pod", "analytics-86c"]),
            question: "Why did the analytics container last terminate?",
            raw,
            must_survive: &["OOMKilled", "re:137"],
        });
    }

    // 3. git log --stat: which commit last touched a specific file.
    {
        let mut raw = String::new();
        let commits = [
            (
                "a1b2c3d",
                "refactor: split scheduler",
                "src/scheduler.rs",
                40,
                12,
            ),
            ("e4f5061", "fix: token refresh race", "src/auth.rs", 8, 3),
            ("99aa77b", "docs: update README", "README.md", 60, 0),
            ("cc12d34", "perf: cache lookups", "src/cache.rs", 22, 5),
        ];
        for (h, subj, file, add, del) in commits {
            raw.push_str(&format!(
                "commit {h}0000000000000000000000000000000000\nAuthor: Dev <dev@example.com>\nDate:   Mon Jun 9 12:00:00 2026 +0000\n\n    {subj}\n\n {file} | {} {}{}\n 1 file changed, {add} insertions(+), {del} deletions(-)\n\n",
                add + del,
                "+".repeat(add.min(20)),
                "-".repeat(del.min(20)),
            ));
        }
        v.push(Task {
            id: "gitlog:which-commit",
            comp: Comp::Filter(&["git", "log", "--stat"]),
            question: "Which commit fixed the auth token refresh, and what file did it touch?",
            raw,
            must_survive: &["e4f5061", "src/auth.rs"],
        });
    }

    // 4. cargo build: the single hard error buried in a monorepo's worth of
    //    `Compiling …` progress chatter (the noise the compactor actually
    //    strips — it samples 10, elides the rest, and keeps every diagnostic
    //    block in full). Without this chatter the input is all-signal and the
    //    compactor is a no-op, which would make the answer-survival test
    //    vacuous; the chatter is what an agent really wades through.
    {
        let mut raw = String::new();
        raw.push_str("    Updating crates.io index\n");
        for i in 0..6 {
            raw.push_str(&format!("  Downloading dep-{i} v1.{i}.0\n"));
        }
        raw.push_str("    Blocking waiting for file lock on build directory\n");
        for i in 0..140 {
            raw.push_str(&format!(
                "   Compiling crate-{i:03} v0.{}.{}\n",
                i % 9,
                i % 5
            ));
        }
        for i in 0..25 {
            raw.push_str(&format!(
                "warning: unused variable: `tmp{i}`\n  --> src/lib.rs:{}:9\n   |\n   = note: `#[warn(unused_variables)]` on by default\n\n",
                100 + i
            ));
        }
        raw.push_str("error[E0599]: no method named `unwrap_or_default` found for struct `Config` in the current scope\n  --> src/config.rs:88:18\n   |\n88 |     let v = cfg.unwrap_or_default();\n   |                 ^^^^^^^^^^^^^^^^^^ method not found in `Config`\n\nerror: could not compile `app` (lib) due to 1 previous error\n");
        v.push(Task {
            id: "cargo:hard-error",
            comp: Comp::Filter(&["cargo", "build"]),
            question: "What is the compile error and where is it?",
            raw,
            must_survive: &["E0599", "unwrap_or_default", "src/config.rs:88"],
        });
    }

    // 5. pytest: the one FAILED test and its assertion, in a *non-verbose*
    //    run (matches the bare `pytest` argv — a `pytest -v` run deliberately
    //    passes through untouched, since `-v` signals the user wants raw). The
    //    removable noise here is the `warnings summary` block real suites emit,
    //    not the terse dot progress — that is what the compactor folds.
    {
        let mut raw = String::from(
            "============================= test session starts ==============================\n",
        );
        raw.push_str("collected 61 items\n\n");
        raw.push_str(
            "tests/test_api.py ............................................        [ 72%]\n",
        );
        raw.push_str(
            "tests/test_billing.py ...................F                            [100%]\n\n",
        );
        raw.push_str(
            "=================================== FAILURES ===================================\n",
        );
        raw.push_str(
            "____________________________ test_proration ____________________________________\n\n",
        );
        raw.push_str("    def test_proration():\n>       assert invoice.total == 1999\nE       assert 2099 == 1999\nE        +  where 2099 = Invoice(...).total\n\ntests/test_billing.py:142: AssertionError\n\n");
        raw.push_str(
            "=============================== warnings summary ===============================\n",
        );
        for i in 0..40 {
            raw.push_str(&format!(
                "tests/test_api.py::test_case_{i:02}\n  /usr/lib/python3.11/site-packages/dep/mod.py:{}: DeprecationWarning: legacy() is deprecated, use modern()\n    warnings.warn(\"legacy() is deprecated\", DeprecationWarning)\n",
                200 + i
            ));
        }
        raw.push_str("-- Docs: https://docs.pytest.org/en/stable/how-to/capture-warnings.html\n");
        raw.push_str(
            "=========================== short test summary info ============================\n",
        );
        raw.push_str("FAILED tests/test_billing.py::test_proration - assert 2099 == 1999\n");
        raw.push_str(
            "=========================== 1 failed, 60 passed in 4.21s =======================\n",
        );
        v.push(Task {
            id: "pytest:one-failure",
            comp: Comp::Filter(&["pytest"]),
            question: "Which test failed and what was the assertion mismatch?",
            raw,
            must_survive: &["test_proration", "2099", "1999"],
        });
    }

    // 6. netstat: which process owns the listening SSH port.
    {
        let mut raw = String::from("Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n");
        for i in 0..50 {
            raw.push_str(&format!(
                "tcp        0      0 10.0.0.5:{:<5}         10.0.2.{}:54321        ESTABLISHED 12{i:02}/python\n",
                40000 + i,
                i % 250
            ));
        }
        raw.push_str("tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      812/sshd\n");
        v.push(Task {
            id: "netstat:listen-ssh",
            comp: Comp::Filter(&["netstat", "-tlnp"]),
            question: "Which process is listening on the SSH port?",
            raw,
            must_survive: &["re:0\\.0\\.0\\.0:22", "LISTEN", "sshd"],
        });
    }

    // 6b. strace: the failed syscall that crashed the program, buried far past
    // the fold head in the TAIL. This pins the reconstruction-class bug a naive
    // head-only fold would cause — the failure is the *last* syscall, so a
    // "keep first N, drop the rest" fold would silently lose the answer.
    // Routes the real production path: filter_command → auto::compact (no
    // strace dispatch_named arm) → Kind::Strace → strace::compact.
    {
        let mut raw = String::new();
        for i in 0..130 {
            raw.push_str(&format!("read(3, \"chunk-{i}\", 4096) = 4096\n"));
            raw.push_str("getpid() = 4823\n");
        }
        // The crash: the very last syscall before the program died.
        raw.push_str(
            "openat(AT_FDCWD, \"/etc/app/secret.key\", O_RDONLY) = -1 ENOENT (No such file or directory)\n",
        );
        raw.push_str("+++ exited with 1 +++\n");
        v.push(Task {
            id: "strace:tail-failure",
            comp: Comp::Filter(&["strace", "-f", "./app"]),
            question: "Which syscall failed and why did the program exit non-zero?",
            raw,
            must_survive: &["/etc/app/secret.key", "ENOENT"],
        });
    }

    // 7. ndjson logs: the single ERROR with its request id, amid INFO noise.
    {
        let mut raw = String::new();
        for i in 0..80 {
            raw.push_str(&format!(
                "{{\"level\":\"info\",\"ts\":\"2026-06-09T10:{:02}:00Z\",\"msg\":\"handled request\",\"path\":\"/health\"}}\n",
                i % 60
            ));
        }
        raw.push_str("{\"level\":\"error\",\"ts\":\"2026-06-09T10:31:07Z\",\"msg\":\"payment gateway timeout\",\"request_id\":\"req-9f3a2b\",\"upstream\":\"stripe\"}\n");
        for i in 0..40 {
            raw.push_str(&format!(
                "{{\"level\":\"info\",\"ts\":\"2026-06-09T11:{:02}:00Z\",\"msg\":\"handled request\",\"path\":\"/health\"}}\n",
                i % 60
            ));
        }
        v.push(Task {
            id: "ndjson:error-request",
            comp: Comp::Module("ndjson", ndjson::compact),
            question: "What error occurred and what was its request id?",
            raw,
            must_survive: &["req-9f3a2b", "payment gateway timeout"],
        });
    }

    // 8. lockfile: pin of a specific transitive dependency.
    {
        let mut raw = String::new();
        let pkgs = [
            ("anyhow", "1.0.86"),
            ("serde", "1.0.210"),
            ("tokio", "1.39.2"),
            ("regex", "1.10.6"),
            ("clap", "4.5.16"),
        ];
        for (name, ver) in pkgs {
            raw.push_str(&format!(
                "[[package]]\nname = \"{name}\"\nversion = \"{ver}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"deadbeef{}\"\n\n",
                name.len()
            ));
        }
        v.push(Task {
            id: "lockfile:pinned-version",
            comp: Comp::Module("lockfile", lockfile::compact),
            question: "What version of serde is pinned?",
            raw,
            must_survive: &["serde", "1.0.210"],
        });
    }

    // 9. JSON: a specific field deep in a large config response.
    {
        let mut items = String::new();
        for i in 0..60 {
            items.push_str(&format!(
                "    {{\"id\": {i}, \"name\": \"flag_{i}\", \"enabled\": {}}},\n",
                i % 2 == 0
            ));
        }
        let raw = format!(
            "{{\n  \"service\": \"billing\",\n  \"region\": \"us-east-1\",\n  \"max_retries\": 7,\n  \"feature_flags\": [\n{items}    {{\"id\": 999, \"name\": \"dark_launch\", \"enabled\": true}}\n  ]\n}}\n"
        );
        v.push(Task {
            id: "json:config-field",
            comp: Comp::Module("json", tokenade_json::compact),
            question: "What is max_retries and which region is this service in?",
            raw,
            must_survive: &["max_retries", "re:7", "us-east-1"],
        });
    }

    // 10. df -h: the nearly-full filesystem amid many roomy ones. Reconstruction
    // class (disk.rs rebuilds a per-mount summary, TOP_ENTRIES_CAP=60). The full
    // mount is placed in the MIDDLE, not the top, so a sort/cap that favoured
    // size or position would drop it. Routes df → sysinfo_dispatch → disk.rs.
    {
        let mut raw = String::from("Filesystem      Size  Used Avail Use% Mounted on\n");
        for i in 0..35 {
            raw.push_str(&format!(
                "/dev/loop{i:<3}     50G   2.0G   48G   4% /snap/app/{i}\n"
            ));
        }
        raw.push_str("/dev/nvme0n1p3  200G   196G  1.2G  99% /var/lib/postgresql\n");
        for i in 0..35 {
            raw.push_str(&format!(
                "/dev/loop{:<3}     50G   2.0G   48G   4% /snap/lib/{i}\n",
                i + 100
            ));
        }
        v.push(Task {
            id: "disk:full-mount",
            comp: Comp::Filter(&["df", "-h"]),
            question: "Which filesystem is about to run out of space, and where is it mounted?",
            raw,
            must_survive: &["/var/lib/postgresql", "99%"],
        });
    }

    // 11. docker ps -a: the one crashed container among many healthy ones.
    // Reconstruction class (docker_table rebuilds the row table). The exited
    // container sits in the middle. Routes docker → docker_dispatch.
    {
        let mut raw = String::from(
            "CONTAINER ID   IMAGE              COMMAND                  CREATED        STATUS                      PORTS     NAMES\n",
        );
        for i in 0..20 {
            raw.push_str(&format!(
                "a1b2c3d4e5{i:02}   nginx:1.27         \"/docker-entrypoint.…\"   2 days ago     Up 2 days                   80/tcp    web-{i:02}\n"
            ));
        }
        raw.push_str(
            "deadbeef0099   billing-api:2.3    \"/app/server\"            5 hours ago    Exited (137) 3 minutes ago            billing-api\n",
        );
        for i in 0..20 {
            raw.push_str(&format!(
                "f9e8d7c6b5{i:02}   redis:7            \"docker-entrypoint.s…\"   2 days ago     Up 2 days                   6379/tcp  cache-{i:02}\n"
            ));
        }
        v.push(Task {
            id: "docker:exited-container",
            comp: Comp::Filter(&["docker", "ps", "-a"]),
            question: "Which container crashed, and what exit code did it report?",
            raw,
            must_survive: &["billing-api", "Exited (137)"],
        });
    }

    // 12. git diff --numstat: the one huge change among many tiny ones.
    // Reconstruction class. git emits rows in PATH order, not by magnitude, so
    // the biggest change sits at a random position — here deep in the list, past
    // any head/tail window. A head-or-edge fold drops it; the realistic review
    // question "what changed the most?" then can't be answered. Routes
    // git → git dispatcher → diff::dispatch (--numstat).
    {
        let mut raw = String::new();
        for i in 0..70 {
            raw.push_str(&format!(
                "{}\t{}\tsrc/mod_{i:02}/handler.rs\n",
                2 + i % 5,
                1 + i % 3
            ));
        }
        raw.push_str("1450\t12\tdb/migrations/0099_backfill_accounts.sql\n");
        for i in 0..70 {
            raw.push_str(&format!("{}\t{}\ttests/case_{i:02}.rs\n", 1 + i % 4, i % 2));
        }
        v.push(Task {
            id: "git:numstat-biggest",
            comp: Comp::Filter(&["git", "diff", "--numstat"]),
            question: "Which file had by far the largest change in this diff?",
            raw,
            must_survive: &["0099_backfill_accounts.sql", "1450"],
        });
    }

    v
}
