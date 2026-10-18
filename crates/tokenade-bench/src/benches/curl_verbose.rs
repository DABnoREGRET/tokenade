//! `curl_verbose` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// A realistic 80-line `curl -v` transcript (TLS handshake + cert chain
/// dominate; the request/response is tiny).
pub(crate) fn synth_curl_verbose() -> String {
    let mut t = String::from(
        "*   Trying 1.2.3.4:443...\n* Connected to api.example.com (1.2.3.4) port 443\n* ALPN: offers h2,http/1.1\n",
    );
    for i in 0..30 {
        t.push_str(&format!(
            "* TLSv1.3 (IN), TLS handshake, Encrypted Extensions ({i}):\n"
        ));
    }
    t.push_str(
        "* SSL connection using TLSv1.3 / TLS_AES_256_GCM_SHA384\n\
* Server certificate:\n*  subject: CN=api.example.com\n",
    );
    for _ in 0..12 {
        t.push_str("*  subjectAltName: DNS:api.example.com\n");
    }
    t.push_str(
        "*  start date: Jan 1 00:00:00 2024 GMT\n\
*  expire date: Jan 1 00:00:00 2025 GMT\n\
*  issuer: CN=Let's Encrypt R3, O=Let's Encrypt, C=US\n\
*  SSL certificate verify ok.\n\
> GET /api/v1/users?limit=20 HTTP/2\n> Host: api.example.com\n> user-agent: curl/8.4.0\n> accept: */*\n> authorization: Bearer xxx\n>\n< HTTP/2 200\n< content-type: application/json\n< content-length: 42\n< date: Mon, 01 Jan 2024 00:00:00 GMT\n< server: nginx\n< x-request-id: abc-123\n<\n{\"users\":[{\"id\":1,\"name\":\"alice\"}]}\n* Connection #0 to host api.example.com left intact\n",
    );
    t
}

pub(crate) fn task_curl_verbose(results: &mut Vec<RunResult>) {
    let raw = synth_curl_verbose();
    let t = Instant::now();
    let (out, sav) = tokenade_logs::compact_curl_verbose_with_savings(&raw);
    let ms = t.elapsed().as_millis();
    // Quality: request line + status + body all preserved; TLS noise folded.
    let kept_request = out.contains("GET /api/v1/users?limit=20 HTTP/2");
    let kept_status = out.contains("HTTP/2 200");
    let kept_body = out.contains("\"alice\"");
    let folded =
        out.contains("TLS-handshake lines elided") && out.contains("cert-chain lines elided");
    let dropped_cleanup = !out.contains("left intact");
    let quality = if kept_request && kept_status && kept_body && folded && dropped_cleanup {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:curl-verbose-compact",
        "logs",
        "synth-curl-v",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}
