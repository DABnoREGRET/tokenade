//! Gathering a repository into one reviewable blob.
//!
//! `pack` and `review-context` differ in what they select, not in how they
//! read it — the file walk, the git-tracked listing and the per-file byte cap
//! are shared.

use super::*;

/// Collect the repo-relative paths changed vs `base` (or vs HEAD, staged +
/// unstaged, when `base` is None). Returns an empty vec if git fails or the
/// tree is clean — the caller decides what to do with "nothing to review".
pub(crate) fn review_changed_files(root: &std::path::Path, base: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["diff".into(), "--name-only".into()];
    match base {
        // Three-dot: files changed on HEAD since it diverged from base —
        // the natural "what this branch changed" set for a review.
        Some(b) => args.push(format!("{b}...HEAD")),
        // No base: everything different from HEAD in the working tree
        // (staged + unstaged), the "review my pending work" set.
        None => args.push("HEAD".into()),
    }
    let mut g = tokenade_core::process_util::internal_command("git");
    g.current_dir(root).args(&args);
    let out = g.output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// List the repo's tracked + untracked-but-not-ignored files via
/// `git ls-files`. Respects `.gitignore` for free. Returns `None` (not empty)
/// when git is unavailable or the dir isn't a repo, so the caller can fall
/// back to a plain directory walk.
pub(crate) fn git_tracked_files(root: &std::path::Path) -> Option<Vec<String>> {
    let mut g = tokenade_core::process_util::internal_command("git");
    g.current_dir(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"]);
    let out = g.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
    )
}

/// Recursive directory walk fallback for non-git dirs. Skips `.git`, common
/// vendor dirs, and dotfiles-of-dirs that almost never belong in an export.
/// Returns repo-relative paths. Bounded by `MAX_PACK_FILES` to avoid pathology.
pub(crate) fn walk_repo_files(root: &std::path::Path) -> Vec<String> {
    const MAX_PACK_FILES: usize = 20_000;
    const SKIP_DIRS: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        "vendor",
    ];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_PACK_FILES {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out
}

/// Read a file for packing: bounded size, binary-skipped. Returns `None` for
/// binaries, oversize files, or unreadable paths — those never enter the
/// bundle (and the caller doesn't tokenize bytes it can't show).
pub(crate) fn read_packable(abs: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(abs).ok()?;
    let meta = file.metadata().ok()?;
    if meta.len() > max_bytes {
        return None;
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take(max_bytes).read_to_end(&mut buf).ok()?;
    // Binary heuristic: a NUL in the first 8 KiB.
    let probe = &buf[..buf.len().min(8192)];
    if probe.contains(&0) {
        return None;
    }
    // SECURITY: redact secrets from every packed body BEFORE it enters the
    // bundle (and before budget tokenization, so counts stay consistent). A
    // bundle that embeds a `.env`/config would otherwise leak. Idempotent.
    String::from_utf8(buf)
        .ok()
        .map(|s| tokenade_core::redact::redact(&s))
}

/// `tokenade pack` — budget-bounded repo export (a token-aware repomix).
///
/// Bundles repo files into one markdown document an agent can paste/Read in a
/// single shot. Scope is the whole repo (git-tracked, gitignore-respected) or,
/// with `--diff`/`--base`, only the files git says changed. A `--budget N`
/// token cap greedily includes files until full and reports — never hides —
/// what was omitted. Token counts use the same estimator as the rest of
/// tokenade (`tokenizer::estimate`).
pub(crate) fn cmd_pack(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade pack [PATH] [--budget N] [--diff | --base <ref>] [--out FILE] [--json]\n\
             \n\
             Budget-bounded repo export → one markdown bundle.\n\
             \n\
             Scope:\n\
             \x20 (default)        all git-tracked + untracked-not-ignored files\n\
             \x20 --diff           only files changed vs HEAD (staged + unstaged)\n\
             \x20 --base <ref>     only files changed on HEAD since it forked <ref>\n\
             \n\
             Budget:\n\
             \x20 --budget N       cap at ~N tokens on the file sections, headings\n\
             \x20                  and fences included; over-budget files are listed,\n\
             \x20                  not silently dropped (default: unbounded). That\n\
             \x20                  list is capped too: it keeps at least ten names\n\
             \x20                  and then counts the rest. The summary header and\n\
             \x20                  that ten-name floor sit outside the cap, so a very\n\
             \x20                  small budget is exceeded by those two and by\n\
             \x20                  nothing else.\n\
             \n\
             Output:\n\
             \x20 --out FILE       write the bundle to FILE (default: stdout)\n\
             \x20 --json           emit a machine-readable plan instead of markdown\n\
             \n\
             PATH defaults to the current directory."
        );
        return 0;
    }

    // ── Parse args ──────────────────────────────────────────────────
    let mut budget: Option<usize> = None;
    let mut base: Option<String> = None;
    let mut diff_scope = false;
    let mut out_file: Option<String> = None;
    let mut as_json = false;
    let mut path_arg: Option<String> = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--budget" => match it.next().and_then(|s| s.trim().parse::<usize>().ok()) {
                Some(n) if n > 0 => budget = Some(n),
                // `0` means UNBOUNDED, matching `skeleton --budget 0`. Same flag
                // on two commands must not mean opposite things: it used to be
                // refused here ("0 would omit everything"), which is only true
                // if you read 0 as a literal ceiling — nobody asks for a zero
                // budget, they ask for no ceiling.
                Some(_) => budget = Some(usize::MAX),
                None => {
                    eprintln!("tokenade pack: --budget needs a positive integer");
                    return 2;
                }
            },
            "--base" => match it.next() {
                Some(b) => base = Some(b.clone()),
                None => {
                    eprintln!("tokenade pack: --base needs a git ref");
                    return 2;
                }
            },
            "--diff" => diff_scope = true,
            "--out" => match it.next() {
                Some(f) => out_file = Some(f.clone()),
                None => {
                    eprintln!("tokenade pack: --out needs a file path");
                    return 2;
                }
            },
            "--json" => as_json = true,
            s if s.starts_with("--") => {
                eprintln!("tokenade pack: unknown flag '{s}'. Try `tokenade pack --help`.");
                return 2;
            }
            s => {
                if path_arg.is_some() {
                    eprintln!("tokenade pack: unexpected extra argument {s}");
                    return 2;
                }
                path_arg = Some(s.to_string());
            }
        }
    }

    let root = path_arg
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // A path that was NAMED and does not exist is an error, not an empty
    // result. It used to fall through to the directory walk, find nothing, and
    // report "no files in scope" with exit code 0 — so a script could not tell
    // a typo from a genuinely empty scope. An existing-but-empty directory
    // still succeeds with an empty result below, because that is what it is.
    if path_arg.is_some() && !root.exists() {
        eprintln!(
            "tokenade pack: {}: no such file or directory",
            root.display()
        );
        return 1;
    }
    let root = root.canonicalize().unwrap_or(root);

    // ── Enumerate files per scope ───────────────────────────────────
    // A PATH that is a FILE is the scope. Both enumerators below walk a
    // directory, so `tokenade pack src/lru.rs` came back "no files in scope"
    // — for a path the caller had just named, and with exit code 0. The help
    // says `[PATH]`, not `[DIR]`.
    let (paths, scope_desc) = if root.is_file() {
        (
            vec![root.display().to_string()],
            format!("one file ({})", root.display()),
        )
    } else if diff_scope || base.is_some() {
        let changed = review_changed_files(&root, base.as_deref());
        let desc = match &base {
            Some(b) => format!("files changed on HEAD since `{b}`"),
            None => "files changed vs HEAD (staged + unstaged)".to_string(),
        };
        (changed, desc)
    } else {
        match git_tracked_files(&root) {
            Some(p) => (
                p,
                // `--cached --others --exclude-standard`: what git tracks AND
                // what it merely does not ignore. Saying "git-tracked" alone
                // understated it — a file written this session and never
                // added IS in the bundle, which is what an export wants, and
                // the label was the only thing that disagreed.
                "all git-tracked files, plus untracked files git does not ignore".to_string(),
            ),
            None => (
                walk_repo_files(&root),
                "all files (directory walk — not a git repo)".to_string(),
            ),
        }
    };

    if paths.is_empty() {
        // Asked for JSON, answer in JSON — an empty scope is a RESULT, and a
        // caller parsing stdout must not be handed nothing at all.
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "budget": budget,
                    "scope": scope_desc,
                    "included": [],
                    "omitted": [],
                    "note": "no files in scope",
                })
            );
        } else {
            eprintln!("tokenade pack: no files in scope ({scope_desc})");
        }
        return 0;
    }

    // ── Read + tokenize. Sort by path for deterministic, predictable order.
    const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024; // 2 MiB per file
    let mut sorted = paths;
    sorted.sort();
    sorted.dedup();
    let mut entries: Vec<tokenade_core::pack::PackEntry> = Vec::with_capacity(sorted.len());
    let mut skipped_binary_or_large = 0usize;
    for rel in &sorted {
        let abs = root.join(rel);
        let Some(body) = read_packable(&abs, MAX_FILE_BYTES) else {
            skipped_binary_or_large += 1;
            continue;
        };
        let tokens = tokenade_core::tokenizer::estimate(&body);
        let lang = tokenade_core::file_search::language_from_path(std::path::Path::new(rel))
            .unwrap_or("")
            .to_string();
        entries.push(tokenade_core::pack::PackEntry {
            path: rel.clone(),
            tokens,
            lang,
            body,
            included: false,
        });
    }

    // A DEFAULT ceiling, not an opt-in one. MEASURED 2026-08-04: bare
    // `tokenade pack crates/tokenade-core/src` returned 8 569 402 bytes —
    // roughly 2,1 million tokens, more than any agent can hold, from one
    // source directory. The budget machinery worked all along (`--budget
    // 20000` gave 126 203 bytes); nothing set it unless the caller thought to.
    // Same shape as `skeleton`, capped the same morning for the same reason:
    // a guard nobody activates protects nobody. `--budget 0` still returns
    // everything for the caller who genuinely wants the whole tree.
    let budget = budget.or(Some(
        tokenade_core::coefficients::DEFAULT_CAP_READ_TOKENS as usize,
    ));
    let plan = tokenade_core::pack::plan_pack(entries, budget);

    // ── Emit ────────────────────────────────────────────────────────
    let rendered = if as_json {
        // The omitted manifest obeys the budget here exactly as it does in the
        // markdown render — it was bounded there and left unbounded here, so
        // `--budget 200 --json` returned 21x the cap and grew with the repo
        // rather than with the budget. `omitted_count`/`omitted_tokens` still
        // cover EVERY file, so bounding the list loses no information: the
        // remainder is `omitted_count - omitted_listed`.
        let all_omitted: Vec<&tokenade_core::pack::PackEntry> =
            plan.entries.iter().filter(|e| !e.included).collect();
        let included: Vec<serde_json::Value> = plan
            .entries
            .iter()
            .filter(|e| e.included)
            .map(|e| serde_json::json!({ "path": e.path, "tokens": e.tokens }))
            .collect();
        let doc = |omitted: Vec<serde_json::Value>, listed: usize| {
            serde_json::json!({
                "root": root.display().to_string(),
                "scope": scope_desc,
                "budget": plan.budget,
                "included_count": plan.included_count,
                "included_tokens": plan.included_tokens,
                "omitted_count": plan.omitted_count,
                "omitted_tokens": plan.omitted_tokens,
                "omitted_listed": listed,
                "skipped_binary_or_large": skipped_binary_or_large,
                "included": included.clone(),
                "omitted": omitted,
            })
        };
        // Room is what the budget has left once everything BUT the omitted
        // array is paid for — measured on the real document, not guessed.
        //
        // NOT minus `included_tokens`: the markdown render subtracts it
        // because it embeds the file BODIES, and this document does not — it
        // carries paths and sizes, which the serialization below already
        // costs. Subtracting them here left no room at all and pinned every
        // budget to the floor of ten.
        let room = plan.budget.map(|b| {
            let without = serde_json::to_string_pretty(&doc(Vec::new(), 0)).unwrap_or_default();
            b.saturating_sub(tokenade_core::tokenizer::estimate(&without))
        });
        let lines: Vec<String> = all_omitted
            .iter()
            .map(|e| format!("{{ \"path\": \"{}\", \"tokens\": {} }},", e.path, e.tokens))
            .collect();
        let listed = tokenade_core::pack::omitted_fit(&lines, room);
        let omitted: Vec<serde_json::Value> = all_omitted[..listed]
            .iter()
            .map(|e| serde_json::json!({ "path": e.path, "tokens": e.tokens }))
            .collect();
        serde_json::to_string_pretty(&doc(omitted, listed)).unwrap_or_else(|_| "{}".to_string())
    } else {
        tokenade_core::pack::render_markdown(
            &plan,
            &scope_desc,
            &root.display().to_string(),
            skipped_binary_or_large,
        )
    };

    match &out_file {
        Some(f) => {
            if let Err(e) = std::fs::write(f, &rendered) {
                eprintln!("tokenade pack: could not write {f}: {e}");
                return 1;
            }
            eprintln!(
                "tokenade pack: wrote {} ({} file(s), ~{} tokens{})",
                f,
                plan.included_count,
                plan.included_tokens,
                if plan.omitted_count > 0 {
                    format!(", {} omitted over budget", plan.omitted_count)
                } else {
                    String::new()
                }
            );
        }
        None => println!("{rendered}"),
    }
    0
}

/// `tokenade review-context` — graphsift-style token-budgeted review builder.
///
/// Ranks changed files + their direct import-graph neighbours + a few
/// importance hubs, greedily packs them into a hard token budget, and reports
/// what made the cut. The pure ranking/fit math lives in
/// `tokenade_core::review_context`; this wrapper does the IO (git diff, index
/// load, file tokenization).
pub(crate) fn cmd_review_context(rest: &[String]) -> i32 {
    use std::collections::{HashMap, HashSet};
    use tokenade_core::review_context as rc;

    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade review-context [--budget N] [--base <ref>] [--json]\n\
             \n\
             Build a token-budgeted code-review context: rank changed files +\n\
             their direct import-graph neighbours + a few importance hubs, then\n\
             greedily pack them into a hard token budget (diff-aware).\n\
             \n\
             --budget N    hard token ceiling for the included set (default 8000)\n\
             --base <ref>  diff against <ref>…HEAD (default: working tree vs HEAD)\n\
             --json        machine-readable plan (included/dropped + token totals)\n\
             \n\
             Tiers (high → low): changed · dependency-of-changed · dependent-on-\n\
             changed · importance-hub. Importance = import-graph in-degree."
        );
        return 0;
    }

    // Manual flag validation: two value flags + one bool flag, no positionals.
    let value_flags = ["--budget", "--base"];
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        if a == "--json" {
            i += 1;
            continue;
        }
        if value_flags.contains(&a.as_str()) {
            i += 2; // skip flag + its value
            continue;
        }
        eprintln!("tokenade review-context: unexpected argument `{a}` (see --help)");
        return 2;
    }

    let budget: usize = get_flag(rest, "--budget")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8000)
        .clamp(100, 10_000_000);
    let base = get_flag(rest, "--base");
    let json_mode = rest.iter().any(|a| a == "--json");

    let root = root();
    let changed_raw = review_changed_files(&root, base);
    let changed: HashSet<String> = changed_raw.iter().cloned().collect();
    if changed.is_empty() {
        let where_ = base.map_or("the working tree vs HEAD".to_string(), |b| {
            format!("{b}…HEAD")
        });
        if json_mode {
            println!(
                "{}",
                serde_json::json!({
                    "budget": budget,
                    "changed_files": 0,
                    "included": [],
                    "dropped": [],
                    "note": format!("no changes detected in {where_}"),
                })
            );
        } else {
            println!(
                "tokenade review-context: no changes detected in {where_} — nothing to review."
            );
        }
        return 0;
    }

    let idx = fresh_index();

    // Import-graph in-degree (how many repo files import each file) → the
    // importance proxy. Also the relationship sets used for the tiers.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for importees in idx.imports.values() {
        for f in importees {
            *in_degree.entry(f.as_str()).or_insert(0) += 1;
        }
    }
    let max_in_degree = in_degree.values().copied().max().unwrap_or(0);

    // deps_of_changed: files a changed file imports (dependencies).
    // dependents_of_changed: files that import a changed file (blast radius).
    let mut deps_of_changed: HashSet<String> = HashSet::new();
    let mut dependents_of_changed: HashSet<String> = HashSet::new();
    for (file, importees) in &idx.imports {
        let imports_a_changed = importees.iter().any(|d| changed.contains(d));
        if imports_a_changed {
            dependents_of_changed.insert(file.clone());
        }
        if changed.contains(file) {
            for d in importees {
                deps_of_changed.insert(d.clone());
            }
        }
    }

    // Top importance hubs (bounded) so we never read the whole repo: only the
    // 30 most-imported files compete for leftover budget.
    let mut hubs: Vec<(&str, usize)> = in_degree.iter().map(|(&f, &d)| (f, d)).collect();
    hubs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let hub_set: HashSet<&str> = hubs.iter().take(30).map(|(f, _)| *f).collect();

    // Candidate universe = changed ∪ deps ∪ dependents ∪ top hubs.
    let mut universe: HashSet<String> = HashSet::new();
    universe.extend(changed.iter().cloned());
    universe.extend(deps_of_changed.iter().cloned());
    universe.extend(dependents_of_changed.iter().cloned());
    universe.extend(hub_set.iter().map(|s| s.to_string()));

    let mut candidates: Vec<rc::Candidate> = Vec::new();
    for path in &universe {
        let is_changed = changed.contains(path);
        let imports_changed = dependents_of_changed.contains(path);
        let imported_by_changed = deps_of_changed.contains(path);
        let importance = rc::normalize(
            in_degree.get(path.as_str()).copied().unwrap_or(0),
            max_in_degree,
        );
        let Some((reason, score)) =
            rc::score_file(is_changed, imports_changed, imported_by_changed, importance)
        else {
            continue;
        };
        // Tokenize the file body; unreadable files (deleted in the diff,
        // generated, binary) are simply skipped.
        let abs = root.join(path);
        let Ok(body) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let tokens = tokenade_core::tokenizer::estimate(&body);
        candidates.push(rc::Candidate {
            path: path.clone(),
            score,
            tokens,
            reason,
        });
    }

    let plan = rc::greedy_fit(candidates, budget);
    let candidate_tokens = plan.candidate_tokens();

    // Record the budgeting as savings: the naive "include everything" cost
    // folded down to the budgeted set.
    if candidate_tokens > plan.total_tokens {
        tokenade_core::gain::record(
            "review-context",
            "review-context",
            "context-builder",
            tokenade_core::Savings::new(candidate_tokens, plan.total_tokens),
        );
    }

    if json_mode {
        let to_json = |c: &rc::Candidate| {
            serde_json::json!({
                "path": c.path,
                "reason": c.reason.tag(),
                "score": (c.score * 1000.0).round() / 1000.0,
                "tokens": c.tokens,
            })
        };
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "budget": budget,
            "base": base,
            "changed_files": changed.len(),
            "candidates": plan.included.len() + plan.dropped.len(),
            "included": plan.included.iter().map(to_json).collect::<Vec<_>>(),
            "dropped": plan.dropped.iter().map(to_json).collect::<Vec<_>>(),
            "included_tokens": plan.total_tokens,
            "candidate_tokens": candidate_tokens,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }

    let where_ = base.map_or("working tree vs HEAD".to_string(), |b| format!("{b}…HEAD"));
    println!("tokenade review-context — budget {budget} tokens");
    println!("  base: {where_}");
    println!(
        "  {} changed file(s), {} candidate(s) considered",
        changed.len(),
        plan.included.len() + plan.dropped.len()
    );
    let pct = if budget > 0 {
        (plan.total_tokens as f64 / budget as f64 * 100.0).round() as u64
    } else {
        0
    };
    println!(
        "\n  INCLUDED ({} files · {} / {} tokens · {}%)",
        plan.included.len(),
        plan.total_tokens,
        budget,
        pct
    );
    for c in &plan.included {
        println!(
            "    [{:<10}] {:<48} {:>8} tok",
            c.reason.tag(),
            c.path,
            c.tokens
        );
    }
    if !plan.dropped.is_empty() {
        let over: usize = plan.dropped.iter().map(|c| c.tokens).sum();
        println!(
            "\n  DROPPED ({} files · {} tokens over budget)",
            plan.dropped.len(),
            over
        );
        for c in plan.dropped.iter().take(10) {
            println!(
                "    [{:<10}] {:<48} {:>8} tok",
                c.reason.tag(),
                c.path,
                c.tokens
            );
        }
        if plan.dropped.len() > 10 {
            println!("    ... (+{} more)", plan.dropped.len() - 10);
        }
    }
    if candidate_tokens > plan.total_tokens {
        let saved_pct = (1.0 - plan.total_tokens as f64 / candidate_tokens as f64) * 100.0;
        println!(
            "\n  context saved: {candidate_tokens} → {} tokens ({saved_pct:.1}%)",
            plan.total_tokens
        );
    }
    0
}

/// Walk one level of each known skills directory, returning every
/// `<dir>/<skill>/SKILL.md` path that exists.
pub(crate) fn collect_skill_files() -> Vec<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(&home).join(".claude/skills"));
    }
    roots.push(root().join(".claude/skills"));

    let mut files = Vec::new();
    for dir in roots {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let skill = p.join("SKILL.md");
                if skill.is_file() {
                    files.push(skill);
                }
            }
        }
    }
    files.sort();
    files.dedup();
    files
}
