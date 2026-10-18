//! JSON that is a document: an outline, and an OpenAPI spec.
//!
//! A 2 MB API spec rendered as raw JSON is unreadable and unaffordable; as an
//! endpoint list it is both.

/// Render a JSON document as markdown: OpenAPI specs get an endpoint
/// reference (title / `## METHOD /path` / params / responses — the shape a
/// RAG pass can segment and an agent can answer from); anything else gets a
/// generic depth-capped outline with sampled values.
pub fn json_to_markdown(v: &serde_json::Value) -> String {
    if let Some(md) = openapi_to_markdown(v) {
        return md;
    }
    const CAP_CHARS: usize = 24_000;
    let mut out = String::new();
    json_outline(v, 0, &mut out);
    if out.chars().count() > CAP_CHARS {
        let mut s: String = out.chars().take(CAP_CHARS).collect();
        s.push_str("\n… (outline truncated)\n");
        return s;
    }
    out
}

/// One outline line per key with a scalar sample; nested objects recurse to
/// depth 4; arrays show their length and outline the first element.
pub(super) fn json_outline(v: &serde_json::Value, depth: usize, out: &mut String) {
    use serde_json::Value as V;
    if depth > 4 {
        return;
    }
    let pad = "  ".repeat(depth);
    match v {
        V::Object(m) => {
            for (k, val) in m.iter().take(60) {
                match val {
                    V::Object(_) => {
                        out.push_str(&format!("{pad}- {k}:\n"));
                        json_outline(val, depth + 1, out);
                    }
                    V::Array(a) => {
                        out.push_str(&format!("{pad}- {k}: [{} item(s)]\n", a.len()));
                        if let Some(first) = a.first() {
                            json_outline(first, depth + 1, out);
                        }
                    }
                    scalar => {
                        let s = scalar.to_string();
                        let s: String = s.chars().take(100).collect();
                        out.push_str(&format!("{pad}- {k}: {s}\n"));
                    }
                }
            }
            if m.len() > 60 {
                out.push_str(&format!("{pad}  (+{} more keys)\n", m.len() - 60));
            }
        }
        V::Array(a) => {
            out.push_str(&format!("{pad}[{} item(s)]\n", a.len()));
            if let Some(first) = a.first() {
                json_outline(first, depth + 1, out);
            }
        }
        scalar => {
            out.push_str(&format!("{pad}{}\n", scalar));
        }
    }
}

/// OpenAPI/Swagger → endpoint reference markdown. `None` when the document
/// isn't an OpenAPI spec.
pub(super) fn openapi_to_markdown(v: &serde_json::Value) -> Option<String> {
    if v.get("openapi").is_none() && v.get("swagger").is_none() {
        return None;
    }
    let paths = v.get("paths")?.as_object()?;
    let info = v.get("info");
    let title = info
        .and_then(|i| i.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("API");
    let version = info
        .and_then(|i| i.get("version"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let mut out = format!("# {title} {version} (OpenAPI)\n");
    if let Some(d) = info
        .and_then(|i| i.get("description"))
        .and_then(|d| d.as_str())
    {
        let d: String = d.chars().take(400).collect();
        out.push_str(&format!("> {}\n", d.replace('\n', " ")));
    }
    const CAP_CHARS: usize = 24_000;
    let mut dropped = 0usize;
    for (path, methods) in paths {
        let Some(methods) = methods.as_object() else {
            continue;
        };
        for (method, op) in methods {
            if !["get", "put", "post", "delete", "patch", "head", "options"]
                .contains(&method.as_str())
            {
                continue;
            }
            if out.chars().count() > CAP_CHARS {
                dropped += 1;
                continue;
            }
            out.push_str(&format!("\n## {} {path}\n", method.to_ascii_uppercase()));
            for key in ["summary", "description"] {
                if let Some(s) = op.get(key).and_then(|s| s.as_str()) {
                    let s: String = s.chars().take(300).collect();
                    out.push_str(&format!("{}\n", s.replace('\n', " ")));
                    break;
                }
            }
            if let Some(params) = op.get("parameters").and_then(|p| p.as_array()) {
                for p in params {
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let loc = p.get("in").and_then(|n| n.as_str()).unwrap_or("");
                    let req = p.get("required").and_then(|r| r.as_bool()).unwrap_or(false);
                    let ty = p
                        .pointer("/schema/type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    out.push_str(&format!(
                        "- param `{name}` ({loc}{}{})\n",
                        if ty.is_empty() {
                            String::new()
                        } else {
                            format!(", {ty}")
                        },
                        if req { ", required" } else { "" },
                    ));
                }
            }
            if let Some(resps) = op.get("responses").and_then(|r| r.as_object()) {
                let list: Vec<String> = resps
                    .iter()
                    .map(|(code, r)| {
                        let d = r.get("description").and_then(|d| d.as_str()).unwrap_or("");
                        format!("{code} {d}")
                    })
                    .collect();
                if !list.is_empty() {
                    out.push_str(&format!("- responses: {}\n", list.join(" · ")));
                }
            }
        }
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n({dropped} more operation(s) beyond the size cap)\n"
        ));
    }
    // Compact schema reference: property names + enum values are exactly
    // what "which values can X take" questions need.
    let schemas = v
        .pointer("/components/schemas")
        .or_else(|| v.get("definitions"))
        .and_then(|s| s.as_object());
    if let Some(schemas) = schemas {
        out.push_str("\n## Schemas\n");
        for (name, schema) in schemas.iter().take(40) {
            let mut props: Vec<String> = Vec::new();
            if let Some(ps) = schema.get("properties").and_then(|p| p.as_object()) {
                for (pname, p) in ps.iter().take(30) {
                    let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let en = p
                        .get("enum")
                        .and_then(|e| e.as_array())
                        .map(|a| {
                            let vals: Vec<String> = a
                                .iter()
                                .map(|x| {
                                    x.as_str()
                                        .map(str::to_string)
                                        .unwrap_or_else(|| x.to_string())
                                })
                                .collect();
                            format!(" ∈ [{}]", vals.join(", "))
                        })
                        .unwrap_or_default();
                    props.push(format!(
                        "{pname}{}{en}",
                        if ty.is_empty() {
                            String::new()
                        } else {
                            format!(": {ty}")
                        }
                    ));
                }
            }
            if props.is_empty() {
                out.push_str(&format!("- {name}\n"));
            } else {
                out.push_str(&format!("- {name} {{ {} }}\n", props.join(", ")));
            }
        }
    }
    Some(out)
}
