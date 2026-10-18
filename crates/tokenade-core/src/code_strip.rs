//! Language-aware source code compactor.
//!
//! Strips comments, docstrings, and excess blank lines from source
//! files so the agent can read the LOGIC without paying for the
//! prose. Tuned by language — Rust has `///`-doc + `//` line + `/*
//! */` block, Python has triple-quoted docstrings + `#` line, etc.
//!
//! Strip modes:
//!   - [`StripMode::Lite`]   — drop blank-line runs and trailing
//!     spaces only. Keep all comments.
//!   - [`StripMode::Comments`] — Lite + drop line/block comments.
//!     Keep docstrings.
//!   - [`StripMode::Full`]   — Comments + drop docstrings + drop
//!     imports (Python) / use clauses (Rust) optionally.
//!
//! Use [`StripMode::Comments`] when the agent wants to read the
//! actual logic of a heavily-commented file; use [`StripMode::Full`]
//! when only the structural skeleton matters and comments would be
//! noise.
//!
//! Returns the compact source as a String — caller decides whether
//! to embed in a tool response, write to disk, etc.

mod comments;
mod delivery;
mod docstrings;
mod line;
pub(crate) use comments::*;
pub use delivery::*;
pub(crate) use docstrings::*;
pub use line::*;

/// Aggressiveness of the strip pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripMode {
    /// Blank-line runs collapsed, trailing whitespace dropped.
    Lite,
    /// Lite + line and block comments stripped, docstrings preserved.
    Comments,
    /// Comments + docstrings + imports/use-clauses dropped.
    Full,
}

/// Supported source languages. Detected from file extension via
/// [`language_for_extension`]. Unknown extensions return None and
/// the caller falls back to plain Lite-mode pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Java,
    Kotlin,
    Swift,
    Ruby,
    C,
    Cpp,
    Csharp,
    Php,
    Shell,
    Scala,
    Html,
    Css,
}

impl Language {
    /// Short name for a banner, so a stripped source file says WHICH language
    /// was parsed — the same shape the format strippers already print
    /// (`[tokenade:toml]`, `[tokenade:ini]`).
    pub fn tag(self) -> &'static str {
        match self {
            Language::Rust => "rs",
            Language::Python => "py",
            Language::JavaScript => "js",
            Language::TypeScript => "ts",
            Language::Go => "go",
            Language::Java => "java",
            Language::Kotlin => "kt",
            Language::Swift => "swift",
            Language::Ruby => "rb",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Csharp => "cs",
            Language::Php => "php",
            Language::Shell => "sh",
            Language::Scala => "scala",
            Language::Html => "html",
            Language::Css => "css",
        }
    }

    /// True if this language uses `//` / `/* */` style comments (C
    /// family). The Comments-mode parser uses a shared C-family
    /// state machine for these languages.
    fn is_c_family(self) -> bool {
        matches!(
            self,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Go
                | Language::Java
                | Language::Kotlin
                | Language::Swift
                | Language::C
                | Language::Cpp
                | Language::Csharp
                | Language::Php
                | Language::Scala
        )
    }

    /// True if this language uses `#` line comments only (no block
    /// comment syntax we care about — Python's `"""` are docstrings,
    /// not comments).
    fn is_hash_family(self) -> bool {
        matches!(self, Language::Python | Language::Ruby | Language::Shell)
    }
}

/// Map a file extension (no leading dot) to a [`Language`]. Returns
/// None for extensions we don't have rules for. Case-insensitive.
pub fn language_for_extension(ext: &str) -> Option<Language> {
    let e = ext.to_ascii_lowercase();
    Some(match e.as_str() {
        "rs" => Language::Rust,
        // `.pyx` (Cython) and `.pyw` (Windows-GUI Python) are Python source
        // with Python comment syntax; they were in `is_source_code_extension`
        // — so never auto-compacted — and in no language map, which is the
        // exact shape that hands a file back whole while reporting success.
        "py" | "pyi" | "pyx" | "pyw" => Language::Python,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
        "go" => Language::Go,
        "java" => Language::Java,
        "kt" | "kts" => Language::Kotlin,
        "swift" => Language::Swift,
        "rb" | "rake" => Language::Ruby,
        "c" | "h" => Language::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
        "cs" => Language::Csharp,
        "php" => Language::Php,
        "sh" | "bash" | "zsh" | "ksh" | "fish" => Language::Shell,
        "scala" | "sc" => Language::Scala,
        "html" | "htm" => Language::Html,
        "css" | "scss" | "sass" | "less" => Language::Css,
        _ => return None,
    })
}

/// Strip `source` according to `mode` for the given `language`.
/// Pure function — no I/O, no panics on malformed input (worst case
/// returns the original source unchanged).
pub fn strip(source: &str, language: Language, mode: StripMode) -> String {
    let mut s = source.to_string();
    if mode == StripMode::Comments || mode == StripMode::Full {
        s = if language.is_c_family() {
            strip_c_family_comments(&s, language)
        } else if language.is_hash_family() {
            strip_hash_comments(&s)
        } else if matches!(language, Language::Html) {
            strip_html_comments(&s)
        } else if matches!(language, Language::Css) {
            // The note here was right all along; the function it called did
            // not know it, and cut every `url(http://…)` in half.
            strip_c_family_comments(&s, language)
        } else {
            s
        };
    }
    if mode == StripMode::Full {
        s = strip_imports(&s, language);
        s = strip_docstrings(&s, language);
    }
    // Final pass: collapse runs of blank lines + trim trailing
    // whitespace. Always applied (cheapest win).
    collapse_blanks_and_trim(&s)
}

#[cfg(test)]
include!("code_strip_tests.rs");
