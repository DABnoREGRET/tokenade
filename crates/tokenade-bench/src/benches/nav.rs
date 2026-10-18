//! **Navigating CODE: find, skim, summarise.**
//!
//! The family `main.rs` had never filed. What a navigation bench measures
//! is not the number of bytes returned but whether the answer is enough:
//! a symbol found without its file, a skeleton without its signatures, an
//! overview without the paths, only save tokens by forcing the agent to
//! come back.

use super::super::*;

/// In-process `find -type f` over source files — realistic baseline for "what
/// files are in this repo?".
pub(crate) fn find_files_list(repo: &Path) -> String {
    walk_files(repo)
        .iter()
        .map(|p| p.strip_prefix(repo).unwrap_or(p).display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- per-repo tasks -------------------------------------------------------

pub(crate) fn task_nav_find_symbol(
    results: &mut Vec<RunResult>,
    name: &str,
    repo: &Path,
    idx: &index::Index,
) {
    // Use a real public function from the repo as the search target.
    let Some(target) = idx.symbols.iter().find(|s| s.kind == "fn").cloned() else {
        return;
    };
    let pat = Regex::new(&format!(
        r"(?:fn|def|class|function|func)\s+{}\b",
        regex::escape(&target.name)
    ))
    .unwrap();
    let t = Instant::now();
    let baseline = grep_repo(repo, &pat);
    let baseline_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let hits = idx.find(&target.name, 10);
    let answer = hits
        .iter()
        .map(|s| s.location_line())
        .collect::<Vec<_>>()
        .join("\n");
    let ms = t.elapsed().as_millis();
    let quality = if answer.contains(&target.name) && !hits.is_empty() {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "nav:find-symbol",
        "symbol-index",
        name,
        &baseline,
        &answer,
        baseline_ms + ms,
        quality,
    );
}

/// Benchmark the BM25 semantic search added in v3. Baseline is `grep -i` on
/// each query term across the corpus (what an agent does when it doesn't
/// know exact identifiers). STS path: `semantic::SemanticIndex::search` —
/// returns top-10 file ranking.
///
/// Quality: 1.0 if the top-3 hits contain a file whose name shares a token
/// with the query (a recall surrogate for concept-search), 0.0 otherwise.
pub(crate) fn task_nav_semantic(
    results: &mut Vec<RunResult>,
    name: &str,
    repo: &Path,
    idx: &index::Index,
) {
    // Synthesise three realistic concept queries from the most common symbol
    // name fragments in this repo. This avoids hand-curated queries that
    // could be cherry-picked.
    let mut fragments: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &idx.symbols {
        for piece in s.name.split(|c: char| !c.is_alphanumeric()) {
            let piece = piece.to_lowercase();
            if piece.len() >= 4 {
                *fragments.entry(piece).or_default() += 1;
            }
        }
    }
    let mut sorted: Vec<(String, usize)> = fragments.into_iter().collect();
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let queries: Vec<&str> = sorted.iter().take(3).map(|(q, _)| q.as_str()).collect();
    if queries.is_empty() {
        return;
    }

    let sem = tokenade_core::semantic::SemanticIndex::build_from_index(idx, repo);

    for q in queries {
        let pat = Regex::new(&format!("(?i){}", regex::escape(q))).unwrap();
        let t = Instant::now();
        let baseline = grep_repo(repo, &pat);
        let baseline_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let hits = sem.search(q, 10);
        let answer = hits
            .iter()
            .map(|h| format!("{:5.2} {} [{}]", h.score, h.file, h.matched.join(",")))
            .collect::<Vec<_>>()
            .join("\n");
        let ms = t.elapsed().as_millis();
        // Recall surrogate: a top-3 hit qualifies if its FILE actually
        // contains the literal query token somewhere (symbol name, docstring,
        // source). Reading just the top-3 to verify keeps this honest without
        // re-grepping the whole repo, and matches "did the agent get to the
        // right file?" — which is what an agent really cares about.
        let mut quality = 0.0_f64;
        for h in hits.iter().take(3) {
            let p = repo.join(&h.file);
            if h.file.to_lowercase().contains(q) {
                quality = 1.0;
                break;
            }
            if let Ok(content) = fs::read_to_string(&p) {
                if content.to_lowercase().contains(q) {
                    quality = 1.0;
                    break;
                }
            }
        }
        record(
            results,
            "nav:semantic-search",
            "semantic-bm25",
            name,
            &baseline,
            &answer,
            baseline_ms + ms,
            quality,
        );
    }
}

pub(crate) fn task_overview_files(
    results: &mut Vec<RunResult>,
    name: &str,
    repo: &Path,
    idx: &index::Index,
) {
    let baseline = find_files_list(repo);
    let t = Instant::now();
    let fw = tokenade_core::framework::detect(repo);
    let map = idx.structure_map_with_framework(fw);
    let ms = t.elapsed().as_millis();
    let quality = if map.contains("files") { 1.0 } else { 0.0 };
    record(
        results,
        "overview:files-list",
        "symbol-index",
        name,
        &baseline,
        &map,
        ms,
        quality,
    );
}

/// Declaration regex used both to pick the "most useful" review file and as
/// the ground truth for skeleton fidelity.
pub(crate) fn decl_regex() -> Regex {
    Regex::new(
        r"(?m)^[\t ]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn|def|class|struct|enum|trait|interface|type|function|func)\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    )
    .unwrap()
}

pub(crate) fn task_review_skeleton(
    results: &mut Vec<RunResult>,
    name: &str,
    repo: &Path,
    idx: &index::Index,
) {
    // Pick the file with the most symbols (most representative for review).
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for s in &idx.symbols {
        *counts.entry(s.file.as_str()).or_insert(0) += 1;
    }
    let Some((best, _)) = counts.iter().max_by_key(|(_, n)| **n) else {
        return;
    };
    let file = repo.join(best);
    let Ok(full) = fs::read_to_string(&file) else {
        return;
    };
    if full.len() > 500_000 {
        return;
    }
    let t = Instant::now();
    let sk = tokenade_core::skeleton::skeleton(&full);
    let ms = t.elapsed().as_millis();
    // Adversarial fidelity: every top-level declaration name in the original
    // must still appear in the skeleton.
    let re = decl_regex();
    let mut seen: HashSet<String> = HashSet::new();
    let (mut total, mut kept) = (0usize, 0usize);
    for cap in re.captures_iter(&full) {
        let n = cap.get(1).unwrap().as_str();
        if !seen.insert(n.to_string()) {
            continue;
        }
        total += 1;
        if sk.contains(n) {
            kept += 1;
        }
    }
    let quality = if total == 0 {
        1.0
    } else {
        kept as f64 / total as f64
    };
    record(
        results,
        "review:file-skeleton",
        "context-compression",
        name,
        &full,
        &sk,
        ms,
        quality,
    );
}

pub(crate) fn task_filter_gitlog(results: &mut Vec<RunResult>, name: &str, fixtures: &Path) {
    let path = fixtures.join(format!("{name}-gitlog.txt"));
    let Ok(log) = fs::read_to_string(&path) else {
        return;
    };
    if log.is_empty() {
        return;
    }
    let t = Instant::now();
    let r = filter::filter_command(&["git".into(), "log".into()], &log);
    let ms = t.elapsed().as_millis();
    // Adversarial: every commit hash (signal we must not lose) preserved.
    let hash_re = Regex::new(r"(?m)^commit ([0-9a-f]{7,})").unwrap();
    let (mut total, mut kept) = (0usize, 0usize);
    for cap in hash_re.captures_iter(&log) {
        total += 1;
        let h = cap.get(1).unwrap().as_str();
        let short = &h[..h.len().min(8)];
        if r.output.contains(short) {
            kept += 1;
        }
    }
    let quality = if total == 0 {
        1.0
    } else {
        kept as f64 / total as f64
    };
    record(
        results,
        "output:filter-gitlog",
        "output-filter",
        name,
        &log,
        &r.output,
        ms,
        quality,
    );
}

// ---- Iter #23: source maps ---------------------------------------------

// ---- Iter #44: tcpdump packet capture --------------------------------

// ---- Iter #43: vmstat system monitoring -----------------------------

// ---- Iter #42: multi-container k8s logs ------------------------------

// ---- Iter #41: cargo bench / criterion output ------------------------

// ---- Iter #40: netstat / ss / lsof network listings ------------------

// ---- Iter #39: grep / ripgrep / git grep search output ---------------

// ---- Iter #38: du disk-usage output -----------------------------------

// ---- Iter #37: find / tree / ls -laR directory listings --------------

// ---- Iter #36: systemctl status block format --------------------------

// ---- Iter #35: JSON Lines / NDJSON cloud-log streams ------------------

// ---- Iter #34: DNS lookup output (dig / nslookup / host) --------------

// ---- Iter #33: git log / git show structured compaction ---------------

// ---- Iter #32: Kubernetes manifest YAML (multi-doc) -------------------

// ---- Iter #31: package-manager install output -------------------------

// ---- Iter #30: GitHub Actions workflow logs ----------------------------

// ---- Iter #29: Ansible playbook output ---------------------------------

// ---- Iter #28: kubectl describe block format ---------------------------

// ---- Iter #27: JVM build-tool output (Maven / Gradle / SBT) ------------

// ---- Iter #26: strace / ltrace output ----------------------------------

// ---- Iter #25: GraphQL SDL schemas -------------------------------------

// ---- Iter #24: OpenAPI / Swagger specs ---------------------------------

// ---- Iter #22: systemd journal / dmesg ---------------------------------

// ---- Iter #21: nginx/Apache access logs --------------------------------

// ---- Iter #20: Prometheus metrics --------------------------------------

// ---- Iter #19: SQL query result compaction -----------------------------

// ---- Iter #18: lockfile flattening -------------------------------------

// ---- Iter #17: markdown documents --------------------------------------

// ---- Iter #16: env dump + secret redaction -----------------------------

// ---- Iter #15: auto-routing -------------------------------------------

// ---- Iter #14: tabular CLI compaction ----------------------------------

// ---- Iter #13: terraform plan compaction -------------------------------

// ---- Iter #12: archive listings ----------------------------------------

// ---- Iter #11: unified-diff / patch compaction --------------------------

// ---- Iter #10: PDF extraction ------------------------------------------
