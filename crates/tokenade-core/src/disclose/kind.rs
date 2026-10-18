//! What a blob IS: the kind, and the file extension that follows from it.
//!
//! A stash entry is only useful if the next tool can open it, which means the
//! extension has to match the content — sniffed from the bytes when no
//! Content-Type says so.

use super::*;

/// Sniff the content kind from raw bytes. Returns a stable label
/// suitable for use as a file extension component.
///
/// The detection ladder, in priority order:
///   1. Binary magic bytes (PDF, PNG, JPEG, GZIP, ZIP, MP4, WASM, …).
///   2. JSON shape (after trimming whitespace, starts with `{` or `[`).
///   3. HTML / XML (doctype, root tag, `<html`, `<?xml`).
///   4. CSV (≥2 lines, ≥2 commas per line, no `{`/`<` markers).
///   5. Falls back to the structural `auto::detect` pipeline which
///      identifies 60+ format families.
///   6. Default: `txt` when nothing matches.
pub fn detect_kind(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    // 1. Binary magic.
    if let Some(b) = sniff_binary_magic(bytes) {
        return b;
    }
    let trimmed = text.trim_start();
    let lowered_prefix = trimmed
        .get(..trimmed.len().min(64))
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    // 2. JSON: `{...}` or `[...]`.
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return "json";
    }
    // 3. HTML / XML / SVG.
    if lowered_prefix.starts_with("<!doctype html")
        || lowered_prefix.starts_with("<html")
        || lowered_prefix.contains("<head>")
        || lowered_prefix.contains("<body")
    {
        return "html";
    }
    if lowered_prefix.starts_with("<?xml") {
        return "xml";
    }
    if lowered_prefix.starts_with("<svg") {
        return "svg";
    }
    // 4. CSV — what makes a table a table is that its rows AGREE on how many
    // fields they have. The old rule looked at the FIRST LINE ONLY (≥2 commas,
    // no `{`/`<`) and never compared it with anything.
    //
    // MEASURED 2026-08-04 on stash blob `40ba7ca5a9f3`: a line-numbered Rust
    // excerpt opening on `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]`
    // — 5 commas, no `{`, no `<` — was labelled `.csv`. This label is not
    // cosmetic: it tells the NEXT reader how to parse a document it never saw,
    // and reading that file back ran the CSV compactor, which cut every line
    // at its first comma (`#[derive(Clone`, losing `Copy, Debug, PartialEq,
    // Eq, Hash`) and doubled the quotes inside string literals.
    //
    // Commas inside double quotes do not separate fields, so counting them raw
    // would reject the legitimate `a,b,c` / `"x,y",z,w` table this is meant to
    // catch.
    {
        let sample: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(5)
            .collect();
        let n = unquoted_commas(sample.first().copied().unwrap_or(""));
        if sample.len() >= 2
            && n >= 2
            && sample.iter().all(|l| unquoted_commas(l) == n)
            && !sample[0].contains('{')
            && !sample[0].contains('<')
        {
            return "csv";
        }
    }
    // 5. Structural compactor detection (60+ format detectors). This
    // catches log shapes, env dumps, terraform plans, etc.
    let kind = crate::auto::detect(text);
    if let Some(ext) = kind_to_extension(&kind) {
        return ext;
    }
    "txt"
}

/// Commas that actually separate fields — those outside a double-quoted
/// field. RFC 4180 escapes a quote by doubling it, and since `""` inside a
/// quoted field flips the state twice it needs no special case.
pub(super) fn unquoted_commas(line: &str) -> usize {
    let mut in_quotes = false;
    let mut n = 0;
    for b in line.bytes() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => n += 1,
            _ => {}
        }
    }
    n
}

/// Map the rich `auto::Kind` enum to a short extension label. Returns
/// `None` for `Kind::Unknown` (caller falls back to "txt"). Lossy by
/// design — many kinds collapse to a single suffix (`.log` for any
/// log-shaped output, regardless of journal vs k8s vs syslog).
pub(super) fn kind_to_extension(kind: &crate::auto::Kind) -> Option<&'static str> {
    use crate::auto::Kind;
    Some(match kind {
        Kind::Html => "html",
        Kind::JUnit => "junit.xml",
        Kind::JupyterNotebook => "ipynb",
        Kind::Json => "json",
        Kind::Yaml => "yaml",
        Kind::UnifiedDiff => "diff",
        Kind::TerraformPlan => "tfplan.txt",
        Kind::CurlVerbose => "curl.txt",
        Kind::StackTrace => "trace.txt",
        Kind::ArchiveListing => "ls.txt",
        // The stash keeps the BLOCK, not its transcription: the extension says
        // what to do with it (`base64 -d`), not what it looks like.
        Kind::Base64Blob => "b64",
        Kind::TabularCli => "tab.txt",
        // An aligned block stays text: it is its padding that is removed,
        // not its nature.
        Kind::AlignedBlock => "txt",
        Kind::DashTable => "tab.txt",
        Kind::Csv => "csv",
        Kind::Tsv => "tsv",
        Kind::Logs => "log",
        Kind::Challenge => "html",
        Kind::EnvDump => "env",
        Kind::Markdown => "md",
        Kind::Lockfile => "lock",
        Kind::Minified => "min.js",
        Kind::SqlResult => "sql.txt",
        Kind::PrometheusMetrics => "prom",
        Kind::AccessLog => "access.log",
        Kind::Journal => "journal.log",
        Kind::SourceMap => "map",
        Kind::OpenApi => "openapi.json",
        Kind::Har => "capture.har",
        Kind::Email => "message.eml",
        Kind::Calendar => "calendar.ics",
        Kind::Contacts => "contacts.vcf",
        Kind::GraphQl => "graphql",
        Kind::Strace => "strace.log",
        Kind::JvmBuild => "build.log",
        Kind::KubectlDescribe => "kube.txt",
        Kind::Ansible => "ansible.log",
        Kind::GhActions => "ci.log",
        Kind::Ndjson => "ndjson",
        Kind::Dns => "dns.txt",
        Kind::Netstat => "netstat.txt",
        Kind::Disk => "disk.txt",
        Kind::Vmstat => "vmstat.txt",
        Kind::Systemd => "systemd.txt",
        Kind::Tcpdump => "tcpdump.txt",
        Kind::ContainerLogs => "container.log",
        Kind::Serp => "serp.json",
        Kind::Tsc => "tsc.txt",
        Kind::CargoBench => "bench.txt",
        Kind::FileList => "filelist.txt",
        Kind::SearchOut => "search.txt",
        Kind::PkgInstall => "pkg.txt",
        Kind::GitLog => "gitlog.txt",
        Kind::ClaudeTranscript => "claude.jsonl",
        Kind::TomlConfig => "toml",
        Kind::Dockerfile => "Dockerfile",
        Kind::Ini => "ini",
        Kind::Makefile => "mk",
        Kind::CMake => "cmake",
        Kind::Bazel => "bazel",
        Kind::Hcl => "hcl",
        Kind::Proto => "proto",
        Kind::Xml => "xml",
        Kind::LaTeX => "tex",
        Kind::BibTeX => "bib",
        Kind::SqlSource => "sql",
        Kind::KvBlocks => "txt",
        Kind::A11yTree => "a11y.txt",
        Kind::K8sManifest => "yaml",
        Kind::CommandChain => "chain.txt",
        Kind::CargoBuild => "cargo.txt",
        Kind::BundleAssets => "build.txt",
        Kind::GTest => "gtest.txt",
        Kind::Tap => "tap.txt",
        Kind::Unknown => return None,
    })
}

/// Sniff binary magic bytes from a (potentially lossy-decoded) text
/// view. Only handles ASCII-safe magic prefixes — non-ASCII magic
/// (PNG 0x89, ELF 0x7F, MS Office, etc.) would have been replaced
/// with U+FFFD before reaching this code path. Callers that hold the
/// raw bytes should use [`detect_kind_bytes`] instead, which runs
/// the full `infer` 1500+-pattern database against the raw slice.
pub(super) fn sniff_binary_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("pdf");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some("zip");
    }
    if bytes.starts_with(b"OggS") {
        return Some("ogg");
    }
    // Netpbm, BINARY variants only. `P4`/`P5`/`P6` carry raw pixel bytes after
    // an ASCII header; `P1`/`P2`/`P3` are the ASCII variants and are genuinely
    // readable text, so claiming them would hide content a caller can use.
    // Worth listing: the format is advertised as supported, it is small enough
    // to fall under every size heuristic, and over a pipe there is no filename
    // to fall back on — a 23-byte `.pnm` came back as its own bytes.
    if bytes.len() >= 3
        && bytes[0] == b'P'
        && matches!(bytes[1], b'4' | b'5' | b'6')
        && bytes[2].is_ascii_whitespace()
    {
        return Some("pnm");
    }
    None
}

/// L67 — bytes-aware format detection. Identifies ~1500 magic
/// patterns via the `infer` crate (images, audio, video, archives,
/// Office documents, ELF binaries, fonts, …). Falls back to
/// [`detect_kind`] on a lossy-decoded text view when the bytes
/// aren't a known binary.
///
/// Returns the same extension labels used by [`detect_kind`] so
/// downstream code (composite extension, parsing hints) doesn't need
/// to know which detector fired.
pub fn detect_kind_bytes(bytes: &[u8]) -> String {
    if let Some(t) = infer::get(bytes) {
        let ext = map_infer_extension(t.extension());
        // Same veto as `disclose::binary::detect_binary`, for the same reason:
        // an ASCII signature cannot distinguish a header from a sentence, and
        // this function names the stash extension the caller then acts on.
        // `bytes` here are RAW — this guard must never be added to
        // `detect_kind`, which is handed a lossy-decoded view where every
        // non-ASCII byte has already become a printable U+FFFD.
        if crate::textshape::is_text_encoded_format(ext)
            || !crate::textshape::looks_like_text(bytes)
        {
            return ext.to_string();
        }
    }
    // No binary signature → treat as text. Lossy UTF-8 may garble
    // exotic encodings (UTF-16, Latin-1) but for the purpose of
    // STRUCTURAL detection the first 4 bytes of a JSON / HTML /
    // YAML doc are always ASCII even in non-UTF-8 files.
    detect_kind(&String::from_utf8_lossy(bytes)).to_string()
}

/// Map `infer`'s extension strings to the labels we emit on the
/// stash file. Most pass through unchanged; we collapse a few
/// near-equivalents (`tif`/`tiff`, `mid`/`midi`).
pub(super) fn map_infer_extension(ext: &str) -> &'static str {
    match ext {
        // Images
        "png" => "png",
        "jpg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        "ico" => "ico",
        "tif" | "tiff" => "tif",
        "heif" => "heif",
        "avif" => "avif",
        "jxr" => "jxr",
        "psd" => "psd",
        // Documents
        "pdf" => "pdf",
        "doc" => "doc",
        "docx" => "docx",
        "xls" => "xls",
        "xlsx" => "xlsx",
        "ppt" => "ppt",
        "pptx" => "pptx",
        "epub" => "epub",
        "rtf" => "rtf",
        // Archives
        "zip" => "zip",
        "tar" => "tar",
        "gz" => "gz",
        "bz2" => "bz2",
        "xz" => "xz",
        "7z" => "7z",
        "rar" => "rar",
        "z" => "z",
        "lz" => "lz",
        "zst" => "zst",
        "ar" => "ar",
        // Audio
        "mp3" => "mp3",
        "wav" => "wav",
        "ogg" => "ogg",
        "flac" => "flac",
        "m4a" => "m4a",
        "aac" => "aac",
        "opus" => "opus",
        "amr" => "amr",
        "mid" | "midi" => "mid",
        "aiff" => "aiff",
        // Video
        "mp4" => "mp4",
        "webm" => "webm",
        "mkv" => "mkv",
        "mov" => "mov",
        "avi" => "avi",
        "flv" => "flv",
        "wmv" => "wmv",
        "m4v" => "m4v",
        "mpg" => "mpg",
        "3gp" => "3gp",
        // Fonts
        "ttf" => "ttf",
        "otf" => "otf",
        "woff" => "woff",
        "woff2" => "woff2",
        "eot" => "eot",
        // Executables / programs
        "exe" => "exe",
        "elf" => "elf",
        "wasm" => "wasm",
        "class" => "class",
        "dex" => "dex",
        "deb" => "deb",
        "rpm" => "rpm",
        "apk" => "apk",
        "swf" => "swf",
        "msi" => "msi",
        "dmg" => "dmg",
        "iso" => "iso",
        // Catch-all — pass through so the agent at least sees the
        // raw extension `infer` produced (`crx`, `nes`, etc.).
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

/// L68 — parse a raw HTTP `Content-Type` header into our extension
/// label. Strips the `; charset=…` parameter, lowercases, and maps
/// to the same vocabulary the content sniffer uses. Returns `None`
/// when the type is genuinely unknown so the caller can fall back
/// to content sniffing.
///
/// The most common cases (json/html/xml/css/csv/text/octet-stream)
/// are matched explicitly; everything else is bucketed by the slash
/// prefix (`image/*` → "bin", `audio/*` → "bin", …) — content
/// sniffing then picks the right specific format from the bytes.
pub fn mime_to_extension(content_type: &str) -> Option<&'static str> {
    let head = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match head.as_str() {
        "application/json" | "text/json" => "json",
        "application/ld+json" => "json",
        "application/graphql-response+json" | "application/graphql+json" => "graphql.json",
        "application/x-ndjson" | "application/ndjson" => "ndjson",
        "application/xml" | "text/xml" => "xml",
        "application/atom+xml" | "application/rss+xml" => "xml",
        "text/html" | "application/xhtml+xml" => "html",
        "text/css" => "css",
        "text/csv" => "csv",
        "text/tab-separated-values" => "tsv",
        "text/yaml" | "application/yaml" | "application/x-yaml" => "yaml",
        "text/markdown" => "md",
        "text/plain" => "txt",
        "application/pdf" => "pdf",
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/octet-stream" => "bin",
        "application/wasm" => "wasm",
        "application/javascript" | "text/javascript" => "js",
        "application/typescript" | "text/typescript" => "ts",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        other => {
            // Bucket by top-level type — we still know it's a
            // binary blob even if the subtype is exotic.
            if let Some(rest) = other
                .strip_prefix("image/")
                .or_else(|| other.strip_prefix("audio/"))
                .or_else(|| other.strip_prefix("video/"))
                .or_else(|| other.strip_prefix("font/"))
            {
                let _ = rest; // ignored — bucket result is `bin`
                return Some("bin");
            }
            return None;
        }
    })
}

/// True when `text` looks like a GraphQL response body — i.e. parses
/// as JSON AND the top-level object has at least one of `data` or
/// `errors`. The shape that distinguishes GraphQL from arbitrary
/// JSON.
pub fn looks_like_graphql(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    obj.contains_key("data") || obj.contains_key("errors")
}

/// Compose the probabilistic extension hint, e.g. `api.rest.json`,
/// `graphql.json`, `html`, `csv`, `bin`, `txt`.
///
/// Rules (highest priority wins):
///   1. `File(path)` with a non-empty extension → use the file's
///      extension verbatim (the path is authoritative).
///   2. `GraphQl` → `graphql.json` (regardless of detected kind —
///      the caller knows the protocol).
///   3. `Web` + detected kind = json → `graphql.json` (if shape
///      matches) else `api.rest.json`.
///   4. `Web` + detected kind ∈ {html, xml, csv, bin formats} →
///      `api.rest.<ext>`.
///   5. Default → just the detected kind extension.
pub fn compose_extension(text: &str, hint: &SourceHint) -> String {
    if let SourceHint::File(p) = hint {
        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            if !ext.is_empty() {
                return ext.to_string();
            }
        }
    }
    // L68 — when the caller has the HTTP Content-Type, prefer it
    // over content sniffing. The server knows what it sent; we
    // shouldn't second-guess.
    let kind_from_ct = if let SourceHint::Web {
        content_type: Some(ct),
    } = hint
    {
        mime_to_extension(ct)
    } else {
        None
    };
    let kind: String = kind_from_ct
        .map(|s| s.to_string())
        .unwrap_or_else(|| detect_kind(text).to_string());
    match hint {
        SourceHint::GraphQl => "graphql.json".to_string(),
        SourceHint::Web { .. } => match kind.as_str() {
            "graphql.json" => "graphql.json".to_string(),
            "json" => {
                if looks_like_graphql(text) {
                    "graphql.json".to_string()
                } else {
                    "api.rest.json".to_string()
                }
            }
            "html" => "html".to_string(),
            // A distilled web page IS markdown. `api.rest.md` would claim it
            // is a REST response, and the label reaches the stash FILENAME,
            // so it tells the next reader what to parse it as.
            "md" => "md".to_string(),
            "xml" => "api.rest.xml".to_string(),
            "csv" => "api.rest.csv".to_string(),
            "ndjson" => "api.rest.ndjson".to_string(),
            "pdf" | "png" | "jpg" | "gif" | "gz" | "zip" | "mp4" | "wasm" | "ogg" | "bin" => {
                "bin".to_string()
            }
            "txt" => "api.rest.txt".to_string(),
            other => format!("api.rest.{other}"),
        },
        SourceHint::File(_) | SourceHint::Shell | SourceHint::Mcp | SourceHint::Unknown => kind,
    }
}
