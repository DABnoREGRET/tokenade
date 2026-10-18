//! Diagnostic: list declarations the strict ground-truth regex finds in a
//! given repo that the production index does NOT find — pinpoints the source
//! of the `adv:index-recall` gap so we can fix the per-language regex specs.

use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let skip = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "vendor",
        ".venv",
        "venv",
        "__pycache__",
        "benchmarks",
        "competitors",
    ];
    let Ok(es) = fs::read_dir(dir) else { return };
    for e in es.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !skip.contains(&n.as_str()) && !n.starts_with('.') {
                walk(&p, out);
            }
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            if ["rs", "py", "js", "ts", "go"].contains(&ext) {
                out.push(p);
            }
        }
    }
}

fn main() {
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    let repo = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "benchmarks/repos/ruff".into());
    let repo = PathBuf::from(repo);
    let strict =
        Regex::new(r"(?m)^(?:pub )?(?:async )?(?:fn|def|class)\s+([A-Za-z_][A-Za-z0-9_]*)")
            .unwrap();

    let mut files = Vec::new();
    walk(&repo, &mut files);

    let mut truth: HashSet<(String, String)> = HashSet::new();
    for f in &files {
        let Ok(c) = fs::read_to_string(f) else {
            continue;
        };
        if c.len() > 2_000_000 {
            continue;
        }
        let rel = f.strip_prefix(&repo).unwrap().display().to_string();
        for cap in strict.captures_iter(&c) {
            truth.insert((cap.get(1).unwrap().as_str().to_string(), rel.clone()));
        }
    }

    let idx = tokenade_core::index::Index::build(&repo).unwrap();
    let found: HashSet<String> = idx.symbols.iter().map(|s| s.name.clone()).collect();

    let unique_truth: HashSet<&String> = truth.iter().map(|(n, _)| n).collect();
    let missed: Vec<&(String, String)> = truth.iter().filter(|(n, _)| !found.contains(n)).collect();

    println!("repo: {}", repo.display());
    println!("strict unique names: {}", unique_truth.len());
    println!("index symbol names:  {}", found.len());
    println!("missed by index: {} occurrences", missed.len());
    println!("sample of misses (up to 30):");
    let mut shown: HashSet<&str> = HashSet::new();
    for (n, f) in missed.iter() {
        if shown.insert(n.as_str()) {
            println!("  {n}  ({f})");
            if shown.len() >= 30 {
                break;
            }
        }
    }
}
