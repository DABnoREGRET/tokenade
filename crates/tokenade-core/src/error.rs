//! Unified error type for `tokenade-core`.
//!
//! ## Written and NOT adopted — the status, so it stops being invisible
//!
//! Zero callers in the whole workspace (measured 2026-09-12, code only,
//! comments and strings excluded). This is the attempt that
//! `docs/ARCHITECTURE.md` talks about under "A unified `TokenadeError` is on the
//! refactor backlog": the type exists, the migration did not happen.
//!
//! Kept because it is the intended direction and the work is done, not
//! because it serves. Same declared status as `cachekey`, `query_parser` and
//! `nudge` — and the reason to write it down is the one `docs/ARCHITECTURE.md`
//! tells about `cache_align` and `path_safety`: a module that looks wired in
//! costs real work to whoever believes it.
//!
//! Before this module, every function returned its own ad-hoc error
//! (`EmbedError`, `HybridError`, `RerankError`,
//! `std::io::Error`, …) and consumers had to `match` 5+ types. This
//! type doesn't replace those — they remain for callers who need
//! granular handling — but provides a single `From`-convertible
//! umbrella for the dispatch / CLI / MCP layers that only care about
//! "did it work, and if not, what was the human-readable reason?"
//!
//! ## What problem this solves
//!
//! 232 `.unwrap()` calls across `tokenade-core` (October 2026 audit)
//! were latent panics. Each could crash an MCP request, fail an
//! install, or silently corrupt the gain ledger. Migrating them to
//! `?` against `TokenadeError` is a mechanical change that turns
//! panics into structured errors the MCP / CLI surfaces can render.
//!
//! Pattern for migration:
//! ```ignore
//! // before:
//! let s = fs::read_to_string(p).unwrap();
//! // after:
//! let s = fs::read_to_string(&p).map_err(|e| TokenadeError::Io {
//!     path: p.clone(),
//!     source: e,
//! })?;
//! ```
//!
//! Or, more terse via `From<io::Error>`:
//! ```ignore
//! let s = fs::read_to_string(&p)?;  // -> TokenadeError::IoBare
//! ```
//!
//! When the path context is meaningful (almost always for fs
//! errors), prefer the explicit `TokenadeError::Io { path, source }`
//! variant — the rendered error tells the user *which* file failed.

use std::path::PathBuf;

#[derive(Debug)]
pub enum TokenadeError {
    /// File-system error WITH path context. Use this when the caller
    /// knows which file/dir failed — almost always for fs ops.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File-system error without path context. Use only when the
    /// caller really doesn't know the path (rare).
    IoBare(std::io::Error),
    /// SQL operation failure (dedup.db, index.db, semantic.db, fold_store.db, …).
    Sql(String),
    /// JSON parse / serialise failure with a short description of
    /// what we were trying to do (e.g., "decoding hook input").
    Json {
        context: &'static str,
        source: serde_json::Error,
    },
    /// Embedding model failure — wrapping [`crate::embedding::EmbedError`].
    Embed(String),
    /// Reranker model failure — wrapping [`crate::reranker::RerankError`].
    Rerank(String),
    /// A path the user passed escapes the project root, doesn't
    /// exist, or hides behind a refused symlink.
    /// A configuration value was missing or out of range.
    Config { key: &'static str, reason: String },
    /// MCP protocol / wire error — bad JSON-RPC shape, etc.
    Protocol(String),
    /// Something internal we didn't classify yet. Use sparingly —
    /// every long-lived `Internal` is a candidate for a new variant.
    Internal(String),
}

impl std::fmt::Display for TokenadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenadeError::Io { path, source } => {
                write!(f, "I/O error on `{}`: {source}", path.display())
            }
            TokenadeError::IoBare(e) => write!(f, "I/O error: {e}"),
            TokenadeError::Sql(s) => write!(f, "SQL error: {s}"),
            TokenadeError::Json { context, source } => {
                write!(f, "JSON error while {context}: {source}")
            }
            TokenadeError::Embed(s) => write!(f, "embedding error: {s}"),
            TokenadeError::Rerank(s) => write!(f, "reranker error: {s}"),
            TokenadeError::Config { key, reason } => {
                write!(f, "config error on `{key}`: {reason}")
            }
            TokenadeError::Protocol(s) => write!(f, "protocol error: {s}"),
            TokenadeError::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

impl std::error::Error for TokenadeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TokenadeError::Io { source, .. } => Some(source),
            TokenadeError::IoBare(e) => Some(e),
            TokenadeError::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TokenadeError {
    fn from(e: std::io::Error) -> Self {
        TokenadeError::IoBare(e)
    }
}

impl From<rusqlite::Error> for TokenadeError {
    fn from(e: rusqlite::Error) -> Self {
        TokenadeError::Sql(e.to_string())
    }
}

impl From<crate::embedding::EmbedError> for TokenadeError {
    fn from(e: crate::embedding::EmbedError) -> Self {
        TokenadeError::Embed(e.to_string())
    }
}

impl From<crate::reranker::RerankError> for TokenadeError {
    fn from(e: crate::reranker::RerankError) -> Self {
        TokenadeError::Rerank(e.to_string())
    }
}

/// Conventional alias for `Result<T, TokenadeError>`.
pub type TokenadeResult<T> = Result<T, TokenadeError>;

/// Convenience constructor — `io_err(&path, e)` saves the `Io {...}`
/// boilerplate at call sites where we have both pieces.
pub fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> TokenadeError {
    TokenadeError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_path_for_io_variant() {
        let e = io_err("/etc/passwd", std::io::Error::other("denied"));
        let s = format!("{e}");
        assert!(s.contains("/etc/passwd"));
        assert!(s.contains("denied"));
    }

    #[test]
    fn display_for_config_carries_key() {
        let e = TokenadeError::Config {
            key: "memory.cap",
            reason: "must be > 0".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("memory.cap"));
        assert!(s.contains("> 0"));
    }

    #[test]
    fn implements_std_error_with_source() {
        let path = PathBuf::from("/tmp/x");
        let e = io_err(path, std::io::Error::other("oops"));
        let _ = std::error::Error::source(&e).expect("io variant carries source");
    }

    #[test]
    fn auto_converts_from_io_error() {
        fn inner() -> TokenadeResult<String> {
            let s = std::fs::read_to_string("/this/file/really/does/not/exist")?;
            Ok(s)
        }
        let err = inner().unwrap_err();
        assert!(matches!(err, TokenadeError::IoBare(_)));
    }
}
