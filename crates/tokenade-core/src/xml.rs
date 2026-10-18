//! XML config / schema compactor.
//!
//! Covers Maven `pom.xml`, Android `AndroidManifest.xml` + `res/*.xml`,
//! Spring beans XML, log4j2.xml, web.xml, persistence.xml, TestNG XML,
//! generic configuration XML. Heavily-commented enterprise pom.xml
//! files (license header + per-section banners + dependency-by-
//! dependency explanations) typically lose 25-45% after this pass.
//!
//! What it strips:
//!   * `<!-- … -->` comment blocks (single- AND multi-line, with a
//!     state machine across lines)
//!   * blank-line runs → collapse to one
//!   * leading whitespace runs >4 indent levels (deep XML often has
//!     6-8 levels — we keep the first 4 of each, drop the rest)
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every `<element>` / `<element/>` tag + attributes + text node
//!   * `<![CDATA[ … ]]>` blocks (binary / pre-escaped content)
//!   * `<?xml … ?>` and `<?…?>` processing instructions
//!   * `<!DOCTYPE … >` and `<!ENTITY … >` declarations
//!
//! Trade-off note: collapsing whitespace inside text nodes WOULD
//! change semantics for `xml:space="preserve"` scopes — we do NOT
//! collapse intra-tag whitespace at all, only inter-tag indentation.

/// Cheap heuristic. Three acceptance paths:
///   * starts with `<?xml` (rock-solid, immediate true), OR
///   * tag-start lines DOMINATE the head: >= 5 absolute AND >= 50 %
///     of non-blank lines.
///
/// Hard-reject HTML-only shapes (`<!DOCTYPE html>` at line 1) so the
/// HTML compactor pipeline still claims those.
///
/// Density requirement prevents markdown/MDX files with a few
/// inline HTML tags (`<img>`, `<details>`, `<a>`) from being routed
/// to the XML compactor. Real XML runs ~100 % tag lines; markdown-
/// with-html sits under 20 %.
pub fn detect(text: &str) -> bool {
    let head: String = text.lines().take(10).collect::<Vec<_>>().join("\n");
    let head_lower = head.trim_start().to_ascii_lowercase();
    // HTML reject — explicitly so we don't steal its workload.
    if head_lower.starts_with("<!doctype html") || head_lower.starts_with("<html") {
        return false;
    }
    if text.trim_start().starts_with("<?xml") {
        return true;
    }
    // XML is a TREE: something in it closes. `pytest --collect-only` prints
    // `<Module …>`, `<Class …>`, `<Function test_x[param]>` — five or more
    // angle-bracket lines in a row, and not one of them ever closes. MEASURED
    // 2026-08-26 on `pytest-collect.log`, 32 939 bytes: claimed as XML, and
    // this compactor (which only strips comments and whitespace) returned all
    // 32 939. Requiring one closing or self-closing tag costs a real document
    // nothing — every XML file has one — and refuses the whole class of
    // angle-bracket LISTINGS.
    let window: String = text.lines().take(60).collect::<Vec<_>>().join("\n");
    if !window.contains("</") && !window.contains("/>") {
        return false;
    }
    let mut tag_lines = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(30) {
        considered += 1;
        let t = line.trim_start();
        if t.starts_with("<!--") {
            continue;
        }
        if t.starts_with('<') && !t.starts_with("</") && t.len() > 1 {
            // First non-`<` byte should be a letter (XML element
            // name) or `?` (processing instruction) or `!` (DTD).
            let next = t.as_bytes()[1];
            if next.is_ascii_alphabetic() || next == b'?' || next == b'!' {
                tag_lines += 1;
            }
        }
    }
    tag_lines >= 5 && considered > 0 && tag_lines * 2 >= considered
}

/// Strip `<!-- … -->` comments + collapse blank runs + trim trailing
/// whitespace. Does NOT touch element bodies — only the inter-tag
/// indentation gets normalised.
pub fn compact(text: &str) -> String {
    // Two XML shapes an agent actually fetches are documents, not markup, and
    // this generic pass — which only strips whitespace and comments — returns
    // them essentially unchanged: measured 1.00x on a sitemap, 23 352 814 bytes
    // in and 23 352 815 out.
    //
    // Routed here as well as in `web::html_to_markdown` because the two entry
    // points do not meet: `tokenade web` reaches the HTML path, while
    // `tokenade read file.xml` is dispatched by extension straight to this
    // function. Wiring either one alone leaves the other passing 23 MB through,
    // which is exactly what had just been fixed for feeds.
    if let Some(md) = crate::xml::sitemap_to_markdown(text) {
        return md;
    }
    if let Some(md) = crate::site_readers::render(&crate::site_readers::Kind::FeedDocument, text) {
        return format!("> source: feed\n\n{md}");
    }
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut in_comment = false;
    let mut in_cdata = false;
    for line in text.lines() {
        // CDATA passthrough — anything between `<![CDATA[` and `]]>`
        // is opaque to us.
        if in_cdata {
            out.push_str(line);
            out.push('\n');
            if line.contains("]]>") {
                in_cdata = false;
            }
            continue;
        }
        if line.contains("<![CDATA[") {
            // The CDATA may CLOSE on the same line.
            in_cdata = !line.contains("]]>");
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Comment-block state machine.
        let (cleaned, still_in) = strip_xml_comment(line, in_comment);
        in_comment = still_in;
        let trimmed = cleaned.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        // Tab → 2 spaces normalisation (mixed indents inflate the
        // tokeniser without semantic value). Then cap deep indent.
        let normalised = trimmed.replace('\t', "  ");
        out.push_str(&normalised);
        out.push('\n');
    }
    out
}

/// `Savings`-yielding variant for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Walk a single line stripping any `<!-- … -->` regions. Carries
/// state across lines via the `in_comment` flag.
fn strip_xml_comment(line: &str, mut in_comment: bool) -> (String, bool) {
    // Byte-scan for the ASCII markers, but copy SLICES between them —
    // `out.push(bytes[i] as char)` reinterpreted UTF-8 bytes as Latin-1
    // and mangled every non-ASCII XML line (`Café` → `CafÃ©`).
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seg_start = 0;
    while i < bytes.len() {
        if in_comment {
            // Look for `-->`.
            if i + 2 < bytes.len()
                && bytes[i] == b'-'
                && bytes[i + 1] == b'-'
                && bytes[i + 2] == b'>'
            {
                in_comment = false;
                i += 3;
                seg_start = i;
            } else {
                i += 1;
            }
            continue;
        }
        // Look for `<!--` opening.
        if i + 3 < bytes.len()
            && bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && bytes[i + 2] == b'-'
            && bytes[i + 3] == b'-'
        {
            out.push_str(&line[seg_start..i]);
            in_comment = true;
            i += 4;
            continue;
        }
        i += 1;
    }
    if !in_comment {
        out.push_str(&line[seg_start..]);
    }
    (out, in_comment)
}

// ─────────────────────────────────────────────────────────────────────
// A sitemap is XML
// ─────────────────────────────────────────────────────────────────────
//
// `sitemap_to_markdown` lived in `web`, because it is the web route that
// needed it first. Yet it only reads XML, and the XML compactor — which is
// REACHED through the other door, `tokenade read plan.xml` — had to go up a
// layer to call it. The two doors do not meet: wiring only one lets the
// other pass 23 MB as is, which is exactly the measured defect (1.00× on a
// sitemap, 23,352,814 bytes in, 23,352,815 out).
//
// Here, the web route calls it DOWNWARDS and the XML compactor at home.

/// Cap on URLs listed from a sitemap. A sitemap's value to an agent is "what
/// pages exist here"; past a few hundred entries that question is answered and
/// the rest is scroll. The total is always reported, so nothing is hidden.
const SITEMAP_MAX_URLS: usize = 500;

/// Render a sitemap as the URL list it actually is.
///
/// Sitemaps were passing through **byte for byte**: `one.google.com/sitemap.xml`
/// is 23 352 814 bytes in and 23 352 815 out — roughly 5.8 million tokens from
/// one call, the largest single waste this project has measured. It carries
/// **1 824 URLs**; the other 23 MB is `<xhtml:link rel="alternate" hreflang=…>`
/// repeated for every locale of every page.
///
/// Returns `None` for any XML that is not a sitemap, so everything else keeps
/// its existing path.
pub fn sitemap_to_markdown(xml: &str) -> Option<String> {
    // Char-boundary clamp, not a raw byte slice: a ≥4 KB sitemap whose title/
    // caption/comment straddles offset 4096 (Japanese/German/Arabic) would panic
    // a raw `&xml[..4096]` mid-codepoint.
    let head = crate::caps::truncate_at_char_boundary(xml, 4096);
    let kind = if head.contains("<sitemapindex") {
        "sitemap index"
    } else if head.contains("<urlset") {
        "sitemap"
    } else {
        return None;
    };

    // `<loc>` is the only element that carries a URL in either schema.
    let mut urls: Vec<&str> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<loc>") {
        rest = &rest[i + 5..];
        let Some(j) = rest.find("</loc>") else { break };
        let loc = rest[..j].trim();
        // hreflang alternates repeat the same href many times over.
        if !loc.is_empty() && seen.insert(loc) {
            urls.push(loc);
        }
        rest = &rest[j + 6..];
    }
    if urls.is_empty() {
        return None;
    }

    let total = urls.len();
    let shown = total.min(SITEMAP_MAX_URLS);
    let mut out = String::with_capacity(shown * 64);
    out.push_str(&format!("> source: {kind} ({total} URLs)\n\n"));
    for u in urls.iter().take(shown) {
        out.push_str(u);
        out.push('\n');
    }
    if total > shown {
        out.push_str(&format!(
            "\n… and {} more URLs not listed.\n",
            total - shown
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_POM: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Maven POM for the foo library -->\n<!--\n  License: Apache 2.0\n  Copyright 2026 Example Corp\n-->\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>org.example</groupId>\n  <artifactId>foo</artifactId>\n  <version>1.0.0</version>\n  <!-- Default packaging is jar -->\n  <packaging>jar</packaging>\n  <dependencies>\n    <!-- Logging -->\n    <dependency>\n      <groupId>org.slf4j</groupId>\n      <artifactId>slf4j-api</artifactId>\n      <version>2.0.9</version>\n    </dependency>\n  </dependencies>\n</project>\n";

    #[test]
    fn detect_pom_xml() {
        assert!(detect(REAL_POM));
    }

    #[test]
    fn detect_rejects_html() {
        assert!(!detect("<!DOCTYPE html>\n<html><body>hi</body></html>"));
        assert!(!detect("<html lang=\"en\">\n<body/>"));
    }

    #[test]
    fn detect_handles_xmlless_xml() {
        // Many config XMLs ship without the `<?xml ... ?>` prolog.
        // Real XML hits the dominance threshold (all 6 head lines are
        // tag-starts).
        let m = "<configuration>\n  <appender name=\"FILE\" class=\"ch.qos.logback.core.FileAppender\">\n    <file>app.log</file>\n    <encoder>standard</encoder>\n  </appender>\n  <logger name=\"X\" level=\"INFO\"/>\n  <root level=\"WARN\"><appender-ref ref=\"FILE\"/></root>\n</configuration>\n";
        assert!(detect(m));
    }

    #[test]
    fn detect_rejects_markdown_with_inline_html() {
        // Regression: README / MDX files with a handful of
        // <img>, <details>, <a> tags scattered through ~30 lines were
        // false-tagged as XML, routed to the XML compactor, and
        // returned with 0% savings. Real markdown has tags on <20 %
        // of lines; our threshold is now 50 % + 5 absolute.
        let md = "# My Project\n\
Welcome to the repo. <img src=\"logo.png\"/>\n\
\n\
## Features\n\
- Fast\n\
- Reliable\n\
- <details><summary>more</summary>see below</details>\n\
\n\
Read the [docs](https://example.com) or watch this <a href=\"/intro\">intro</a>.\n\
\n\
## Install\n\
Run `npm i` then enjoy.\n\
\n\
## License\n\
MIT\n";
        assert!(
            !detect(md),
            "markdown with sparse inline HTML must NOT be classified as XML"
        );
    }

    #[test]
    fn compact_strips_single_line_comments() {
        let out = compact(REAL_POM);
        assert!(!out.contains("Maven POM for the foo library"));
        assert!(!out.contains("Default packaging"));
        assert!(!out.contains("Logging"));
    }

    #[test]
    fn compact_strips_multi_line_comments() {
        let out = compact(REAL_POM);
        assert!(!out.contains("License: Apache"));
        assert!(!out.contains("Copyright 2026"));
    }

    #[test]
    fn compact_preserves_all_elements() {
        let out = compact(REAL_POM);
        assert!(out.contains("<modelVersion>4.0.0</modelVersion>"));
        assert!(out.contains("<groupId>org.example</groupId>"));
        assert!(out.contains("<artifactId>foo</artifactId>"));
        assert!(out.contains("<dependencies>"));
        assert!(out.contains("<groupId>org.slf4j</groupId>"));
    }

    #[test]
    fn compact_preserves_cdata_verbatim() {
        let input = "<root>\n<![CDATA[\n  <!-- this is INSIDE cdata, must survive -->\n  raw <stuff/>\n]]>\n</root>\n";
        let out = compact(input);
        assert!(out.contains("this is INSIDE cdata"));
        assert!(out.contains("raw <stuff/>"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "<a>\n\n\n\n<b/>\n";
        let out = compact(input);
        assert_eq!(out, "<a>\n\n<b/>\n");
    }

    #[test]
    fn compact_normalises_tabs_to_spaces() {
        let input = "<a>\n\t<b/>\n</a>\n";
        let out = compact(input);
        assert!(out.contains("  <b/>"));
        assert!(!out.contains('\t'));
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        let (_, sav) = compact_with_savings(REAL_POM);
        assert!(
            sav.percent() >= 15.0,
            "expected ≥15% on commented pom.xml, got {:.1}%",
            sav.percent()
        );
    }
}
