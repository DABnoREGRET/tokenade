// The tests of `main`.
//
// Spliced in by `include!` from `main.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod help_passthrough_tests {
    use super::cmd_forwards_help;

    #[test]
    fn shim_exec_and_wrappers_forward_help_but_plain_cmds_do_not() {
        // `shim-exec` included: a stale shim sends `shim-exec df -- -h`, which
        // must reach the removal message rather than the global banner.
        for c in ["shim-exec", "raw", "wrap", "filter", "execute"] {
            assert!(
                cmd_forwards_help(c),
                "{c} must forward -h to its inner tool"
            );
        }
        // Commands that own their args must still get the safe help fallback.
        for c in ["query", "index", "impact", "semantic", "install"] {
            assert!(!cmd_forwards_help(c), "{c} must NOT forward -h");
        }
    }
}

#[cfg(test)]
mod root_is_the_project_not_the_cwd {
    /// The other half of the same contract: the index is keyed on the project,
    /// so the copy handed to a command must be re-based to the directory the
    /// command runs in — otherwise every path printed, and every path
    /// accepted, is relative to somewhere the user is not.
    #[test]
    fn fresh_index_rebases_to_the_cwd() {
        // The FAMILY of `main`, not the trunk alone: `fresh_index` and `root()`
        // moved down into `commands/`, and reading `main.rs` alone would make
        // this guard say the function has disappeared.
        // The dispatcher AND the commands it dispatches to: `root()` stayed in
        // `main.rs`, `fresh_index` moved down into `commands/index_cmds.rs`,
        // and a guard reading only one of the two would announce the
        // disappearance of what it is looking for.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let src = format!(
            "{}{}",
            tokenade_core::source_guard::module_family(&dir, "main"),
            tokenade_core::source_guard::module_family(&dir, "commands"),
        );
        let needle = concat!("fn ", "fresh_index() -> index::Index {");
        let i = src
            .find(needle)
            .expect("fresh_index moved or was renamed — this guard has stopped looking");
        let body_end = src[i..].find("\n}\n").expect("fresh_index body");
        // Comments name `rebase_to` while explaining it; only code counts.
        let body: String = src[i..i + body_end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("rebase_to"),
            "fresh_index stopped re-basing: `query` would hand an agent a path \
             its next Read cannot open"
        );
    }

    /// `semantic` names files for the reader to open, exactly like `query`,
    /// so it owes the same base. Two commands in one tool printing paths on
    /// two different bases is worse than either choice alone.
    #[test]
    fn cmd_semantic_rebases_to_the_cwd() {
        // The subject moved to `semantic_cli.rs`; the guard follows it. It had
        // said itself that it had stopped looking, which is exactly what is
        // asked of it — a probe gone mute after a move is worse than no probe.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/semantic_cli.rs"),
        )
        .expect("semantic_cli.rs");
        let needle = concat!("fn ", "cmd_semantic(rest: &[String]) -> i32 {");
        let i = src
            .find(needle)
            .expect("cmd_semantic moved or was renamed — this guard has stopped looking");
        let body_end = src[i..].find("\n}\n").expect("cmd_semantic body");
        let body: String = src[i..i + body_end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("rebase_to"),
            "cmd_semantic stopped re-basing: its paths would not open from the \
             directory the command ran in, while `query`'s would"
        );
    }

    #[test]
    fn root_resolves_through_project_root_of() {
        // The FAMILY of `main`, not the trunk alone: `fresh_index` and `root()`
        // moved down into `commands/`, and reading `main.rs` alone would make
        // this guard say the function has disappeared.
        // The dispatcher AND the commands it dispatches to: `root()` stayed in
        // `main.rs`, `fresh_index` moved down into `commands/index_cmds.rs`,
        // and a guard reading only one of the two would announce the
        // disappearance of what it is looking for.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let src = format!(
            "{}{}",
            tokenade_core::source_guard::module_family(&dir, "main"),
            tokenade_core::source_guard::module_family(&dir, "commands"),
        );
        // The needle is SPLIT so this line is not itself a match — the
        // self-match trap, in the shape it takes when the guard sits above the code it
        // reads rather than below it. Splitting on the first `#[cfg(test)]`
        // does not work here: main.rs has one long before `root()`.
        let needle = concat!("fn ", "root() -> PathBuf {");
        let i = src
            .find(needle)
            .expect("root() moved or was renamed — this guard has stopped looking");
        let body_end = src[i..].find("\n}\n").expect("root() body");
        // Comments are not code. The body NAMES `project_root_of` in a note
        // explaining why it calls it, so a plain `contains` stays true after
        // the call is deleted — the trap where a sweep read comments as
        // data and accused an innocent.
        let body: String = src[i..i + body_end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("project_root_of"),
            "root() went back to answering the raw cwd: an agent inside a \
             subdirectory would get its own index of that subtree, and `query` \
             would answer `no symbol matching` about a symbol one crate over"
        );
    }
}

#[cfg(test)]
mod includes_never_depend_on_untracked_files {
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Top-level directories `.gitignore` excludes, as plain names.
    fn ignored_dirs() -> Vec<String> {
        let body = std::fs::read_to_string(repo_root().join(".gitignore")).unwrap_or_default();
        body.lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
            .filter(|l| l.ends_with('/'))
            .map(|l| l.trim_matches('/').to_string())
            .filter(|l| !l.is_empty() && !l.contains('*'))
            .collect()
    }

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    #[test]
    fn no_include_str_points_into_a_gitignored_directory() {
        let ignored = ignored_dirs();
        assert!(
            ignored.iter().any(|d| d == ".claude"),
            "anti-vacuity: .gitignore must still exclude the directory that \
             caused this guard, else the test proves nothing. Got {ignored:?}"
        );

        let mut files = Vec::new();
        rs_files(&repo_root().join("crates"), &mut files);
        assert!(
            files.len() > 50,
            "anti-vacuity: found only {} sources",
            files.len()
        );

        let mut bad = Vec::new();
        for f in &files {
            let Ok(body) = std::fs::read_to_string(f) else {
                continue;
            };
            let mut rest = body.as_str();
            while let Some(at) = rest.find("include_str!(\"") {
                rest = &rest[at + "include_str!(\"".len()..];
                let Some(end) = rest.find('"') else { break };
                let raw = &rest[..end];
                rest = &rest[end..];
                // Resolve against the including file's directory, then look for
                // an ignored directory anywhere in the resulting path.
                let joined = f.parent().unwrap_or(Path::new(".")).join(raw);
                let as_text = joined.to_string_lossy().replace('\\', "/");
                for dir in &ignored {
                    if as_text.contains(&format!("/{dir}/")) {
                        bad.push(format!("{} -> {raw}", f.display()));
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "these `include_str!` calls depend on gitignored files, so the \
             crate cannot be built from a clean checkout: {bad:#?}"
        );
    }
}

#[cfg(test)]
mod guidance_names_real_commands {
    use super::KNOWN_COMMANDS;

    /// We write rules into every user's project and push a nudge at every
    /// subagent. Naming a command the binary does not have turns our own
    /// guidance into a trap: the agent follows it and gets "unknown command"
    /// on the first noisy command it tries. That is exactly what happened to
    /// `exec-compact`, on the released build, from three separate sites.
    #[test]
    fn every_command_our_guidance_names_actually_exists() {
        let mut texts: Vec<String> = vec![
            crate::install::claude_md::GLOBAL_CLAUDE_MD_BLOCK.to_string(),
            tokenade_core::project_scaffold::scaffold_block(true),
            tokenade_core::project_scaffold::scaffold_block(false),
        ];
        texts.push(include_str!("hooks/subagent_pretool.rs").to_string());
        // The rules files checked into THIS repository are guidance too, and
        // they are hand-written, so the generated-text checks above never see
        // them. One of them told agents to run `tokenade auto <path>`, a
        // command that does not exist — the same defect as `exec-compact`,
        // hiding in the one place the gate was not looking.
        // Read at runtime, like `.claude/CLAUDE.md` below: tokenade itself
        // REMOVES a root `CLAUDE.md` / `AGENTS.md` that held only its own
        // lecture (every reader has it globally), and an `include_str!` then
        // made the test binary unbuildable in the very repo it ships from.
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(name);
            if let Ok(body) = std::fs::read_to_string(&p) {
                texts.push(body);
            }
        }
        // `.claude/CLAUDE.md` is scaffolded, not tracked — `.gitignore` carries
        // `/.claude/`. It was an `include_str!` here, which is resolved at
        // COMPILE time and therefore REQUIRED the file: the workspace test
        // binary could not be built from a clean checkout at all, so CI ran
        // `cargo test --workspace` against a tree that never had it. Read at
        // runtime and skipped when absent: the check still runs on a machine
        // where the file exists, and stops being a build-time dependency on an
        // untracked file everywhere else.
        let scaffolded =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/CLAUDE.md");
        if let Ok(body) = std::fs::read_to_string(&scaffolded) {
            texts.push(body);
        }

        for text in &texts {
            let mut rest = text.as_str();
            while let Some(at) = rest.find("`tokenade ") {
                rest = &rest[at + "`tokenade ".len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if name.is_empty() {
                    continue;
                }
                assert!(
                    KNOWN_COMMANDS.contains(&name.as_str()),
                    "our guidance tells agents to run `tokenade {name}`, which is not a command"
                );
            }
        }
    }
}

#[cfg(test)]
mod shorten_path_for_column_tests {
    use super::shorten_path_for_column;

    #[test]
    fn an_accented_project_path_does_not_kill_the_listing() {
        // The exact shape that reached 1.1.9: `semantic-status` printed its
        // header and then panicked with "start byte index 13 is not a char
        // boundary; it is inside 'é'". The listing was lost.
        let p = format!("/home/user/Développement/{}é{}", "a".repeat(26), "b".repeat(4));
        let out = shorten_path_for_column(&p, 46);
        assert!(out.starts_with('…'), "{out}");
        assert_eq!(out.chars().count(), 46, "{out}");
        assert!(p.ends_with(out.trim_start_matches('…')), "not a tail of the path: {out}");

        // Every offset, so no single lucky alignment carries the test.
        for pad in 0..60 {
            let p = format!("/home/user/Développement/{}é{}", "a".repeat(pad), "b".repeat(4));
            let out = shorten_path_for_column(&p, 46);
            assert!(out.chars().count() <= 46, "pad={pad}: {out}");
        }
    }

    #[test]
    fn ascii_behaviour_is_exactly_what_it_was() {
        let p = "/home/user/".to_string() + &"a".repeat(60);
        let out = shorten_path_for_column(&p, 46);
        // What the byte slice produced before, on input where it was correct.
        assert_eq!(out, format!("…{}", &p[p.len() - 45..]));
        assert_eq!(out.chars().count(), 46);
    }

    #[test]
    fn a_path_that_fits_is_untouched() {
        assert_eq!(shorten_path_for_column("/short/path", 46), "/short/path");
        // Fits in COLUMNS while overflowing in BYTES: 40 accented characters
        // are 80 bytes. Measuring bytes here would shorten a row that fits.
        let accented = "é".repeat(40);
        assert_eq!(shorten_path_for_column(&accented, 46), accented);
    }

    #[test]
    fn degenerate_widths_do_not_panic() {
        assert_eq!(shorten_path_for_column("/home/user/project", 0), "");
        assert_eq!(shorten_path_for_column("/home/user/project", 1), "…");
        assert_eq!(shorten_path_for_column("abc", 3), "abc");
        assert_eq!(shorten_path_for_column("abcd", 3), "…cd");
    }
}

#[cfg(test)]
mod every_injection_is_booked {
    //! `log_emitted_additional_context` exists because our own
    //! `additionalContext` was "billed to the user and debited nowhere".
    //! Nothing held that promise open, and it had leaked again: the loop-guard
    //! nudge was booked on the plain bash-rewrite route and NOT on the
    //! `cd`-peel route beside it — and measured 2026-09-06, 59.1% of bash
    //! commands (39 484 of 66 827) are `cd`-prefixed, so the unbooked route
    //! was the majority one. The quota-gate message, the PDF hint, the
    //! WebFetch-recovery hint and the Gemini style preamble booked nothing at
    //! all; `record_nudge` looks like a debit and is accounting-neutral
    //! (before == after, saved 0).
    //!
    //! The rule is about DELIVERY, not about building a value: a function that
    //! merely assembles the JSON (`subagent_start_reply`) is fine, because its
    //! caller books it. A function that PRINTS one must debit it.

    fn rust_sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    // `src/tests/` holds assertions ABOUT the payloads, not
                    // emissions of them.
                    if p.file_name().is_some_and(|f| f == "tests") {
                        continue;
                    }
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    if let Ok(b) = std::fs::read_to_string(&p) {
                        out.push((p.display().to_string(), b));
                    }
                }
            }
        }
        out
    }

    /// Function bodies, by brace matching from each `fn NAME(`.
    fn fn_bodies(src: &str) -> Vec<(String, String)> {
        let bytes = src.as_bytes();
        let mut out = Vec::new();
        for (at, _) in src.match_indices("fn ") {
            let after = &src[at + 3..];
            let Some(paren) = after.find('(') else { continue };
            let name = after[..paren].trim();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            let Some(open) = src[at + 3 + paren..].find('{') else {
                continue;
            };
            let mut i = at + 3 + paren + open;
            let (mut depth, start) = (0i32, i);
            while i < bytes.len() {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if depth == 0 && i > start {
                out.push((name.to_string(), src[start..=i.min(bytes.len() - 1)].to_string()));
            }
        }
        out
    }

    fn mentions_outside_comments(body: &str, needle: &str) -> bool {
        body.lines().any(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && l.contains(needle)
        })
    }

    #[test]
    fn a_function_that_prints_additional_context_books_its_cost() {
        let sources = rust_sources();
        assert!(
            sources.len() >= 20,
            "walked only {} source files — the walk stopped finding the crate",
            sources.len()
        );
        // Builders: they name the key but never print it, so their CALLER is
        // the delivery site. Without this hop `cmd_hookwrap_subagent_start`
        // was exempt — it prints `subagent_start_reply(..)`, and the key lives
        // one call away. That mutation survived until the hop existed.
        let mut builders: std::collections::HashSet<String> = Default::default();
        for (_, src) in &sources {
            for (name, body) in fn_bodies(src) {
                if mentions_outside_comments(&body, "\"additionalContext\"")
                    && !body.contains("println!")
                {
                    builders.insert(name);
                }
            }
        }
        assert!(
            !builders.is_empty(),
            "no additionalContext builder found — the sweep lost the shape"
        );

        let mut emitting = 0usize;
        let mut unbooked: Vec<String> = Vec::new();
        for (path, src) in &sources {
            for (name, body) in fn_bodies(src) {
                let names_key = mentions_outside_comments(&body, "\"additionalContext\"");
                let calls_builder = builders
                    .iter()
                    .any(|b| *b != name && mentions_outside_comments(&body, &format!("{b}(")));
                if !names_key && !calls_builder {
                    continue;
                }
                // Assembling a value is not delivering it; the caller books.
                if !body.contains("println!") {
                    continue;
                }
                emitting += 1;
                // The DEBIT must be code, not prose. A first pass asked
                // `body.contains(...)`, and the comment that explains the
                // debit names the function — so deleting the call left the
                // guard green. Two of six mutations survived on that.
                if !mentions_outside_comments(&body, "log_emitted_additional_context(") {
                    unbooked.push(format!("{name}() in {path}"));
                }
            }
        }
        assert!(
            emitting >= 8,
            "the sweep found only {emitting} functions printing additionalContext \
             — it stopped matching the source it reads"
        );
        unbooked.sort();
        unbooked.dedup();
        assert!(
            unbooked.is_empty(),
            "these functions PRINT additionalContext without booking its cost, so \
             the tokens are billed to the user and debited nowhere — the exact \
             leak `log_emitted_additional_context` exists to close \
             (`record_nudge` is NOT a debit: before == after, saved 0):\n  {}",
            unbooked.join("\n  ")
        );
    }
}

#[cfg(test)]
mod tests;

