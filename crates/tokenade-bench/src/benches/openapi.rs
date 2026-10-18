//! `openapi` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_openapi(n_endpoints: usize, n_schemas: usize, schema_padding: usize) -> String {
    let mut paths = String::from("{");
    for i in 0..n_endpoints {
        if i > 0 {
            paths.push(',');
        }
        // Alternate methods to mimic real specs.
        let methods = match i % 4 {
            0 => "get",
            1 => "post",
            2 => "put",
            _ => "delete",
        };
        paths.push_str(&format!(
            r##""/api/v1/resources/{i}":{{"{methods}":{{"summary":"Operation {i}","operationId":"op_{i}","parameters":[{{"name":"id","in":"path","schema":{{"type":"string"}}}}],"responses":{{"200":{{"description":"ok","content":{{"application/json":{{"schema":{{"$ref":"#/components/schemas/Item"}}}}}}}}}}}}}}"##
        ));
    }
    paths.push('}');

    let mut schemas = String::from("{");
    let pad = "x".repeat(schema_padding);
    for j in 0..n_schemas {
        if j > 0 {
            schemas.push(',');
        }
        schemas.push_str(&format!(
            r#""Schema_{j}":{{"type":"object","description":"{pad}","properties":{{"id":{{"type":"string"}},"name":{{"type":"string"}}}}}}"#
        ));
    }
    schemas.push('}');

    format!(
        r#"{{"openapi":"3.0.3","info":{{"title":"Synthetic API","version":"1.2.3","description":"Adversarial bench fixture"}},"servers":[{{"url":"https://api.example.com"}}],"paths":{paths},"components":{{"schemas":{schemas},"securitySchemes":{{"bearerAuth":{{"type":"http","scheme":"bearer"}}}}}}}}"#
    )
}

/// Realistic OpenAPI spec: 80 endpoints + 40 schemas × 2 KB each.
pub(crate) fn task_openapi_real(results: &mut Vec<RunResult>) {
    let raw = synth_openapi(80, 40, 2_000);
    let (out, sav, ms) = measure(&raw, openapi::compact_with_savings);
    let quality = if out.contains("version=3.0.3")
        && out.contains("title=Synthetic API")
        && out.contains("api_version=1.2.3")
        && out.contains("endpoints (80):")
        && out.contains("schemas (40):")
        && out.contains("/api/v1/resources/0")
        && out.contains("/api/v1/resources/79")
        && out.contains("bearerAuth")
        && !out.contains("xxxxxxxx")
        && sav.percent() > 85.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:openapi-compact",
        "openapi",
        "synth-80ep-40sc",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #47: 1 000-endpoint, 500-schema, 5 MB spec (matches what
// you actually fetch from a real productivity API at scale). Must compact
// >90% and run under 2 s.
pub(crate) fn adv_openapi_huge(results: &mut Vec<RunResult>) {
    let raw = synth_openapi(1_000, 500, 8_000);
    let (out, sav, ms) = measure(&raw, openapi::compact_with_savings);
    let quality = if out.contains("endpoints (1000):")
        && out.contains("schemas (500):")
        && sav.percent() > 90.0
        && ms < 2000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:openapi:huge-1000ep",
        "openapi",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #48: hostile shape — missing top-level `info`, paths with
// non-object values (a real bug we've seen in poorly-generated specs),
// truncated JSON. Must not panic; truncated input passes through unchanged;
// quirky-but-valid input still extracts what it can.
pub(crate) fn adv_openapi_hostile(results: &mut Vec<RunResult>) {
    // Truncated JSON.
    let truncated = r#"{"openapi":"3.0.0","paths":{"/x":{"get":"#;
    let st = Instant::now();
    let (out1, _) = openapi::compact_with_savings(truncated);
    let ms1 = st.elapsed().as_millis();

    // Valid OpenAPI but with weird path values (non-object). detect() passes,
    // compact() must skip non-object methods cleanly.
    let weird = r#"{"openapi":"3.0.0","info":{"title":"W","version":"1"},"paths":{"/a":"oops not an object","/b":{"get":{"summary":"ok"}}}}"#;
    let (out2, _) = openapi::compact_with_savings(weird);

    // Combined contract: no panic, truncated passes through verbatim,
    // weird-but-valid still surfaces /b GET.
    let quality = if out1 == truncated
        && out2.contains("/b")
        && out2.contains("ok")
        && !out2.contains("oops")
        && ms1 < 50
    {
        1.0
    } else {
        0.0
    };
    // Report on the truncated case (raw + out) so size accounting is honest.
    record(
        results,
        "adv:openapi:hostile",
        "openapi",
        "adversarial",
        truncated,
        &out1,
        ms1,
        quality,
    );
}
