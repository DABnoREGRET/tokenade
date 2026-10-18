//! **Where the index agrees to LOOK.**
//!
//! An index that descends into `node_modules`, `target/` or `.git` no longer
//! measures the repo: it measures its dependencies, and does so slowly. This
//! module carries the list of always-skipped directories, the size bound of an
//! indexable file, and the path normalisation that makes an index readable
//! from another machine.

/// Directories the index always skips. Exposed so the benchmark and CLI can
/// use the **same scope** when comparing against grep-style ground truth —
/// otherwise "recall" measurements are apples-to-oranges.
///
/// The dot-prefixed entries are included for clarity / documentation; the
/// walker also unconditionally skips any directory whose name starts with
/// `.`, so they would be filtered anyway. Non-dotted vendor / build
/// directories MUST be listed explicitly here — that's where every
/// language ecosystem differs from the next.
///
/// Sourced from codegraph 0.9.4's `DEFAULT_IGNORE_DIRS` set
/// (extraction/index.ts:116-150) so our recall stays competitive on the
/// same fixtures their benchmark uses.
/// Directory names that never hold hand-written source, at any depth.
///
/// Split from [`BUILD_DIR_FALLBACK`] on 2026-08-28. The old single list was
/// applied unconditionally, and a name is not evidence: measured over the 300
/// checked-out repositories, **5 009 git-TRACKED source files in 118 of them**
/// lived under one of those names and were invisible to the index — and so to
/// `query`, `map`, `semantic` and `impact`. `bin/` alone accounted for 388
/// files across 58 repositories, which is where a project keeps its CLI
/// scripts.
pub const HARD_SKIP_DIRS: &[&str] = &[
    // VCS
    ".git",
    // Generic JS / TS dependency dirs. `node_modules` stays HARD even though
    // one corpus repository commits it: indexing a committed dependency tree
    // is the pathology, not the miss.
    "node_modules",
    "bower_components",
    "jspm_packages",
    "web_modules",
    ".yarn",
    ".pnpm-store",
    ".output",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".vite",
    ".parcel-cache",
    ".angular",
    ".docusaurus",
    "storybook-static",
    ".vinxi",
    ".nitro",
    ".vercel",
    ".netlify",
    ".wrangler",
    // Python.
    ".venv",
    "venv",
    "__pycache__",
    "__pypackages__",
    ".pixi",
    ".pdm-build",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".hypothesis",
    ".ipynb_checkpoints",
    ".eggs",
    // Rust + Java/Kotlin/Gradle/Scala tool caches.
    ".cargo",
    ".gradle",
    ".metals",
    ".bloop",
    // Ruby: bundler vendoring.
    ".bundle",
    // Swift / iOS.
    "Pods",
    "Carthage",
    "DerivedData",
    ".swiftpm",
    // Dart / Flutter.
    ".dart_tool",
    ".pub-cache",
    // C / C++ native build trees.
    ".cxx",
    ".externalNativeBuild",
    "vcpkg_installed",
    // Lua.
    "lua_modules",
    ".luarocks",
    // IDEs.
    ".idea",
    ".vscode",
    // Credential stores and per-user tool caches. These are HARD rather than
    // left to the dot-directory fallback because an index run whose root is a
    // HOME directory would otherwise walk them: measured over the 300 corpus
    // repositories, not one of these names holds a single tracked source file,
    // so the rule costs nothing and closes that case. `.config` is deliberately
    // NOT here — 5 repositories track source under it.
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".npm",
    ".cache",
    ".local",
    ".gem",
    ".m2",
    ".ivy2",
    ".sbt",
    ".rustup",
    ".nvm",
    ".pyenv",
    ".rbenv",
    ".terraform",
    ".serverless",
    ".aws-sam",
    ".stack-work",
    ".direnv",
    ".sass-cache",
    ".history",
    ".Trash",
];

/// Directory names that USUALLY hold build output and sometimes hold source.
///
/// Skipped only when no ignore rule reaches this directory — i.e. when the
/// tree carries no `.gitignore` we could consult. Where the repository does
/// declare its own excludes, that declaration decides, because it is written
/// by someone who knows whether `dist/` is generated here.
///
/// Measured 2026-08-28 over the 300 repositories, counting every file on disk
/// under one of these names with a source extension: **4 299 are tracked by
/// git** (pure gain), 60 are matched by the repository's own `.gitignore`
/// (still excluded, by the right rule), and **2 are neither** — the entire
/// cost of the change.
///
/// `benchmarks` and `competitors` were here for one reason, written in the old
/// comment: not indexing tokenade's OWN checkouts when run from this
/// repository. Our `.gitignore` already carries `/competitors/` and
/// `/benchmarks/repos/*/`, so the ignore layer does that job — while the
/// unconditional name was hiding OUR OWN 84 tracked files under `benchmarks/`,
/// and other people's benchmark suites in 53 repositories.
pub const BUILD_DIR_FALLBACK: &[&str] = &[
    "dist",
    "build",
    "out",
    "out-tsc",
    "target",
    "bin",
    "obj",
    "vendor",
    "deps",
    "project",
    "_build",
    ".build",
    "benchmarks",
    "competitors",
];

/// Is this a directory name the walk skips, whatever the reason?
///
/// For consumers that want the NAME set rather than the walk decision — the
/// semantic tokenizer drops these as path stopwords, and the bench harness
/// mirrors the walk. Prefer [`should_skip_dir`] when walking.
pub fn is_skip_dir_name(name: &str) -> bool {
    HARD_SKIP_DIRS.contains(&name) || BUILD_DIR_FALLBACK.contains(&name)
}

/// THE single declarer of "does the walk enter this directory".
///
/// Written twice before (both index walkers carried the same two lines), which
/// is how a rule that needed one change would have needed two.
///
/// `ignore_rules_apply` is true when some `.gitignore` layer reaches this
/// directory — the repository has said what it excludes, so its word beats a
/// hardcoded name.
pub fn should_skip_dir(name: &str, ignore_rules_apply: bool) -> bool {
    if HARD_SKIP_DIRS.contains(&name) {
        return true;
    }
    // The repository has declared what it excludes. Its word beats both a
    // hardcoded name and the leading dot.
    if ignore_rules_apply {
        return false;
    }
    // No ignore information reaches here, so fall back on shape: a dot
    // directory, or a name that usually holds build output.
    //
    // The dot rule was UNCONDITIONAL until 2026-08-28, and it is where agent
    // tooling now lives. Measured over the 300 corpus repositories:
    // **19 188 git-tracked source files in 228 of them** sat under a dot
    // directory that the hard list does not name — `.github` in 212
    // repositories, `.claude-plugin` in 53, `.agents` in 27, `.claude` in 39,
    // `.opencode`, `.cursor`, `.codex`, `.pi`. Counted the other way, over
    // everything on disk under such a directory with a source extension:
    // **18 432 tracked, 20 matched by the repository's own `.gitignore`, and
    // ZERO neither** — the change costs nothing measurable.
    name.starts_with('.') || BUILD_DIR_FALLBACK.contains(&name)
}

/// Maximum source-file size we'll bother indexing. Files past this are
/// almost always minified bundles, generated artefacts, or vendored
/// blobs — extracting symbols from them blows the tree-sitter / regex
/// budget and contributes no useful navigation. codegraph uses the same
/// 1 MiB cap (extraction/index.ts:100).
pub const MAX_INDEX_FILE_BYTES: u64 = 1024 * 1024;

/// The full parent directory of a `/`-separated index path — every
/// segment but the basename. `crates/core/src/index.rs` →
/// `crates/core/src`, a bare `main.rs` → `.`. The `map` aggregation
/// buckets by this; splitting on only the first segment collapsed an
/// entire workspace into one `crates/` row.
/// A repo-relative path in the ONE form the whole index speaks: forward
/// slashes.
///
/// `Path::display()` hands back the platform separator, so on Windows every
/// stored path read `crates\\core\\src\\index.rs`. Every consumer here splits on
/// `/` — [`parent_dir`] most visibly — so nothing matched and EVERY file
/// collapsed into the single `.` bucket: `tokenade map` on Windows listed one
/// directory for the whole repository. The index is also a cache shared by
/// path, so storing the platform form made it non-portable for no benefit.
pub(super) fn rel_path_string(path: &std::path::Path, root: &std::path::Path) -> String {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    with_forward_slashes(rel, std::path::MAIN_SEPARATOR)
}

/// Swap `sep` for `/`, and ONLY when `sep` is not already `/`.
///
/// `sep` is a parameter rather than read from the platform so this is testable
/// where the bug is not reproducible. It also keeps the rule honest on Unix,
/// where a backslash is a legal character in a file name and must survive.
pub(super) fn with_forward_slashes(rel: String, sep: char) -> String {
    if sep == '/' {
        rel
    } else {
        rel.replace(sep, "/")
    }
}

pub fn parent_dir(file: &str) -> &str {
    match file.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir,
        _ => ".",
    }
}
