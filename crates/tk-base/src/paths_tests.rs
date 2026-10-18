// The tests of `paths`.
//
// Spliced in by `include!` from `paths.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    /// A dotfiles manager (chezmoi, stow, yadm) publishes `~/.cursor/mcp.json`
    /// or `~/.aider.conf.yml` as a LINK into its own tree. Renaming a temp file
    /// onto the link replaces the link with a regular file: the agent then reads
    /// bytes that live outside the user's dotfiles, their managed copy is
    /// orphaned without a word, and the next `chezmoi apply` silently reverts
    /// whatever we just wrote.
    ///
    /// `mcp_wrap::write_locked` and `atomic_io::write_json_atomic` both
    /// canonicalize for exactly this reason. This is the THIRD writer to reach
    /// an agent config and the first one to run during an install, so it has to
    /// obey the same rule. Note the asymmetry with `write_atomic_repo_scoped`,
    /// which REFUSES a link instead of following it: scope decides. A link a
    /// cloned repo chose for us is an attack; a link the user's own dotfiles
    /// manager chose is the file.
    #[cfg(unix)]
    #[test]
    fn write_atomic_follows_a_dotfiles_symlink_instead_of_detaching_it() {
        let dir = tmp("symlink-follow");
        let real = dir.join("dotfiles/mcp.json");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "{\"mcpServers\":{}}").unwrap();
        let link = dir.join("home/mcp.json");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, "{\"mcpServers\":{\"tokenade\":{}}}").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the write detached the dotfiles symlink and left a regular file"
        );
        assert!(
            fs::read_to_string(&real).unwrap().contains("tokenade"),
            "the write never reached the dotfiles source at {}",
            real.display()
        );
    }

    /// The same write must not widen a config the user tightened. `rename`
    /// gives the destination the TEMP file's mode, which is whatever the umask
    /// allows — and these files can hold an API key.
    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_destination_mode_through_a_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("symlink-mode");
        let real = dir.join("dotfiles/creds.json");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "{}").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("home/creds.json");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, "{\"k\":1}").unwrap();

        // Assert the write LANDED before asserting its mode: an untouched file
        // keeps 0600 for free, so without this the check passes vacuously on
        // exactly the bug it exists to catch.
        assert!(
            fs::read_to_string(&real).unwrap().contains("\"k\":1"),
            "the write never reached the dotfiles source"
        );
        let mode = fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the save widened the file to {mode:o}");
    }

    /// A cloned repo can ship `.claude/CLAUDE.md` as a link to something it
    /// wants written — `~/.ssh/authorized_keys` being the sharp example. The
    /// scaffold is a convenience; no convenience is worth following that link.
    /// Note the asymmetry with `mcp_wrap::write_locked`, which deliberately
    /// FOLLOWS links: that path writes user-scope configs a dotfiles manager
    /// legitimately owns. Scope decides, not the link.
    #[cfg(unix)]
    #[test]
    fn repo_scoped_write_refuses_a_symlinked_component() {
        let root = std::env::temp_dir().join(format!(
            "tkn-repolink-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let outside = root.join("outside.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, "precious").unwrap();
        let linked = root.join("CLAUDE.md");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();

        let err = write_atomic_repo_scoped(&root, &linked, "scaffold").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&outside).unwrap(), "precious");

        // A plain path under the same root still writes.
        let plain = root.join("AGENTS.md");
        write_atomic_repo_scoped(&root, &plain, "ok").unwrap();
        assert_eq!(fs::read_to_string(&plain).unwrap(), "ok");
        let _ = fs::remove_dir_all(&root);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-paths-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn no_op_when_nothing_to_migrate() {
        let d = tmp("noop");
        assert_eq!(migrate_legacy_dir(&d), MigrationOutcome::NotPresent);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn renames_legacy_dir_when_only_old_exists() {
        let d = tmp("rename");
        fs::create_dir_all(d.join(".sts").join("subdir")).unwrap();
        fs::write(d.join(".sts").join("gain.jsonl"), "{\"ts\":1}\n").unwrap();
        let r = migrate_legacy_dir(&d);
        assert_eq!(r, MigrationOutcome::Renamed);
        assert!(!d.join(".sts").exists());
        assert!(d.join(".tokenade").is_dir());
        // Contents preserved.
        let content = fs::read_to_string(d.join(".tokenade").join("gain.jsonl")).unwrap();
        assert!(content.contains("\"ts\":1"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn does_not_overwrite_when_both_exist() {
        let d = tmp("both");
        fs::create_dir_all(d.join(".sts")).unwrap();
        fs::create_dir_all(d.join(".tokenade")).unwrap();
        fs::write(d.join(".sts").join("legacy-file"), "old").unwrap();
        fs::write(d.join(".tokenade").join("new-file"), "new").unwrap();
        let r = migrate_legacy_dir(&d);
        assert_eq!(r, MigrationOutcome::AlreadyMigrated);
        // Both still there.
        assert!(d.join(".sts").join("legacy-file").exists());
        assert!(d.join(".tokenade").join("new-file").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn is_idempotent_on_repeat_calls() {
        let d = tmp("idem");
        fs::create_dir_all(d.join(".sts")).unwrap();
        let first = migrate_legacy_dir(&d);
        let second = migrate_legacy_dir(&d);
        assert_eq!(first, MigrationOutcome::Renamed);
        assert_eq!(second, MigrationOutcome::NotPresent);
        let _ = fs::remove_dir_all(&d);
    }

    /// The point of the helper, stated as a race: readers must never observe a
    /// state file between truncate and write.
    ///
    /// MEASURED with the pattern this replaces (`fs::write`), same shape as
    /// this test: 45 714 of 65 802 reads came back EMPTY — 69 %. Every caller
    /// parses these files and falls back to a zero state, so an empty read is
    /// indistinguishable from "never used" and silently resets the counter.
    #[test]
    fn a_reader_never_catches_an_atomic_write_half_done() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("tokenade-atomic-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("batch-streak-bash-sess");
        write_atomic(&p, "5:1700000000:3:1").unwrap();

        static STOP: AtomicBool = AtomicBool::new(false);
        static TORN: AtomicUsize = AtomicUsize::new(0);
        static READS: AtomicUsize = AtomicUsize::new(0);
        STOP.store(false, Ordering::SeqCst);
        TORN.store(0, Ordering::SeqCst);
        READS.store(0, Ordering::SeqCst);

        let readers: Vec<_> = (0..3)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || {
                    while !STOP.load(Ordering::Relaxed) {
                        if let Ok(s) = fs::read_to_string(&p) {
                            READS.fetch_add(1, Ordering::Relaxed);
                            if s.split(':').count() < 4 {
                                TORN.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                })
            })
            .collect();
        let writers: Vec<_> = (0..2)
            .map(|w| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for i in 0..3000 {
                        let _ = write_atomic(&p, &format!("{i}:1700000000:{i}:{w}"));
                    }
                })
            })
            .collect();
        for t in writers {
            t.join().unwrap();
        }
        STOP.store(true, Ordering::SeqCst);
        for t in readers {
            t.join().unwrap();
        }
        let (reads, torn) = (READS.load(Ordering::SeqCst), TORN.load(Ordering::SeqCst));
        assert!(reads > 1000, "the race never ran: only {reads} reads");
        assert_eq!(torn, 0, "{torn} of {reads} reads saw a half-written file");
        // No scratch files survive a clean run.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Several callers rewrite files the USER owns — an agent's settings, its
    /// MCP config, a trust store, their own CLAUDE.md. `rename` hands the
    /// destination the temporary file's permissions, so saving a 0600 settings
    /// file would publish it at whatever the umask allows. A save is not
    /// allowed to widen what it saves.
    #[cfg(unix)]
    #[test]
    fn rewriting_a_users_file_never_widens_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tokenade-perm-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("settings.json");
        fs::write(&p, "{\"token\":\"secret\"}").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&p, "{\"token\":\"secret2\"}").unwrap();

        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the rewrite widened the file to {mode:o}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"token\":\"secret2\"}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A brand-new file has no mode to carry over; the write must still land.
    #[test]
    fn a_first_write_creates_the_file() {
        let dir = std::env::temp_dir().join(format!("tokenade-first-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("brand-new.md");
        write_atomic(&p, "hello").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod real_home_guard {
    use super::*;

    #[test]
    fn a_temp_home_is_allowed_and_the_account_home_is_not() {
        // A test that DOES override HOME passes — that is the whole contract,
        // and the guard would be useless if it blocked the correct pattern.
        let tmp = std::env::temp_dir().join("tokenade-guard-probe");
        assert!(
            refuse_real_home_from_a_test_binary(&tmp, "uninstall").is_ok(),
            "a temporary home must be allowed"
        );

        // And the account's own home is refused, from this very binary — which
        // IS a test binary, so `running_from_a_test_binary` must be true here.
        assert!(
            running_from_a_test_binary(),
            "this test runs from target/**/deps and the detector says otherwise"
        );
        if let Some(real) = account_home() {
            let err = refuse_real_home_from_a_test_binary(&real, "uninstall")
                .expect_err("the account's real home must be refused from a test");
            assert!(err.contains("refusing to uninstall"), "{err}");
            assert!(
                err.contains("HOME"),
                "the message must say how to fix it: {err}"
            );
        }
    }

    #[test]
    fn the_restore_walk_declines_rather_than_rewriting_the_account_home() {
        // The sharp edge: this one rewrites config files from copies. It must
        // answer 0 restored rather than doing the work.
        if let Some(real) = account_home() {
            assert_eq!(
                restore_pristine_files(&real),
                0,
                "the restore walk ran against the account's real home"
            );
        }
    }

    /// A scrub must not leave the bytes it removed in a backup beside them.
    ///
    /// `write_atomic` takes a pristine copy of a USER's file before this
    /// product first rewrites it — that is what makes "reversible" a claim
    /// about bytes. It skipped that for our own state, and our own state was
    /// spelled one way: a `.tokenade` component. The disclosure stash lives
    /// under the OS cache directory as `…/tokenade/stash`, so every stash file
    /// read as a user file. Nothing normally rewrites one — except
    /// `tokenade scrub-secret`, which is the single command where a copy of
    /// the old bytes is the defect. Measured 2026-09-03: it reported
    /// "scrubbed 3002 occurrence(s)" with 3000 of them still in
    /// `<blob>.tokenade-original`.
    ///
    /// Both directions, because the pristine copy is a feature everywhere
    /// else: a user's config must still get one.
    #[test]
    fn our_own_state_takes_no_pristine_copy_wherever_it_lives() {
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let home = std::env::temp_dir().join(format!(
            "tokenade-pristine-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let prev_home = std::env::var_os("HOME");
        let prev_cache = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));

        let ours = [
            home.join(".tokenade").join("gain.jsonl"),
            home.join(".cache")
                .join("tokenade")
                .join("stash")
                .join("a.log"),
        ];
        for p in &ours {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "secret-value\n").unwrap();
            write_atomic(p, "<redacted>\n").expect("write");
            let pristine = pristine_path(p);
            assert!(
                !pristine.exists(),
                "a copy of the old bytes was left at {}",
                pristine.display()
            );
        }

        // …and somebody else's file under the same cache root is still theirs.
        // "under the cache dir" is not the rule; "under OUR directory in it" is.
        let neighbour = home.join(".cache").join("someone-else").join("config.json");
        std::fs::create_dir_all(neighbour.parent().unwrap()).unwrap();
        std::fs::write(&neighbour, "{\"mine\":true}\n").unwrap();
        write_atomic(&neighbour, "{\"ours\":true}\n").expect("write");
        assert!(
            pristine_path(&neighbour).exists(),
            "another tool's cache file must keep its pristine copy"
        );

        // The control: a file that IS the user's still gets its copy, or this
        // test would pass on a build that stopped protecting anything.
        let theirs = home.join(".claude").join("settings.json");
        std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
        std::fs::write(&theirs, "{\"mine\":true}\n").unwrap();
        write_atomic(&theirs, "{\"ours\":true}\n").expect("write");
        assert!(
            pristine_path(&theirs).exists(),
            "the user's own bytes must still be kept"
        );

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_cache {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
