//! A compaction may drop anything EXCEPT the reason something failed.
//!
//! Every fixture here is a realistic run of a popular tool that fails, with a
//! unique marker planted in the DETAIL line — not the headline. That placement
//! is the whole point: a fold that keeps `--- FAIL: TestX` and drops
//! `engine_test.go:118: expected 42, got 41` tells the agent THAT something
//! broke and never why, so it re-runs the command and pays twice.
//!
//! Written after finding exactly that in two shipped compactors on the same
//! afternoon (`go test`, `docker build`). Both looked correct line by line;
//! both lost the answer. This file exists so the third one fails here instead
//! of in someone's session.

const MARKER: &str = "ZQX_DETAIL_MARKER";

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// `n` lines of plausible success noise, then a failure whose DETAIL carries
/// the marker.
fn case(name: &str) -> (Vec<String>, String) {
    match name {
        "cargo-test" => {
            let mut o: String = (0..900)
                .map(|i| format!("test mod{i}::case ... ok\n"))
                .collect();
            o.push_str("test mod42::broken ... FAILED\n\nfailures:\n\n");
            o.push_str("---- mod42::broken stdout ----\n");
            o.push_str("thread 'mod42::broken' panicked at src/lib.rs:117:9:\n");
            o.push_str(&format!("assertion `left == right` failed: {MARKER}\n"));
            o.push_str("test result: FAILED. 900 passed; 1 failed\n");
            (argv(&["cargo", "test"]), o)
        }
        "cargo-build" => {
            let mut o: String = (0..800)
                .map(|i| format!("   Compiling crate{i} v0.1.0\n"))
                .collect();
            o.push_str("error[E0308]: mismatched types\n   --> src/engine.rs:412:19\n    |\n");
            o.push_str(&format!("412 |     let f: Frame = x;\n    |            ^ expected `Frame`, found `{MARKER}`\n"));
            o.push_str("error: could not compile `app` due to 1 previous error\n");
            (argv(&["cargo", "build"]), o)
        }
        "go-test" => {
            let mut o: String = (0..500)
                .map(|i| format!("ok  \tgithub.com/a/pkg{i}\t0.01s\n"))
                .collect();
            o.push_str("--- FAIL: TestBroken (0.00s)\n");
            o.push_str(&format!(
                "    engine_test.go:118: expected 42, got 41 {MARKER}\n"
            ));
            o.push_str("FAIL\tgithub.com/a/core\t0.11s\nFAIL\n");
            (argv(&["go", "test", "./..."]), o)
        }
        "pytest" => {
            let mut o: String = (0..400)
                .map(|i| format!("tests/test_mod{i}.py ......... [{i}%]\n"))
                .collect();
            o.push_str("=================================== FAILURES ===================================\n");
            o.push_str("____________________________ test_broken _____________________________\n");
            o.push_str(&format!("E       assert 41 == 42  # {MARKER}\n"));
            o.push_str("FAILED tests/test_mod3.py::test_broken - assert 41 == 42\n");
            (argv(&["pytest", "-q"]), o)
        }
        "jest" => {
            let mut o: String = (0..400)
                .map(|i| format!("PASS src/w{i}.test.tsx\n"))
                .collect();
            o.push_str("FAIL src/core/engine.test.ts\n  ● engine › handles frames\n");
            o.push_str(&format!("    Expected: 42\n    Received: 41 {MARKER}\n"));
            o.push_str("Tests:       1 failed, 400 passed, 401 total\n");
            (argv(&["jest"]), o)
        }
        "docker-build" => {
            let mut o: String = (0..900)
                .map(|i| format!("Step {i}/900 : RUN echo l{i}\n"))
                .collect();
            o.push_str("Step 900/900 : RUN npm ci\n ---> Running in a1b2c3\n");
            o.push_str("npm ERR! code ERESOLVE\n");
            o.push_str(&format!(
                "npm ERR! {MARKER} unable to resolve dependency tree\n"
            ));
            o.push_str("The command '/bin/sh -c npm ci' returned a non-zero code: 1\n");
            (argv(&["docker", "build", "."]), o)
        }
        "npm-install" => {
            // What npm ACTUALLY prints at scale: a deprecation warning per
            // offending dependency, then the error block. It does not print a
            // line per installed package — an earlier version of this fixture
            // did, and "fixing" a compactor for output no tool emits is worse
            // than the gap it pretends to close.
            let mut o: String = (0..600)
                .map(|i| format!("npm warn deprecated pkg{i}@1.0.{i}: use pkg{i}-ng instead\n"))
                .collect();
            o.push_str("npm ERR! code ERESOLVE\n");
            o.push_str(&format!("npm ERR! While resolving: app@1.0.0 {MARKER}\n"));
            o.push_str("npm ERR! Fix the upstream dependency conflict\n");
            (argv(&["npm", "install"]), o)
        }
        "make" => {
            let mut o: String = (0..700)
                .map(|i| format!("gcc -c src/f{i}.c -o build/f{i}.o\n"))
                .collect();
            o.push_str("src/engine.c:412:19: error: incompatible types\n");
            o.push_str(&format!("  412 |   Frame f = zqx();  // {MARKER}\n"));
            o.push_str("make: *** [Makefile:12: build/engine.o] Error 1\n");
            (argv(&["make"]), o)
        }
        "pip" => {
            let mut o: String = (0..500)
                .map(|i| format!("Collecting pkg{i}==1.0.{i}\n"))
                .collect();
            o.push_str("ERROR: Cannot install pkg1 and pkg2 because these have conflicting dependencies.\n");
            o.push_str(&format!(
                "The conflict is caused by:\n    pkg1 depends on {MARKER}\n"
            ));
            (argv(&["pip", "install", "-r", "requirements.txt"]), o)
        }
        "gradle" => {
            let mut o: String = (0..600)
                .map(|i| format!("> Task :module{i}:compileJava UP-TO-DATE\n"))
                .collect();
            o.push_str("> Task :app:test FAILED\n\nFAILURE: Build failed with an exception.\n\n");
            o.push_str("* What went wrong:\n");
            o.push_str(&format!(
                "Execution failed for task ':app:test'.\n> {MARKER} There were failing tests.\n"
            ));
            (argv(&["gradle", "build"]), o)
        }
        "mvn" => {
            let mut o: String = (0..600)
                .map(|i| format!("[INFO] Compiling module{i}\n"))
                .collect();
            o.push_str("[ERROR] Failed to execute goal org.apache.maven.plugins:maven-surefire-plugin:test\n");
            o.push_str(&format!(
                "[ERROR] {MARKER} There are test failures.\n[INFO] BUILD FAILURE\n"
            ));
            (argv(&["mvn", "test"]), o)
        }
        "eslint" => {
            let mut o: String = (0..500)
                .map(|i| format!("/repo/src/w{i}.ts\n  {i}:3  warning  Unexpected console statement  no-console\n"))
                .collect();
            o.push_str("/repo/src/core/engine.ts\n");
            o.push_str(&format!(
                "  412:19  error  {MARKER} 'Frame' is not defined  no-undef\n"
            ));
            o.push_str("\n✖ 501 problems (1 error, 500 warnings)\n");
            (argv(&["eslint", "."]), o)
        }
        "terraform" => {
            let mut o: String = (0..500)
                .map(|i| format!("aws_instance.node[{i}]: Refreshing state... [id=i-0{i}]\n"))
                .collect();
            o.push_str("\nError: creating EC2 Instance: InvalidParameterValue\n\n");
            o.push_str(&format!(
                "  with aws_instance.bad,\n  on main.tf line 42: {MARKER}\n"
            ));
            (argv(&["terraform", "apply"]), o)
        }
        "kubectl-apply" => {
            let mut o: String = (0..400)
                .map(|i| format!("deployment.apps/app{i} configured\n"))
                .collect();
            o.push_str("Error from server (Invalid): error when creating \"svc.yaml\":\n");
            o.push_str(&format!(
                "  Service \"api\" is invalid: spec.ports[0].port: {MARKER}\n"
            ));
            (argv(&["kubectl", "apply", "-f", "."]), o)
        }
        "systemctl" => {
            let mut o = String::from(
                "● app.service - App\n   Loaded: loaded\n   Active: failed (Result: exit-code)\n",
            );
            for i in 0..400 {
                o.push_str(&format!(
                    "Aug 02 10:00:{:02} host app[1234]: routine tick {i}\n",
                    i % 60
                ));
            }
            o.push_str(&format!(
                "Aug 02 10:07:00 host app[1234]: fatal: {MARKER} cannot bind port 80\n"
            ));
            o.push_str(
                "Aug 02 10:07:00 host systemd[1]: app.service: Failed with result 'exit-code'.\n",
            );
            (argv(&["systemctl", "status", "app"]), o)
        }
        "git-push" => {
            let mut o: String = (0..300)
                .map(|i| format!("remote: Resolving deltas: {i}%\n"))
                .collect();
            o.push_str("! [rejected]        main -> main (non-fast-forward)\n");
            o.push_str(&format!(
                "error: failed to push some refs to 'origin' {MARKER}\n"
            ));
            (argv(&["git", "push"]), o)
        }
        "ansible" => {
            let mut o: String = (0..400)
                .map(|i| format!("ok: [host{i}] => (item=pkg{i})\n"))
                .collect();
            o.push_str("fatal: [web01]: FAILED! => {\n");
            o.push_str(&format!(
                "    \"msg\": \"{MARKER} Could not find the requested service nginx\"\n}}\n"
            ));
            o.push_str("PLAY RECAP *******\nweb01 : ok=12 changed=0 unreachable=0 failed=1\n");
            (argv(&["ansible-playbook", "site.yml"]), o)
        }
        "mypy" => {
            let mut o: String = (0..600)
                .map(|i| format!("src/mod{i}.py:{i}: note: By default the bodies of untyped functions are not checked\n"))
                .collect();
            o.push_str(&format!("src/core/engine.py:412: error: Argument 1 has incompatible type \"{MARKER}\"; expected \"Frame\"\n"));
            o.push_str("Found 1 error in 1 file (checked 600 source files)\n");
            (argv(&["mypy", "src"]), o)
        }
        "dotnet-build" => {
            let mut o: String = (0..600)
                .map(|i| format!("  Project{i} -> /repo/bin/Project{i}.dll\n"))
                .collect();
            o.push_str(&format!("/repo/src/Engine.cs(412,19): error CS1503: {MARKER} cannot convert from 'ZqxPayload' to 'Frame'\n"));
            o.push_str("Build FAILED.\n    1 Error(s)\n");
            (argv(&["dotnet", "build"]), o)
        }
        "bundle" => {
            let mut o: String = (0..500)
                .map(|i| format!("Using gem{i} 1.0.{i}\n"))
                .collect();
            o.push_str("Bundler could not find compatible versions for gem \"rails\":\n");
            o.push_str(&format!("  In Gemfile:\n    rails (~> 7.1) {MARKER}\n"));
            (argv(&["bundle", "install"]), o)
        }
        "composer" => {
            let mut o: String = (0..400)
                .map(|i| format!("  - Installing vendor/pkg{i} (1.0.{i}): Extracting archive\n"))
                .collect();
            o.push_str(
                "Your requirements could not be resolved to an installable set of packages.\n",
            );
            o.push_str(&format!(
                "  Problem 1\n    - Root composer.json requires php ^8.2 {MARKER}\n"
            ));
            (argv(&["composer", "install"]), o)
        }
        "clippy" => {
            let mut o: String = (0..400)
                .map(|i| format!("warning: unused variable: `x{i}`\n  --> src/m{i}.rs:{i}:9\n"))
                .collect();
            // rustc's gutter is `412 | `, with spaces. An earlier version of
            // this fixture wrote `412|` and "found" a bug that did not exist —
            // a fixture that does not match the real tool tests nothing.
            o.push_str("error: this loop never actually loops\n  --> src/core/engine.rs:412:5\n");
            o.push_str(&format!(
                "   |\n412 |     loop {{ {MARKER}(); }}\n    |     ^^^^^^^^\n"
            ));
            o.push_str(
                "error: could not compile `app` due to 1 previous error; 400 warnings emitted\n",
            );
            (argv(&["cargo", "clippy"]), o)
        }
        "helm" => {
            let mut o: String = (0..300)
                .map(|i| format!("Deleting outdated charts/dep{i}\n"))
                .collect();
            o.push_str("Error: UPGRADE FAILED: cannot patch \"api\" with kind Deployment:\n");
            o.push_str(&format!(
                "  Deployment.apps \"api\" is invalid: {MARKER} spec.selector is immutable\n"
            ));
            (argv(&["helm", "upgrade", "api", "./chart"]), o)
        }
        "rsync" => {
            let mut o: String = (0..600).map(|i| format!("sending file{i}.bin\n")).collect();
            o.push_str(&format!("rsync: [sender] send_files failed to open \"/data/secret\": {MARKER} Permission denied (13)\n"));
            o.push_str("rsync error: some files/attrs were not transferred (code 23)\n");
            (argv(&["rsync", "-a", "src/", "dst/"]), o)
        }
        "psql" => {
            let mut o: String = (0..400).map(|i| format!("INSERT 0 {i}\n")).collect();
            // The marker sits in the SQL itself, not in a trailing `--`
            // comment: comments are something compaction removes ON PURPOSE,
            // so planting the marker there tested the wrong thing.
            o.push_str(&format!(
                "psql:migrate.sql:412: ERROR:  relation \"{MARKER}\" does not exist\n"
            ));
            o.push_str(&format!("LINE 1: INSERT INTO {MARKER} VALUES (1);\n"));
            (argv(&["psql", "-f", "migrate.sql"]), o)
        }
        "tsc" => {
            let mut o: String = (0..600)
                .map(|i| {
                    format!(
                        "src/w{i}.tsx:{i}:12 - info TS6133: '{i}' is declared but never used.\n"
                    )
                })
                .collect();
            o.push_str(&format!("src/core/engine.ts:412:7 - error TS2345: Argument of type '{MARKER}' is not assignable.\n"));
            o.push_str("Found 1 error in 1 file.\n");
            (argv(&["tsc", "-p", "tsconfig.json"]), o)
        }
        "apt" => {
            let mut o: String = (0..400)
                .map(|i| format!("Setting up libpkg{i} (1.0.{i}-1) ...\n"))
                .collect();
            o.push_str("E: Unable to correct problems, you have held broken packages.\n");
            o.push_str(&format!(
                "E: {MARKER} Depends: libfoo (>= 2.0) but 1.0 is to be installed\n"
            ));
            (argv(&["apt-get", "install", "-y", "app"]), o)
        }
        "aws-s3-sync" => {
            let mut o: String = (0..500)
                .map(|i| format!("upload: ./dist/chunk{i}.js to s3://bucket/dist/chunk{i}.js\n"))
                .collect();
            o.push_str(&format!("upload failed: ./dist/big.bin to s3://bucket/dist/big.bin {MARKER} An error occurred (AccessDenied)\n"));
            (
                argv(&["aws", "s3", "sync", "dist/", "s3://bucket/dist/"]),
                o,
            )
        }
        "ninja" => {
            let mut o: String = (0..600)
                .map(|i| {
                    format!("[{i}/600] Building CXX object src/CMakeFiles/app.dir/f{i}.cpp.o\n")
                })
                .collect();
            o.push_str("FAILED: src/CMakeFiles/app.dir/engine.cpp.o\n");
            o.push_str(&format!(
                "src/engine.cpp:412:19: error: no viable conversion from '{MARKER}' to 'Frame'\n"
            ));
            o.push_str("ninja: build stopped: subcommand failed.\n");
            (argv(&["ninja", "-C", "build"]), o)
        }
        "django" => {
            let mut o: String = (0..300)
                .map(|i| format!("  Applying app.migration{i:04}... OK\n"))
                .collect();
            o.push_str("  Applying app.0301_add_frames...Traceback (most recent call last):\n");
            o.push_str(&format!(
                "django.db.utils.ProgrammingError: {MARKER} column \"frame_id\" does not exist\n"
            ));
            (argv(&["python", "manage.py", "migrate"]), o)
        }
        "brew" => {
            let mut o: String = (0..300)
                .map(|i| format!("==> Pouring formula{i}--1.0.{i}.bottle.tar.gz\n"))
                .collect();
            o.push_str("Error: Cannot install under Rosetta 2 in ARM default prefix.\n");
            o.push_str(&format!(
                "Please run: {MARKER} arch -arm64 brew install app\n"
            ));
            (argv(&["brew", "install", "app"]), o)
        }
        "bazel" => {
            let mut o: String = (0..500)
                .map(|i| format!("INFO: From Compiling src/lib{i}.cc:\n"))
                .collect();
            o.push_str("ERROR: /repo/src/BUILD:42:11: Compiling src/engine.cc failed: (Exit 1)\n");
            o.push_str(&format!(
                "src/engine.cc:412:19: error: {MARKER} no member named 'zqx' in 'Frame'\n"
            ));
            (argv(&["bazel", "build", "//..."]), o)
        }
        "ffmpeg" => {
            let mut o = String::from("ffmpeg version 6.1 Copyright (c) 2000-2023\n");
            for i in 0..500 {
                o.push_str(&format!("frame= {i:4} fps= 24 q=28.0 size=  {i}kB time=00:00:{:02}.00 bitrate= 800kbits/s\n", i % 60));
            }
            o.push_str(&format!(
                "[out#0 @ 0x55] Error muxing a packet: {MARKER} Invalid argument\n"
            ));
            (argv(&["ffmpeg", "-i", "in.mp4", "out.mp4"]), o)
        }
        "alembic" => {
            let mut o: String = (0..250)
                .map(|i| {
                    format!(
                        "INFO  [alembic.runtime.migration] Running upgrade rev{i} -> rev{}\n",
                        i + 1
                    )
                })
                .collect();
            o.push_str("sqlalchemy.exc.OperationalError: (psycopg2.errors.UndefinedTable)\n");
            o.push_str(&format!("relation \"{MARKER}\" does not exist\n"));
            (argv(&["alembic", "upgrade", "head"]), o)
        }
        // BuildKit is the DEFAULT docker builder now; the `Step N/M` outline
        // above is the legacy format. Different shape entirely: `#<id> <phase>`
        // with the failing step's own stdout replayed under `#<id> <elapsed>`.
        "docker-buildkit" => {
            let mut o = String::new();
            for i in 0..600 {
                o.push_str(&format!(
                    "#{i} [builder {i}/600] RUN echo layer{i}\n#{i} DONE 0.1s\n"
                ));
            }
            o.push_str("#601 [builder 600/600] RUN npm ci\n");
            o.push_str("#601 0.512 npm ERR! code ERESOLVE\n");
            o.push_str(&format!(
                "#601 0.513 npm ERR! {MARKER} unable to resolve dependency tree\n"
            ));
            o.push_str("#601 ERROR: process \"/bin/sh -c npm ci\" did not complete successfully: exit code: 1\n");
            (argv(&["docker", "build", "."]), o)
        }
        "go-test-verbose" => {
            let mut o = String::new();
            for i in 0..500 {
                o.push_str(&format!(
                    "=== RUN   TestCase{i}\n--- PASS: TestCase{i} (0.00s)\n"
                ));
            }
            o.push_str("=== RUN   TestBroken\n--- FAIL: TestBroken (0.00s)\n");
            o.push_str(&format!(
                "    engine_test.go:118: expected 42, got 41 {MARKER}\n"
            ));
            o.push_str("FAIL\n");
            (argv(&["go", "test", "-v", "./..."]), o)
        }
        "pytest-verbose" => {
            let mut o: String = (0..500)
                .map(|i| {
                    format!("tests/test_mod{i}.py::test_case PASSED                      [{i}%]\n")
                })
                .collect();
            o.push_str("tests/test_broken.py::test_zqx FAILED                          [100%]\n");
            o.push_str(&format!("E       assert 41 == 42  # {MARKER}\n"));
            (argv(&["pytest", "-v"]), o)
        }
        "cargo-nocapture" => {
            let mut o = String::new();
            for i in 0..500 {
                o.push_str(&format!("test mod{i}::case ... ok\nprintln from case{i}\n"));
            }
            o.push_str("test mod::broken ... FAILED\n");
            o.push_str(&format!(
                "thread 'mod::broken' panicked at src/lib.rs:9:5:\n{MARKER}\n"
            ));
            (argv(&["cargo", "test", "--", "--nocapture"]), o)
        }
        other => panic!("unknown case {other}"),
    }
}

#[test]
fn the_reason_a_command_failed_survives_compaction() {
    const CASES: &[&str] = &[
        "cargo-test",
        "cargo-build",
        "go-test",
        "pytest",
        "jest",
        "docker-build",
        "npm-install",
        "make",
        "pip",
        "gradle",
        "mvn",
        "eslint",
        "terraform",
        "kubectl-apply",
        "systemctl",
        "git-push",
        "ansible",
        "mypy",
        "dotnet-build",
        "bundle",
        "composer",
        "clippy",
        "helm",
        "rsync",
        "psql",
        "tsc",
        "apt",
        "aws-s3-sync",
        "ninja",
        "django",
        "brew",
        "bazel",
        "ffmpeg",
        "alembic",
        "docker-buildkit",
    ];
    let mut lost = Vec::new();
    let mut unfolded = Vec::new();
    for name in CASES {
        let (a, raw) = case(name);
        let out = tokenade_core::filter::filter_command(&a, &raw).output;
        if !out.contains(MARKER) {
            lost.push(format!(
                "{name}: {} raw chars -> {} kept, marker GONE",
                raw.len(),
                out.len()
            ));
        }
        // The other half of the contract. Without it this gate is satisfied by
        // a compactor that stopped compacting — passthrough keeps every marker
        // ever planted, and a silent regression to passthrough is exactly the
        // kind of thing a one-sided test blesses.
        //
        if out.len() * 2 > raw.len() {
            unfolded.push(format!(
                "{name}: {} raw chars -> {} kept (< 2x is not a fold)",
                raw.len(),
                out.len()
            ));
        }
    }
    assert!(
        lost.is_empty(),
        "compaction dropped the reason these commands failed:\n  {}",
        lost.join("\n  ")
    );
    assert!(
        unfolded.is_empty(),
        "these commands are no longer being compacted at all:\n  {}",
        unfolded.join("\n  ")
    );
}

/// Asking for verbose output means asking for the bytes — as long as they can
/// be delivered.
///
/// `explicit_raw_intent` reads any verbose flag as a request for raw output,
/// and three fixtures written to expect folding on `go test -v`, `pytest -v`
/// and `cargo test -- --nocapture` are what surfaced the rule. It held at ANY
/// size until 2026-10-02, when the operator chose a budget instead
/// (`raw_request_honoured`): past the delivery threshold the stash cut the
/// passthrough to its head and tail anyway — `docker system df -v` kept one
/// container of ten and no named volume. Both sides are pinned here.
#[test]
fn a_verbose_invocation_is_returned_untouched() {
    const VERBOSE: &[(&[&str], &str)] = &[
        (&["go", "test", "-v", "./..."], "=== RUN   TestCase"),
        (&["pytest", "-v"], "::test_case PASSED"),
        (&["cargo", "test", "--", "--nocapture"], "println from case"),
        (&["jest", "--verbose"], "PASS src/w"),
    ];
    let threshold = tokenade_core::disclose::threshold_tokens();
    for (a, needle) in VERBOSE {
        let run = |n: usize| -> String {
            (0..n)
                .map(|i| format!("{needle}{i}\n--- PASS: case{i} (0.00s)\n"))
                .collect()
        };
        let small = run(60);
        assert!(tokenade_core::tokenizer::estimate(&small) <= threshold);
        let out = tokenade_core::filter::filter_command(&argv(a), &small).output;
        assert_eq!(
            out,
            small,
            "`{}` asked for the bytes and must get them",
            a.join(" ")
        );

        let large = run(2_000);
        assert!(tokenade_core::tokenizer::estimate(&large) > threshold);
        let out = tokenade_core::filter::filter_command(&argv(a), &large).output;
        assert!(
            out.len() < large.len(),
            "`{}` past the delivery threshold must reach its compactor",
            a.join(" ")
        );
    }
}
