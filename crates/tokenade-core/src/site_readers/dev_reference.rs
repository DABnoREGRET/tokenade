//! **Stack Exchange and crates.io**: the question already asked, and the
//! crate already published.
//!
//! Two sites an agent consults for a FACT, not for reading: the accepted
//! answer, or a crate's description and versions. Both serve a SPA to an
//! anonymous client — crates.io yields **32 extractable characters** — and
//! both expose a public API that carries exactly what one came looking for.

use super::*;

/// Rebuild a Stack Exchange question page as structured Q&A markdown.
pub(crate) fn stackexchange_page(html: &str) -> Option<String> {
    const CAP_CHARS: usize = 24_000;
    // Post bodies live in `s-prose js-post-body` divs. Non-greedy to the
    // first `</div>` — plain-prose posts have no nested divs; a snippet
    // widget can truncate a body early, which degrades to a shorter (never
    // wrong) answer.
    let body_re = re(r#"(?is)<div class="s-prose js-post-body"[^>]*>(.*?)</div>"#);
    let title = crate::web::meta_content(html, "og:title").or_else(|| {
        re(r#"(?is)<h1[^>]*itemprop="name"[^>]*>(.*?)</h1>"#)
            .captures(html)
            .map(|c| crate::web::fragment_to_markdown(c.get(1).unwrap().as_str()))
    })?;
    let mut out = format!("# {}\n", title.trim());

    let answer_open = re(r#"(?s)<div[^>]*\bid="answer-\d+"[^>]*>"#);
    let starts: Vec<(usize, bool)> = answer_open
        .find_iter(html)
        .map(|m| (m.start(), m.as_str().contains("accepted-answer")))
        .collect();
    // Question body: the first post body before the first answer.
    let head_end = starts.first().map(|(s, _)| *s).unwrap_or(html.len());
    let q_md = body_re
        .captures(&html[..head_end])
        .map(|c| crate::web::fragment_to_markdown(c.get(1).unwrap().as_str()))
        .unwrap_or_default();
    if q_md.is_empty() && starts.is_empty() {
        return None;
    }
    if !starts.is_empty() {
        out.push_str(&format!("> {} answer(s) — stack exchange\n", starts.len()));
    }
    if !q_md.is_empty() {
        out.push_str(&format!("\n## Question\n\n{q_md}\n"));
    }
    let score_re = re(r#"data-score="(-?\d+)""#);
    let mut answers: Vec<(i64, bool, String)> = Vec::new();
    for (i, &(start, accepted)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|(s, _)| *s).unwrap_or(html.len());
        let slice = &html[start..end];
        let Some(c) = body_re.captures(slice) else {
            continue;
        };
        let body = crate::web::fragment_to_markdown(c.get(1).unwrap().as_str());
        if body.is_empty() {
            continue;
        }
        let score = score_re
            .captures(slice)
            .and_then(|c| c[1].parse::<i64>().ok())
            .unwrap_or(0);
        answers.push((score, accepted, body));
    }
    // Highest-voted first, accepted breaking ties — the render order IS the
    // retrieval prior.
    answers.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut len = 0usize;
    let mut dropped = 0usize;
    for (score, accepted, body) in &answers {
        if len > CAP_CHARS {
            dropped += 1;
            continue;
        }
        let tag = if *accepted { ", accepted" } else { "" };
        let entry = format!("\n## Answer ({score} votes{tag})\n\n{body}\n");
        len += entry.chars().count();
        out.push_str(&entry);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n({dropped} lower-voted answer(s) beyond the size cap not shown)\n"
        ));
    }
    Some(out.trim_end().to_string())
}

/// Render the crates.io registry API response for one crate.
pub(crate) fn crates_api(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let c = v.get("crate")?;
    let s = |key: &str| {
        c.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let name = s("name");
    if name.is_empty() {
        return None;
    }
    let mut out = format!("# {name} (crates.io)\n");
    let desc = s("description");
    if !desc.is_empty() {
        out.push_str(&format!("> {desc}\n"));
    }
    let mut facts: Vec<String> = Vec::new();
    for (label, key) in [
        ("latest stable", "max_stable_version"),
        ("newest", "newest_version"),
    ] {
        let val = s(key);
        if !val.is_empty() {
            facts.push(format!("{label}: {val}"));
        }
    }
    for (label, key) in [
        ("downloads", "downloads"),
        ("recent downloads", "recent_downloads"),
    ] {
        if let Some(n) = c.get(key).and_then(|x| x.as_u64()) {
            facts.push(format!("{label}: {n}"));
        }
    }
    let updated = s("updated_at");
    if !updated.is_empty() {
        facts.push(format!(
            "updated: {}",
            crate::caps::truncate_at_char_boundary(&updated, 10)
        ));
    }
    if !facts.is_empty() {
        out.push_str(&format!("\n{}\n", facts.join(" • ")));
    }
    for (label, key) in [
        ("Repository", "repository"),
        ("Documentation", "documentation"),
        ("Homepage", "homepage"),
    ] {
        let val = s(key);
        if !val.is_empty() {
            out.push_str(&format!("- {label}: {val}\n"));
        }
    }
    for (label, key) in [("Keywords", "keywords"), ("Categories", "categories")] {
        let items: Vec<String> = v
            .get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.get("id").and_then(|i| i.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !items.is_empty() {
            out.push_str(&format!("- {label}: {}\n", items.join(", ")));
        }
    }
    let versions: Vec<String> = v
        .get("versions")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .take(5)
                .filter_map(|ver| {
                    let num = ver.get("num")?.as_str()?;
                    let date = ver
                        .get("created_at")
                        .and_then(|d| d.as_str())
                        .map(|d| crate::caps::truncate_at_char_boundary(d, 10))
                        .unwrap_or("");
                    let yanked = ver.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false);
                    Some(format!(
                        "{num} ({date}){}",
                        if yanked { " [yanked]" } else { "" }
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if !versions.is_empty() {
        out.push_str(&format!("- Recent versions: {}\n", versions.join(", ")));
    }
    Some(out.trim_end().to_string())
}
