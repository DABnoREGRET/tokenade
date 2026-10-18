//! Compacting something already compacted must not keep eating it.
//!
//! This happens in the field: a hook folds a tool result, the agent hands the
//! folded text to `tokenade read`, an MCP result passes the proxy and then the
//! auto compactor. Every compactor therefore sees its own output sooner or
//! later. Determinism (`determinism.rs`) says the same input gives the same
//! output; it says nothing about what a SECOND pass does, and the anti-inflation
//! guard only watches for growth — a pass that quietly removes more content
//! each time it runs is invisible to both.
//!
//! The property is a fixed point: `compact(compact(x)) == compact(x)`.

use std::path::PathBuf;

fn corpus(dir: &str, ext: &str) -> Vec<(String, String)> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir);
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == ext).unwrap_or(false))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| {
            let body = std::fs::read_to_string(&p).ok()?;
            if body.trim().is_empty() {
                return None;
            }
            Some((p.file_stem()?.to_string_lossy().to_string(), body))
        })
        .collect()
}

/// Every WORD the first pass kept must survive the second.
///
/// Not byte equality: a second pass legitimately re-collapses wrappers, so the
/// indentation shifts. What may never happen is content the agent was shown
/// once disappearing when the same text goes through tokenade twice, which the
/// hook-then-CLI path does.
///
/// It asked about NAMES until 2026-08-27, and that was a proxy for content
/// which fires on a correct fold. `fold_redundant_names` exists to un-name a
/// container whose name is the concatenation of its children's text; when a
/// sibling is removed upstream, a container that was not quite redundant
/// becomes exactly redundant, its aggregate name goes, and every word of it is
/// still on the children. Measured on `hn-front`: the cell named
/// "Consider applying … Guidelines | FAQ | … | Contact Search:" lost its name
/// and `Guidelines`, `Apply to YC`, `Contact` and `Consider applying` were
/// each still present, once, as links.
///
/// That false alarm blocked a real improvement for an iteration, so the
/// invariant now asks the question it meant:
/// no word may leave the output. A genuine loss takes its words with it.
#[test]
fn a_second_a11y_pass_loses_no_content() {
    fn names(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .lines()
            .filter_map(|l| {
                let start = l.find('"')?;
                let mut escaped = false;
                for (i, c) in l.char_indices().skip_while(|(i, _)| *i <= start) {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    match c {
                        '\\' => escaped = true,
                        '"' => return Some(l[start + 1..i].to_string()),
                        _ => {}
                    }
                }
                None
            })
            .collect();
        v.sort();
        v
    }
    let fx = corpus("a11y", "txt");
    assert!(!fx.is_empty(), "a11y corpus moved");
    // Words of two characters or more, lowercased. Punctuation and one-letter
    // tokens carry no content and would only make the assertion noisy.
    fn words(names: &[String]) -> std::collections::BTreeSet<String> {
        names
            .iter()
            .flat_map(|n| n.split(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.chars().count() >= 2)
            .map(str::to_lowercase)
            .collect()
    }
    for (stem, body) in fx {
        let once = tokenade_core::web::compact_a11y_tree(&body);
        let twice = tokenade_core::web::compact_a11y_tree(&once);
        let (a, b) = (words(&names(&once)), words(&names(&twice)));
        let lost: Vec<&String> = a.difference(&b).collect();
        assert!(
            lost.is_empty(),
            "{stem}: {} word(s) vanished on a second pass, e.g. {:?}",
            lost.len(),
            &lost[..lost.len().min(5)]
        );
    }
}

/// The invariant above notices a word that leaves.
///
/// It compares pass 1 against pass 2, so a loss that happens IDENTICALLY in
/// both is invisible to it by construction — mutating the compactor to drop
/// every `link` leaves both sides equally poorer and the assertion green.
/// That says nothing about whether the assertion works, so this feeds it a
/// second side that really did lose something.
#[test]
fn the_content_invariant_notices_a_word_that_leaves() {
    let fx = corpus("a11y", "txt");
    let (_stem, body) = fx.into_iter().next().expect("a11y corpus moved");
    let once = tokenade_core::web::compact_a11y_tree(&body);
    // A "second pass" that dropped one named line.
    let victim = once
        .lines()
        .find(|l| l.contains('"') && l.trim_start().starts_with("- link"))
        .expect("no named link in the fixture")
        .to_string();
    let lossy: String = once
        .lines()
        .filter(|l| *l != victim.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    fn words(s: &str) -> std::collections::BTreeSet<String> {
        s.lines()
            .filter_map(|l| {
                let start = l.find('"')?;
                let rest = &l[start + 1..];
                let end = rest.find('"')?;
                Some(rest[..end].to_string())
            })
            .flat_map(|n| {
                n.split(|c: char| !c.is_alphanumeric())
                    .filter(|w| w.chars().count() >= 2)
                    .map(str::to_lowercase)
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    let (a, b) = (words(&once), words(&lossy));
    let lost: Vec<&String> = a.difference(&b).collect();
    assert!(
        !lost.is_empty(),
        "the invariant cannot see a deleted named line — it would pass on real loss"
    );
}

/// A command compactor must reach a fixed point: the fixtures that do not are
/// listed here so a new one cannot appear unnoticed.
#[test]
fn command_compaction_reaches_a_fixed_point() {
    // These differ by a line-count banner that re-counts itself, which is a
    // wart, not a loss. Anything NEW joining them needs a look.
    const KNOWN: &[&str] = &[
        "whoami_groups",
        "whoami_all",
        "ipconfig_display",
        "wmic_os_full",
        "wmic_logicaldisk",
        "wmic_cpu",
        "wmic_bios",
        "launchctl_list_full",
    ];
    let mut unstable = Vec::new();
    let mut checked = 0usize;
    for dir in ["macos", "windows"] {
        for (stem, body) in corpus(dir, "txt") {
            checked += 1;
            let (_, once, _) = tokenade_core::auto::compact(&body);
            let (_, twice, _) = tokenade_core::auto::compact(&once);
            if once != twice && !KNOWN.contains(&stem.as_str()) {
                unstable.push(stem);
            }
        }
    }
    assert!(
        checked > 100,
        "only {checked} fixtures seen — corpus path moved?"
    );
    assert!(
        unstable.is_empty(),
        "compacting an already-compacted output changed it again: {unstable:?}"
    );
}

/// Markdown handed back to tokenade must still be read as markdown.
///
/// The realistic path: `tokenade web <url> > page.md`, then anything that puts
/// those bytes through the Bash compactor (`cat page.md`, a heredoc, an MCP
/// result). Markdown's own detector is LAST in the chain, so a document that
/// also resembles a makefile or an INI file used to lose to the first claimant,
/// which then rebuilt it from its own parse — MEASURED: 186 headings out of 186
/// gone on a real page, at a 7 % "gain" the anti-inflation guard cannot see.
#[test]
fn delivered_web_markdown_survives_the_command_compactor() {
    let pages = corpus("pages", "html");
    assert!(
        pages.len() > 20,
        "page corpus moved ({} found)",
        pages.len()
    );
    let heads = |s: &str| {
        s.lines()
            .filter(|l| {
                let t = l.trim_start();
                let h = t.chars().take_while(|c| *c == '#').count();
                (1..=6).contains(&h) && t[h..].starts_with(' ')
            })
            .count()
    };
    let mut damaged = Vec::new();
    for (stem, html) in &pages {
        let md = tokenade_core::web::html_to_markdown(html);
        let delivered = tokenade_core::markdown::compact(&md);
        let before = heads(&delivered);
        if before < 3 {
            continue; // nothing structural to lose
        }
        let (_, again, _) = tokenade_core::auto::compact(&delivered);
        let after = heads(&again);
        // Losing a few to deduplication is fine; losing the outline is not.
        if after * 2 < before {
            damaged.push(format!("{stem} {before}->{after}"));
        }
    }
    assert!(
        damaged.is_empty(),
        "the document outline was destroyed by a second pass: {damaged:?}"
    );
}

/// `cmd::dispatch` may not be used as a fallback until it can say no.
///
/// It is the content-shape ladder over every per-command compactor and its own
/// module documents it as "used when the program name is unknown" — but nothing
/// calls it, and the hole is worth 4.1 % against 34.3 % on this corpus. The
/// obvious fix is to plug it in under `Kind::Unknown`. Do not: the ladder takes
/// the FIRST compactor that returns `Some`, and none of them proves it
/// understood the input, so a wrong parser rebuilds the output from the little
/// it could read. The "gain" that produces is destruction — 4.1 % becomes
/// 74.7 % while `top` keeps 6 % of its words instead of all of them.
///
/// This test states the defect so it stays visible: when a module learns to
/// recognise its own input, its fixture leaves the list, and when the list is
/// empty the fallback can be wired.
#[test]
fn a_content_shape_fallback_would_destroy_content() {
    fn words(s: &str) -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric() && c != '.' && c != '_' && c != '/' && c != '-')
            .filter(|w| w.len() >= 4)
            .map(|w| w.to_lowercase())
            .collect()
    }
    let mut mangled = Vec::new();
    let mut checked = 0usize;
    for dir in ["macos", "windows"] {
        for (stem, body) in corpus(dir, "txt") {
            let Some(by_shape) = tokenade_core::cmd::dispatch(&body) else {
                continue;
            };
            let prog = stem.split('_').next().unwrap_or("").to_string();
            if !tokenade_core::filter::knows_program(&prog) {
                continue;
            }
            let by_name = tokenade_core::filter::filter_command(&[prog], &body).output;
            let all = words(&body);
            if all.len() < 40 {
                continue;
            }
            checked += 1;
            let keep = |t: &str| {
                let w = words(t);
                all.iter().filter(|x| w.contains(*x)).count() as f64 / all.len() as f64
            };
            let (shape, named) = (keep(&by_shape), keep(&by_name));
            // Half the words the right compactor kept is not a compaction
            // difference, it is a different document.
            if shape * 2.0 < named {
                mangled.push(format!(
                    "{stem} {:.0}% vs {:.0}%",
                    shape * 100.0,
                    named * 100.0
                ));
            }
        }
    }
    assert!(checked > 20, "only {checked} fixtures reached the ladder");
    assert!(
        !mangled.is_empty(),
        "cmd::dispatch now agrees with name-routed compaction on every fixture \
         ({checked} checked) — the reason it is not wired into auto::compact is \
         gone, so wire it and delete this test"
    );
}

/// A code sample must arrive labelled, not preceded by a bare word.
///
/// Documentation sites print the language as a visible chip beside the copy
/// button, which converts to a paragraph holding one word and then an
/// unlabelled fence. The agent gets a line that says nothing and a code block
/// whose language it must guess — on the kind of reference page it reads most.
#[test]
fn a_code_sample_keeps_the_language_the_page_showed() {
    const LANGS: &[&str] = &[
        "js",
        "javascript",
        "ts",
        "typescript",
        "python",
        "py",
        "bash",
        "sh",
        "shell",
        "html",
        "css",
        "json",
        "yaml",
        "sql",
        "rust",
        "go",
        "java",
        "ruby",
        "php",
        "xml",
        "toml",
    ];
    let mut orphans = Vec::new();
    for (stem, html) in corpus("pages", "html") {
        let md = tokenade_core::web::html_to_markdown(&html);
        let out = tokenade_core::markdown::compact(&md);
        let lines: Vec<&str> = out.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.trim() != "```" {
                continue;
            }
            let mut j = i;
            while j > 0 && lines[j - 1].trim().is_empty() {
                j -= 1;
            }
            if j == 0 || j == i {
                continue;
            }
            if LANGS.contains(&lines[j - 1].trim().to_ascii_lowercase().as_str()) {
                orphans.push(format!("{stem}:{}", j));
            }
        }
    }
    assert!(
        orphans.is_empty(),
        "a language label was delivered as its own paragraph instead of on the fence: {orphans:?}"
    );
}
