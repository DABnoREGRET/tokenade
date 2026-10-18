// The tests of `model_fetch`.
//
// Spliced by `include!` from `model_fetch.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal tar (no gzip) with the given (name, bytes) entries
    /// so we can exercise `read_tar_entries` without a fixture file.
    fn make_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        const BLOCK: usize = 512;
        let mut out = Vec::new();
        for (name, data) in files {
            let mut hdr = [0u8; BLOCK];
            let nb = name.as_bytes();
            hdr[..nb.len()].copy_from_slice(nb);
            let size_oct = format!("{:011o}\0", data.len());
            hdr[124..124 + size_oct.len()].copy_from_slice(size_oct.as_bytes());
            hdr[156] = b'0'; // regular file
                             // checksum: spaces during compute, then octal. read_tar_entries
                             // ignores the checksum, so spaces are fine.
            for b in &mut hdr[148..156] {
                *b = b' ';
            }
            out.extend_from_slice(&hdr);
            out.extend_from_slice(data);
            let pad = (BLOCK - (data.len() % BLOCK)) % BLOCK;
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        out.extend_from_slice(&[0u8; BLOCK * 2]); // end-of-archive
        out
    }

    #[test]
    fn reads_multiple_entries_by_basename() {
        let tar = make_tar(&[
            ("tokenizer.json", b"{\"tok\":1}"),
            ("./model.safetensors", b"WEIGHTS"),
            ("config.json", b"{}"),
        ]);
        let entries = read_tar_entries(&tar).unwrap();
        assert_eq!(entries.len(), 3);
        let get = |n: &str| {
            entries
                .iter()
                .find(|(name, _)| name == n)
                .map(|(_, b)| b.clone())
        };
        assert_eq!(get("tokenizer.json").unwrap(), b"{\"tok\":1}");
        assert_eq!(get("model.safetensors").unwrap(), b"WEIGHTS");
        assert_eq!(get("config.json").unwrap(), b"{}");
    }

    #[test]
    fn truncated_tar_errors() {
        let mut tar = make_tar(&[("model.safetensors", b"abcdef")]);
        tar.truncate(512 + 2); // header + partial data
        assert!(read_tar_entries(&tar).is_err());
    }

    #[test]
    fn override_dir_missing_files_errors() {
        // A dir that exists but lacks the model files must be rejected, not
        // silently accepted.
        let tmp = std::env::temp_dir().join(format!("tk-model-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        assert!(!dir_is_complete(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Every model file must carry a well-formed baked digest, or the HF
    /// fallback would either refuse a legitimate file or (worse) install one
    /// unchecked. Guards the const table against a rename/typo.
    #[test]
    fn every_model_file_has_a_baked_digest() {
        for name in MODEL_FILES {
            let entry = MODEL_FILE_SHA256.iter().find(|(n, _)| *n == name);
            let (_, digest) = entry.unwrap_or_else(|| panic!("no baked sha256 for {name}"));
            assert_eq!(digest.len(), 64, "sha256 hex must be 64 chars for {name}");
            assert!(
                digest
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "sha256 for {name} must be lowercase hex"
            );
        }
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("") — the canonical empty-input digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn promote_files_installs_every_model_file_plus_ok() {
        let dest = std::env::temp_dir().join(format!("tk-promote-ok-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let files: Vec<(String, Vec<u8>)> = MODEL_FILES
            .iter()
            .map(|f| {
                let body = if *f == "model.safetensors" {
                    b"WEIGHTS".to_vec()
                } else {
                    f.as_bytes().to_vec()
                };
                (f.to_string(), body)
            })
            .collect();
        promote_files(&dest, &files).unwrap();
        assert!(
            dir_is_complete(&dest),
            "every MODEL_FILES entry must be present"
        );
        assert!(dest.join(".ok").is_file(), ".ok marker must be written");
        assert_eq!(
            fs::read(dest.join("model.safetensors")).unwrap(),
            b"WEIGHTS"
        );
        let _ = fs::remove_dir_all(&dest);
    }

    /// Two promotes running at once inside one process must not stage through
    /// the same directory. Keyed on the pid alone they did, and each promote
    /// begins by wiping its staging dir — so one silently deleted the other's
    /// files and reported success with an incomplete `dest`.
    #[test]
    fn concurrent_promotes_do_not_share_a_staging_dir() {
        let parent = std::env::temp_dir().join(format!("tk-promote-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(&parent).unwrap();
        let files: Vec<(String, Vec<u8>)> = MODEL_FILES
            .iter()
            .map(|n| (n.to_string(), n.as_bytes().to_vec()))
            .collect();
        // Same parent, different dest — the shape the two promote tests have.
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let dest = parent.join(format!("d{i}"));
                let files = files.clone();
                std::thread::spawn(move || {
                    promote_files(&dest, &files).map(|_| dir_is_complete(&dest))
                })
            })
            .collect();
        for h in handles {
            assert_eq!(
                h.join().unwrap(),
                Ok(true),
                "a concurrent promote lost its staged files"
            );
        }
        let _ = fs::remove_dir_all(&parent);
    }

    /// A files set missing one of MODEL_FILES must fail closed: the atomic
    /// promote errors before the rename, so `dest` is never created.
    #[test]
    fn promote_files_missing_entry_fails_closed() {
        let dest = std::env::temp_dir().join(format!("tk-promote-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let files = vec![
            ("tokenizer.json".to_string(), b"TOK".to_vec()),
            ("config.json".to_string(), b"{}".to_vec()),
            // model.safetensors intentionally absent.
        ];
        assert!(promote_files(&dest, &files).is_err());
        assert!(
            !dest.exists(),
            "partial set must not be promoted into place"
        );
        let _ = fs::remove_dir_all(&dest);
    }

    /// The security core of the HF fallback: bytes whose sha256 does not match
    /// the baked digest are rejected and NOTHING is written into `dest`. A
    /// throwaway localhost server returns wrong bytes so the real
    /// `download_from_hf` path (http_get → sha256 → compare) is exercised
    /// end-to-end without reaching Hugging Face.
    #[test]
    fn hf_fallback_sha_mismatch_fails_closed() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let _g = flip_env_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            // One request is enough: the first file mismatches and aborts.
            if let Ok((mut stream, _)) = listener.accept() {
                let mut scratch = [0u8; 2048];
                let _ = stream.read(&mut scratch);
                let body = b"not-the-real-model-bytes";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });

        let dest = std::env::temp_dir().join(format!("tk-hf-mismatch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);

        // Bypass any ambient proxy so the loopback fetch is direct.
        std::env::set_var("NO_PROXY", "*");
        std::env::set_var("TOKENADE_MODEL_HF_BASE", format!("http://127.0.0.1:{port}"));
        let res = download_from_hf(&dest);
        std::env::remove_var("TOKENADE_MODEL_HF_BASE");
        std::env::remove_var("NO_PROXY");
        let _ = server.join();

        let err = res.expect_err("sha mismatch must error");
        assert!(err.contains("sha256 mismatch"), "unexpected error: {err}");
        assert!(
            !dest.exists(),
            "fail closed: no files may be written on mismatch"
        );
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn env_is_set_truthiness() {
        std::env::remove_var("TK_TEST_FLAG_XYZ");
        assert!(!env_is_set("TK_TEST_FLAG_XYZ"));
        std::env::set_var("TK_TEST_FLAG_XYZ", "1");
        assert!(env_is_set("TK_TEST_FLAG_XYZ"));
        std::env::set_var("TK_TEST_FLAG_XYZ", "0");
        assert!(!env_is_set("TK_TEST_FLAG_XYZ"));
        std::env::set_var("TK_TEST_FLAG_XYZ", "false");
        assert!(!env_is_set("TK_TEST_FLAG_XYZ"));
        std::env::set_var("TK_TEST_FLAG_XYZ", "");
        assert!(!env_is_set("TK_TEST_FLAG_XYZ"));
        std::env::remove_var("TK_TEST_FLAG_XYZ");
    }

    // ── Part B: HF-primary / R2-fallback fetch order ───────────────────────

    /// Serialise the flip tests + the HF mismatch test: all mutate the
    /// process-global env (TOKENADE_MODEL_HF_BASE / _UPDATE_ENDPOINT / _NO_HF /
    /// NO_PROXY).
    /// The crate-wide environment lock, not a private one: these tests point
    /// `TOKENADE_UPDATE_ENDPOINT` / `TOKENADE_MODEL_HF_BASE` / `NO_PROXY` at
    /// loopback, and a test elsewhere changing the environment under
    /// `ENV_LOCK` meanwhile could send the download elsewhere — two of them
    /// hung past 60 s in a full run (2026-10-03).
    fn flip_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::platform::ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Save/restore a set of env vars around a test body (restores on drop).
    struct EnvVars(Vec<(String, Option<String>)>);
    impl EnvVars {
        fn set(pairs: &[(&str, Option<String>)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
                .collect();
            for (k, v) in pairs {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
            EnvVars(saved)
        }
    }
    impl Drop for EnvVars {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    /// A loopback server that answers every request with wrong bytes, so any
    /// HF file fetch fails its sha256 gate. Returns the bound port; the thread
    /// is detached (never joined) so a test that never contacts it can't hang.
    fn spawn_bad_bytes_server() -> u16 {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { break };
                let mut scratch = [0u8; 2048];
                let _ = stream.read(&mut scratch);
                let body = b"not-the-real-model-bytes";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });
        port
    }

    /// A loopback port with no listener → connection refused (fast failure for
    /// the R2 fallback without touching the real network).
    fn closed_loopback_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    /// HF is primary: the fetch order tries Hugging Face first, then R2. With
    /// both forced to fail we can prove the order from the combined error —
    /// the HF sha256 gate ran and HF is named before R2.
    #[test]
    fn download_into_tries_hf_before_r2() {
        let _g = flip_env_lock();
        let hf = spawn_bad_bytes_server();
        let r2 = closed_loopback_port();
        let dest = std::env::temp_dir().join(format!("tk-flip-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let _env = EnvVars::set(&[
            ("TOKENADE_MODEL_NO_HF", None),
            ("NO_PROXY", Some("*".to_string())),
            (
                "TOKENADE_MODEL_HF_BASE",
                Some(format!("http://127.0.0.1:{hf}")),
            ),
            (
                "TOKENADE_UPDATE_ENDPOINT",
                Some(format!("http://127.0.0.1:{r2}")),
            ),
        ]);

        let err = download_into(&dest).expect_err("both sources must fail");
        assert!(err.contains("Hugging Face"), "HF must be tried: {err}");
        assert!(
            err.contains("sha256 mismatch"),
            "HF sha256 gate must run: {err}"
        );
        assert!(err.contains("R2"), "R2 fallback must be tried: {err}");
        assert!(
            err.find("Hugging Face") < err.rfind("R2"),
            "HF must precede R2 in attempt order: {err}"
        );
        assert!(!dest.exists(), "fail closed: nothing promoted");
        let _ = fs::remove_dir_all(&dest);
    }

    /// `TOKENADE_MODEL_NO_HF=1` forces R2-only: HF must never be contacted, so
    /// the sha256 gate never runs and the error names the escape hatch.
    #[test]
    fn no_hf_env_forces_r2_only() {
        let _g = flip_env_lock();
        let hf = spawn_bad_bytes_server(); // must NOT be contacted
        let r2 = closed_loopback_port();
        let dest = std::env::temp_dir().join(format!("tk-flip-nohf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let _env = EnvVars::set(&[
            ("TOKENADE_MODEL_NO_HF", Some("1".to_string())),
            ("NO_PROXY", Some("*".to_string())),
            (
                "TOKENADE_MODEL_HF_BASE",
                Some(format!("http://127.0.0.1:{hf}")),
            ),
            (
                "TOKENADE_UPDATE_ENDPOINT",
                Some(format!("http://127.0.0.1:{r2}")),
            ),
        ]);

        let err = download_into(&dest).expect_err("R2-only must fail");
        assert!(
            err.contains("TOKENADE_MODEL_NO_HF"),
            "R2-only escape hatch must be named: {err}"
        );
        assert!(
            !err.contains("sha256 mismatch"),
            "HF must not be attempted when NO_HF is set: {err}"
        );
        let _ = fs::remove_dir_all(&dest);
    }
}

#[cfg(test)]
mod the_wait_is_announced_once {
    /// The notice belongs at the single decision point, not in each downloader.
    /// `download_into` picks between Hugging Face and the R2 fallback at
    /// runtime, so a copy in either one announces the wait on some machines and
    /// not others — and the copy nobody remembered to update is the one that
    /// goes quiet. Read off the source, because the defect this guards is a
    /// second copy appearing, which no behavioural test can see.
    const SRC: &str = include_str!("model_fetch.rs");

    fn body_of(name: &str) -> &'static str {
        let at = SRC
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{name} is gone — this guard needs rewriting"));
        let open = SRC[at..].find('{').expect("no body") + at;
        let mut depth = 0usize;
        for (i, c) in SRC[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &SRC[open..open + i];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated body for {name}")
    }

    const NOTICE: &str = "first retrieval on this machine";

    #[test]
    fn the_decision_point_announces_the_download() {
        assert!(
            body_of("ensure_model_dir").contains(NOTICE),
            "the ~132 MB fetch is synchronous: a caller that hears nothing for \
             a minute and a half reads it as a hang and retries"
        );
    }

    #[test]
    fn neither_downloader_announces_it_again() {
        for f in ["download_into", "download_from_hf", "download_from_r2"] {
            assert!(
                !body_of(f).contains(NOTICE),
                "{f} carries a second copy of the notice — one of the two paths \
                 will drift silent"
            );
        }
    }

    /// The figure in the notice must match the cap the fetch actually enforces,
    /// or the line is a guess wearing a number.
    #[test]
    fn the_size_in_the_notice_is_the_size_that_is_fetched() {
        let mb = super::MODEL_HF_FILE_MAX_BYTES / (1024 * 1024);
        assert_eq!(mb, 128, "the per-file cap moved; the notice says ~132 MB");
        assert!(SRC.contains("~132 MB"), "the notice lost its figure");
    }
}
