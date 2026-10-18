//! OpenAPI 3.x and Swagger 2.0 spec compaction.
//!
//! Specs are JSON (occasionally YAML; we cover JSON here, the YAML path
//! already runs through [`crate::yaml`]) and routinely span 100 KB – 5 MB
//! when an agent fetches `/openapi.json` from a real product. The shape is
//! always the same:
//!
//! ```text
//! { "openapi": "3.0.3",        // or "swagger": "2.0"
//!   "info":   { "title": "...", "version": "...", "description": "..." },
//!   "paths":  { "/users": { "get": {...}, "post": {...} }, "/users/{id}": {...}, … }
//!   "components": { "schemas": { "User": {…2 000 lines…}, "Pet": {…}, … },
//!                   "parameters": {…}, "responses": {…}, "securitySchemes": {…} } }
//! ```
//!
//! 99% of the bytes are nested `schemas`/`parameters`/`responses` blobs the
//! agent doesn't read; what it wants is the *list* of endpoints and the
//! *list* of available schemas by name. [`compact`] keeps:
//!   * `openapi`/`swagger` version + `info.title`/`info.version`/`info.description`
//!   * a per-path table of `METHOD /path — summary` (capped at 200 endpoints)
//!   * a flat list of `components.schemas` names (capped at 200)
//!   * security scheme names
//!   * a footer with the byte size of every dropped section so an agent who
//!     genuinely needs the raw spec can still find it.
//!
//! Drops: per-endpoint `parameters`, `requestBody`, `responses` bodies, the
//! `schemas` themselves, `parameters`/`responses` top-level dictionaries.

use serde_json::Value;
use std::fmt::Write;

/// Past this number of endpoints, the listing is folded.
pub const ENDPOINTS_CAP: usize = 200;
/// Paths listed (methods only) once the endpoints exceed [`ENDPOINTS_CAP`].
pub const PATHS_CAP: usize = 1000;
/// Past this number of schemas, the listing is folded.
pub const SCHEMAS_CAP: usize = 200;

/// HTTP methods OpenAPI considers path operations.
const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

pub fn detect(text: &str) -> bool {
    // Cheap head check: must look like JSON object that declares its
    // OpenAPI/Swagger version.
    let head: &str = text.get(..text.len().min(512)).unwrap_or(text);
    let t = head.trim_start();
    if !t.starts_with('{') {
        return false;
    }
    let has_openapi = t.contains("\"openapi\"") || t.contains("\"swagger\"");
    if !has_openapi {
        return false;
    }
    // Must have a paths object somewhere (otherwise it's not a useful spec).
    text.contains("\"paths\"")
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return text.to_string(),
    };
    let obj = match v.as_object() {
        Some(o) => o,
        None => return text.to_string(),
    };

    let version = obj
        .get("openapi")
        .and_then(|x| x.as_str())
        .or_else(|| obj.get("swagger").and_then(|x| x.as_str()))
        .unwrap_or("(unknown)");

    let info = obj.get("info").and_then(|x| x.as_object());
    let title = info
        .and_then(|i| i.get("title"))
        .and_then(|x| x.as_str())
        .unwrap_or("(no title)");
    let api_version = info
        .and_then(|i| i.get("version"))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let description = info
        .and_then(|i| i.get("description"))
        .and_then(|x| x.as_str())
        .map(|s| s.trim());

    let bytes_of = |key: &str| -> usize {
        obj.get(key)
            .map(|v| serde_json::to_string(v).map_or(0, |s| s.len()))
            .unwrap_or(0)
    };

    // Collect path/method/summary triples.
    let mut endpoints: Vec<(String, String, String)> = Vec::new();
    if let Some(paths) = obj.get("paths").and_then(|x| x.as_object()) {
        for (path, methods_obj) in paths.iter() {
            let Some(methods_obj) = methods_obj.as_object() else {
                continue;
            };
            for m in METHODS {
                if let Some(op) = methods_obj.get(*m).and_then(|x| x.as_object()) {
                    let summary = op
                        .get("summary")
                        .and_then(|x| x.as_str())
                        .or_else(|| op.get("operationId").and_then(|x| x.as_str()))
                        .or_else(|| op.get("description").and_then(|x| x.as_str()))
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    endpoints.push((path.clone(), m.to_uppercase(), summary));
                }
            }
        }
    }
    // Stable order: by path then by method (deterministic for tests).
    endpoints.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // Schemas: OpenAPI 3 lives in `components.schemas`; Swagger 2 in `definitions`.
    let schemas: Vec<String> = obj
        .get("components")
        .and_then(|c| c.as_object())
        .and_then(|c| c.get("schemas"))
        .and_then(|x| x.as_object())
        .map(|m| m.keys().cloned().collect())
        .or_else(|| {
            obj.get("definitions")
                .and_then(|x| x.as_object())
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default();

    // Security scheme names (OpenAPI 3 → components.securitySchemes,
    //                       Swagger 2 → securityDefinitions).
    let security_schemes: Vec<String> = obj
        .get("components")
        .and_then(|c| c.as_object())
        .and_then(|c| c.get("securitySchemes"))
        .and_then(|x| x.as_object())
        .map(|m| m.keys().cloned().collect())
        .or_else(|| {
            obj.get("securityDefinitions")
                .and_then(|x| x.as_object())
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default();

    let mut out = String::with_capacity(4096);
    let _ = writeln!(
        out,
        "version={version} title={title} api_version={api_version}"
    );
    if let Some(d) = description {
        if !d.is_empty() {
            let one_line = d.lines().next().unwrap_or("").trim();
            if !one_line.is_empty() {
                let _ = writeln!(out, "description: {one_line}");
            }
        }
    }

    // Endpoints table. Width-aligned for readability.
    let n_ep = endpoints.len();
    let _ = write!(out, "\nendpoints ({n_ep}):\n");
    let method_w = endpoints
        .iter()
        .take(ENDPOINTS_CAP)
        .map(|(_, m, _)| m.len())
        .max()
        .unwrap_or(6);
    let path_w = endpoints
        .iter()
        .take(ENDPOINTS_CAP)
        .map(|(p, _, _)| p.len())
        .max()
        .unwrap_or(20)
        .min(60);
    if n_ep <= ENDPOINTS_CAP {
        for (p, m, s) in &endpoints {
            if s.is_empty() {
                let _ = writeln!(out, "  {:<mw$}  {:<pw$}", m, p, mw = method_w, pw = path_w);
            } else {
                let _ = writeln!(
                    out,
                    "  {:<mw$}  {:<pw$}  — {}",
                    m,
                    p,
                    s,
                    mw = method_w,
                    pw = path_w
                );
            }
        }
    } else {
        // Past the cap the summaries go, not the endpoints: sorted by path, a
        // cap on rows dropped every route after the 200th alphabetically —
        // `/users/…` gone from a 500-endpoint spec, unnamed.
        let _ = writeln!(
            out,
            "  (summaries omitted past {ENDPOINTS_CAP} endpoints; one line per path)"
        );
        let mut paths: Vec<(&str, Vec<&str>)> = Vec::new();
        for (p, m, _) in &endpoints {
            match paths.last_mut() {
                Some((lp, ms)) if *lp == p.as_str() => ms.push(m),
                _ => paths.push((p, vec![m])),
            }
        }
        for (p, ms) in paths.iter().take(PATHS_CAP) {
            let _ = writeln!(out, "  {:<pw$}  {}", p, ms.join(" "), pw = path_w);
        }
        if paths.len() > PATHS_CAP {
            let _ = writeln!(out, "  … {} more path(s) elided", paths.len() - PATHS_CAP);
        }
    }

    // Schemas list.
    if !schemas.is_empty() {
        let n_sc = schemas.len();
        let _ = write!(out, "\nschemas ({n_sc}): ");
        let listed: Vec<&str> = schemas
            .iter()
            .take(SCHEMAS_CAP)
            .map(|s| s.as_str())
            .collect();
        out.push_str(&listed.join(", "));
        if n_sc > SCHEMAS_CAP {
            let _ = write!(out, ", … {} more elided", n_sc - SCHEMAS_CAP);
        }
        out.push('\n');
    }

    if !security_schemes.is_empty() {
        let _ = write!(out, "\nsecurity: {}\n", security_schemes.join(", "));
    }

    // Footer.
    let components_bytes = bytes_of("components");
    let definitions_bytes = bytes_of("definitions");
    let paths_bytes = bytes_of("paths");
    let mut dropped: Vec<String> = Vec::new();
    if paths_bytes > 0 {
        dropped.push(format!("paths-detail={paths_bytes}B"));
    }
    if components_bytes > 0 {
        dropped.push(format!("components={components_bytes}B"));
    }
    if definitions_bytes > 0 {
        dropped.push(format!("definitions={definitions_bytes}B"));
    }
    if !dropped.is_empty() {
        let _ = write!(
            out,
            "\ndropped (fetch raw spec for these): {}\n",
            dropped.join(", ")
        );
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_spec(n_paths: usize, schema_size: usize) -> String {
        let mut paths_obj = String::from("{");
        for i in 0..n_paths {
            if i > 0 {
                paths_obj.push(',');
            }
            let _ = write!(
                paths_obj,
                "\"/things/{i}\":{{\"get\":{{\"summary\":\"Get thing {i}\",\"parameters\":[],\"responses\":{{\"200\":{{\"description\":\"ok\"}}}}}}}}"
            );
        }
        paths_obj.push('}');
        let schema_padding = "x".repeat(schema_size);
        format!(
            r#"{{"openapi":"3.0.3","info":{{"title":"Test API","version":"1.0.0","description":"hello"}},"paths":{paths_obj},"components":{{"schemas":{{"Pet":{{"type":"object","description":"{schema_padding}"}},"Error":{{"type":"object"}}}},"securitySchemes":{{"apiKey":{{"type":"apiKey","in":"header","name":"X-API-Key"}}}}}}}}"#
        )
    }

    #[test]
    fn detects_openapi_3() {
        let s = r#"{"openapi":"3.0.0","info":{"title":"x","version":"1"},"paths":{}}"#;
        assert!(detect(s));
    }

    #[test]
    fn detects_swagger_2() {
        let s = r#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#;
        assert!(detect(s));
    }

    #[test]
    fn rejects_plain_json() {
        assert!(!detect(r#"{"foo":"bar","paths":{}}"#));
        assert!(!detect(r#"{"openapi":"3.0.0"}"#)); // no paths
        assert!(!detect("not json"));
    }

    #[test]
    fn keeps_endpoints_drops_schema_bodies() {
        let s = synth_spec(5, 5000);
        let out = compact(&s);
        assert!(out.contains("version=3.0.3"));
        assert!(out.contains("title=Test API"));
        assert!(out.contains("api_version=1.0.0"));
        assert!(out.contains("description: hello"));
        assert!(out.contains("endpoints (5):"));
        assert!(out.contains("Get thing 0"));
        assert!(out.contains("Get thing 4"));
        assert!(out.contains("GET"));
        assert!(out.contains("/things/0"));
        // Schema list yes, body no.
        assert!(out.contains("Pet"));
        assert!(out.contains("Error"));
        assert!(!out.contains("xxxxxxxx"));
        // Security scheme name.
        assert!(out.contains("apiKey"));
        // Footer.
        assert!(out.contains("dropped"));
    }

    #[test]
    fn past_the_cap_every_path_stays_without_its_summary() {
        let s = synth_spec(300, 100);
        let out = compact(&s);
        assert!(out.contains("endpoints (300):"));
        assert!(!out.contains("elided"), "{out}");
        // Lexicographically last — the row a cap on 200 dropped.
        assert!(out.contains("/things/99 "), "{out}");
        assert!(out.contains("/things/299 "), "{out}");
        assert!(!out.contains("Get thing"), "{out}");
    }

    #[test]
    fn past_the_cap_one_line_carries_all_methods_of_a_path() {
        let mut paths = String::from("{");
        for i in 0..250 {
            let _ = write!(paths, "\"/r/{i}\":{{\"get\":{{}}}},");
        }
        paths.push_str("\"/users/{id}\":{\"get\":{},\"put\":{},\"delete\":{}}}");
        let s = format!(
            r#"{{"openapi":"3.0.0","info":{{"title":"t","version":"1"}},"paths":{paths}}}"#
        );
        let out = compact(&s);
        let line = out
            .lines()
            .find(|l| l.contains("/users/{id}"))
            .unwrap_or_else(|| panic!("{out}"));
        assert!(line.ends_with("DELETE GET PUT"), "{line}");
        assert_eq!(out.matches("/users/{id}").count(), 1, "{out}");
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not an openapi spec at all";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn malformed_passes_through() {
        // Looks like an OpenAPI spec by the head check but is truncated JSON.
        let t = r#"{"openapi":"3.0.0","paths":{"/x":"#;
        let out = compact(t);
        assert_eq!(out, t);
    }

    #[test]
    fn savings_real_shape() {
        // 50 endpoints, 100KB of schema padding.
        let s = synth_spec(50, 100_000);
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("endpoints (50):"));
        assert!(sav.percent() > 90.0, "got {sav:?}");
    }

    #[test]
    fn handles_swagger_2_definitions() {
        let s = r#"{"swagger":"2.0","info":{"title":"Old","version":"1.0"},"paths":{"/x":{"get":{"summary":"Hello"}}},"definitions":{"Foo":{"type":"object"},"Bar":{"type":"object"}},"securityDefinitions":{"basic":{"type":"basic"}}}"#;
        let out = compact(s);
        assert!(out.contains("version=2.0"));
        assert!(out.contains("endpoints (1):"));
        assert!(out.contains("Hello"));
        assert!(out.contains("schemas (2)"));
        assert!(out.contains("Foo"));
        assert!(out.contains("basic"));
    }
}
