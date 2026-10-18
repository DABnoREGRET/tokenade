//! **The corpus, and what a capture declares.**
//!
//! A capture is a corpus file plus what it takes to replay it: its
//! route, its REAL extension (routing is done by it, and probes in
//! `.dat` reach no compactor), and the program its key names.
//! The bench reads this corpus; it does not interpret it.

/// A real capture from the corpus: the output of a real program, frozen under
/// `crates/tokenade-core/tests/fixtures/{linux,macos,windows}`.
/// Which way this capture reaches a compactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Named after a program: goes through `filter_command`.
    Command,
    /// Routed by its CONTENT and its extension: the `auto` path, the one
    /// a `tokenade read` takes.
    Format,
    /// A PAGE: the `web` path, with its own extractor.
    ///
    /// Forty-one real pages, twenty-one megabytes, and **no
    /// instrument measured them**. It is the same hole as before — the
    /// FORMAT captures were outside the bench's corpus — one door
    /// further, and on the heaviest family of the usage ranking:
    /// `html-to-markdown`, **21.5 MB still delivered, 40.8 % of the pool**, over
    /// 6,536 real calls.
    Page,
    /// A saved RESULTS page: the `search <file>` path, which
    /// queries nobody and goes through the recognised engine's extractor.
    ///
    /// Six real pages, 1.1 MB, in the repo since July and **outside
    /// every instrument**: the usage ranking could not place it, and
    /// the operator was right to refuse to rank it before costing it.
    /// Measured on 2026-09-09: 1,066,197 → 20,709 bytes. Measured through `web`,
    /// the same page returns 47,853 — that is the wrong route, the same mistake one
    /// door further.
    Serp,
}

#[derive(Debug, Clone)]
pub struct Capture {
    /// `linux/npm_build`, `macos/locale_ru_RU/df_h` — relative path, without
    /// extension. It is the key of the richness ratchet.
    pub key: String,
    /// `linux`, `macos`, `windows` — resolved UNDER a locale directory.
    pub os: String,
    /// `locale_ru_RU`, `syslocale_de`, or `EN/FR` by default.
    pub locale: String,
    /// The file name without extension: `df_h`, `ps_get_psdrive`.
    pub stem: String,
    /// The FULL file name, extension included — `auto` routing
    /// reads it, so removing it sends the capture nowhere.
    pub file_name: String,
    pub route: Route,
    pub body: String,
}

/// **The single enumerator of the captured corpus, on the bench side.**
///
/// There used to be two: `compactor_review::corpus_outputs` (which reads
/// `["linux", "macos", "windows"]`) and `per_command::walk_fixtures` (which reads
/// `["macos", "windows"]`). The two copies did NOT agree, and that is the
/// defect: `per_command`, the bench whose whole point is to measure the
/// savings per command, **never saw the linux corpus** when it was added —
/// 30 captures on the product's platform, missing from a measurement that
/// presents itself as complete. Same family as the ratchet that displayed
/// 211 lines while looking elsewhere, and as `corpus.rs` on the test side, whose
/// header already tells this story for the guards.
///
/// The TESTS corpus has its own enumerator (`tests/support/corpus.rs`):
/// it lives on the other side of the crate boundary, and linking it here
/// would require a dev-dependency cycle. The two headers
/// cite each other.
pub fn captures() -> Vec<Capture> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("tokenade-core").join("tests").join("fixtures"))
        .unwrap_or_default();
    let mut out = Vec::new();
    for (sub, route) in [
        ("linux", Route::Command),
        ("macos", Route::Command),
        ("windows", Route::Command),
        // The FORMAT captures were outside the bench's corpus, and they are
        // those of the `auto:*` compactors — `auto:json` and `auto:grep` are
        // the 1st and 3rd actionable rows of the ranking. The bench could
        // measure nothing on them: same hole as before, one door further.
        ("formats", Route::Format),
        ("pages", Route::Page),
        ("serp", Route::Serp),
    ] {
        walk_captures(&root.join(sub), &root, route, &mut out);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// The captures of a single routing path.
pub fn captures_on(route: Route) -> Vec<Capture> {
    captures()
        .into_iter()
        .filter(|c| c.route == route)
        .collect()
}
pub(crate) fn walk_captures(
    dir: &std::path::Path,
    root: &std::path::Path,
    route: Route,
    out: &mut Vec<Capture>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.is_dir() {
            walk_captures(&path, root, route, out);
            continue;
        }
        // A COMMAND capture is a `.txt`; a FORMAT capture carries
        // the extension that routes it, so we cannot filter on it.
        if route == Route::Command && path.extension().and_then(|x| x.to_str()) != Some("txt") {
            continue;
        }
        // A PAGE is an `.html`: `web` routes on the extension, and the
        // directory also holds a `fidelity_baseline.json` that is not a
        // page. Measured while writing the guard — without this filter, the sweep
        // would have passed a test baseline to the page extractor.
        if matches!(route, Route::Page | Route::Serp)
            && path.extension().and_then(|x| x.to_str()) != Some("html")
        {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // The parent directory is the OS, EXCEPT under a locale directory, where
        // the OS is one level up.
        let mut comps = path.components().rev().skip(1);
        let parent = comps
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let (os, locale) = if parent.starts_with("locale_") || parent.starts_with("syslocale_") {
            let real = comps
                .next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            (real, parent)
        } else {
            (parent, "EN/FR".to_string())
        };
        let key = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(Capture {
            key,
            os,
            locale,
            stem,
            file_name,
            route,
            body,
        });
    }
}

/// **What a fold costs to ANNOUNCE**, measured on the corpus's real folds.
///
/// Every fold announces itself — a product invariant — so folding a document costs
/// at least the banner. An `op` whose residue per call is below that
/// cannot be improved by folding more: the output would GROW.
///
/// Returned as the SHORTEST banner the product actually emits, and not
/// as a chosen constant. A chosen constant is what earlier rounds
/// named: an invented ceiling next to the one that exists. Here there was
/// none, so it is measured.
///
/// Returns `None` if the corpus produced no banner — an instrument that
/// measured nothing does not return a number.
pub fn announcement_cost() -> Option<u64> {
    observed_announcements().into_iter().min()
}

/// The lengths of ALL the banners the corpus makes the product emit.
///
/// Returned separately so that the floor is ATTESTABLE: a guard can check
/// that no observed banner is shorter than it, and that at least one
/// equals it. Without them, `announcement_cost` could return any
/// constant without a test seeing it.
pub fn observed_announcements() -> Vec<u64> {
    captures_on(Route::Command)
        .into_iter()
        .filter_map(|c| {
            tokenade_core::cmd::dispatch_named(&resolve_program(&c.stem, &c.body)?, &c.body)
        })
        .flat_map(|out| {
            out.lines()
                .filter(|l| is_an_announcement(l))
                .map(|l| l.trim().len() as u64)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A line the product wrote ITSELF to say it folded.
pub fn is_an_announcement(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("[tokenade") || t.starts_with("[disclose")
}

/// The program this capture is the output of: the longest prefix the
/// router recognises, removing one `_` segment at a time.
pub(crate) fn resolve_program(stem: &str, body: &str) -> Option<String> {
    let mut cand = stem.to_string();
    loop {
        if tokenade_core::cmd::dispatch_named(&cand, body).is_some() {
            return Some(cand);
        }
        let i = cand.rfind('_')?;
        cand.truncate(i);
        if cand.is_empty() {
            return None;
        }
    }
}

/// **The program this capture is the output of.**
///
/// The LONGEST prefix the router recognises, tried with a hyphen as
/// with an underscore: `tasklist_svc_full` is `tasklist-svc`, not
/// `tasklist`, and `ps_get_psdrive` is `ps-get`, not `ps`.
///
/// An earlier pass merged FIVE copies of this question on the test side
/// (`corpus::program_of`). On the bench side it had one more, in
/// `per_command`, which cut at the first underscore and did not try the
/// hyphen form — so the two corpora did not exercise the same compactors, and nothing
/// said so. The two headers cite each other; the crate boundary prevents
/// making them a single one, so at least they say the same thing.
/// The command that produced this capture — see
/// [`tokenade_core::corpus_key::command_of`], which holds the rule, the table of
/// ambiguous names and the refusal that names them.
pub fn command_of(key: &str) -> Result<Option<String>, String> {
    tokenade_core::corpus_key::command_of(key)
}

/// The route to exercise with this capture — see
/// [`tokenade_core::corpus_key::program_of`].
///
/// An earlier pass merged FIVE copies of this question on the test side; a sixth
/// remained here, and the two diverged in the only way that matters: the bench
/// used it to RANK, which it cannot do.
pub fn program_of(key: &str) -> Option<String> {
    tokenade_core::corpus_key::program_of(key)
}
