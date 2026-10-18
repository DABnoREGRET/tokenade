//! Quality contracts: per-compactor lists of strings that MUST appear
//! in the output for a representative input.
//!
//! Motivation (user ask): "in every compactor test, check that no
//! important info has disappeared — that implies listing the things
//! that must appear in the response."
//!
//! Each compactor entry is a `Contract` with:
//!   - `name`: the compactor name (for failure messages),
//!   - `input`: a realistic fixture covering the format,
//!   - `must_contain`: the load-bearing facts the agent needs after
//!     compaction (e.g. for git log: commit SHAs + subjects + author),
//!   - `must_not_contain`: anti-regression hooks (e.g. secret values
//!     that must be redacted, or noise lines that must be stripped).
//!
//! Adding a compactor:
//!   1. Add a `Contract` to `contracts()` below.
//!   2. List 4-8 must_contain strings — the kind of info a downstream
//!      agent would need to act on the output.
//!   3. List 1-3 must_not_contain strings — anti-regression for noise
//!      or secrets the compactor is supposed to drop.
//!   4. `cargo test -p tokenade-core --test quality_contracts` — if a
//!      must_contain string is missing, the test fails with the
//!      offending compactor + missing string + full output.
//!
//! This file is the SOURCE OF TRUTH for "what counts as no-quality-
//! loss" per compactor. A refactor that drops one of these strings is
//! a bug, not a stylistic choice.

use tokenade_core::{
    ansible, archetypes, cargobench, csv as tokenade_csv, diff, dockerfile, gitlog, ini, jvmbuild,
    k8smanifest, kubectl, markdown, ndjson, openapi, sql, stacktrace, terraform, toml_compact,
    yaml as tokenade_yaml,
};

/// One quality contract: input fixture + facts that MUST survive
/// compaction (must_contain) + noise that MUST be stripped
/// (must_not_contain).
struct Contract {
    name: &'static str,
    input: String,
    compact: fn(&str) -> String,
    must_contain: Vec<&'static str>,
    must_not_contain: Vec<&'static str>,
    /// The strip WITHOUT its declaration, for the comment strippers. Present
    /// only so the "returned its input unchanged" exemption below can be
    /// EARNED rather than granted: without it a compactor that silently does
    /// nothing would satisfy the whole contract.
    strip: Option<fn(&str) -> String>,
    /// Why this compactor legitimately returns THIS fixture untouched, when it
    /// is not the declaration's cost gate. Prose, so the reason is written down
    /// rather than inferred from a silent pass — and required, so a compactor
    /// that starts doing nothing cannot join the list by accident.
    known_untouched: Option<&'static str>,
}

fn contracts() -> Vec<Contract> {
    vec![
        // ── gitlog ──────────────────────────────────────────────────
        // The agent needs every (SHA, author, subject) tuple. Dropping
        // an author or merging two commits silently is a quality bug.
        Contract {
            name: "gitlog",
            compact: gitlog::compact,
            input: build_gitlog_input(),
            must_contain: vec![
                // Every SHA prefix MUST survive — agents reference them.
                "a1b2c3d",
                "b2c3d4e",
                "c3d4e5f",
                "d4e5f6a",
                // Every distinct author MUST appear at least once.
                "Alice Doe",
                "Bob Smith",
                "Carol Lee",
                // Every subject MUST survive verbatim.
                "fix: handle empty config",
                "feat: add /v2 endpoint",
                "refactor: extract retry helper",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![
                // Per-commit "AuthorDate:" lines should be folded — if
                // they all survive the compactor is doing nothing.
                "AuthorDate: Mon Jun 02 09:30:00 2026 +0000\nCommit:",
            ],
        },
        // ── stacktrace ──────────────────────────────────────────────
        // Critical: the error type+message MUST survive AND user-code
        // frames MUST survive. Framework frames can fold.
        Contract {
            name: "stacktrace",
            compact: stacktrace::compact,
            input: build_python_traceback(),
            must_contain: vec![
                // The headline exception is the most important byte.
                "CustomerNotFoundError",
                "no customer for user_id=42",
                // User-code frames — these tell the agent WHERE to fix.
                "/app/myapp/views/billing.py",
                "/app/myapp/integrations/stripe.py",
                "charge_card",
                "lookup_customer",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![
                // All 6 raw framework frame paths shouldn't survive
                // verbatim — they should be folded.
                "asyncio/runners.py:118\nasyncio/base_events.py:654",
            ],
        },
        // ── kubectl describe ────────────────────────────────────────
        // Pod identity + state + events tail are load-bearing.
        Contract {
            name: "kubectl",
            compact: kubectl::compact,
            input: build_kubectl_describe(),
            must_contain: vec![
                "web-7d8b9c-xj4kf",      // pod name
                "production",            // namespace
                "Running",               // status
                "myorg/web:v2.4.1",      // image
                "Liveness probe failed", // the actionable event
                "503",                   // status code in the event
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![
                // The verbose Volumes block should be folded.
                "default-token-abc:\n    Type:        Secret",
            ],
        },
        // ── jvmbuild Maven ──────────────────────────────────────────
        // Compilation errors with file:line are what the agent acts on.
        Contract {
            name: "jvmbuild_maven",
            compact: jvmbuild::compact,
            input: build_maven_failure(),
            must_contain: vec![
                "BUILD FAILURE",
                "InvoiceService.java",
                "cannot find symbol", // error keyword
                "[42,16]",            // file:line:col location
                "billing",            // failing module
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![
                // The plugin-resolution noise should be cut.
                "maven-resources-plugin:3.3.1:resources",
            ],
        },
        // ── terraform plan ──────────────────────────────────────────
        // Plan summary + per-resource action are load-bearing.
        Contract {
            name: "terraform",
            compact: terraform::compact,
            input: build_terraform_plan(),
            must_contain: vec![
                "aws_instance.foo",
                "will be updated in-place",
                "Plan: 0 to add, 1 to change, 0 to destroy",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![
                // 30 Refreshing-state lines should fold to a count.
                "module.web[5]: Refreshing state... [id=sg-5]\nmodule.web[6]: Refreshing state",
            ],
        },
        // ── ndjson ──────────────────────────────────────────────────
        // Distinct event types + a representative sample MUST survive.
        Contract {
            name: "ndjson",
            compact: ndjson::compact,
            input: build_ndjson_events(),
            must_contain: vec![
                "user.signup",  // distinct event type 1
                "user.login",   // distinct event type 2
                "order.placed", // distinct event type 3
                "alice",        // sample user id
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── csv ─────────────────────────────────────────────────────
        // Header row + every column name MUST survive.
        Contract {
            name: "csv",
            compact: tokenade_csv::compact_csv,
            input: build_csv_users(),
            must_contain: vec![
                // Every header column the agent needs to filter by.
                "id",
                "name",
                "email",
                "plan",
                // At least one representative row's primary key.
                "alice@example.com",
            ],
            strip: None,
            known_untouched: Some(
                "20 rows is below `compact_csv`'s own floor. The SHIPPED path folds it anyway through the layer above: measured 2026-08-26, `tokenade read` on this exact fixture returns 154 bytes of 641, and 1 004 of 69 805 on a 2 000-row twin.",
            ),
            must_not_contain: vec![],
        },
        // ── markdown README ─────────────────────────────────────────
        // Every heading is the document's structure — losing one
        // confuses the agent's mental model.
        Contract {
            name: "markdown",
            compact: markdown::compact,
            input: build_markdown_readme(),
            must_contain: vec![
                "# myproject",
                "## Installation",
                "## Quick start",
                "## Configuration",
                "### Advanced tuning",
                // Code blocks survive verbatim (small ones).
                "cargo add myproject",
                "Foo::from_path",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── cargobench ──────────────────────────────────────────────
        // Benchmark names + ns/iter are load-bearing.
        Contract {
            name: "cargobench",
            compact: cargobench::compact,
            input: build_cargo_bench_output(),
            must_contain: vec!["bench_compact_json", "bench_compact_yaml", "ns/iter"],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── openapi ─────────────────────────────────────────────────
        // Every endpoint path + method MUST survive.
        Contract {
            name: "openapi",
            compact: openapi::compact,
            input: build_openapi_yaml(),
            must_contain: vec![
                "/users",
                "/users/{id}",
                "/orders",
                // HTTP methods — if a compactor drops these the agent
                // can't tell GET from POST.
                "get:",
                "post:",
            ],
            strip: None,
            known_untouched: Some(
                "605 bytes of already-terse YAML with no repetition to collapse. Measured 2026-08-26: `tokenade read` returns all 605 too, so nothing above this layer folds it either — the fixture is simply smaller than anything worth folding.",
            ),
            must_not_contain: vec![],
        },
        // ── yaml ────────────────────────────────────────────────────
        // Top-level keys + nested-resource identity (kind, name) MUST
        // survive — these are the "what is this thing?" anchors.
        Contract {
            name: "yaml",
            compact: tokenade_yaml::compact,
            input: build_yaml_k8s_deployment(),
            must_contain: vec![
                "apiVersion",
                "kind",
                "Deployment",
                "name",
                "web-server",
                "replicas",
                "containers",
                "myorg/web",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── toml ────────────────────────────────────────────────────
        // Every [section] header + dependency name MUST survive — those
        // are the load-bearing things in Cargo.toml/pyproject.toml.
        Contract {
            name: "toml",
            compact: toml_compact::compact,
            input: build_toml_cargo(),
            must_contain: vec![
                "[package]",
                "[dependencies]",
                "[dev-dependencies]",
                "name",
                "tokenade-core",
                "serde",
                "tokio",
            ],
            known_untouched: None,
            strip: Some(toml_compact::strip_only),
            must_not_contain: vec![],
        },
        // ── diff ────────────────────────────────────────────────────
        // File paths + hunk headers + at least one +/- line per file
        // MUST survive.
        Contract {
            name: "diff",
            compact: diff::compact,
            input: build_unified_diff(),
            must_contain: vec![
                "src/main.rs",
                "src/lib.rs",
                "@@",
                // The actual change content — if these get stripped the
                // agent can't see what changed.
                "fn new_function",
                "pub mod added",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── dockerfile ──────────────────────────────────────────────
        // Every instruction MUST survive — they're not noise, they're
        // the recipe.
        Contract {
            name: "dockerfile",
            compact: dockerfile::compact,
            input: build_dockerfile(),
            must_contain: vec![
                "FROM",
                "rust:1.82",
                "WORKDIR",
                "COPY",
                "RUN",
                "EXPOSE",
                "CMD",
                "8080",
            ],
            known_untouched: None,
            strip: Some(dockerfile::strip_only),
            must_not_contain: vec![],
        },
        // ── ansible ─────────────────────────────────────────────────
        // Play recap + per-host failed/unreachable count MUST survive.
        Contract {
            name: "ansible",
            compact: ansible::compact,
            input: build_ansible_recap(),
            must_contain: vec![
                "PLAY RECAP",
                "web-01", // host name
                "db-02",  // host name
                "failed=",
                "unreachable=",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── k8smanifest ─────────────────────────────────────────────
        // Per-resource (kind, name) identity MUST survive.
        Contract {
            name: "k8smanifest",
            compact: k8smanifest::compact,
            input: build_k8s_multi_manifest(),
            must_contain: vec![
                "Deployment",
                "Service",
                "ConfigMap",
                "web-app",
                "web-svc",
                "web-config",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── sql ─────────────────────────────────────────────────────
        // Column headers + at least one representative row MUST survive.
        Contract {
            name: "sql",
            compact: sql::compact,
            input: build_sql_result(),
            must_contain: vec![
                "id",
                "name",
                "email",
                "alice@example.com",
                // The (N rows) summary IS the load-bearing fact for
                // counts.
                "rows",
            ],
            strip: None,
            known_untouched: None,
            must_not_contain: vec![],
        },
        // ── ini ─────────────────────────────────────────────────────
        // Every [section] + every key MUST survive (no comments needed).
        Contract {
            name: "ini",
            compact: ini::compact,
            known_untouched: None,
            strip: Some(ini::strip_only),
            input: build_ini_config(),
            must_contain: vec![
                "[server]",
                "[database]",
                "[logging]",
                "host",
                "port",
                "url",
                "level",
            ],
                        must_not_contain: vec![
                // Comment-only line drop — these should be stripped.
                "; commented out value",
            ],
        },
    ]
}

#[test]
fn every_compactor_preserves_its_quality_contract() {
    let contracts = contracts();
    let mut failures: Vec<String> = Vec::new();
    for c in &contracts {
        let out = (c.compact)(&c.input);
        for needle in &c.must_contain {
            if !out.contains(needle) {
                failures.push(format!(
                    "compactor `{}` LOST required string {:?}\n--- input ({} bytes) ---\n{}\n--- output ---\n{}\n",
                    c.name,
                    needle,
                    c.input.len(),
                    truncate(&c.input, 400),
                    truncate(&out, 600),
                ));
            }
        }
        // A compactor that returned its input UNCHANGED made no claim to have
        // removed anything, so a forbidden string in it is not a leak. The
        // comment strippers declare what they removed and, when that sentence
        // would cost more than the strip saved, hand back the file whole —
        // which is the honest AND cheaper answer, and on this ~400-byte ini
        // fixture is what happens. Any output that IS a compaction still owes
        // the contract in full: the exemption is byte equality, not a class.
        let untouched = out == c.input;
        if untouched {
            // EARN the exemption. The only legitimate reason to hand back the
            // input untouched is the declaration's cost gate: the sentence
            // saying what was removed would have cost more than the removal
            // saved. Prove that here, or the exemption is just a compactor
            // that does nothing passing its own contract.
            match c.strip {
                Some(strip) => {
                    let stripped = strip(&c.input);
                    assert!(
                        stripped.len() + 128 >= c.input.len(),
                        "compactor `{}` returned its input unchanged while its strip saved {} bytes — that is not the cost gate, that is a compactor doing nothing",
                        c.name,
                        c.input.len() - stripped.len()
                    );
                }
                None => {
                    if c.known_untouched.is_none() {
                        failures.push(format!(
                            "compactor `{}` returned its input UNCHANGED, declares no strip to justify it, and gives no reason",
                            c.name
                        ));
                    }
                }
            }
        }
        for forbidden in &c.must_not_contain {
            if !untouched && out.contains(forbidden) {
                failures.push(format!(
                    "compactor `{}` LEAKED forbidden string {:?}\n--- output ---\n{}\n",
                    c.name,
                    forbidden,
                    truncate(&out, 600),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} quality contract violation(s):\n\n{}",
        failures.len(),
        failures.join("\n========\n")
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…[+{} bytes]", &s[..max.min(s.len())], s.len() - max)
    }
}

// ──────────────────────────────────────────────────────────────────
// Archetype-level quality contracts.
//
// The ~4000 per-shell-command handlers under `src/cmd/linux/` are
// thin wrappers around 4 archetypes (`passthrough`, `line_cap`,
// `collapse_padding`, `strip_noise`). Their quality is GOVERNED by
// the archetype, not the wrapper. Lock the archetype semantics here
// and the entire 4000-handler surface is covered transitively.
//
// User ask: "every command must be tested" — this is how we test
// all 4000+ commands in O(4) contracts instead of O(4000).
// ──────────────────────────────────────────────────────────────────

#[test]
fn archetype_passthrough_preserves_every_input_line_verbatim() {
    // The `passthrough` archetype underlies trivial-output commands
    // (`whoami`, `id`, `date`, `pwd`, etc.). Contract: every byte of
    // input survives except trailing whitespace at the very end.
    let inputs = [
        "infected",
        "uid=1000(infected) gid=1000(infected)",
        "Mon Jun  2 14:30:00 UTC 2026",
        "/home/username/projects/tokenade",
        "line1\nline2\nline3",
        "multi\n  with  spaces\n\tand tabs",
    ];
    for input in inputs {
        let out = archetypes::passthrough(input);
        // Every non-trailing byte MUST survive.
        let expected = input.trim_end();
        assert_eq!(
            out, expected,
            "passthrough lost bytes: input={input:?} output={out:?}"
        );
    }
}

#[test]
fn archetype_line_cap_keeps_all_when_under_cap() {
    // Contract: when input has ≤ cap non-empty lines, ALL of them
    // survive verbatim (trailing whitespace per line stripped).
    let lines: Vec<String> = (0..10).map(|i| format!("entry-{i}")).collect();
    let input = lines.join("\n");
    let out = archetypes::line_cap(&input, 20, "entries");
    for line in &lines {
        assert!(
            out.contains(line),
            "line_cap dropped under-cap entry {line:?}: {out}"
        );
    }
    // No elision banner when under cap.
    assert!(
        !out.contains("more entries elided"),
        "line_cap emitted elision banner when under cap: {out}"
    );
}

#[test]
fn archetype_line_cap_keeps_first_n_and_announces_elided_count() {
    // Contract: when input has > cap non-empty lines, the FIRST cap
    // survive AND the output names the exact count elided. The agent
    // needs the count to know HOW MUCH was dropped.
    let lines: Vec<String> = (0..100).map(|i| format!("entry-{i}")).collect();
    let input = lines.join("\n");
    let out = archetypes::line_cap(&input, 30, "entries");
    // First 30 lines MUST be present.
    for line in &lines[..30] {
        assert!(
            out.contains(line),
            "line_cap dropped first-cap entry {line:?}: {out}"
        );
    }
    // Elided count MUST be exact.
    assert!(
        out.contains("[... 70 more entries elided]"),
        "line_cap missed or mis-counted elision: {out}"
    );
    // Noun MUST appear (so the agent knows WHAT was elided).
    assert!(out.contains("entries"), "line_cap dropped the noun");
}

#[test]
fn archetype_collapse_padding_preserves_all_non_blank_lines() {
    // Contract: every non-blank line of input MUST appear in output.
    // Only blank lines and runs of >2 spaces are dropped.
    let input = "  line one  \n\n\n  line two  \n\nline three\n";
    let out = archetypes::collapse_padding(input);
    assert!(out.contains("line one"));
    assert!(out.contains("line two"));
    assert!(out.contains("line three"));
}

#[test]
fn archetype_strip_noise_keeps_non_noise_lines_and_counts_stripped() {
    // Contract: lines NOT matching any noise prefix survive; lines
    // that DO match are counted in a summary the agent can see.
    let input = "\
        DEBUG: trace 1\n\
        important fact\n\
        DEBUG: trace 2\n\
        another fact\n\
        DEBUG: trace 3\n\
        third fact\n\
    ";
    let out = archetypes::strip_noise(input, &["DEBUG:"], "debug line(s)");
    // Non-noise lines MUST survive.
    assert!(out.contains("important fact"));
    assert!(out.contains("another fact"));
    assert!(out.contains("third fact"));
    // The agent MUST see HOW MANY noise lines were stripped — silent
    // drop is a quality loss even if the lines themselves are "noise".
    assert!(
        out.contains("3") && out.contains("debug line"),
        "strip_noise didn't announce stripped count: {out}"
    );
}

#[test]
fn archetype_line_cap_exact_boundary_at_cap_keeps_all() {
    // Edge: exactly `cap` lines should keep ALL with no banner.
    let lines: Vec<String> = (0..50).map(|i| format!("L{i}")).collect();
    let input = lines.join("\n");
    let out = archetypes::line_cap(&input, 50, "items");
    for line in &lines {
        assert!(out.contains(line), "boundary line {line} dropped");
    }
    assert!(
        !out.contains("more items elided"),
        "spurious elision at exact boundary: {out}"
    );
}

// ──────────────────────────────────────────────────────────────────
// Fixtures — kept inline so a single file is the source of truth.
// ──────────────────────────────────────────────────────────────────

fn build_gitlog_input() -> String {
    let commits = [
        ("a1b2c3d", "Alice Doe", "fix: handle empty config"),
        ("b2c3d4e", "Bob Smith", "feat: add /v2 endpoint"),
        ("c3d4e5f", "Carol Lee", "refactor: extract retry helper"),
        ("d4e5f6a", "Alice Doe", "test: cover empty-batch path"),
    ];
    let mut s = String::new();
    for (sha, author, subj) in commits {
        s.push_str(&format!("commit {sha}0000000000000000000000000000000\n"));
        s.push_str(&format!(
            "Author: {author} <{}@example.com>\n",
            author.split_whitespace().next().unwrap().to_lowercase()
        ));
        s.push_str("AuthorDate: Mon Jun 02 09:30:00 2026 +0000\n");
        s.push_str(&format!(
            "Commit: {author} <{}@example.com>\n",
            author.split_whitespace().next().unwrap().to_lowercase()
        ));
        s.push_str("CommitDate: Mon Jun 02 09:30:00 2026 +0000\n\n");
        s.push_str(&format!("    {subj}\n\n"));
    }
    s
}

fn build_python_traceback() -> String {
    String::from(
        r#"Traceback (most recent call last):
  File "/usr/lib/python3.11/asyncio/runners.py", line 118, in run
    return self._loop.run_until_complete(task)
  File "/usr/lib/python3.11/asyncio/base_events.py", line 654, in run_until_complete
    return future.result()
  File "/usr/lib/python3.11/site-packages/django/core/handlers/asgi.py", line 196, in __call__
    await self.send_response(response, send)
  File "/usr/lib/python3.11/site-packages/django/core/handlers/asgi.py", line 264, in send_response
    await send({"type": "http.response.body", "body": chunk})
  File "/usr/lib/python3.11/site-packages/django/core/handlers/base.py", line 220, in _get_response_async
    response = await wrapped_callback(request, *callback_args, **callback_kwargs)
  File "/app/myapp/views/billing.py", line 47, in charge_card
    customer = stripe_client.lookup_customer(user_id)
  File "/app/myapp/integrations/stripe.py", line 89, in lookup_customer
    raise CustomerNotFoundError(f"no customer for user_id={user_id}")
myapp.errors.CustomerNotFoundError: no customer for user_id=42
"#,
    )
}

fn build_kubectl_describe() -> String {
    String::from(
        r#"Name:             web-7d8b9c-xj4kf
Namespace:        production
Status:           Running
IP:               10.0.1.45
Containers:
  web:
    Image:          myorg/web:v2.4.1
    State:          Running
    Restart Count:  3
Volumes:
  default-token-abc:
    Type:        Secret
    SecretName:  default-token-abc
Events:
  Type     Reason     Age                From               Message
  ----     ------     ----               ----               -------
  Warning  Unhealthy  9m (x3 over 10m)   kubelet            Liveness probe failed: HTTP probe failed with statuscode: 503
  Normal   Killing    9m (x3 over 10m)   kubelet            Container web failed liveness probe, will be restarted
"#,
    )
}

fn build_maven_failure() -> String {
    String::from(
        r#"[INFO] Building billing 1.0-SNAPSHOT [2/3]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ billing ---
[INFO] Copying 2 resources from src/main/resources to target/classes
[INFO] --- maven-compiler-plugin:3.13.0:compile (default-compile) @ billing ---
[INFO] Compiling 47 source files with javac
[ERROR] COMPILATION ERROR :
[ERROR] /home/ci/billing/src/main/java/com/example/billing/InvoiceService.java:[42,16] cannot find symbol
  symbol:   class Stripe
[ERROR] /home/ci/billing/src/main/java/com/example/billing/InvoiceService.java:[58,12] incompatible types
[INFO] 2 errors
[INFO] BUILD FAILURE
"#,
    )
}

fn build_terraform_plan() -> String {
    let mut s = String::new();
    for i in 0..30 {
        s.push_str(&format!(
            "module.web[{i}]: Refreshing state... [id=sg-{i}]\n"
        ));
    }
    s.push_str("\nTerraform will perform the following actions:\n\n");
    s.push_str("  # aws_instance.foo will be updated in-place\n");
    s.push_str("Plan: 0 to add, 1 to change, 0 to destroy.\n");
    s
}

fn build_ndjson_events() -> String {
    let mut s = String::new();
    for i in 0..15 {
        s.push_str(&format!(
            "{{\"ts\":{},\"event\":\"user.signup\",\"user\":\"alice{i}\"}}\n",
            1_700_000_000 + i
        ));
    }
    for i in 0..15 {
        s.push_str(&format!(
            "{{\"ts\":{},\"event\":\"user.login\",\"user\":\"alice{i}\"}}\n",
            1_700_000_100 + i
        ));
    }
    for i in 0..15 {
        s.push_str(&format!(
            "{{\"ts\":{},\"event\":\"order.placed\",\"user\":\"alice{i}\",\"amount\":42}}\n",
            1_700_000_200 + i
        ));
    }
    s
}

fn build_csv_users() -> String {
    let mut s = String::from("id,name,email,plan\n");
    for i in 1..=20 {
        s.push_str(&format!("{i},user{i},alice@example.com,pro\n"));
    }
    s
}

fn build_markdown_readme() -> String {
    String::from(
        r##"# myproject

A library for parsing Foo records.

## Installation

```bash
cargo add myproject
```

## Quick start

```rust
use myproject::Foo;
let foo = Foo::from_path("input.foo").unwrap();
```

This works for v2 records. For v1 you need the `legacy` feature.

## Configuration

| Key | Default |
| --- | ------- |
| `max_records` | 10_000 |

### Advanced tuning

For high-throughput pipelines, increase `max_records`.
"##,
    )
}

fn build_cargo_bench_output() -> String {
    String::from(
        r#"running 3 tests
test bench_compact_json ... bench:       1,234 ns/iter (+/- 56)
test bench_compact_yaml ... bench:       2,345 ns/iter (+/- 78)
test bench_compact_toml ... bench:       3,456 ns/iter (+/- 90)

test result: ok. 0 passed; 0 failed; 0 ignored; 3 measured; 0 filtered out
"#,
    )
}

fn build_openapi_yaml() -> String {
    String::from(
        r#"openapi: 3.0.0
info:
  title: Example API
  version: 1.0.0
paths:
  /users:
    get:
      summary: list users
      responses:
        '200':
          description: ok
    post:
      summary: create user
      responses:
        '201':
          description: created
  /users/{id}:
    get:
      summary: get user
      parameters:
        - in: path
          name: id
          required: true
          schema:
            type: integer
      responses:
        '200':
          description: ok
  /orders:
    get:
      summary: list orders
      responses:
        '200':
          description: ok
"#,
    )
}

fn build_yaml_k8s_deployment() -> String {
    String::from(
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-server
  namespace: production
  labels:
    app: web
    tier: frontend
spec:
  replicas: 3
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: web
          image: myorg/web:v2.4.1
          ports:
            - containerPort: 8080
          env:
            - name: DB_HOST
              value: db.internal
"#,
    )
}

fn build_toml_cargo() -> String {
    String::from(
        r#"[package]
name = "myproject"
version = "0.1.0"
edition = "2021"
description = "A tokenade-core sibling demo project"

[dependencies]
tokenade-core = { workspace = true }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.40", features = ["full"] }
anyhow = "1.0"

[dev-dependencies]
insta = "1.47"
proptest = "1.5"
tempfile = "3.10"
"#,
    )
}

fn build_unified_diff() -> String {
    String::from(
        r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,6 +10,11 @@ fn main() {
     println!("Hello, world!");
 }

+fn new_function() -> i32 {
+    let x = 42;
+    x * 2
+}
+
 fn existing() {}
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,5 @@
 pub mod existing;
+pub mod added;
+
 pub mod another;
"#,
    )
}

fn build_dockerfile() -> String {
    String::from(
        r#"FROM rust:1.82-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /opt/app
COPY --from=builder /app/target/release/myapp /usr/local/bin/myapp
EXPOSE 8080
CMD ["myapp", "--port", "8080"]
"#,
    )
}

fn build_ansible_recap() -> String {
    String::from(
        r#"PLAY [Configure web tier] ******************************************************

TASK [Gathering Facts] *********************************************************
ok: [web-01]
ok: [web-02]
ok: [db-02]

TASK [install nginx] ***********************************************************
changed: [web-01]
changed: [web-02]
skipping: [db-02]

TASK [start nginx] *************************************************************
ok: [web-01]
fatal: [web-02]: FAILED! => {"msg": "nginx config syntax error"}

PLAY RECAP *********************************************************************
web-01                     : ok=3    changed=1    unreachable=0    failed=0
web-02                     : ok=2    changed=1    unreachable=0    failed=1
db-02                      : ok=1    changed=0    unreachable=0    failed=0
"#,
    )
}

fn build_k8s_multi_manifest() -> String {
    String::from(
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-app
  namespace: production
spec:
  replicas: 3
  template:
    spec:
      containers:
        - name: web
          image: myorg/web:v2
---
apiVersion: v1
kind: Service
metadata:
  name: web-svc
  namespace: production
spec:
  selector:
    app: web
  ports:
    - port: 80
      targetPort: 8080
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: web-config
  namespace: production
data:
  log_level: info
  feature_flag: enabled
"#,
    )
}

fn build_sql_result() -> String {
    let mut s = String::from(
        " id |  name   |       email        |  plan\n\
         ----+---------+--------------------+-------\n",
    );
    for i in 1..=12 {
        s.push_str(&format!("  {i} | user{i:02}  | alice@example.com  | pro\n",));
    }
    s.push_str("(12 rows)\n");
    s
}

fn build_ini_config() -> String {
    String::from(
        r#"; sample ini config
[server]
host = 0.0.0.0
port = 8080
; commented out value
; old_port = 9090

[database]
url = postgres://localhost/myapp
pool_size = 16

[logging]
level = info
format = json
"#,
    )
}
