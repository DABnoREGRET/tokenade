//! Read-only command REPLAY — skip re-executing a command whose result is
//! provably unchanged, and skip re-delivering its output.
//!
//! The exec-dedup lever answers a repeated command AFTER re-running it, on
//! byte-identical output. This lever goes one step earlier: at PreToolUse,
//! when the SAME read-only command re-runs in the SAME cwd and nothing that
//! could change its result has moved, rewrite it to `tokenade replay <hash>`
//! — a ~40-token answer instead of a full re-execution + full re-delivery.
//!
//! "Provably unchanged" is three independent guards, ALL required:
//!   1. session write-generation unchanged (the agent edited nothing since),
//!   2. a state fingerprint unchanged — factual, not heuristic: for git
//!      commands the mtimes/sizes of `.git/HEAD` + `.git/index`, for
//!      directory listings the target directory's mtime (catches OUTSIDE
//!      writers: the user, a build, another agent),
//!   3. freshness + context-liveness: the prior run is recent (≤300 s), in
//!      THIS session, and NO compaction happened in between — so its full
//!      output is still literally in the model's context. A compaction (or
//!      staleness) voids the replay: telling the agent "unchanged, see
//!      above" about output that scrolled out would leave it blind.
//!
//! Disable with `TOKENADE_NO_REPLAY=1`.

use std::path::Path;

/// Commands eligible for replay: read-only, deterministic given (cwd,
/// filesystem state). Deliberately tight — expanding this list must come
/// with a matching `state_fingerprint` story.
pub(crate) fn replay_eligible(cmd: &str) -> bool {
    let norm = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    const PREFIXES: &[&str] = &[
        "git status",
        "git diff",
        "git log",
        "git branch",
        "git remote",
        "git show",
        "ls",
        "pwd",
        "wc ",
        "du ",
        "tree",
        "find ",
    ];
    // Shell metacharacters disqualify: pipes/redirects/substitutions can
    // smuggle writes or nondeterminism past the read-only assumption.
    if norm.contains('|')
        || norm.contains('>')
        || norm.contains('<')
        || norm.contains('$')
        || norm.contains('`')
        || norm.contains(';')
        || norm.contains("&&")
    {
        return false;
    }
    // `find` grows teeth with -delete/-exec/-ok — those write.
    if norm.starts_with("find ")
        && (norm.contains("-delete") || norm.contains("-exec") || norm.contains("-ok"))
    {
        return false;
    }
    PREFIXES.iter().any(|p| {
        let pt = p.trim_end();
        // Word-boundary prefix: "git status --short" yes, "git statusx" no.
        norm == pt || norm.starts_with(&format!("{pt} "))
    })
}

/// Where this checkout's git state actually lives.
///
/// In an ordinary checkout that is `<cwd>/.git`. In a LINKED WORKTREE it is a
/// FILE holding `gitdir: <path>`, and the worktree's own `HEAD` and `index`
/// live at that path — `<cwd>/.git/HEAD` does not exist at all. Fingerprinting
/// the non-existent paths hashed them both as "absent", the same value every
/// time, so the whole git half of the fingerprint was constant and only the
/// directory mtime remained: a branch switch or a `git add` left a cached
/// `git status` looking fresh. MEASURED 2026-09-07 in a worktree — `.git`
/// reads `gitdir: …/.git/worktrees/linked`, and that directory holds `HEAD`
/// (`ref: refs/heads/feat`, a different branch from the main checkout) and
/// `index`.
///
/// Best effort by design: anything unreadable or unparseable falls back to
/// `<cwd>/.git`, which is what the caller used to use unconditionally, and the
/// fingerprint is allowed to be conservative — it fails toward re-running the
/// command, never toward serving a stale answer.
fn git_dir_of(cwd: &Path) -> std::path::PathBuf {
    let dot_git = cwd.join(".git");
    let Ok(md) = std::fs::metadata(&dot_git) else {
        return dot_git;
    };
    // States the ordinary case rather than discovering it through an error.
    // Deliberately NOT falsifiable: `read_to_string` on a directory already
    // returns Err (verified — `is a directory`), so deleting this line changes
    // no behaviour on any platform. It is intent, not a gate.
    if md.is_dir() {
        return dot_git;
    }
    let Ok(text) = std::fs::read_to_string(&dot_git) else {
        return dot_git;
    };
    let Some(rest) = text.lines().find_map(|l| l.trim().strip_prefix("gitdir:")) else {
        return dot_git;
    };
    let pointed = Path::new(rest.trim());
    if pointed.as_os_str().is_empty() {
        return dot_git;
    }
    if pointed.is_absolute() {
        pointed.to_path_buf()
    } else {
        cwd.join(pointed)
    }
}

/// Factual state fingerprint for an eligible command. `None` = can't build
/// one → NO replay (fail-open to normal execution).
pub(crate) fn state_fingerprint(cwd: &Path, cmd: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let stat = |p: &Path, h: &mut std::collections::hash_map::DefaultHasher| -> bool {
        match std::fs::metadata(p) {
            Ok(md) => {
                md.len().hash(h);
                if let Ok(m) = md.modified() {
                    m.hash(h);
                }
                true
            }
            Err(_) => {
                // Absent file still fingerprints (as "absent") — a later
                // appearance changes the hash.
                0u8.hash(h);
                true
            }
        }
    };
    if cmd.trim_start().starts_with("git ") {
        // Working-tree-affecting git state: HEAD (branch moves), index
        // (stages), and the top-level dir mtime (untracked file appears).
        // git status/diff also reflect FILE CONTENT changes — those bump
        // the session write-generation when the agent makes them, and the
        // cwd/index mtimes when anything else does. A same-second content
        // rewrite that touches none of these can theoretically slip
        // through; the 300 s freshness window bounds that exposure.
        let git_dir = git_dir_of(cwd);
        stat(&git_dir.join("HEAD"), &mut h);
        stat(&git_dir.join("index"), &mut h);
        stat(cwd, &mut h);
        return Some(h.finish());
    }
    // Listing-style commands: fingerprint the target directory (first
    // non-flag argument, else cwd).
    let target = cmd
        .split_whitespace()
        .skip(1)
        .find(|t| !t.starts_with('-'))
        .map(|t| {
            let p = Path::new(t);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .unwrap_or_else(|| cwd.to_path_buf());
    stat(&target, &mut h);
    stat(cwd, &mut h);
    Some(h.finish())
}

fn replay_state_path(skey: &str, key_hash: &str) -> std::path::PathBuf {
    super::read_staging::read_pending_dir().join(format!("replay-{skey}-{key_hash}"))
}

/// The SESSION's compaction mark, whatever key the caller holds.
///
/// PreCompact writes it under the session (`{skey}`); replay reads it under
/// its CONTEXT key (`{skey}-{transcript hash}`), and read `{skey}-{hash}-compact-ts`
/// — a file nothing ever writes. Guard 3b ("no compaction since") was dead on
/// every agent that sends a `transcript_path`: after a summary, a repeated
/// command was still answered "unchanged — see above" for an output the model
/// no longer had (2026-10-03, `sed -n 30,60p build.rs`, re-run after the
/// session's compaction). Guard 1 had been fixed for the same split; this is
/// the other reader of it.
pub(crate) fn compact_ts_path(skey: &str) -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
        .join(format!("{}-compact-ts", session_of(skey)))
}

/// PreCompact marks the session: prior outputs are no longer in context.
pub(crate) fn mark_compaction(skey: &str) {
    let p = compact_ts_path(skey);
    tokenade_core::platform::ensure_parent_dir(&p);
    let _ = tokenade_core::paths::write_atomic(
        &p,
        &tokenade_core::platform::now_unix_secs().to_string(),
    );
}

fn norm_key(cwd: &Path, cmd: &str) -> String {
    let norm = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    tokenade_core::dedup_ref::hash_content(format!("replay:{}|{norm}", cwd.display()).as_bytes())
}

/// The SESSION a replay key belongs to. Replay state is context-scoped
/// (`ctx_replay_key`: `{skey}-{hash of the transcript}`), but the write
/// generation is bumped per SESSION (`writegen-{skey}`, by Edit/Write and by a
/// Bash command that writes). Reading it under the context key read a counter
/// nothing ever bumps: guard 1 always passed, and a `git diff` stored before an
/// Edit of `src/a.rs` was replayed after it. `skey` is alphanumeric, so the
/// first `-` is the boundary.
fn session_of(key: &str) -> &str {
    key.split('-').next().unwrap_or(key)
}

/// Guard 1 ("the agent edited nothing since") is only as good as whatever
/// bumps the write generation, and only two agents have that: Claude Code
/// (the `pre-edit-snapshot` hook on Edit/Write) and Cursor (`postToolUse` on
/// `Write`/`Delete`). Codex, Droid, Copilot, Grok, Qwen… reach this same Bash
/// hook, but their own edits (`apply_patch`, `Edit`, `Create`) are seen by no
/// tokenade hook: the generation never moves, and a `git diff` replayed after
/// an edit in a subdirectory (HEAD, index and cwd untouched) answered the
/// state from before. Replay is an optimisation; on those agents it is off.
fn agent_keeps_the_write_generation() -> bool {
    use tokenade_core::detect::Agent;
    matches!(
        tokenade_core::detect::from_env(),
        Agent::ClaudeCode | Agent::Cursor
    )
}

/// PostToolUse: remember this run so/// PostToolUse: remember this run so an identical near-future re-run can
/// replay. Registers the output in the dedup store (recoverable via
/// expand-ref) and the validity metadata beside it.
pub(crate) fn store_replay(skey: &str, cwd: &Path, cmd: &str, output: &str) {
    if std::env::var("TOKENADE_NO_REPLAY").is_ok() || output.len() > 256 * 1024 {
        return;
    }
    if !replay_eligible(cmd) {
        return;
    }
    let Some(fp) = state_fingerprint(cwd, cmd) else {
        return;
    };
    let gen = super::session_lifecycle::current_write_generation(session_of(skey));
    let key = norm_key(cwd, cmd);
    let content_hash = tokenade_core::dedup_ref::hash_content(output.as_bytes());
    tokenade_core::dedup_ref::register(cwd, &content_hash, output);
    let p = replay_state_path(skey, &key);
    tokenade_core::platform::ensure_parent_dir(&p);
    let _ = tokenade_core::paths::write_atomic(
        &p,
        &format!(
            "{gen}:{}:{fp}:{content_hash}",
            tokenade_core::platform::now_unix_secs()
        ),
    );
}

/// PreToolUse: if every guard passes, return the replacement command.
pub(crate) fn try_replay_rewrite(skey: &str, cwd: &Path, cmd: &str, exe: &str) -> Option<String> {
    if std::env::var("TOKENADE_NO_REPLAY").is_ok()
        || !replay_eligible(cmd)
        || !agent_keeps_the_write_generation()
    {
        return None;
    }
    // Class-1 endgame: replay state is CONTEXT-scoped (callers pass the
    // transcript-derived ctx as `skey`), so a sibling context has a
    // different state file and can never be told "see above" falsely.
    // Counter fallback only when no transcript discriminator exists.
    if !skey.contains('-') && super::read_pretool::sidechain_window_active(skey) {
        return None;
    }
    let key = norm_key(cwd, cmd);
    let meta = std::fs::read_to_string(replay_state_path(skey, &key)).ok()?;
    let mut it = meta.trim().split(':');
    let (gen0, ts0, fp0, hash) = (
        it.next()?.parse::<u64>().ok()?,
        it.next()?.parse::<u64>().ok()?,
        it.next()?.parse::<u64>().ok()?,
        it.next()?.to_string(),
    );
    // Guard 3a: freshness — the prior output must still plausibly sit in
    // the model's live context window.
    let age = tokenade_core::platform::now_unix_secs().saturating_sub(ts0);
    let max_age: u64 = tokenade_core::web_replay::max_age_secs();
    if age > max_age {
        return None;
    }
    // Guard 3b: no compaction since — a summary squashed the transcript,
    // the prior output is GONE from context; replaying "see above" would
    // point at nothing.
    if let Ok(cts) = std::fs::read_to_string(compact_ts_path(skey)) {
        if cts.trim().parse::<u64>().map(|c| c >= ts0).unwrap_or(false) {
            return None;
        }
    }
    // Guard 1: the agent edited nothing since.
    if super::session_lifecycle::current_write_generation(session_of(skey)) != gen0 {
        return None;
    }
    // Guard 2: nothing else moved the observable state either.
    if state_fingerprint(cwd, cmd)? != fp0 {
        return None;
    }
    // The stored output must still be recoverable for the tail + expand-ref.
    tokenade_core::dedup_ref::lookup(cwd, &hash)?;
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "replay_rewrite",
        serde_json::json!({"age_secs": age, "hash": hash}),
    );
    Some(format!("{exe} replay {hash} {age}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard no. 1 — "the agent edited nothing" — read under the CONTEXT key
    /// (`{skey}-{transcript hash}`) while `Edit` bumps the SESSION generation
    /// (`{skey}`): a `git diff` replayed after editing a tracked file in a
    /// subdirectory (neither HEAD, nor the index, nor the cwd move) answered
    /// "unchanged".
    fn a_hand_built_repo(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tk-replay-gen-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(d.join(".git").join("index"), b"idx").unwrap();
        std::fs::write(d.join("src").join("a.rs"), "fn a() {}\n").unwrap();
        d
    }

    #[test]
    fn an_edit_voids_the_replay_of_git_diff() {
        let _home = tokenade_core::platform::ScratchHome::new("replay-gen-edit");
        std::env::set_var("TOKENADE_AGENT", "claude-code");
        let repo = a_hand_built_repo("edit");
        let parsed =
            serde_json::json!({"session_id": "replaygen1", "transcript_path": "/x/t.jsonl"});
        let ctx = crate::hooks::ctx_replay_key(&parsed, "replaygen1");
        assert!(
            ctx.contains('-'),
            "the fixture must take the transcript-scoped key"
        );
        store_replay(
            &ctx,
            &repo,
            "git diff",
            "diff --git a/src/a.rs b/src/a.rs\n+fn a() {}\n",
        );
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_some(),
            "anti-vacuity"
        );
        crate::hooks::session_lifecycle::bump_write_generation(Some(&parsed.to_string()));
        std::fs::write(repo.join("src").join("a.rs"), "fn a() { todo!() }\n").unwrap();
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_none(),
            "an Edit happened: the stored `git diff` is stale"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_compaction_voids_a_context_scoped_replay() {
        let _home = tokenade_core::platform::ScratchHome::new("replay-compact");
        std::env::set_var("TOKENADE_AGENT", "claude-code");
        let repo = a_hand_built_repo("compact");
        let parsed =
            serde_json::json!({"session_id": "replaycmp1", "transcript_path": "/x/t.jsonl"});
        let ctx = crate::hooks::ctx_replay_key(&parsed, "replaycmp1");
        assert!(
            ctx.contains('-'),
            "the fixture must take the transcript-scoped key"
        );
        store_replay(
            &ctx,
            &repo,
            "git diff",
            "diff --git a/src/a.rs b/src/a.rs\n+fn a() {}\n",
        );
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_some(),
            "anti-vacuity"
        );
        // PreCompact marks the SESSION, the way the hook does.
        mark_compaction("replaycmp1");
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_none(),
            "the summary dropped the output: `see above` would point at nothing"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_bash_write_voids_the_replay_too() {
        let _home = tokenade_core::platform::ScratchHome::new("replay-gen-bash");
        std::env::set_var("TOKENADE_AGENT", "claude-code");
        let repo = a_hand_built_repo("bash");
        let parsed =
            serde_json::json!({"session_id": "replaygen2", "transcript_path": "/x/t.jsonl"});
        let ctx = crate::hooks::ctx_replay_key(&parsed, "replaygen2");
        store_replay(
            &ctx,
            &repo,
            "git diff",
            "diff --git a/src/a.rs b/src/a.rs\n",
        );
        let read_only = "python3 - <<'PY'\nprint(open('src/a.rs').read())\nPY";
        crate::hooks::session_lifecycle::bump_write_generation_if_bash_writes(&parsed, read_only);
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_some(),
            "a read-only script must not void the replay"
        );
        let writer = "python3 - <<'PY'\nopen('src/a.rs','w').write('fn b() {}')\nPY";
        crate::hooks::session_lifecycle::bump_write_generation_if_bash_writes(&parsed, writer);
        assert!(try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// Codex's edits reach no tokenade hook: nothing can prove "unchanged".
    #[test]
    fn no_replay_where_edits_are_unseen() {
        let _home = tokenade_core::platform::ScratchHome::new("replay-gen-codex");
        let repo = a_hand_built_repo("codex");
        let parsed =
            serde_json::json!({"session_id": "replaygen3", "transcript_path": "/x/t.jsonl"});
        let ctx = crate::hooks::ctx_replay_key(&parsed, "replaygen3");
        store_replay(
            &ctx,
            &repo,
            "git diff",
            "diff --git a/src/a.rs b/src/a.rs\n",
        );
        std::env::set_var("TOKENADE_AGENT", "claude-code");
        assert!(
            try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_some(),
            "anti-vacuity"
        );
        std::env::set_var("TOKENADE_AGENT", "codex");
        assert!(try_replay_rewrite(&ctx, &repo, "git diff", "tokenade").is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn which_bash_commands_write() {
        use crate::hooks::session_lifecycle::bash_writes_files as w;
        for c in [
            "sed -i 's/a/b/' src/x.rs",
            "sed -E -i 's/a/b/' x",
            "perl -pi -e 's/a/b/' x",
            "cat > notes.md <<'EOF'\nx\nEOF",
            "echo x >> log.txt",
            "git restore src/x.rs",
            "git stash pop",
            "cargo fmt --all",
            "rm -rf build",
            "python3 - <<'PY'\np.write_text('x')\nPY",
            "cmd | tee out.txt",
        ] {
            assert!(w(c), "{c}");
        }
        for c in [
            "git status --short",
            "cargo test 2>&1 | tail -3",
            "ls > /dev/null",
            "cargo fmt --all --check",
            "python3 - <<'PY'\nprint(1)\nPY",
            "grep -rn foo src",
        ] {
            assert!(!w(c), "{c}");
        }
    }

    #[test]
    fn eligibility_is_tight() {
        assert!(replay_eligible("git status --short"));
        assert!(replay_eligible("ls -la src"));
        assert!(replay_eligible("git  diff   --stat"));
        assert!(!replay_eligible("git status | head"));
        assert!(!replay_eligible("git push"));
        assert!(!replay_eligible("ls $(pwd)"));
        assert!(!replay_eligible("cargo build"));
        assert!(!replay_eligible("git statusx"));
        assert!(!replay_eligible("find . -delete")); // still read-only prefixed…
    }

    /// In a LINKED WORKTREE the git half of the fingerprint must still move.
    ///
    /// `.git` is a FILE there, so `<cwd>/.git/HEAD` and `<cwd>/.git/index` do
    /// not exist and both hashed as "absent" — the same value every time.
    /// Only the directory mtime was left, so staging a file or switching
    /// branch did not invalidate a cached `git status`, and the stale answer
    /// was served for the whole freshness window.
    ///
    /// The layout is built by hand rather than by running `git`: the
    /// fingerprint reads exactly these three paths, so a real repository adds
    /// nothing, and a test that shells out asserts NOTHING on a machine with
    /// no git — besides spawning a process this file has no other reason to.
    #[test]
    fn a_worktree_fingerprints_its_own_head_and_index() {
        let base = std::env::temp_dir().join(format!("tk-wt-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let main = base.join("main");
        let real = main.join(".git").join("worktrees").join("linked");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(main.join(".git").join("HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(main.join(".git").join("index"), b"main-index").unwrap();
        // The worktree's own state: a DIFFERENT branch from the main checkout.
        std::fs::write(real.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(real.join("index"), b"worktree-index").unwrap();

        let linked = base.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", real.display())).unwrap();

        // The shape this is all about.
        assert!(
            std::fs::metadata(linked.join(".git")).unwrap().is_file(),
            "this fixture only means something if .git is a FILE"
        );
        assert!(
            !linked.join(".git").join("HEAD").exists(),
            "and if <cwd>/.git/HEAD does not exist"
        );
        assert_eq!(
            super::git_dir_of(&linked),
            real,
            "the pointer is not followed"
        );

        // HEAD alone, with nothing in the working tree touched: rewriting the
        // same bytes moves its mtime and nothing else. Without HEAD in the
        // fingerprint this is invisible.
        let fp0 = state_fingerprint(&linked, "git status").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(real.join("HEAD"), "ref: refs/heads/other\n").unwrap();
        let fp1 = state_fingerprint(&linked, "git status").unwrap();
        assert_ne!(
            fp0, fp1,
            "HEAD is not part of the fingerprint in a worktree"
        );

        // The index alone, the same way — a `git add` moves only this.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(real.join("index"), b"worktree-index-staged").unwrap();
        let fp2 = state_fingerprint(&linked, "git status").unwrap();
        assert_ne!(
            fp1, fp2,
            "the index is not part of the fingerprint in a worktree"
        );

        // Touching the MAIN checkout's state must not move the worktree's
        // fingerprint: they are different branches and different answers.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(main.join(".git").join("index"), b"main-index-changed").unwrap();
        assert_eq!(
            fp2,
            state_fingerprint(&linked, "git status").unwrap(),
            "the worktree is fingerprinting the MAIN checkout's state"
        );

        // And an ordinary checkout still works the way it always did.
        let m0 = state_fingerprint(&main, "git status").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(main.join(".git").join("index"), b"main-index-again").unwrap();
        assert_ne!(
            m0,
            state_fingerprint(&main, "git status").unwrap(),
            "a plain checkout must still fingerprint its index"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A `gitdir:` pointer may be RELATIVE, and it is relative to the
    /// worktree, not to wherever the process happens to be running.
    #[test]
    fn a_relative_gitdir_pointer_resolves_against_the_worktree() {
        let d = std::env::temp_dir().join(format!("tk-relgd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("elsewhere")).unwrap();
        std::fs::write(d.join(".git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(super::git_dir_of(&d), d.join("elsewhere"));

        // An absolute one is taken as it stands.
        let abs = d.join("abs-target");
        std::fs::create_dir_all(&abs).unwrap();
        std::fs::write(d.join(".git"), format!("gitdir: {}\n", abs.display())).unwrap();
        assert_eq!(super::git_dir_of(&d), abs);

        // Anything unreadable or unparseable falls back to `<cwd>/.git`,
        // which is what the code did unconditionally before.
        std::fs::write(d.join(".git"), "not a pointer\n").unwrap();
        assert_eq!(super::git_dir_of(&d), d.join(".git"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fingerprint_moves_with_dir_mtime() {
        let d = std::env::temp_dir().join(format!("tk-replay-fp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let a = state_fingerprint(&d, "ls").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(d.join("new-file"), b"x").unwrap();
        let b = state_fingerprint(&d, "ls").unwrap();
        assert_ne!(a, b, "new file must change the listing fingerprint");
        let _ = std::fs::remove_dir_all(&d);
    }
}
