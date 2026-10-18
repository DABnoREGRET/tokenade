//! Source-map (`*.map`) compaction.
//!
//! Source maps are JSON documents emitted by JS/TS/CSS bundlers. They're
//! used for debugging minified bundles back to original source. Real maps
//! are routinely 1–50 MB on a single line:
//!
//! ```text
//! {"version":3,"file":"main.min.js",
//!  "sources":["./src/a.js","./src/b.js",…1 000 more],
//!  "sourcesContent":["// full source of a.js …",… complete source of every input],
//!  "names":["foo","bar",… 10 000 identifiers],
//!  "mappings":"AAAA;EAAA,IAAI,GAAG,iBAAiB,IAAI,…  base64 VLQ for hundreds of KB"}
//! ```
//!
//! An agent that opens a `.map` file by mistake — common when chasing a
//! stack trace — gets megabytes of base64 noise for at most a few hundred
//! bytes of useful structure (which file? which sources?).
//!
//! [`compact`] keeps:
//!   * `version` and `file`,
//!   * the list of `sources` (truncated past 50 entries),
//!   * a one-line summary of the dropped fields with their byte size.
//!
//! Drops:
//!   * `mappings` (only useful programmatically),
//!   * `sourcesContent` (the agent already has the source on disk),
//!   * `names` (verbose identifier table).

use serde_json::Value;
use std::fmt::Write;

/// Past this number of `sources` entries, the listing is folded.
pub const SOURCES_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    // The opening brace + `"version":3` token must be in the leading bytes,
    // but `"mappings"` can be anywhere because real source maps inline tens
    // of KB of `sourcesContent` BEFORE the mappings field. We scan the whole
    // input for that one — it's a single linear pass and source maps are
    // typically a SINGLE LINE so we visit each byte once anyway.
    let head: &str = text.get(..text.len().min(256)).unwrap_or(text);
    let t = head.trim_start();
    t.starts_with('{')
        && (t.contains("\"version\":3") || t.contains("\"version\": 3"))
        && text.contains("\"mappings\"")
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // We deliberately use serde_json on the WHOLE thing rather than a
    // streaming approach: source maps are valid JSON and agents send them
    // through us all-at-once; serde_json handles a 50 MB string in ~100 ms.
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return text.to_string(),
    };
    let obj = match v.as_object() {
        Some(o) => o,
        None => return text.to_string(),
    };

    let version = obj.get("version").and_then(|x| x.as_i64()).unwrap_or(0);
    let file = obj
        .get("file")
        .and_then(|x| x.as_str())
        .unwrap_or("(no file)");
    let source_root = obj.get("sourceRoot").and_then(|x| x.as_str());

    // Compute dropped sizes for the summary footer.
    let bytes_of = |key: &str| -> usize {
        obj.get(key)
            .map(|v| serde_json::to_string(v).map_or(0, |s| s.len()))
            .unwrap_or(0)
    };
    let mappings_bytes = bytes_of("mappings");
    let sources_content_bytes = bytes_of("sourcesContent");
    let names_bytes = bytes_of("names");

    // Render.
    let mut out = String::with_capacity(2048);
    let _ = write!(out, "version={version} file={file}");
    if let Some(sr) = source_root {
        let _ = write!(out, ", sourceRoot={sr}");
    }
    out.push('\n');

    if let Some(sources) = obj.get("sources").and_then(|x| x.as_array()) {
        let n = sources.len();
        let _ = write!(out, "\nsources ({n}):\n");
        // A bundle's map lists hundreds of `node_modules` files, often BEFORE
        // the app's own: the first 50 rows were all dependencies and the
        // sources the reader wrote never showed. A package is one line with
        // its file count; the cap applies to the files listed one by one.
        let mut own: Vec<&str> = Vec::new();
        let mut packages: Vec<(String, usize)> = Vec::new();
        for path in sources.iter().filter_map(|s| s.as_str()) {
            match package_of(path) {
                Some(pkg) => match packages.iter_mut().find(|(p, _)| *p == pkg) {
                    Some((_, c)) => *c += 1,
                    None => packages.push((pkg, 1)),
                },
                None => own.push(path),
            }
        }
        for path in own.iter().take(SOURCES_CAP) {
            out.push_str("  ");
            out.push_str(path);
            out.push('\n');
        }
        if own.len() > SOURCES_CAP {
            let _ = writeln!(out, "  … {} more source(s) elided", own.len() - SOURCES_CAP);
        }
        if !packages.is_empty() {
            let _ = writeln!(out, "  packages ({}):", packages.len());
            for (pkg, c) in &packages {
                let _ = writeln!(out, "    {pkg} ({c} file(s))");
            }
        }
    }

    // Footer: surface what we dropped so the agent knows the size of the
    // hidden content (in case they really need the raw map for tooling).
    let mut dropped: Vec<String> = Vec::new();
    if mappings_bytes > 0 {
        dropped.push(format!("mappings={mappings_bytes}B"));
    }
    if sources_content_bytes > 0 {
        dropped.push(format!("sourcesContent={sources_content_bytes}B"));
    }
    if names_bytes > 0 {
        dropped.push(format!("names={names_bytes}B"));
    }
    if !dropped.is_empty() {
        let _ = write!(
            out,
            "\ndropped (use raw .map file for these): {}\n",
            dropped.join(", ")
        );
    }
    out.trim_end().to_string()
}

/// `webpack://app/./node_modules/@scope/pkg/lib/x.js` → `@scope/pkg`. The
/// LAST `node_modules/` wins: a nested copy is its own package.
fn package_of(path: &str) -> Option<String> {
    let rest = &path[path.rfind("node_modules/")? + "node_modules/".len()..];
    let mut parts = rest.split('/');
    let first = parts.next().filter(|p| !p.is_empty())?;
    if first.starts_with('@') {
        Some(format!("{first}/{}", parts.next()?))
    } else {
        Some(first.to_string())
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_map(n_sources: usize, sources_content_size: usize, mappings_size: usize) -> String {
        let mut s = String::from(r#"{"version":3,"file":"main.min.js","sources":["#);
        for i in 0..n_sources {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "\"webpack:///./src/file_{i}.js\"");
        }
        s.push_str(r#"],"sourcesContent":["#);
        for i in 0..n_sources {
            if i > 0 {
                s.push(',');
            }
            s.push('"');
            s.push_str(&"x".repeat(sources_content_size / n_sources.max(1)));
            s.push('"');
        }
        s.push_str(r#"],"names":["foo","bar","baz"],"mappings":""#);
        s.push_str(&"A".repeat(mappings_size));
        s.push_str("\"}");
        s
    }

    #[test]
    fn detects_source_map() {
        assert!(detect(r#"{"version":3,"file":"x.js","mappings":"AAAA"}"#));
        assert!(detect(
            r#"{"version": 3, "file":"x.js", "mappings":"AAAA"}"#
        ));
        assert!(!detect(r#"{"version":3,"file":"x.js"}"#)); // no mappings
        assert!(!detect("not json")); // not even json
    }

    #[test]
    fn keeps_sources_drops_mappings_and_content() {
        let m = synth_map(5, 5000, 10000);
        let out = compact(&m);
        assert!(out.contains("version=3"));
        assert!(out.contains("file=main.min.js"));
        assert!(out.contains("webpack:///./src/file_0.js"));
        assert!(out.contains("webpack:///./src/file_4.js"));
        // No mappings VLQ in the output.
        assert!(!out.contains("AAAAAAAA"));
        // Footer reports dropped sizes.
        assert!(out.contains("mappings="));
        assert!(out.contains("sourcesContent="));
    }

    #[test]
    fn dependencies_fold_to_packages_and_own_sources_all_show() {
        let mut srcs: Vec<String> = Vec::new();
        for i in 0..120 {
            srcs.push(format!(
                "webpack://app/./node_modules/react-dom/cjs/f{i}.js"
            ));
        }
        for i in 0..30 {
            srcs.push(format!(
                "webpack://app/./node_modules/@babel/runtime/helpers/h{i}.js"
            ));
        }
        srcs.push("webpack://app/./node_modules/a/node_modules/lodash/x.js".into());
        srcs.push("webpack://app/./src/App.tsx".into());
        srcs.push("webpack://app/./src/index.ts".into());
        let list: Vec<String> = srcs.iter().map(|s| format!("\"{s}\"")).collect();
        let m = format!(
            r#"{{"version":3,"file":"main.js","sources":[{}],"mappings":"AAAA"}}"#,
            list.join(",")
        );
        let out = compact(&m);
        assert!(
            out.contains("src/App.tsx") && out.contains("src/index.ts"),
            "{out}"
        );
        assert!(out.contains("    react-dom (120 file(s))"), "{out}");
        assert!(out.contains("    @babel/runtime (30 file(s))"), "{out}");
        assert!(out.contains("    lodash (1 file(s))"), "{out}");
        assert!(out.contains("packages (3):"), "{out}");
        assert!(!out.contains("elided"), "{out}");
    }

    #[test]
    fn caps_long_source_lists() {
        let m = synth_map(200, 1000, 1000);
        let out = compact(&m);
        assert!(out.contains("sources (200):"));
        assert!(out.contains("more source(s) elided"));
        // File 0 kept, file 100 dropped (cap 50).
        assert!(out.contains("file_0.js"));
        assert!(!out.contains("file_100.js"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not a source map at all";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        // Realistic: 30 sources, 50 KB of embedded source content, 200 KB of
        // base64 VLQ mappings.
        let m = synth_map(30, 50_000, 200_000);
        let (out, sav) = compact_with_savings(&m);
        assert!(out.contains("30"));
        assert!(sav.percent() > 95.0, "got {:?}", sav);
    }
}
