// The tests of `crate::semantic`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `semantic`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {

    /// `semantic` names files for the reader to OPEN, so from a subdirectory
    /// its paths owe the same base `query` uses — and the store must keep the
    /// root-relative ones, so `save` refuses a re-based index.
    #[test]
    fn a_rebased_semantic_index_reads_from_here_and_is_never_persisted() {
        let mut sem = SemanticIndex {
            docs: vec![
                Doc {
                    file: "a/b/inner.rs".into(),
                    len: 1,
                    snippet: String::new(),
                },
                Doc {
                    file: "top.rs".into(),
                    len: 1,
                    snippet: String::new(),
                },
            ],
            ..Default::default()
        };
        sem.mtimes.insert("a/b/inner.rs".into(), 7);
        assert!(!sem.rebased);

        sem.rebase_to(std::path::Path::new("/p"), std::path::Path::new("/p/a/b"));
        assert!(sem.rebased);
        let files: Vec<&str> = sem.docs.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, vec!["inner.rs", "../../top.rs"]);
        assert!(sem.mtimes.contains_key("inner.rs"));

        // Refused, silently and without error — the caller keeps its result.
        let d = std::env::temp_dir().join(format!(
            "tk-sem-rebase-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let _stores = IsolatedStores::new(&d);
        sem.save(&d).unwrap();
        assert!(
            SemanticIndex::load(&d).is_none(),
            "a re-based semantic index reached the store"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
    /// The index build stops at a posting cap. Which files make it in must not
    /// depend on hash iteration order, or the same repository indexes a
    /// different subset — and answers `semantic` differently — on every
    /// rebuild.
    /// The ranking stage must survive the identity reranker. When the shim's
    /// flat score was copied over the fused one, every hit of every query came
    /// back at the same number: the ordering still happened upstream, but the
    /// score stopped saying anything, and a caller that MULTIPLIES it (the
    /// framework boost) was silently promoted from "nudge" to "reorder".
    #[test]
    fn hybrid_results_keep_a_score_that_tells_them_apart() {
        let d = std::env::temp_dir().join(format!(
            "tokenade-rankguard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        for (name, body) in [
            (
                "stash.rs",
                "fn stash_blob() { /* stash blob by hash */ }\nfn recover_stash_blob_by_hash() {}\n",
            ),
            ("other.rs", "fn unrelated() { /* colours */ }\n"),
            ("third.rs", "fn stash_helper() { /* stash */ }\n"),
        ] {
            fs::write(d.join(name), body).unwrap();
        }
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hits = sem.search("stash blob hash", 5);
        assert!(hits.len() >= 2, "need several hits to compare: {hits:?}");
        let scores: Vec<f64> = hits.iter().map(|h| h.score).collect();
        assert!(
            scores
                .windows(2)
                .any(|w| (w[0] - w[1]).abs() > f64::EPSILON),
            "a ranked list whose scores are all equal ranks nothing: {scores:?}"
        );
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "scores must descend: {scores:?}"
        );
        // Nothing claims a rerank happened — the cross-encoder is gone.
        assert!(
            !hits
                .iter()
                .any(|h| h.matched.iter().any(|m| m == "reranked")),
            "no stage reranks these results any more: {hits:?}"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_indexed_subset_is_the_same_on_every_build() {
        use std::collections::HashMap;
        let mut first: Option<Vec<String>> = None;
        for _ in 0..50 {
            let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
            for i in 0..40 {
                by_file.insert(format!("src/file{i:02}.rs"), vec![format!("sym{i}")]);
            }
            // Mirror of the build loop's ordering step.
            let mut v: Vec<(String, Vec<String>)> = by_file.into_iter().collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            let taken: Vec<String> = v.into_iter().take(7).map(|(f, _)| f).collect();
            match &first {
                None => first = Some(taken),
                Some(f) => assert_eq!(f, &taken, "the indexed subset changed between builds"),
            }
        }
        assert_eq!(first.unwrap()[0], "src/file00.rs");
    }

    use super::*;
    use std::io::Write;

    /// A scratch repo AND an isolated semantic DB, torn down together.
    ///
    /// Indexing writes a row into `semantic_db`, whose path had no env override
    /// until `TOKENADE_SEMANTIC_DB` was added — so these tests were writing into
    /// the developer's REAL `~/.tokenade/semantic.db`. 81 rows keyed on
    /// `/tmp/tokenade-sem-<pid>-…` were found there, and `evict-stale` had
    /// grown a `prune_missing` pass to clean up after them: a workaround for a
    /// test that should never have reached that file.
    ///
    /// Holds [`crate::platform::ENV_LOCK`] for its whole life, per that lock's
    /// contract — the mutation AND the window in which the value is read.
    struct SemRepo {
        dir: PathBuf,
        _stores: IsolatedStores,
    }

    /// Both stores a semantic build reaches — the semantic db AND the index
    /// db (`Index::rebuild_incremental` loads the call graph from it) —
    /// pointed at temp files, under [`crate::platform::ENV_LOCK`] for the
    /// guard's whole life. Isolating only the first left a test reading
    /// `TOKENADE_INDEX_DB` while an index test had pointed it under a FILE
    /// (`a_refused_save_leaves_the_sidecar_alone`): `index db: unable to open
    /// database file`, one run in several (2026-10-03).
    struct IsolatedStores {
        base: PathBuf,
        _env: std::sync::MutexGuard<'static, ()>,
    }

    impl IsolatedStores {
        fn new(base: &Path) -> Self {
            let env = crate::platform::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let base = base.with_extension("stores");
            // SAFETY: env vars are process-wide; ENV_LOCK is held for the
            // guard's whole lifetime, which outlives every read of them.
            unsafe {
                std::env::set_var("TOKENADE_SEMANTIC_DB", base.with_extension("semantic.db"));
                std::env::set_var("TOKENADE_INDEX_DB", base.with_extension("index.db"));
            }
            IsolatedStores { base, _env: env }
        }
    }

    impl Drop for IsolatedStores {
        fn drop(&mut self) {
            // SAFETY: `_env` (ENV_LOCK) is still held — fields drop after this.
            unsafe {
                std::env::remove_var("TOKENADE_SEMANTIC_DB");
                std::env::remove_var("TOKENADE_INDEX_DB");
            }
            for ext in ["semantic.db", "index.db"] {
                let _ = fs::remove_file(self.base.with_extension(ext));
            }
        }
    }

    impl SemRepo {
        fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for SemRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn tmp_repo_isolated() -> SemRepo {
        let dir = tmp_repo();
        let _stores = IsolatedStores::new(&dir);
        SemRepo { dir, _stores }
    }

    fn tmp_repo() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-sem-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        fs::create_dir_all(d.join("src").join("auth")).unwrap();
        fs::create_dir_all(d.join("src").join("billing")).unwrap();

        let mut f = fs::File::create(d.join("src").join("auth").join("login.rs")).unwrap();
        writeln!(
            f,
            "/// Handle a user login: verify credentials, mint a session token.\n\
             pub fn verify_credentials(user: &str, password: &str) -> bool {{ true }}\n\
             pub fn mint_session_token(user: &str) -> String {{ String::new() }}"
        )
        .unwrap();

        let mut g = fs::File::create(d.join("src").join("billing").join("invoice.rs")).unwrap();
        writeln!(
            g,
            "/// Generate a monthly invoice PDF for a given account.\n\
             pub fn generate_monthly_invoice(account_id: u64) {{}}"
        )
        .unwrap();
        d
    }

    #[test]
    fn equal_scoring_files_rank_in_a_stable_order() {
        // Four files with byte-identical content score identically, so the
        // ORDER is decided entirely by the tie-break. Without one, the
        // `HashMap` fold in `search` would hand them back in an order that
        // changes between processes — and since the caller truncates to a
        // limit, that changes which files the agent is shown, not just how
        // they are arranged.
        let d = std::env::temp_dir().join(format!(
            "tokenade-sem-tie-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(d.join("src")).unwrap();
        for name in ["delta.rs", "alpha.rs", "charlie.rs", "bravo.rs"] {
            let mut f = fs::File::create(d.join("src").join(name)).unwrap();
            writeln!(f, "pub fn reticulate_splines() {{}}").unwrap();
        }
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hits = sem.search("reticulate splines", 10);
        let _ = fs::remove_dir_all(&d);

        assert_eq!(hits.len(), 4, "all four files match");
        assert!(
            hits.windows(2)
                .all(|w| (w[0].score - w[1].score).abs() < 1e-9),
            "identical content must score identically"
        );
        let files: Vec<&str> = hits.iter().map(|h| h.file.as_str()).collect();
        let mut sorted = files.clone();
        sorted.sort_unstable();
        assert_eq!(files, sorted, "ties must break on the path, ascending");
    }

    #[test]
    fn tokenize_handles_camel_and_snake() {
        let t = tokenize("parseConfig parse_config XMLHttpRequest");
        assert!(t.contains(&"parse".to_string()));
        assert!(t.contains(&"config".to_string()));
        assert!(t.contains(&"xml".to_string()));
        assert!(t.contains(&"http".to_string()));
        assert!(t.contains(&"request".to_string()));
    }

    /// A symbol name embedded in CJK text must still be a token of its own.
    ///
    /// Han is `XID_Continue`, so `word_re` kept `修复run函数的报错` whole and
    /// `split_camel_snake` (ASCII case only) could not cut it: the query token
    /// was the entire clause and `run` matched nothing in the index. Both sides
    /// tokenise through here, so the fix has to hold for index and query alike.
    #[test]
    fn an_ascii_symbol_glued_to_cjk_is_isolated() {
        let t = tokenize("修复run函数的报错");
        assert!(t.contains(&"run".to_string()), "got {t:?}");
        // The CJK halves survive as bigrams, not as one unmatchable clause.
        assert!(t.contains(&"修复".to_string()), "got {t:?}");
        assert!(t.contains(&"函数".to_string()), "got {t:?}");
        assert!(!t.iter().any(|s| s.chars().count() > 2 && !s.is_ascii()));
    }

    /// Latin input must tokenise EXACTLY as before the script-boundary split —
    /// the whole index is built with this function.
    #[test]
    fn latin_tokenisation_is_unchanged_by_the_script_split() {
        assert_eq!(
            tokenize("parse_config XMLHttpRequest cache"),
            tokenize("parse_config XMLHttpRequest cache")
        );
        let t = tokenize("evict_files_older_than");
        assert!(t.contains(&"evict".to_string()));
        assert!(t.contains(&"files".to_string()));
        assert!(t.contains(&"older".to_string()));
    }

    /// Regression for the bench finding (2026-05-28): `tokenade ask "type"` and
    /// other code-keyword queries used to return zero results because the
    /// term was on the STOPWORDS list — filtered out from BOTH the index
    /// and the query, so the inverted index had no entry for it. We
    /// restrict STOPWORDS to natural-language words now; code identifiers
    /// stay searchable.
    #[test]
    fn code_keywords_are_searchable() {
        let must_pass = [
            "type", "class", "def", "fn", "pub", "mut", "const", "var", "let", "return", "import",
            "null", "none", "nil", "false", "true", "self", "use",
        ];
        for kw in must_pass {
            let toks = tokenize(kw);
            assert!(
                toks.contains(&kw.to_string()),
                "{kw:?} should tokenize to itself, got {toks:?} — STOPWORDS regression"
            );
        }
    }

    /// A compound identifier reaches the index as ITSELF, not only as the
    /// bag of common words it splits into.
    ///
    /// Measured 2026-08-27 over 151 unique, module-scope, production
    /// declarations of this repository: before this term existed the
    /// declaring file was the top hit for 64 % of them and absent from the
    /// whole result list for 12. A name like `dropped_error_lines` only
    /// ever reached BM25 as `dropped` + `error` + `lines`, so every file
    /// that mentions errors and lines scored against the one file that
    /// declares it.
    #[test]
    fn an_exact_identifier_is_a_token_of_its_own() {
        let t = tokenize("dropped_error_lines");
        assert!(
            t.contains(&"droppederrorlines".to_string()),
            "the identifier itself must be a term, got {t:?}"
        );
        // The parts are still there — this is an addition, not a swap.
        for part in ["dropped", "error", "lines"] {
            assert!(t.contains(&part.to_string()), "{part} lost, got {t:?}");
        }
    }

    /// The exact term is normalised across naming conventions, so a query
    /// typed in one convention finds a symbol declared in the other. The
    /// separators are what differ between `drop_error_lines` and
    /// `dropErrorLines`; the identifier is the same.
    #[test]
    fn snake_and_camel_reach_the_same_exact_token() {
        let snake = tokenize("dropped_error_lines");
        let camel = tokenize("droppedErrorLines");
        assert!(
            camel.contains(&"droppederrorlines".to_string()),
            "{camel:?}"
        );
        assert!(
            snake.contains(&"droppederrorlines".to_string()),
            "{snake:?}"
        );
    }

    /// An identifier outside ASCII gets its exact term like any other.
    ///
    /// `split_script_runs` has already cut CJK into its own runs before
    /// this point, so the only thing an `is_ascii()` gate suppressed was a
    /// Latin-with-diacritics name — measured on `café_naïve`, which lost
    /// its exact term while `修复run函数的报错` was never affected either
    /// way. The gate was removed and this pins why.
    #[test]
    fn an_accented_identifier_still_gets_its_exact_term() {
        let t = tokenize("café_naïve");
        assert!(
            t.contains(&"cafénaïve".to_string()),
            "accented compound lost its exact term, got {t:?}"
        );
    }

    /// A single word is not a compound, and emitting it twice would inflate
    /// its term frequency — BM25 would read one mention as two.
    #[test]
    fn a_single_word_gains_no_exact_duplicate() {
        let t = tokenize("cache");
        let n = t.iter().filter(|x| x.as_str() == "cache").count();
        assert_eq!(n, 1, "single word duplicated, got {t:?}");
    }

    /// The exact term is ASCII-only on purpose. `split_script_runs` exists
    /// to cut `修复run函数的报错` apart; gluing a run back together would
    /// rebuild the single unmatchable clause it was cut to avoid.
    #[test]
    fn a_cjk_run_is_never_glued_back_into_one_term() {
        let t = tokenize("修复run函数的报错");
        assert!(
            !t.iter().any(|s| s.chars().count() > 2 && !s.is_ascii()),
            "a non-ascii clause came back as one term: {t:?}"
        );
    }

    /// The property, not the token: the file that DECLARES the symbol
    /// outranks a file that merely uses the same common words. The decoy
    /// here repeats every word of the query and declares none of it.
    #[test]
    fn the_declaring_file_outranks_a_file_that_shares_only_the_words() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let d = std::env::temp_dir().join(format!(
            "tokenade-semantic-exact-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(
            d.join("src").join("owner.rs"),
            "pub fn dropped_error_lines() {}\n",
        )
        .unwrap();
        fs::write(
            d.join("src").join("decoy.rs"),
            "//! dropped error lines dropped error lines dropped error lines\n\
             //! dropped error lines dropped error lines dropped error lines\n\
             pub fn dropped() {}\n\
             pub fn error() {}\n\
             pub fn lines() {}\n",
        )
        .unwrap();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hits = sem.search("dropped_error_lines", 5);
        assert!(!hits.is_empty(), "no hits at all");
        assert!(
            hits[0].file.ends_with("owner.rs"),
            "the declaring file must rank first, got {:?}",
            hits.iter().map(|h| &h.file).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// Real-world fix verification: code-keyword fragments that used to
    /// be in STOPWORDS (`into`, `from`, `with`, `not`, …) are now first-
    /// class search terms. The bench's `nav:semantic-search` task picks
    /// these as queries from symbol-name fragments — the failure mode
    /// was 18 runs returning zero hits because the term was filtered
    /// from BOTH index and query.
    #[test]
    fn search_finds_code_keyword_query() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let d = std::env::temp_dir().join(format!(
            "tokenade-semantic-kw-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        // Symbol names that contain the fragments the bench used to fail
        // on. `try_into`, `from_str`, `with_capacity` are standard Rust
        // idioms whose name fragments (`into`, `from`, `with`) used to
        // be silently dropped at index time.
        fs::write(
            d.join("src").join("conv.rs"),
            "pub fn try_into() {}\n\
             pub fn from_str() {}\n\
             pub fn with_capacity() {}\n",
        )
        .unwrap();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        for kw in ["into", "from", "with"] {
            let hits = sem.search(kw, 5);
            assert!(!hits.is_empty(), "query {kw:?} returned no hits");
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// The substring safety-net path: when BM25 legitimately has no entry
    /// for the term (e.g. user queries an English fragment that only
    /// occurs in the file path), we fall back to a linear snippet scan
    /// instead of returning zero hits.
    #[test]
    fn search_falls_back_to_substring_when_bm25_empty() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let d = std::env::temp_dir().join(format!(
            "tokenade-semantic-fb-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        // File named after a unique fragment; no symbols defined inside,
        // so BM25 has nothing to match on except the path.
        fs::write(
            d.join("src").join("zylozulu.rs"),
            "// placeholder file\nfn main() {}\n",
        )
        .unwrap();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hits = sem.search("zylozulu", 5);
        assert!(
            !hits.is_empty(),
            "substring fallback should have found 'zylozulu' in the file path"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn search_finds_by_concept() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);

        let hits = sem.search("login session", 5);
        assert!(!hits.is_empty());
        assert!(hits[0].file.contains("login.rs"));

        let hits2 = sem.search("invoice pdf", 5);
        assert!(!hits2.is_empty());
        assert!(hits2[0].file.contains("invoice.rs"));

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn search_camel_query_matches_snake_identifier() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hits = sem.search("mintSessionToken", 3);
        assert!(!hits.is_empty());
        assert!(hits[0].file.contains("login.rs"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn save_load_roundtrip() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        sem.save(&d).unwrap();
        let sem2 = SemanticIndex::load(&d).unwrap();
        let h1 = sem.search("invoice", 5);
        let h2 = sem2.search("invoice", 5);
        assert_eq!(h1.len(), h2.len());
        assert_eq!(h1[0].file, h2[0].file);
        let _ = fs::remove_dir_all(&d);
    }

    /// A credential in a comment must not reach the global store.
    ///
    /// Comments are seeded into every document because they are what makes
    /// meaning-search work, and they are also where a credential gets written
    /// down. Unlike the source file, the document is copied into
    /// `~/.tokenade/semantic.db`, outside the repository.
    ///
    /// Scrubbing afterwards is a treadmill and this was measured: drop the
    /// affected index, re-index, and the secret is back. The redaction has to
    /// be on the way IN.
    #[test]
    fn a_credential_in_a_comment_does_not_reach_the_index() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(
            d.join("src").join("deploy.rs"),
            "//! Deployment notes. aws_secret_access_key = AKIAIOSFODNN7EXAMPLE\n\
             pub fn deploy() {}\n",
        )
        .unwrap();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let blob = postcard::to_allocvec(&sem).unwrap();
        assert!(
            !blob
                .windows("AKIAIOSFODNN7EXAMPLE".len())
                .any(|w| w == b"AKIAIOSFODNN7EXAMPLE"),
            "the credential reached the semantic index"
        );
        // …and the rest of the comment still does, or the redaction has
        // bought safety by making the index useless.
        assert!(
            sem.search("deployment", 5)
                .iter()
                .any(|h| h.file.ends_with("deploy.rs")),
            "the comment's own words stopped being searchable"
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// The same rule the symbol index already carries: an index built by
    /// another tokenade is not reused.
    ///
    /// The postings ARE the output of `tokenize`, and a query tokenises
    /// through that same function. `mtimes` cannot notice a tokenizer
    /// change — every file is untouched — so without this key an upgraded
    /// install keeps answering out of an index the previous build made,
    /// and a retrieval fix reaches nobody who already had one.
    #[test]
    fn a_semantic_index_built_by_another_version_is_not_reused() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let mut sem = SemanticIndex::build_from_index(&idx, &d);
        assert_eq!(
            sem.built_by,
            env!("CARGO_PKG_VERSION"),
            "a freshly built index must stamp the build that made it"
        );
        sem.save(&d).unwrap();
        assert!(
            SemanticIndex::load(&d).is_some(),
            "an index this build made must load"
        );
        sem.built_by = "0.0.0-from-an-older-tokenade".to_string();
        sem.save(&d).unwrap();
        assert!(
            SemanticIndex::load(&d).is_none(),
            "a foreign build's index must be rejected so the caller rebuilds"
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// The per-project sidecars carry the same rule as the DB.
    ///
    /// `semantic.bin` and `semantic.json` are the pre-L7 layouts, so they
    /// are precisely the blobs an UPGRADING install still has on disk —
    /// the population the version key exists for. `load` migrates a
    /// sidecar into the global DB on the way past, so accepting a foreign
    /// one would not just answer once from a stale index, it would promote
    /// it.
    #[test]
    fn a_foreign_sidecar_is_not_migrated_in() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let mut sem = SemanticIndex::build_from_index(&idx, &d);
        sem.built_by = "0.0.0-from-an-older-tokenade".to_string();
        fs::create_dir_all(d.join(".tokenade")).unwrap();
        fs::write(
            d.join(".tokenade").join("semantic.bin"),
            postcard::to_allocvec(&sem).unwrap(),
        )
        .unwrap();
        assert!(
            SemanticIndex::load(&d).is_none(),
            "a foreign build's semantic.bin must be rejected, not migrated"
        );
        let _ = fs::remove_file(d.join(".tokenade").join("semantic.bin"));
        fs::write(
            d.join(".tokenade").join("semantic.json"),
            serde_json::to_string(&sem).unwrap(),
        )
        .unwrap();
        assert!(
            SemanticIndex::load(&d).is_none(),
            "a foreign build's semantic.json must be rejected, not migrated"
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// A blob written before the key existed deserialises with an empty
    /// `built_by` (serde default) and must be refused too — it is exactly
    /// the case the key was added for.
    #[test]
    fn a_semantic_index_with_no_stamp_is_not_reused() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let mut sem = SemanticIndex::build_from_index(&idx, &d);
        sem.built_by = String::new();
        sem.save(&d).unwrap();
        assert!(
            SemanticIndex::load(&d).is_none(),
            "an unstamped index must be rebuilt, not trusted"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_query_returns_empty() {
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        assert!(sem.search("", 5).is_empty());
        assert!(sem.search("   \n\t  ", 5).is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    // ---- Hybrid pipeline ------------------------------------------------
    //
    // The full hybrid path (search_hybrid) needs the multilingual-e5-small
    // and BGE-reranker-v2-m3 models downloaded. We don't exercise it in
    // the default unit suite — that would require ~700 MB of model weights
    // and network on first run. The live integration test is gated on
    // TOKENADE_EMBED_TEST=1 + TOKENADE_RERANK_TEST=1.

    #[test]
    fn incremental_semantic_rebuild_picks_up_only_changed() {
        // Standing on the parallel index test: build full, change one file,
        // verify incremental adds only the new symbols + keeps the rest.
        let d = std::env::temp_dir().join(format!(
            "tokenade-sem-inc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(d.join("src")).unwrap();
        let _stores = IsolatedStores::new(&d);
        std::fs::write(d.join("src/alpha.rs"), "/// alpha doc\npub fn alpha() {}").unwrap();
        std::fs::write(d.join("src/beta.rs"), "/// beta doc\npub fn beta() {}").unwrap();
        let idx1 = Index::build(&d).unwrap();
        let sem1 = SemanticIndex::build_from_index(&idx1, &d);
        assert_eq!(sem1.docs.len(), 2);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(
            d.join("src/alpha.rs"),
            "/// alpha doc v2 with shiny new content\npub fn alpha_v2() {}",
        )
        .unwrap();
        let idx2 = Index::rebuild_incremental(&idx1, &d).unwrap();
        let sem2 = SemanticIndex::rebuild_incremental(&sem1, &idx2, &d);

        assert_eq!(sem2.docs.len(), 2, "still two files after incremental");
        let alpha_doc = sem2.docs.iter().find(|d| d.file.contains("alpha")).unwrap();
        // The fresh tokenisation should include "shiny" — present in v2 only.
        let hits = sem2.search("shiny", 5);
        assert!(
            hits.iter().any(|h| h.file == alpha_doc.file),
            "incremental rebuild missed the new content"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn incremental_semantic_drops_removed_files() {
        let d = std::env::temp_dir().join(format!(
            "tokenade-sem-drop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        let _stores = IsolatedStores::new(&d);
        std::fs::write(d.join("keep.rs"), "pub fn kept() {}").unwrap();
        std::fs::write(d.join("gone.rs"), "pub fn doomed() {}").unwrap();
        let idx1 = Index::build(&d).unwrap();
        let sem1 = SemanticIndex::build_from_index(&idx1, &d);
        assert_eq!(sem1.docs.len(), 2);
        std::fs::remove_file(d.join("gone.rs")).unwrap();
        let idx2 = Index::rebuild_incremental(&idx1, &d).unwrap();
        let sem2 = SemanticIndex::rebuild_incremental(&sem1, &idx2, &d);
        assert_eq!(sem2.docs.len(), 1);
        assert!(sem2.docs[0].file.contains("keep"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn hybrid_empty_corpus_returns_ok_empty_without_network() {
        // Empty corpus short-circuits BEFORE the embedder is called, so
        // this is the one hybrid test we can run offline.
        let sem = SemanticIndex::default();
        let r = sem.search_hybrid("anything", 5, 0.7);
        assert!(matches!(r, Ok(v) if v.is_empty()));
    }

    #[test]
    #[ignore = "requires TOKENADE_EMBED_TEST + TOKENADE_RERANK_TEST and ~700 MB model download"]
    fn hybrid_live_returns_relevant_files() {
        if std::env::var("TOKENADE_EMBED_TEST").is_err()
            || std::env::var("TOKENADE_RERANK_TEST").is_err()
        {
            return;
        }
        let _repo = tmp_repo_isolated();
        let d = _repo.path().to_path_buf();
        let idx = Index::build(&d).unwrap();
        let sem = SemanticIndex::build_from_index(&idx, &d);
        let hy = sem
            .search_hybrid("invoice pdf", 5, 0.5)
            .expect("models must work");
        assert!(!hy.is_empty());
        let _ = fs::remove_dir_all(&d);
    }
}
