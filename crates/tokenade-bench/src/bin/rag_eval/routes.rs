//! **The routes measured outside code search**: passages in a long
//! document, real agent questions replayed, and questions about a
//! command's output (the store).

#[allow(unused_imports)]
use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// **The second RAG: finding a passage in a long document.**
///
/// A different use case from the first, so a different bench — and that is the point:
/// a setting that wins on code search can lose here. The
/// questions of the `docs-bench` corpus are RARE needles (`ZK-5560`), which
/// lexical search always finds: 10/10, so nothing to learn. Here the query
/// is a section's TITLE and the answer a rare word from its body — no
/// artificial needle, and the section's depth is recorded,
/// because that is what the embedding ceiling decides.
pub(crate) fn doc_rag_eval(pages: &Path, budget: usize) {
    let mut rd: Vec<PathBuf> = std::fs::read_dir(pages)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("html"))
        .collect();
    rd.sort();
    let (mut asked, mut hit, mut deep_asked, mut deep_hit, mut chars) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for p in rd {
        let Ok(html) = std::fs::read_to_string(&p) else {
            continue;
        };
        let md = tokenade_core::web_pipeline::distill_page(
            &html,
            "https://bench.test/p",
            Some("text/html"),
        )
        .md;
        // Split by heading: the same unit the product files into
        // sections.
        let mut secs: Vec<(String, String)> = Vec::new();
        let mut head = String::new();
        let mut body = String::new();
        for line in md.lines() {
            if line.starts_with("## ") || line.starts_with("### ") {
                if !head.is_empty() && body.len() > 200 {
                    secs.push((head.clone(), std::mem::take(&mut body)));
                } else {
                    body.clear();
                }
                head = line.trim_start_matches('#').trim().to_string();
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        if !head.is_empty() && body.len() > 200 {
            secs.push((head, body));
        }
        if secs.len() < 4 {
            continue;
        }
        // Word count over the whole page: the answer must be a word
        // that exists only HERE, otherwise "found" proves nothing.
        let mut freq: HashMap<String, usize> = HashMap::new();
        for w in md.split(|c: char| !c.is_alphanumeric()) {
            if w.len() >= 6 {
                *freq.entry(w.to_lowercase()).or_insert(0) += 1;
            }
        }
        for (depth, (h, b)) in secs.iter().enumerate() {
            if h.split_whitespace().count() < 2 {
                continue;
            }
            let Some(gold) = b
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() >= 6)
                .find(|w| {
                    freq.get(&w.to_lowercase()).copied().unwrap_or(0) == 1
                        && !h.to_lowercase().contains(&w.to_lowercase())
                })
            else {
                continue;
            };
            let out = tokenade_core::rag::prompt_scoped_extract_rag(&md, h, budget)
                .unwrap_or_else(|| md.clone());
            let ok = out.contains(gold);
            asked += 1;
            chars += out.len();
            if ok {
                hit += 1;
            }
            if depth >= 192 {
                deep_asked += 1;
                if ok {
                    deep_hit += 1;
                }
            }
        }
    }
    let d = asked.max(1) as f64;
    println!(
        "docrag: {hit}/{asked} sections found ({:.3}) · deep {deep_hit}/{deep_asked} · {} chars returned on average",
        hit as f64 / d,
        chars / asked.max(1)
    );
}

/// **Replay the REAL questions asked by agents.**
///
/// Extracted from the transcripts (`tokenade read <file> --prompt "<question>"`),
/// they look like none of my synthetic corpora: **none of them quotes
/// a symbol of the file**. They are concept questions — "what is the
/// retry policy", "how are help handlers registered". It is the distribution
/// the product really serves.
///
/// Without human judgement there is no ground truth here; what this mode
/// measures is therefore what a setting CHANGES: the share of questions whose
/// returned passage differs, and what it costs. A setting that changes nothing on
/// the real distribution deserves neither debate nor a line of code.
pub(crate) fn real_eval(list: &Path, repo: &Path, budget: usize) {
    #[derive(serde::Deserialize)]
    struct RealCase {
        route: String,
        target: String,
        q: String,
    }
    let Ok(raw) = std::fs::read_to_string(list) else {
        eprintln!("unreadable list of real questions: {}", list.display());
        return;
    };
    let Ok(cases) = serde_json::from_str::<Vec<RealCase>>(&raw) else {
        eprintln!(
            "the list is not `[{{route,target,q}}]` JSON: {}",
            list.display()
        );
        return;
    };
    let (mut done, mut chars, mut empty) = (0usize, 0usize, 0usize);
    let mut digests: Vec<String> = Vec::new();
    for c in &cases {
        if c.route != "read" {
            continue;
        }
        let t = c.target.trim_matches(['"', '\'']);
        let path = repo.join(t);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let out =
            tokenade_core::rag::prompt_scoped_extract_rag(&body, &c.q, budget).unwrap_or_default();
        done += 1;
        chars += out.len();
        if out.is_empty() {
            empty += 1;
        }
        // A short fingerprint of the returned passage: two configurations can be
        // compared without the bench needing to keep the texts.
        let mut h: u64 = 1469598103934665603;
        for b in out.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        digests.push(format!("{}\t{:016x}\t{}", t, h, out.len()));
        if std::env::var("RAG_SHOW").is_ok() {
            let head: String = out
                .chars()
                .take(300)
                .map(|c| if c == '\n' { ' ' } else { c })
                .collect();
            println!(
                "@{done}\t{}\t{head}",
                c.q.chars().take(50).collect::<String>()
            );
        }
    }
    println!(
        "real: {done} questions replayed · {empty} without a passage · {} chars returned on average",
        chars / done.max(1)
    );
    // The fingerprints go to standard output: two runs can be
    // compared with `diff`, which says EXACTLY which questions moved.
    for d in digests {
        println!("#{d}");
    }
}

/// **The fourth route: a question about a COMMAND'S OUTPUT.**
///
/// `expand-ref <hash> --prompt "…"` goes through `rag::answer_over_blob`. The
/// blobs live in the local store; 19 of those agents actually
/// queried are still there. This mode replays them — same bytes,
/// same questions — and says what a setting CHANGES.
pub(crate) fn stash_eval(list: &Path, budget_note: usize) {
    #[derive(serde::Deserialize)]
    struct RealCase {
        route: String,
        target: String,
        q: String,
    }
    let Ok(raw) = std::fs::read_to_string(list) else {
        eprintln!("unreadable list: {}", list.display());
        return;
    };
    let Ok(cases) = serde_json::from_str::<Vec<RealCase>>(&raw) else {
        eprintln!("list is not JSON: {}", list.display());
        return;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let stash = PathBuf::from(home).join(".cache/tokenade/stash");
    let files: Vec<String> = std::fs::read_dir(&stash)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let (mut done, mut kept, mut whole) = (0usize, 0usize, 0usize);
    // **What the question NAMES must be found in the answer.**
    // A literal token from the question (`updatedToolOutput`, `access-by-path`)
    // that exists in the blob is a requirement checkable without a judge: if it
    // is missing from the returned passage, the retriever cut the answer.
    let (mut named, mut named_kept) = (0usize, 0usize);
    for c in &cases {
        if !c.route.starts_with("expand") {
            continue;
        }
        let h = c.target.trim_matches(['"', '\'']);
        let Some(f) = files.iter().find(|f| f.starts_with(h)) else {
            continue;
        };
        let Ok(blob) = std::fs::read_to_string(stash.join(f)) else {
            continue;
        };
        let toks = tokenade_core::tokenizer::estimate(&blob);
        // `--force`: search EVEN under the fold threshold. The threshold says "this
        // blob is short enough to be served as is"; but the agent
        // ASKED a question, and the documentation promises that `--prompt` is
        // the cheapest option in tokens spent.
        let force = std::env::var("RAG_FORCE_SCOPE").is_ok();
        let out = if force {
            tokenade_core::rag::prompt_scoped_extract_dispatch(
                &blob,
                &c.q,
                tokenade_core::disclose::webfetch_scoped_budget_for(toks),
            )
            .unwrap_or_default()
        } else {
            tokenade_core::rag::answer_over_blob(&blob, &c.q).unwrap_or_default()
        };
        done += 1;
        let _ = toks;
        kept += out.len();
        // Returning the WHOLE blob is not an answer: it is the admission that the
        // retriever did not search.
        if out.len() >= blob.len() {
            whole += 1;
        }
        let wanted: Vec<String> = c
            .q
            .split(|ch: char| ch.is_whitespace() || ",;:?!()'\"".contains(ch))
            .map(|w| w.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-'))
            .filter(|w| {
                w.len() >= 6
                    && (w.contains('_') || w.contains('-') || w.chars().any(|ch| ch.is_uppercase()))
                    && blob.contains(w)
            })
            .map(|w| w.to_string())
            .collect();
        for w in &wanted {
            named += 1;
            if out.contains(w.as_str()) {
                named_kept += 1;
            }
        }
        let mut hsh: u64 = 1469598103934665603;
        for b in out.as_bytes() {
            hsh ^= *b as u64;
            hsh = hsh.wrapping_mul(1099511628211);
        }
        println!(
            "#{h}\t{:016x}\t{}\t{}\t{toks} tokens",
            hsh,
            out.len(),
            blob.len()
        );
        if std::env::var("RAG_SHOW").is_ok() && out.len() < blob.len() {
            println!(
                "Q<{}>\n{}\n---8<---",
                c.q,
                out.chars().take(900).collect::<String>()
            );
        }
    }
    let _ = budget_note;
    println!(
        "stash: {done} blobs replayed · {whole} returned WHOLE · {} chars kept on average · \
         named tokens found {named_kept}/{named}",
        kept / done.max(1)
    );
}

/// **The WEB route, on the pages agents actually read.**
///
/// The questions come from the transcripts (`tokenade web <url> --prompt`), the
/// pages are fetched once and cached by URL fingerprint —
/// a page that has changed since remains a real page, which is the point; what
/// is measured is SEARCH, not the stability of the web.
///
/// The same judge-free measure as for the store: a literal token named by the
/// question and present in the page must survive into the returned passage.
pub(crate) fn web_eval(list: &Path, cache: &Path, budget: usize) {
    #[derive(serde::Deserialize)]
    struct RealCase {
        route: String,
        target: String,
        q: String,
    }
    let Ok(raw) = std::fs::read_to_string(list) else {
        return;
    };
    let Ok(cases) = serde_json::from_str::<Vec<RealCase>>(&raw) else {
        return;
    };
    let (mut done, mut kept, mut whole, mut named, mut named_kept, mut src) =
        (0, 0usize, 0, 0, 0, 0usize);
    for c in &cases {
        if c.route != "web" {
            continue;
        }
        let url = c.target.trim_matches(['"', '\'']);
        let h = tokenade_core::dedup_ref::hash_content(url.as_bytes());
        let mut f = cache.join(format!("{}.html", &h[..16.min(h.len())]));
        if !f.exists() {
            // The bench's fingerprint is `sha256sum`'s, not ours: we
            // fall back to a search by content.
            let Some(alt) = std::fs::read_dir(cache).ok().and_then(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .find(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains(url)))
            }) else {
                continue;
            };
            f = alt;
        }
        let Ok(html) = std::fs::read_to_string(&f) else {
            continue;
        };
        if html.len() < 2000 {
            continue;
        }
        // The question is asked of the PAGE, not of the cut: `md` is the view
        // after the hard ceiling, `full_md` the markdown before it. The delivered
        // route makes this choice; the bench must measure the same.
        let d = tokenade_core::web_pipeline::distill_page(&html, url, Some("text/html"));
        let md = d.full_md.clone().unwrap_or(d.md);
        let scoped = tokenade_core::rag::prompt_scoped_extract_dispatch(&md, &c.q, budget);
        let fell_back = scoped.is_none();
        let out = scoped.unwrap_or_else(|| md.clone());
        done += 1;
        if fell_back {
            let in_md =
                c.q.split_whitespace()
                    .filter(|w| w.len() >= 8)
                    .map(|w| (w, md.contains(w)))
                    .collect::<Vec<_>>();
            println!(
                "FALLBACK\t{}\t{}\tmd={} chars\twords in the md: {:?}",
                &url[..url.len().min(52)],
                c.q.chars().take(40).collect::<String>(),
                md.len(),
                in_md
            );
        }
        kept += out.len();
        src += md.len();
        if out.len() >= md.len() {
            whole += 1;
        }
        for w in
            c.q.split(|ch: char| ch.is_whitespace() || ",;:?!()'\"".contains(ch))
        {
            let w = w.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-');
            if w.len() >= 6
                && (w.contains('_') || w.contains('-') || w.chars().any(|ch| ch.is_uppercase()))
                && md.contains(w)
            {
                named += 1;
                if out.contains(w) {
                    named_kept += 1;
                }
            }
        }
    }
    println!(
        "web: {done} pages · {whole} returned WHOLE · {} chars kept out of {} · named tokens {named_kept}/{named}",
        kept / done.max(1),
        src / done.max(1)
    );
}
