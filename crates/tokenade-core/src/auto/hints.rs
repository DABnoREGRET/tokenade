//! **What a file's NAME suggests — and how the content
//! corrects it.**
//!
//! An extension is a clue, not a proof: `.log` holds JSON,
//! `.txt` holds a diff, and a `.json` can be a notebook. The path
//! clue therefore opens a PRESUMPTION that `refine_hint_by_content` must
//! confirm before it decides anything.
//!
//! The reverse — taking the extension at its word — routes to the wrong
//! compactor, and a compactor that did not recognise its input REBUILDS it
//! from what it thought it read. That is destruction, not compaction.

use super::*;

/// Kinds whose compactor REBUILDS output from parsed records (anything it
/// can't parse is dropped) — safe only on homogeneous documents.
pub(crate) fn kind_rebuilds_from_parse(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::ArchiveListing
            | Kind::TabularCli
            | Kind::DashTable
            | Kind::SqlResult
            | Kind::Journal
            | Kind::Strace
            | Kind::PkgInstall
            | Kind::GitLog
            | Kind::Dns
            | Kind::Ndjson
            | Kind::FileList
            | Kind::Disk
            | Kind::SearchOut
            | Kind::Netstat
            | Kind::CargoBench
            | Kind::ContainerLogs
            | Kind::Vmstat
            | Kind::Tcpdump
            | Kind::AccessLog
            | Kind::PrometheusMetrics
            | Kind::Tsc
            | Kind::GraphQl
    )
}

/// Map a file extension hint to a confident `Kind` without scanning
/// content. Returns `None` when the extension doesn't unambiguously
/// pin a format (e.g. `.txt`). The Read interceptor uses this on the
/// hot path so a `Cargo.toml` Read skips the 30-detector chain and
/// goes straight to TomlConfig — both faster AND more accurate
/// (content-only detection can mis-fire on borderline shapes).
///
/// Extension is matched case-insensitively. Special "extensions"
/// like `BUILD` / `WORKSPACE` / `Dockerfile` / `Makefile` / `Justfile`
/// / `CMakeLists.txt` are recognised via the FULL basename instead.
pub fn kind_from_path_hint(file_path: &str) -> Option<Kind> {
    let basename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let basename_lc = basename.to_ascii_lowercase();
    // Distinctive basenames first.
    match basename_lc.as_str() {
        "dockerfile" | "containerfile" => return Some(Kind::Dockerfile),
        "makefile" | "gnumakefile" | "justfile" => return Some(Kind::Makefile),
        "cmakelists.txt" => return Some(Kind::CMake),
        "build" | "build.bazel" | "workspace" | "workspace.bazel" => return Some(Kind::Bazel),
        "package.json" => return Some(Kind::Json),
        "pom.xml" => return Some(Kind::Xml),
        _ => {}
    }
    let ext_lc = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    Some(match ext_lc.as_str() {
        "json" => Kind::Json,
        "har" => Kind::Har,
        "email" | "eml" => Kind::Email,
        "calendar" | "ics" => Kind::Calendar,
        "contacts" | "vcf" | "vcard" => Kind::Contacts,
        "yaml" | "yml" => Kind::Yaml,
        "toml" => Kind::TomlConfig,
        "md" | "markdown" => Kind::Markdown,
        "csv" => Kind::Csv,
        "tsv" => Kind::Tsv,
        "ini" | "cfg" | "conf" | "properties" => Kind::Ini,
        "proto" => Kind::Proto,
        // Declared 2026-08-26. SDL was reachable by CONTENT only, so a
        // `.graphql` whose first 8 KB is comments and imports fell to
        // `Unknown` and shipped whole.
        "graphql" | "gql" => Kind::GraphQl,
        "tf" | "tfvars" | "hcl" => Kind::Hcl,
        "xml" | "xsd" | "xsl" | "xslt" | "svg" => Kind::Xml,
        "bzl" => Kind::Bazel,
        "cmake" => Kind::CMake,
        "mk" => Kind::Makefile,
        "jsonl" | "ndjson" => Kind::Ndjson,
        "ipynb" => Kind::JupyterNotebook,
        "html" | "htm" => Kind::Html,
        "diff" | "patch" => Kind::UnifiedDiff,
        "tex" | "ltx" | "sty" => Kind::LaTeX,
        "bib" => Kind::BibTeX,
        "sql" => Kind::SqlSource,
        "log" => Kind::Logs,
        "lock" => Kind::Lockfile,
        "map" => Kind::SourceMap,
        "plan" => Kind::TerraformPlan,
        "prom" | "metrics" => Kind::PrometheusMetrics,
        _ => return None,
    })
}

/// Known source-code extensions. Files with these extensions are
/// HARD-REJECTED from auto-compaction at Read time, regardless of
/// what content detection would say. #570: production showed
/// `mcp/src/lib.rs` being claimed by the SERP detector and
/// compacted from 713→68 tokens (90 % data loss), `auto.rs` by INI
/// at 165→41 (75 %), `.sh` scripts as cmake (4×), `.py` files as
/// makefile (3×). Small slices of code can coincidentally match
/// the content fingerprints of unrelated formats. Code files are
/// never the target of auto-compaction — they want skeleton /
/// code_strip / passthrough, not format-specific compactors.
pub fn is_source_code_extension(file_path: &str) -> bool {
    let Some(ext) = std::path::Path::new(file_path).extension() else {
        return false;
    };
    let Some(ext) = ext.to_str() else {
        return false;
    };
    let ext_lc = ext.to_ascii_lowercase();
    matches!(
        ext_lc.as_str(),
        // Rust / Go / C / C++ / Zig / Crystal / Nim / Dart / V
        "rs" | "go" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx"
        | "zig" | "cr" | "nim" | "dart"
        // Python
        | "py" | "pyi" | "pyx" | "pyw"
        // JavaScript / TypeScript
        | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"
        // JVM
        | "java" | "kt" | "kts" | "scala" | "groovy"
        // .NET
        | "cs" | "fs" | "fsx" | "vb"
        // Functional / scripting
        | "rb" | "php" | "swift" | "lua" | "pl" | "pm" | "ex" | "exs"
        | "erl" | "hs" | "clj" | "cljs" | "cljc" | "ml" | "mli" | "rkt"
        // Shell / batch
        | "sh" | "bash" | "zsh" | "fish" | "ksh" | "bat" | "ps1" | "psm1"
        // Other code-shaped
        | "r" | "jl" | "tcl" | "vim"
    )
}

/// Path-hint-aware variant of [`compact`]. Tries the extension hint
/// first; only falls back to content detection when the extension
/// doesn't pin a kind (e.g. `.log`, no extension at all). Saves the
/// scan cost on hot-path Reads.
/// An extension pins a FAMILY; the content can name a MEMBER of it, and the
/// member's compactor is the better one.
///
/// `.xml` pinned `Kind::Xml`, whose compactor strips comments and whitespace
/// and nothing else — so a JUnit report read from a file never reached the
/// JUnit compactor at all, however clearly its content said so. Measured
/// 2026-08-26: 22 671 bytes in, 22 671 out, and 257 of the 279 `.xml` files in
/// a corpus of real repos are JUnit reports.
///
/// Only ever narrows, never widens: a hint that names nothing more specific is
/// returned unchanged.
pub(crate) fn refine_hint_by_content(text: &str) -> impl Fn(Kind) -> Kind + '_ {
    move |kind| match kind {
        Kind::Xml if crate::junit::detect(text) => Kind::JUnit,
        // `.log` is a CONTAINER, not a format.
        //
        // It is settled that a path hint wins outright rather than competing,
        // and the reason holds: a filename is an assertion by the user, and
        // `.csv` or `.rs` asserts what the bytes ARE. `.log` asserts only that
        // somebody redirected a program's output into a file. What is inside
        // is a git log, a journald dump, an access log, a cargo build — and
        // the content detector knows which, while the extension cannot.
        //
        // So this is not the hint competing. It is the one extension whose
        // assertion is weaker than the content's, refined by the mechanism
        // that already exists for exactly that (the `Xml` → `JUnit` arm above
        // is the same idea: the hint had the family right and the content had
        // the format).
        //
        // Measured 2026-08-27 across the fuzz corpora, naming each `run.log`:
        //
        //   journal    Journal 178 tokens  vs  Logs 837  (Logs folded NOTHING)
        //   cargo    CargoBuild 175        vs  Logs 532  (folded nothing)
        //   accesslog AccessLog  97        vs  Logs 261
        //   gitlog       GitLog 980        vs  Logs 516
        //
        // Three of the four are also smaller, and the fourth is not — and that
        // is deliberately not the criterion (fewer tokens is not
        // better). `git log` routed to its own compactor keeps commit
        // structure that a template fold flattens; 980 tokens of the right
        // shape beats 516 of the wrong one. What settles it is that the
        // specific compactor exists and the container extension was hiding it.
        //
        // Only ever narrows: `Unknown` and `Logs` leave the hint alone, so a
        // `.log` that content detection cannot place still gets the Logs
        // compactor, which is what the hint was for.
        Kind::Logs => match detect(text) {
            Kind::Unknown | Kind::Logs => Kind::Logs,
            specific => specific,
        },
        // `.json` DOES assert a format — and several kinds are that format.
        //
        // The `.log` arm above lets ANYTHING specific win, because `.log`
        // asserts nothing about the bytes. `.json` is the opposite case and
        // the rule has to be narrower: the user's assertion is real and
        // correct, so only a JSON DIALECT may refine it. Letting `Csv` or
        // `Markdown` win here would be the hint losing to a misdetection,
        // which was measured and refused.
        //
        // The list is exactly the kinds whose own detectors say they are
        // JSON. From `detectors.rs`, in its own words: OpenAPI specs "ARE
        // valid JSON but a dedicated compactor reduces them by 95%+", source
        // maps "ARE JSON but a dedicated compactor saves 99%+ that generic
        // JSON can't". Every OpenAPI spec on earth is called `openapi.json`
        // or `swagger.json`, so the hint was hiding the compactor written for
        // it: measured on a real spec, 901 tokens through `Kind::Json`
        // against **106** through `Kind::OpenApi`.
        //
        // `.map` and `.ipynb` already have their own hints, so those two are
        // only reachable here when somebody named the file `.json` — which
        // people do.
        //
        // THE LIST IS A MARGIN, NOT A CONDITION, and it survives mutation
        // green for that reason (the redraw replay's two early
        // returns are the same). Widening it to "anything specific wins", as the `.log`
        // arm above does, changes nothing measurable: across 2 985 real
        // `.json`/`.yaml`/`.conf`/`.lock` files the only non-Json answers
        // content ever gives on a `.json` are `OpenApi` and `Ndjson`, both
        // already listed, and five adversarial fixtures built to misdetect
        // (CSV-ish, Markdown-ish, log-ish, diff-ish, table-ish values inside
        // valid JSON) all come back `Json`.
        //
        // It stays because the two hints assert different amounts and that
        // difference is real whether or not this corpus exercises it. A live
        // misdetection was measured in exactly this neighbourhood — a
        // bracketed prose banner read as a JSON array — so detectors here do
        // misfire; that one happened to misfire TOWARD `Json` rather than
        // away from it. A margin that costs nothing is worth keeping on the
        // side of the hint the user actually typed.
        // A `package-lock.json` is a LOCKFILE that happens to be JSON — the
        // extension sends it here, and the JSON compactor's noise-key drop
        // knows `created_at` and `_links`, never `resolved` or `integrity`,
        // which are what a lockfile is made of. The membership question has
        // ONE name (`is_a_json_specialisation`): it was a list here and a
        // ladder in `detectors`, and the two had to agree with nothing saying
        // so.
        Kind::Json => match detect(text) {
            d if detectors::is_a_json_specialisation(d) => d,
            _ => Kind::Json,
        },
        other => other,
    }
}

/// Replay a terminal redraw BEFORE anything decides what these bytes are.
///
/// The replay started life as a post-pass, which is too late: it then runs
/// on the ROUTED output, so a compactor that already claimed the raw text has
/// folded a hundred and twenty frames into its own summary and there is nothing
/// left to undo. Measured on 25 frames of `watch kubectl get pods` — 7 738 bytes
/// came back as 309 with no filename and as **4 301** the moment the file was
/// called `.log`, because the extension hint routes straight to the Logs
/// compactor. Fourteen times worse, decided by the name.
///
/// It belongs here for a second reason too: the replay makes the text MORE
/// detectable, not less. A log line hidden behind `ESC[2J` is not recognised by
/// anything, and the same line after the replay is an ordinary log line.
///
/// Lossless, so it may run ahead of every decision: the frames it drops were
/// overwritten on screen before any reader saw them.
pub(crate) fn normalise_redraws(text: &str) -> std::borrow::Cow<'_, str> {
    let replayed = crate::post_passes::replay_cursor_redraws(text);
    if replayed.len() == text.len() && replayed == text {
        std::borrow::Cow::Borrowed(text)
    } else {
        std::borrow::Cow::Owned(replayed)
    }
}
