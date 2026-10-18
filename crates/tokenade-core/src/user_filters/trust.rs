//! **A PROJECT filter is code you did not write.**
//!
//! A cloned repository can carry `.tokenade/filters/*.toml`, and those filters
//! rewrite what the agent READS. Applying them without asking would turn a
//! `git clone` into an attack vector: a command's output could be doctored
//! without anyone having decided it.
//!
//! Hence the trust store, and hence the census: a rejected filter must be
//! NAMED, otherwise the user believes they are filtering and are not.

use super::*;

/// Parse + validate a single-filter TOML document.
pub fn parse_and_validate(toml_src: &str) -> Result<CompiledFilter, String> {
    let raw: UserFilter = toml::from_str(toml_src).map_err(|e| format!("TOML parse error: {e}"))?;
    raw.compile()
}

/// The directories scanned for filter files, in precedence order: a project-
/// local `.tokenade/filters/` (highest), then the global `~/.tokenade/filters/`.
pub fn filter_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // The PROJECT layer, with a store knob of its own. Without it the only
    // way to exercise this layer was to move the process working directory,
    // which is global: an earlier test helper did exactly that, and it raced a
    // tokenizer test walking a relative `tests/fixtures` into "fixture corpus
    // not found — checked 12". A text sweep cannot police that spelling (the
    // literal goes into a variable and the READ is on the variable), so the
    // race is removed instead of watched. Same mechanism the crate already
    // settled on: "a store that HAS a knob must be isolated with that knob".
    if let Some(dir) = std::env::var_os("TOKENADE_FILTER_PROJECT_DIR") {
        dirs.push(PathBuf::from(dir));
    } else if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".tokenade").join("filters"));
    }
    if let Some(home) = crate::platform::home_dir() {
        dirs.push(home.join(".tokenade").join("filters"));
    }
    dirs
}

/// Trust store for PROJECT-LOCAL filter files (`<cwd>/.tokenade/filters`).
/// A cloned repository must not get to rewrite your agent's tool outputs
/// silently — a malicious filter could FOLD AWAY security warnings. Each
/// project filter file is keyed by content hash: editing the file voids its
/// trust. The user's own `~/.tokenade/filters` are always trusted.
pub(crate) fn trust_store_path() -> Option<PathBuf> {
    crate::platform::home_dir().map(|h| h.join(".tokenade").join("trusted-filters.json"))
}

pub(crate) fn load_trust() -> std::collections::HashMap<String, u64> {
    trust_store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Content hash used as the trust key.
pub fn filter_trust_key(src: &str) -> String {
    crate::dedup_ref::hash_content(src.as_bytes())
}

/// Whether a project-local filter file (by content) has been trusted.
pub fn is_trusted(src: &str) -> bool {
    load_trust().contains_key(&filter_trust_key(src))
}

/// Record trust for a filter file's CURRENT content. Returns the key.
pub fn trust_filter(src: &str) -> std::io::Result<String> {
    let key = filter_trust_key(src);
    let mut m = load_trust();
    m.insert(key.clone(), crate::platform::now_unix_secs());
    if let Some(p) = trust_store_path() {
        crate::platform::ensure_parent_dir(&p);
        crate::paths::write_atomic(&p, &serde_json::to_string_pretty(&m).unwrap_or_default())?;
    }
    Ok(key)
}

/// Largest `*.toml` a compactor definition may be. See the refusal in
/// [`survey_filters`] for why it is checked from the metadata.
pub const MAX_FILTER_FILE_BYTES: u64 = 256 * 1024;

/// What became of one filter file on disk.
#[derive(Debug)]
pub enum FilterOutcome {
    /// Compiled and in force.
    Loaded(Box<CompiledFilter>),
    /// Project-local and not approved yet: it parsed, and it does nothing.
    Untrusted { name: String },
    /// It did not parse or did not validate. Carries the exact reason, which
    /// is the only actionable thing about it and was reaching nobody.
    Invalid { reason: String },
    /// An earlier file already claimed this `name` (project beats global).
    Shadowed { name: String },
}

/// One filter file, and what became of it.
#[derive(Debug)]
pub struct FilterFile {
    pub path: PathBuf,
    /// True for `<cwd>/.tokenade/filters`, false for `~/.tokenade/filters`.
    pub project: bool,
    pub outcome: FilterOutcome,
}

/// The SINGLE walk over [`filter_dirs`]. `load_all`, `trust-filters` and the
/// healthcheck all read this survey, so a file cannot be counted by one and
/// missed by another — which is exactly how a filter that fails to parse used
/// to disappear without a word to anybody.
pub fn survey_filters() -> Vec<FilterFile> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let global = crate::platform::home_dir().map(|h| h.join(".tokenade").join("filters"));
    for dir in filter_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
            .collect();
        paths.sort(); // deterministic order within a dir
                      // "Is this the user's own global config?" — NOT "is it somewhere
                      // under home". `filter_dirs()` yields exactly two entries, so the
                      // question is an EQUALITY, and a prefix test answered a different
                      // one: almost every repository lives under $HOME (`~/src/foo`), so a
                      // prefix test classified them all as trusted global config and ran
                      // their filters WITHOUT consent — the exact outcome the trust store
                      // exists to prevent. MEASURED: a filter shipped in a cloned repo
                      // rewrote a failing test run into "all tests passed, 0 failures".
        let project = global.as_ref().is_none_or(|g| &dir != g);
        for path in paths {
            // Size BEFORE bytes. `<cwd>/.tokenade/filters` ships inside a
            // CLONED repository and this walk runs on the hookwrap path for
            // every command, so an unbounded `read_to_string` here lets a
            // repository decide how much memory each of your commands costs.
            // Measured: a 40 MB filter file parses fine in 624 ms today, and
            // nothing stops it being 2 GB. Same shape as the archive budget
            // — refuse from the metadata, before allocating.
            //
            // The ceiling is calibrated on real files: across a 300-repository
            // corpus the largest compactor definition of any tool is 11 832
            // bytes, and ours run 150 to 6 000. 256 KiB is twenty times the
            // largest one anybody has written.
            match std::fs::metadata(&path) {
                Ok(md) if md.len() > MAX_FILTER_FILE_BYTES => {
                    out.push(FilterFile {
                        path,
                        project,
                        outcome: FilterOutcome::Invalid {
                            reason: format!(
                                "the file is {} bytes; a compactor definition is capped at {} \
                                 (the largest real one measured is under 12 KB)",
                                md.len(),
                                MAX_FILTER_FILE_BYTES
                            ),
                        },
                    });
                    continue;
                }
                Ok(_) => {}
                Err(_) => continue,
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Validity is asked FIRST, and of both layers. A broken file is
            // broken whether or not it is trusted, and "approve this, then it
            // will apply" is a false promise about a file that cannot apply.
            let compiled = match parse_and_validate(&src) {
                Ok(c) => c,
                Err(reason) => {
                    out.push(FilterFile {
                        path,
                        project,
                        outcome: FilterOutcome::Invalid { reason },
                    });
                    continue;
                }
            };
            let name = compiled.name.clone();
            let outcome = if project && !is_trusted(&src) {
                FilterOutcome::Untrusted { name }
            } else if !seen.insert(name.clone()) {
                FilterOutcome::Shadowed { name }
            } else {
                FilterOutcome::Loaded(Box::new(compiled))
            };
            out.push(FilterFile {
                path,
                project,
                outcome,
            });
        }
    }
    out
}

/// Untrusted project-local filter files under `cwd` (path, filter name).
pub fn untrusted_project_filters() -> Vec<(PathBuf, String)> {
    survey_filters()
        .into_iter()
        .filter_map(|f| match f.outcome {
            FilterOutcome::Untrusted { name } if f.project => Some((f.path, name)),
            _ => None,
        })
        .collect()
}

/// Filter files that did not parse or validate (path, reason). They apply to
/// nothing, and until this existed nothing told the user so: `load_all`
/// dropped them silently and the healthcheck only ever looked at trust.
pub fn invalid_filters() -> Vec<(PathBuf, String)> {
    survey_filters()
        .into_iter()
        .filter_map(|f| match f.outcome {
            FilterOutcome::Invalid { reason } => Some((f.path, reason)),
            _ => None,
        })
        .collect()
}

/// Load + compile every `*.toml` filter from [`filter_dirs`]. A file that
/// fails to parse/validate is SKIPPED (fail-open — a bad user filter must never
/// break tokenade), not fatal. The first filter to claim a given `name` wins,
/// so a project-local filter overrides a global one. Cached for the process.
pub fn load_all() -> &'static [CompiledFilter] {
    static CACHE: OnceLock<Vec<CompiledFilter>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out: Vec<CompiledFilter> = Vec::new();
        for f in survey_filters() {
            let path = f.path.display().to_string();
            match f.outcome {
                FilterOutcome::Loaded(c) => out.push(*c),
                // Silent skip on the hot path — the hookwrap output must stay
                // quiet — but never silent to the OPERATOR: `healthcheck` and
                // the debug log both name the file and the reason.
                FilterOutcome::Invalid { reason } => crate::debug_log::log(
                    "user-filters",
                    "invalid_filter_skipped",
                    serde_json::json!({"path": path, "reason": reason}),
                ),
                FilterOutcome::Untrusted { .. } => crate::debug_log::log(
                    "user-filters",
                    "untrusted_project_filter_skipped",
                    serde_json::json!({"path": path}),
                ),
                FilterOutcome::Shadowed { name } => crate::debug_log::log(
                    "user-filters",
                    "shadowed_filter_skipped",
                    serde_json::json!({"path": path, "name": name}),
                ),
            }
        }
        out
    })
}

/// The user filter that most SPECIFICALLY matches `command_line`. Among every
/// filter whose `match_command` matches, the one covering the LONGEST span of
/// the command wins (a precise `^npm run build\b` beats a broad `^npm`), so a
/// narrow filter is never shadowed by a broad one that loaded first. Ties on
/// span are broken by higher `priority`, then by load order (project-local
/// before global). A single match is returned unchanged — full retro-compat.
pub fn match_for_command(command_line: &str) -> Option<&'static CompiledFilter> {
    let mut best: Option<(&'static CompiledFilter, usize)> = None;
    for f in load_all() {
        let Some(m) = f.match_re.find(command_line) else {
            continue;
        };
        let span = m.end() - m.start();
        match best {
            None => best = Some((f, span)),
            Some((cur, cur_span)) => {
                if span > cur_span || (span == cur_span && f.priority > cur.priority) {
                    best = Some((f, span));
                }
            }
        }
    }
    best.map(|(f, _)| f)
}
