//! **Which line is the USER's?**
//!
//! A real trace is dominated by framework frames —
//! `site-packages/django/...`, `node_modules/...`, `/rustc/...` — that nobody
//! is going to fix. The frame that matters is the first one that is not one
//! of them.
//!
//! And once found, it is worth going to READ its source: a trace without the
//! faulty line forces one more round-trip, which costs more than the few
//! lines just saved.

use super::*;

/// Inline the signature of the throwing frame's function into a compacted
/// trace — ONE extra line, looked up from the local source file so the agent
/// can read the error without opening it. Returns the input unchanged when no
/// user frame resolves to a readable local source, the language has no symbol
/// extractor, or `TOKENADE_NO_STACK_ENRICH` is set.
///
/// `root` is the directory frame paths resolve against (the agent's cwd);
/// absolute frame paths are also tried as-is. Vendor/stdlib frames are skipped.
/// This is the only part of the compactor that touches the filesystem, so it
/// runs in the live hook/CLI path (with a real cwd), never in the pure
/// `compact` used by property tests.
pub fn enrich_top_user_frame(compacted: &str, root: &std::path::Path) -> String {
    if std::env::var_os("TOKENADE_NO_STACK_ENRICH").is_some() {
        return compacted.to_string();
    }
    let frame_re = re(r"([A-Za-z0-9_./\\@+-]+\.(py|rs|js|jsx|ts|tsx|mjs|cjs|go|java|rb)):(\d+)");
    // (output line index, path, ext, line-number) for resolvable user frames.
    let mut hits: Vec<(usize, String, String, usize)> = Vec::new();
    for (i, line) in compacted.lines().enumerate() {
        let Some(cap) = frame_re.captures(line) else {
            continue;
        };
        let path = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let ext = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let Ok(lno) = cap
            .get(3)
            .map(|m| m.as_str())
            .unwrap_or("")
            .parse::<usize>()
        else {
            continue;
        };
        if path.is_empty() || is_vendor_path(path) {
            continue;
        }
        hits.push((i, path.to_string(), ext.to_string(), lno));
    }
    if hits.is_empty() {
        return compacted.to_string();
    }
    // Throw site: the LAST user frame for Python (traceback lists the throw
    // site last); the FIRST for the JS / Rust / Java / Go families (error
    // first). Enrich exactly one frame.
    let pick = if matches!(detect_lang(compacted), Lang::Python) {
        hits.len() - 1
    } else {
        0
    };
    let (target_line, path, ext, lno) = &hits[pick];
    let Some(src) = read_frame_source(root, path) else {
        return compacted.to_string();
    };
    let Some(sig) = crate::treesitter::signature_enclosing_line(&src, ext, *lno) else {
        return compacted.to_string();
    };

    let mut out = String::with_capacity(compacted.len() + sig.len() + 12);
    for (i, line) in compacted.lines().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i == *target_line {
            out.push_str("    ↳ ");
            out.push_str(&sig);
            out.push('\n');
        }
    }
    if !compacted.ends_with('\n') {
        out.pop();
    }
    out
}

/// Vendor / stdlib / package-manager frame paths whose source the agent never
/// wants the signature of.
pub(crate) fn is_vendor_path(path: &str) -> bool {
    const MARKERS: &[&str] = &[
        "node_modules/",
        "site-packages/",
        "dist-packages/",
        "/.cargo/registry/",
        "/rustc/",
        "/vendor/",
        "/go/pkg/mod/",
        "/usr/lib/",
        "/usr/local/lib/",
    ];
    MARKERS.iter().any(|m| path.contains(m))
}

/// Read a frame's source file, resolving relative paths under `root` and
/// trying absolute paths as-is. Caps file size so a pathological frame can't
/// load a huge file into memory.
pub(crate) fn read_frame_source(root: &std::path::Path, path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    let candidate = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    let meta = std::fs::metadata(&candidate).ok()?;
    if !meta.is_file() || meta.len() > 2_000_000 {
        return None;
    }
    std::fs::read_to_string(&candidate).ok()
}

// ---- regexes (cached) ----------------------------------------------------

pub(crate) fn re(p: &'static str) -> &'static Regex {
    static CACHE: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Regex>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(Default::default()));
    let mut g = cache.lock().unwrap();
    if let Some(r) = g.get(p) {
        return r;
    }
    let leaked: &'static Regex = Box::leak(Box::new(Regex::new(p).unwrap()));
    g.insert(p, leaked);
    leaked
}

pub(crate) fn re_py_frame() -> &'static Regex {
    // `  File "<path>", line N, in <name>`
    re(r#"(?m)^\s*File\s+"([^"]+)",\s+line\s+(\d+),\s+in\s+(\S+)"#)
}
pub(crate) fn re_js_frame() -> &'static Regex {
    // `    at <name> (<path>:N:M)`  /  `    at <path>:N:M`
    re(r"(?m)^\s*at\s+(?:[^(]+\()?([^():]+):(\d+):(\d+)\)?")
}
pub(crate) fn re_java_frame() -> &'static Regex {
    // `    at com.foo.Bar.baz(Bar.java:N)`
    re(r"(?m)^\s*at\s+([\w.$]+)\(([^)]+)\)")
}

// ---- user/framework classification ---------------------------------------

pub(crate) fn is_python_framework(path: &str) -> bool {
    [
        "/site-packages/",
        "/dist-packages/",
        "/.venv/",
        "/venv/",
        "/usr/lib/python",
        "/usr/local/lib/python",
        "/python3",
        "/lib/python",
        "/conda/",
        "/opt/homebrew/",
    ]
    .iter()
    .any(|m| path.contains(m))
}

pub(crate) fn is_rust_framework(path_or_func: &str) -> bool {
    [
        "/.cargo/registry/",
        "/rustc/",
        "/toolchains/",
        "rustc_",
        "core::",
        "alloc::",
        "std::",
        "tokio::",
        "futures_util::",
        "hyper::",
        "tower::",
    ]
    .iter()
    .any(|m| path_or_func.contains(m))
}

pub(crate) fn is_js_framework(path: &str) -> bool {
    [
        "/node_modules/",
        "node:internal/",
        "node:events",
        "/.next/",
        "/dist/",
    ]
    .iter()
    .any(|m| path.contains(m))
        || path.starts_with("internal/")
        || path.starts_with("node:")
}

pub(crate) fn is_java_framework(fqcn: &str) -> bool {
    [
        "java.",
        "javax.",
        "jdk.",
        "sun.",
        "org.springframework.",
        "org.hibernate.",
        "com.intellij.",
        "org.junit.",
        "kotlin.",
        "scala.",
    ]
    .iter()
    .any(|m| fqcn.starts_with(m))
}

pub(crate) fn is_go_framework(path_or_pkg: &str) -> bool {
    [
        "/go/src/runtime/",
        "GOROOT",
        "/vendor/",
        "runtime.",
        "net/http.",
        "sync.",
    ]
    .iter()
    .any(|m| path_or_pkg.contains(m))
}
