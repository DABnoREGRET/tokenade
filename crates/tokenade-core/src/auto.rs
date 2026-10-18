//! Auto-detect format from text content and dispatch to the right compactor.
//!
//! An agent doesn't always know whether the blob it just received is a
//! Cloudflare challenge page, a `git diff`, a Jupyter notebook, or
//! `terraform plan` output. This module looks at the **first few hundred
//! bytes** + a handful of cheap signals and routes the input to the most
//! appropriate per-format compactor, returning both the result and the
//! kind it detected.
//!
//! Detection rules are ordered from most-specific to least-specific so that
//! a real notebook (which is also valid JSON) routes to `notebook`, not
//! `json`; a `terraform plan` (which contains hash/equality lines that could
//! look like YAML) routes to `terraform`, not `yaml`.

mod detectors;
mod guards;
mod hints;
use crate::{
    accesslog, ansible, archive, cargobench, containerlogs, csv as tokenade_csv, diff, disk, dns,
    email, env as tokenade_env, filelist, ghactions, gitlog, graphql, har, icalendar, journal,
    json as tokenade_json, jvmbuild, k8smanifest, kubectl, lockfile, logs, markdown, metrics,
    ndjson, netstat, notebook, openapi, pkginstall, searchout, serp, sourcemap, sql, stacktrace,
    stealth, strace, systemd, tabular, tcpdump, terraform, tokenizer, vcard, vmstat, web,
    yaml as tokenade_yaml, Savings,
};

use detectors::*;
pub(crate) use guards::*;
pub use hints::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Html,
    A11yTree,
    JupyterNotebook,
    Json,
    Yaml,
    UnifiedDiff,
    TerraformPlan,
    CurlVerbose,
    StackTrace,
    ArchiveListing,
    TabularCli,
    DashTable,
    Csv,
    Tsv,
    Logs,
    Challenge,
    EnvDump,
    Markdown,
    Lockfile,
    Minified,
    Base64Blob,
    SqlResult,
    PrometheusMetrics,
    AccessLog,
    Journal,
    SourceMap,
    OpenApi,
    Har,
    Email,
    Calendar,
    Contacts,
    GraphQl,
    Strace,
    JvmBuild,
    KubectlDescribe,
    Ansible,
    GhActions,
    PkgInstall,
    K8sManifest,
    GitLog,
    Dns,
    Ndjson,
    ClaudeTranscript,
    TomlConfig,
    Dockerfile,
    Ini,
    Makefile,
    CMake,
    Bazel,
    Hcl,
    Proto,
    Xml,
    JUnit,
    LaTeX,
    BibTeX,
    SqlSource,
    /// Blank-line-separated `key : value` records — `pacman -Qi`, `apt show`,
    /// `dpkg -s`. The COMMAND path already folds these through
    /// `archetypes::kv_records`; this is the same fold for the same bytes
    /// arriving as CONTENT.
    KvBlocks,
    /// An ALIGNED `label : value` block, recognised without a word of language.
    ///
    /// The last rung of the ladder, right before `Unknown`: it only claims
    /// what nobody wanted. It is the shape of the system tools
    /// whose named compactor declined the output — because it did not
    /// read that language — and which used to come all the way here whole.
    AlignedBlock,
    Systemd,
    FileList,
    Disk,
    SearchOut,
    Netstat,
    CargoBench,
    ContainerLogs,
    Vmstat,
    Tcpdump,
    Serp,
    /// TypeScript compiler (`tsc`) diagnostics — per-error list.
    Tsc,
    /// Nothing matched; pass through unchanged.
    /// L77 — Multi-section Bash output (`=== A ===`, `--- B ---`,
    /// `### C` markers). Compactor splits + per-section dispatch.
    CommandChain,
    CargoBuild,
    /// A bundler's per-file asset table (`vite build`, `slidev build`).
    BundleAssets,
    /// A GoogleTest binary's run (`[ RUN      ]` / `[  FAILED  ]`).
    GTest,
    /// Test Anything Protocol (`node --test`, `tsx --test`), whatever ran it.
    Tap,
    Unknown,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Html => "html",
            Kind::A11yTree => "a11y-snapshot",
            Kind::JupyterNotebook => "notebook",
            Kind::Json => "json",
            Kind::Yaml => "yaml",
            Kind::UnifiedDiff => "diff",
            Kind::TerraformPlan => "terraform",
            Kind::CurlVerbose => "curl",
            Kind::StackTrace => "stacktrace",
            Kind::ArchiveListing => "archive",
            Kind::TabularCli => "tabular",
            Kind::DashTable => "dash-table",
            Kind::Csv => "csv",
            Kind::Tsv => "tsv",
            Kind::Logs => "logs",
            Kind::Challenge => "challenge",
            Kind::EnvDump => "env",
            Kind::Markdown => "markdown",
            Kind::Lockfile => "lockfile",
            Kind::Minified => "minified",
            Kind::Base64Blob => "base64",
            Kind::SqlResult => "sql",
            Kind::PrometheusMetrics => "metrics",
            Kind::AccessLog => "accesslog",
            Kind::Journal => "journal",
            Kind::SourceMap => "sourcemap",
            Kind::OpenApi => "openapi",
            Kind::Har => "har",
            Kind::Email => "email",
            Kind::Calendar => "calendar",
            Kind::Contacts => "contacts",
            Kind::GraphQl => "graphql",
            Kind::Strace => "strace",
            Kind::JvmBuild => "jvmbuild",
            Kind::KubectlDescribe => "kubectl-describe",
            Kind::Ansible => "ansible",
            Kind::GhActions => "ghactions",
            Kind::PkgInstall => "pkginstall",
            Kind::K8sManifest => "k8s-manifest",
            Kind::GitLog => "git-log",
            Kind::Dns => "dns",
            Kind::Ndjson => "ndjson",
            Kind::ClaudeTranscript => "claude-transcript",
            Kind::TomlConfig => "toml",
            Kind::Dockerfile => "dockerfile",
            Kind::Ini => "ini",
            Kind::Makefile => "makefile",
            Kind::CMake => "cmake",
            Kind::Bazel => "bazel",
            Kind::Hcl => "hcl",
            Kind::Proto => "proto",
            Kind::Xml => "xml",
            Kind::JUnit => "junit",
            Kind::LaTeX => "latex",
            Kind::BibTeX => "bibtex",
            Kind::SqlSource => "sql-source",
            Kind::KvBlocks => "kv-blocks",
            Kind::AlignedBlock => "aligned-block",
            Kind::Systemd => "systemd",
            Kind::FileList => "filelist",
            Kind::Disk => "disk",
            Kind::SearchOut => "grep",
            Kind::Netstat => "netstat",
            Kind::CargoBench => "cargo-bench",
            Kind::ContainerLogs => "container-logs",
            Kind::Vmstat => "vmstat",
            Kind::Tcpdump => "tcpdump",
            Kind::Serp => "serp",
            Kind::Tsc => "tsc",
            Kind::CommandChain => "command-chain",
            Kind::CargoBuild => "cargo-build",
            Kind::BundleAssets => "bundle-assets",
            Kind::GTest => "gtest",
            Kind::Tap => "tap",
            Kind::Unknown => "unknown",
        }
    }
}

/// Inspect `text` and return the most specific [`Kind`] we recognise.
pub fn detect(text: &str) -> Kind {
    let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);
    let trimmed_head = head.trim_start();

    // The body is split into four contiguous family helpers, each returning
    // `Option<Kind>` for a run of the original detectors IN ORIGINAL ORDER.
    // Concatenating the four (A→B→C→D) reproduces the original sequence
    // exactly — detector ORDER is semantic, so these are pure relocations.
    // Unmistakable markdown is claimed here, ahead of everything weaker.
    // Markdown's own detector sits at the END of the chain because its markers
    // (`#`, `- `) are individually weak — which means a real document that also
    // happens to look like something else loses to the first claimant, and the
    // claimant then rebuilds the document from its own parse and throws the
    // rest away. Not hypothetical: MEASURED on the page corpus, a Go package
    // index delivered as markdown was read as a Makefile (`name: description`
    // lines) and came back with **186 of its 186 headings gone**; an MDN page
    // was read as INI and lost 15 headings and 10 of its 11 links. The 2026-07
    // paper-labelled-`api.rest.chain` incident is the same failure one detector
    // over. The bar is high on purpose — see [`looks_strongly_like_markdown`].
    let kind = if detectors::unmistakable_unified_diff(trimmed_head) {
        Kind::UnifiedDiff
    } else if looks_strongly_like_markdown(text) {
        Kind::Markdown
    } else {
        detect_web_and_early_formats(text, head, trimmed_head)
            .or_else(|| detect_data_and_cli(text, head, trimmed_head))
            .or_else(|| detect_tabular_and_markup(text, head, trimmed_head))
            .or_else(|| detect_structured_and_logs(text, head))
            .unwrap_or_else(|| {
                // Last rung: nobody claimed these bytes. If it is
                // an aligned block, its padding is decoration and folds
                // in any language.
                // A wall of base64 arrives HERE: nobody claims it, and
                // that is exactly why it was delivered WHOLE. MEASURED
                // 2026-09-08: the base64 of a 48 632-byte `tar` came back
                // at 48 705 through `tokenade read` — bigger than the input.
                if crate::base64_blob::looks_like_a_base64_blob(text) {
                    Kind::Base64Blob
                } else if crate::archetypes::padding_is_decorative(text) {
                    Kind::AlignedBlock
                } else {
                    Kind::Unknown
                }
            })
    };
    // Mixed-content veto (field incident 2026-07): a multi-section script
    // output (`echo "=== X ==="; cmd1; echo "=== Y ==="; cmd2`) can contain
    // ONE section in a specialized format; the single-format compactors
    // summarize what they parse and DROP everything else, destroying the
    // other sections. When the doc carries several section markers, fall
    // back to the generic pipeline instead of a rebuild-from-parse compactor.
    // Marker = a line `=== … ===` with exactly three `=` on each side — the
    // ubiquitous shell-script convention; pytest's `========== session
    // starts ==========` banners use long runs and are NOT matched, so test
    // output keeps its dedicated route.
    if kind_rebuilds_from_parse(kind) && section_marker_lines(text) >= 2 {
        return Kind::Unknown;
    }
    kind
}

/// Compact the UNRECOGNISED remainder a passthrough compactor kept (the
/// lines outside its own format). "Keep verbatim" protects the content, but
/// a large mixed remainder still deserves compression — so run ONE more
/// detection pass over it (an access log's leftover may be a stack trace, a
/// pip log's leftover may be pytest output). Depth-guarded: a remainder's
/// own remainder falls back to the generic log grouper, never recurses.
pub fn compact_remainder(text: &str) -> String {
    thread_local! {
        static DEPTH: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    }
    let d = DEPTH.with(|c| c.get());
    if d >= 1 || text.len() < 512 {
        // Small remainders aren't worth a banner; deep ones use the generic
        // grouper (lossless-ish fold of repeated shapes).
        return if text.len() < 512 {
            text.to_string()
        } else {
            crate::logs::compact_generic_logs(text)
        };
    }
    DEPTH.with(|c| c.set(d + 1));
    let (_kind, body, _sav) = compact(text);
    DEPTH.with(|c| c.set(d));
    body
}

pub fn compact_with_path_hint(text: &str, file_path: &str) -> (Kind, String, Savings) {
    let text = &*normalise_redraws(text);
    let hinted = kind_from_path_hint(file_path).map(refine_hint_by_content(text));
    if let Some(kind) = hinted {
        let before = tokenizer::estimate(text);
        let out = dispatch_kind(text, kind);
        let after = tokenizer::estimate(&out);
        return guarded(kind, text, out, before, after);
    }
    // #570 — source code files never auto-compact. Skip the 30-detector
    // chain entirely and return passthrough.
    if is_source_code_extension(file_path) {
        let n = tokenizer::estimate(text);
        return (Kind::Unknown, text.to_string(), Savings::new(n, n));
    }
    compact(text)
}

/// L60 — same as [`compact_with_path_hint`] but for `Kind::Logs`
/// also persists fold records keyed by `content_hash` so the agent
/// can drill back into specific lines via `expand_fold`. For other
/// kinds, identical behaviour. `content_hash` should be the 12-char
/// hash `dedup_ref::hash_content` produces over the raw bytes.
pub fn compact_with_path_hint_indexed(
    text: &str,
    file_path: &str,
    content_hash: &str,
) -> (Kind, String, Savings) {
    let text = &*normalise_redraws(text);
    // THE SAME REFINEMENT the other hint door applies.
    //
    // Two findings taught `compact_with_path_hint` that a hint must not hide
    // a specialisation of itself — `.log` over `GitLog`, `.json` over
    // `OpenApi` — and this variant, which is the one the READ HOOK calls, did
    // not get it. One rule, two entry points, applied at one: the shape this
    // journal keeps finding, introduced here by the two entries that were
    // fixing it elsewhere.
    //
    // Measured: an OpenAPI spec named `openapi.json` came back 942 → 837
    // through this door and 942 → **627** through the other. A `git log`
    // saved as `run.log` reached the generic Logs compactor here and its own
    // there.
    //
    // The cost is named rather than hidden: a `.log` refined to a specific
    // kind no longer takes the indexed-records path below, so
    // `expand-ref --grep` loses per-record granularity on it. That is the
    // trade `auto::compact` already makes for the same bytes when there is no
    // filename — it dispatches by kind and indexes nothing — so refining here
    // makes the three doors agree instead of leaving this one alone.
    let hinted = kind_from_path_hint(file_path).map(refine_hint_by_content(text));
    let kind = match hinted {
        Some(k) => k,
        None => {
            // #570 — source code files never auto-compact. Mirror the
            // hard-reject from `compact_with_path_hint` so the Read
            // pretool path (which calls THIS variant) is protected too.
            if is_source_code_extension(file_path) {
                let n = tokenizer::estimate(text);
                return (Kind::Unknown, text.to_string(), Savings::new(n, n));
            }
            return compact(text);
        }
    };
    let before = tokenizer::estimate(text);
    let out = if matches!(kind, Kind::Logs) {
        let (rendered, records) = logs::compact_generic_logs_indexed(text, content_hash);
        for rec in &records {
            let _ = crate::fold_store::save_fold(content_hash, rec);
        }
        rendered
    } else {
        dispatch_kind(text, kind)
    };
    let after = tokenizer::estimate(&out);
    guarded(kind, text, out, before, after)
}

/// Shared dispatch table. Used by both [`compact`] (after content
/// detection) and [`compact_with_path_hint`] (when the extension
/// pinned the kind). Extracted so the two entry points can't drift.
fn dispatch_kind(text: &str, kind: Kind) -> String {
    match kind {
        Kind::Html => web::html_to_markdown(text),
        Kind::A11yTree => web::compact_a11y_tree(text),
        Kind::JupyterNotebook => notebook::compact_ipynb(text),
        Kind::Json => tokenade_json::compact(text),
        // A `.yaml` that IS a set of Kubernetes manifests gets the manifest
        // reader, which names every resource. Routed by the extension to the
        // generic YAML pass, 300 Deployments and one ConfigMap saved nothing
        // and were folded head-and-tail: the ConfigMap, in the middle, was in
        // no line of the answer (2026-10-03). Same bytes, same answer, with
        // or without the path. Only when the YAML pass does not FIT: one
        // manifest (`kubectl get pod -o yaml`) keeps its whole spec.
        Kind::Yaml => {
            let y = tokenade_yaml::compact(text);
            if k8smanifest::detect(text) && !tk_base::delivery::fits(&y) {
                k8smanifest::compact(text)
            } else {
                y
            }
        }
        Kind::UnifiedDiff => diff::compact(text),
        Kind::TerraformPlan => terraform::compact(text),
        Kind::CurlVerbose => logs::compact_curl_verbose(text),
        Kind::StackTrace => stacktrace::enrich_top_user_frame(
            &stacktrace::compact(text),
            &std::env::current_dir().unwrap_or_default(),
        ),
        Kind::ArchiveListing => archive::compact(text),
        Kind::TabularCli => {
            // Reach the per-command compactor when the header names the
            // command, and fall back to the generic table fold otherwise.
            //
            // The two paths disagreed by 6.9× on `ps aux` (0.069 through its
            // own module, 0.481 through the generic one) and 2.4× on `lsof`,
            // decided only by whether tokenade happened to RUN the command or
            // was handed its output. The per-command module also knows what
            // the table MEANS — `ps` surfaces a zombie or a hung process,
            // which ranking by CPU buries — so the gap was never only size.
            let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);
            let per_command = detectors::tabular_command_hint(head)
                .and_then(|prog| crate::cmd::dispatch_auto(prog, text));
            match per_command {
                // The module barely folded — it handed back all but a
                // sliver of what it was given. Falling through was already
                // the plan when the module was ABSENT, and a module that
                // answers with the table it was given is the same situation:
                // serving its answer throws away a fold that works. Measured
                // on `lsof` content, the module returned 9 426 bytes for a
                // 9 427-byte input — ONE byte — while `tabular::compact`
                // gave 3 973 for the same bytes, and the 9 426 is what
                // reached the agent.
                //
                // "Barely" has to be a share, not a strict decrease: a
                // one-byte saving satisfies `folded.len() < text.len()`,
                // which is how the first version of this let lsof through
                // unchanged. The generic pass only runs on this branch, so
                // a module that did its job is never charged for it.
                Some(folded) if folded.len() * 10 >= text.len() * 9 => {
                    let generic = tabular::compact(text);
                    if generic.len() < folded.len() {
                        generic
                    } else {
                        folded
                    }
                }
                // It folded: it wins, for the reason above — it knows what
                // the table MEANS, and that is worth more than bytes.
                Some(folded) => folded,
                None => tabular::compact(text),
            }
        }
        Kind::DashTable => tabular::compact_dash_table(text),
        Kind::Csv => tokenade_csv::compact_csv(text),
        Kind::Tsv => tokenade_csv::compact_tsv(text),
        Kind::Logs => logs::compact_generic_logs(text),
        Kind::Challenge => {
            // #463: a detected anti-bot challenge page is GARBAGE to
            // the agent — markdown-ifying it (the prior behaviour)
            // produced 2× inflation on small challenges (production
            // 30d: 20/20 calls inflated, +129 tokens avg) AND wasted
            // tokens summarising a CAPTCHA wall as if it were the
            // article. Instead emit a brief notice + the 3-tier
            // stealth remediation so the agent knows to switch
            // strategy. Falls back to passthrough on the (impossible
            // unless the detector regressed) case where re-detection
            // returns None.
            match stealth::detect_challenge(text) {
                Some(kind) => stealth::stealth_remediation(kind),
                None => text.to_string(),
            }
        }
        Kind::EnvDump => tokenade_env::compact(text),
        Kind::Markdown => markdown::compact(text),
        Kind::Lockfile => lockfile::compact(text),
        Kind::Minified => crate::minified::compact(text),
        Kind::Base64Blob => crate::base64_blob::compact(text),
        Kind::SqlResult => sql::compact(text),
        Kind::PrometheusMetrics => metrics::compact(text),
        Kind::AccessLog => accesslog::compact(text),
        Kind::Journal => journal::compact(text),
        Kind::SourceMap => sourcemap::compact(text),
        Kind::OpenApi => openapi::compact(text),
        Kind::Har => har::compact(text),
        Kind::Email => email::compact(text),
        Kind::Calendar => icalendar::compact(text),
        Kind::Contacts => vcard::compact(text),
        Kind::GraphQl => graphql::compact(text),
        Kind::Strace => strace::compact(text),
        Kind::JvmBuild => jvmbuild::compact(text),
        Kind::KubectlDescribe => kubectl::compact(text),
        Kind::Ansible => ansible::compact(text),
        Kind::GhActions => ghactions::compact(text),
        Kind::PkgInstall => pkginstall::compact(text),
        Kind::GitLog => gitlog::compact(text),
        Kind::Dns => dns::compact(text),
        Kind::Ndjson => ndjson::compact(text),
        Kind::ClaudeTranscript => crate::transcript_view::compact(text),
        Kind::TomlConfig => crate::toml_compact::compact(text),
        Kind::Dockerfile => crate::dockerfile::compact(text),
        Kind::Ini => crate::ini::compact(text),
        Kind::Makefile => crate::makefile::compact(text),
        Kind::CMake => crate::cmake::compact(text),
        Kind::Bazel => crate::bazel::compact(text),
        Kind::Hcl => crate::hcl::compact(text),
        Kind::Proto => crate::proto::compact(text),
        Kind::Xml => crate::xml::compact(text),
        Kind::JUnit => crate::junit::compact(text),
        Kind::LaTeX => crate::latex::compact(text),
        Kind::BibTeX => crate::bibtex::compact(text),
        Kind::SqlSource => crate::sql_source::compact(text),
        // Reuses the archetype the COMMAND path already routes `pacman -Qi`
        // and `apt show` through. Same bytes, same fold, whether they arrive
        // as a command or as content — the command/content parity.
        Kind::KvBlocks => crate::archetypes::kv_records(text, 3, "kv-blocks", "record(s)")
            .unwrap_or_else(|| text.to_string()),
        Kind::Serp => serp::compact(text),
        Kind::Tsc => crate::tsc::compact(text),
        Kind::ContainerLogs => containerlogs::compact(text),
        Kind::CargoBench => cargobench::compact(text),
        Kind::SearchOut => searchout::compact(text),
        Kind::Disk => disk::compact(text),
        Kind::FileList => filelist::compact(text),
        Kind::Netstat => netstat::compact(text),
        Kind::Vmstat => vmstat::compact(text),
        Kind::Tcpdump => tcpdump::compact(text),
        Kind::K8sManifest => k8smanifest::compact(text),
        Kind::Systemd => systemd::compact(text),
        Kind::CommandChain => crate::command_chain::compact(text),
        Kind::CargoBuild => crate::cmd::linux::cargo::build::compact(text),
        Kind::BundleAssets => crate::cmd::linux::vite_assets::compact(text),
        Kind::GTest => crate::cmd::linux::gtest::compact(text),
        Kind::Tap => crate::cmd::linux::test_runner_dispatcher::tap::compact(text)
            .unwrap_or_else(|| text.to_string()),
        Kind::AlignedBlock => crate::archetypes::collapse_alignment_padding(text),
        Kind::Unknown => text.to_string(),
    }
}

/// Dispatch `text` to the per-format compactor that matches its detected
/// [`Kind`]. Returns the kind, the compacted output, and the token savings.
pub fn compact(text: &str) -> (Kind, String, Savings) {
    let text = &*normalise_redraws(text);
    let before = tokenizer::estimate(text);
    // See PAST a line-number gutter before doing anything else. `cat -n`, `nl`
    // and every agent Read tool that numbers its output break the detector, the
    // deduper AND the disclose summary at once — measured 2026-08-26, a 26 KB
    // markdown article delivered 3 294 bytes as `cat FILE` and 14 743 as
    // `cat -n FILE`, and reading the numbered file back returned 27 653 bytes
    // of a 27 652-byte input. Not a weaker fold: no fold. See `line_gutter`.
    //
    // Only for text nothing else claims. A `.tsv` whose first column is a
    // sequential row counter matches every rule a gutter does, and stripping it
    // would DELETE A DATA COLUMN — but that file detects as `Tsv` on its own,
    // so requiring `Unknown` here removes the whole class rather than guessing
    // at it. The gutter scan rescues what the pipeline had no answer for.
    if detect(text) == Kind::Unknown {
        if let Some((g, body)) = crate::line_gutter::split(text) {
            let kind = detect(&body);
            if kind != Kind::Unknown {
                let out = crate::line_gutter::renumber(&dispatch_kind(&body, kind), text);
                let noted = format!("{}{out}", crate::line_gutter::note(&g));
                let after = tokenizer::estimate(&noted);
                // Only when seeing past it actually WON. A gutter over content no
                // compactor improves must not cost the agent a banner, and must not
                // cost it the numbers it asked for either.
                if after < before {
                    return guarded(kind, text, noted, before, after);
                }
            }
        }
    }
    // See past a DIAGNOSTIC PREFIX, for the same reason and with the same
    // guards as the gutter above. Every agent's shell tool merges stderr
    // into stdout — `tokenade wrap` does it deliberately — so a document
    // arrives behind whatever the tool warned about first, and the JSON
    // detector reads only the head. Measured 2026-09-02: `cargo metadata
    // --no-deps` folds 8 741 tokens to 1 908 on its own and returns all
    // 8 757 of them the moment cargo's one-line `warning: please specify
    // --format-version` is in front, a 4x loss on the shape agents ask for
    // most. Three of five real prefixes lost the document whole; YAML and
    // CSV survive already, their detectors being line-based.
    //
    // The prefix is kept VERBATIM: it is a warning, and hiding a warning
    // to compact the thing it warns about would be a bad trade.
    if detect(text) == Kind::Unknown {
        if let Some((prefix, body)) = split_leading_diagnostics(text) {
            let kind = detect(body);
            if kind != Kind::Unknown {
                let out = dispatch_kind(body, kind);
                let noted = format!("{prefix}{out}");
                let after = tokenizer::estimate(&noted);
                if after < before {
                    return guarded(kind, text, noted, before, after);
                }
            }
        }
    }
    let kind = detect(text);
    let out = dispatch_kind(text, kind);
    let after = tokenizer::estimate(&out);
    guarded(kind, text, out, before, after)
}

#[cfg(test)]
include!("auto_tests.rs");
