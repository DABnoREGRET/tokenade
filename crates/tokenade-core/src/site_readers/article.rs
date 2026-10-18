//! Publisher article pages, described by a spec instead of a function.
//!
//! Britannica, MasterClass and their neighbours differ only in which
//! container holds the body and which JSON-LD type names it — so the reader
//! is one function plus a table of [`ArticleSpec`].

use super::*;

/// Where a site keeps the article prose.
pub enum ArticleBody {
    /// A field of the page's schema.org article object (`articleBody` for a
    /// news site, `abstract` for a paper). Preferred when present: it is the
    /// publisher's own idea of "the text", with zero chrome to trim.
    JsonLdField(&'static str),
    /// Inner HTML of a container, optionally cut at the first occurrence of a
    /// trailing-chrome marker INSIDE it (a promo block the container wraps).
    Container {
        selector: &'static str,
        cut_at: Option<&'static str>,
    },
    /// Everything between two literal markers. For sites whose article is a
    /// RUN of sibling elements with no single wrapper — britannica fences its
    /// with `<!--[BEFORE-ARTICLE]-->` / `<!--[AFTER-ARTICLE]-->` comments, and
    /// every quiz/FAQ/related-questions block sits outside that fence.
    Between {
        start: &'static str,
        end: &'static str,
    },
}

/// Per-site recipe for [`article_page`]. One row per site; everything else
/// (title, byline, date, the thin-parse floor) is shared.
pub struct ArticleSpec {
    pub via: &'static str,
    /// DOM title, tried BEFORE the JSON-LD headline. Set it where the
    /// structured headline is SEO copy rather than the page's own title.
    pub title_sel: Option<&'static str>,
    /// DOM byline/date line, for sites that ship no article JSON-LD at all.
    pub byline_sel: Option<&'static str>,
    /// Report `dateModified` rather than `datePublished`. True for a reference
    /// work that revises entries in place — britannica's JFK entry is stamped
    /// `datePublished: 1999` and `dateModified: 2026`, and "1999" is the wrong
    /// answer to "how current is this?". For NEWS the opposite holds: the
    /// publication date is the fact, and a typo fix must not re-date the story.
    pub prefer_modified: bool,
    pub body: ArticleBody,
    /// Compare the delivered text against the JSON-LD `wordcount` and say so
    /// when the page holds only part of the article. Britannica splits long
    /// entries across sibling URLs while every one of them advertises the
    /// wordcount of the WHOLE piece — without this an agent reads a quarter of
    /// a biography and has no way to know it.
    pub check_wordcount: bool,
    /// Appended verbatim when the page is knowingly partial by nature.
    pub note: Option<&'static str>,
}

/// MasterClass articles, read from the Next.js payload rather than the DOM.
///
/// The visible article is a run of grid `<div>`s whose only stable anchor is
/// the Bootstrap class `col-lg-8` — a layout class, shared with whatever else
/// the page lays out, and worth abandoning a reader over. The same page ships
/// `__NEXT_DATA__`, where the article is already structured: one object per
/// section, title and body separated, plus the byline and dates the DOM only
/// renders as prose.
///
/// It also fixes something the DOM could not: masterclass answers **200 with a
/// full-size page** for a slug that does not exist (`how-to-write-a-short-story`
/// — no `<title>`, an h1 reading "Oops, this page doesn't exist", absent from
/// the sitemap). A DOM reader happily returns that page's navigation as "the
/// article". The payload of a soft-404 carries NO `article` key at all, so
/// requiring it rejects the dead page as a side effect of reading the live one.
pub(super) fn masterclass_article(html: &str) -> Option<String> {
    let raw = re(r#"(?is)<script id="__NEXT_DATA__"[^>]*>(.*?)</script>"#)
        .captures(html)?
        .get(1)?
        .as_str();
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let a = v.pointer("/props/pageProps/article")?;
    let s = |k: &str| {
        a.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.trim().is_empty())
    };

    let mut out = format!("# {}\n", s("title")?.trim());
    let mut meta: Vec<String> = Vec::new();
    if let Some(author) = s("author") {
        meta.push(format!("by {author}"));
    }
    // `updatedAt` over `creationDate`: these are evergreen how-tos revised in
    // place, and the page itself shows "Last updated".
    if let Some(d) = s("updatedAt").or_else(|| s("creationDate")) {
        meta.push(d.split('T').next().unwrap_or(d).to_string());
    }
    if let Some(cat) = a.pointer("/category/name").and_then(|c| c.as_str()) {
        meta.push(cat.to_string());
    }
    // `readTime` is SECONDS as a float (274.2), not minutes — printing it raw
    // would advertise a 274-minute read.
    if let Some(secs) = a
        .get("readTime")
        .and_then(|r| r.as_f64())
        .filter(|s| *s > 0.0)
    {
        meta.push(format!("{} min read", (secs / 60.0).ceil() as u64));
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — masterclass.com\n", meta.join(" • ")));
    }
    if let Some(lede) = s("descriptionPlainText")
        .map(str::to_string)
        .or_else(|| s("description").map(crate::web::fragment_to_markdown))
    {
        out.push_str(&format!("\n{}\n", lede.trim()));
    }

    let mut sections = 0usize;
    for sub in a
        .get("articleSubsections")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        let body_html = sub
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        // Every article ends on a membership pitch, structurally identical to a
        // real section. Its TITLE varies by category ("Want to Learn More About
        // Writing?" / "…About Music?"), so match the copy that does not: all
        // three sampled pages sell the same "MasterClass Annual Membership".
        if body_html.contains("Annual Membership") {
            continue;
        }
        let body = crate::web::fragment_to_markdown(body_html);
        if body.trim().is_empty() {
            continue;
        }
        if let Some(t) = sub
            .get("title")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            out.push_str(&format!("\n## {t}\n"));
        }
        out.push_str(&format!("\n{}\n", body.trim()));
        sections += 1;
    }
    // Title + byline with no body is a payload shape we do not understand;
    // fail open rather than serve a header as an article.
    (sections > 0).then(|| out.trim_end().to_string())
}

/// Indeed job-search results. The page is a ~1.4 MB React shell that distils
/// to ~1200 words of filter chrome and boilerplate; the actual listing is a
/// JSON payload assigned to `window.mosaic.providerData["mosaic-provider-jobcards"]`,
/// holding one structured record per offer. Rendering those records gives the
/// agent title/employer/location/salary/contract per job — the things a job
/// search is FOR — instead of the surrounding page furniture.
pub(super) fn indeed_jobs(html: &str) -> Option<String> {
    const CAP_CHARS: usize = 16_000;
    // Anchor on the ASSIGNMENT, not the bare provider name: the string
    // "mosaic-provider-jobcards" occurs 257 times on a real page (every card's
    // tracking attributes carry it) and `find` would land on the first one.
    let obj = extract_js_object(html, r#"providerData["mosaic-provider-jobcards"]"#)?;
    let v: serde_json::Value = serde_json::from_str(obj).ok()?;
    let results = v
        .pointer("/metaData/mosaicProviderJobCardsModel/results")?
        .as_array()?;
    if results.is_empty() {
        return None;
    }
    // Job links need the LOCALE host (fr.indeed.com ≠ www.indeed.com); take it
    // from the page's own canonical URL rather than assuming.
    let origin = crate::web::meta_content(html, "og:url")
        .or_else(|| {
            re(&format!(
                r#"(?is)<link[^>]+rel=["']canonical["'][^>]+href=\s*{}"#,
                crate::web::ATTR_VALUE
            ))
            .captures(html)
            .and_then(|c| crate::web::quoted_value(&c, 1).map(str::to_string))
        })
        .and_then(|u| {
            let rest = u.split_once("://")?;
            Some(format!("{}://{}", rest.0, rest.1.split('/').next()?))
        })
        .unwrap_or_else(|| "https://www.indeed.com".to_string());

    let title = crate::web::meta_content(html, "og:title")
        .or_else(|| {
            re(r"(?is)<title[^>]*>(.*?)</title>")
                .captures(html)
                .map(|c| crate::web::decode_entities(c[1].trim()))
        })
        .unwrap_or_else(|| "Indeed job search".to_string());
    let mut out = format!(
        "# {title}\n> {} offers on this page — indeed\n",
        results.len()
    );

    let s = |j: &serde_json::Value, k: &str| -> String {
        j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let mut len = 0usize;
    let mut dropped = 0usize;
    for j in results {
        if len > CAP_CHARS {
            dropped += 1;
            continue;
        }
        let titl = s(j, "displayTitle");
        if titl.is_empty() {
            continue;
        }
        let mut facts: Vec<String> = Vec::new();
        for k in ["company", "formattedLocation"] {
            let v = s(j, k);
            if !v.is_empty() {
                facts.push(v);
            }
        }
        if j.get("remoteLocation").and_then(|r| r.as_bool()) == Some(true) {
            facts.push("remote".into());
        }
        if let Some(types) = j.get("jobTypes").and_then(|t| t.as_array()) {
            let t: Vec<&str> = types.iter().filter_map(|x| x.as_str()).collect();
            if !t.is_empty() {
                facts.push(t.join("/"));
            }
        }
        // The publisher's own formatted string first — it is already localised
        // and carries the range; the numeric fields are the fallback, and their
        // `-1` sentinel means "no upper bound", not a salary of minus one.
        let salary = j
            .pointer("/salarySnippet/text")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .or_else(|| {
                let e = j.get("extractedSalary")?;
                let (min, max) = (e.get("min")?.as_f64()?, e.get("max")?.as_f64()?);
                let unit = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
                Some(if max > 0.0 && max != min {
                    format!("{min:.0}–{max:.0} ({unit})")
                } else {
                    format!("from {min:.0} ({unit})")
                })
            });
        if let Some(sal) = salary.filter(|s| !s.trim().is_empty()) {
            facts.push(sal);
        }
        let age = s(j, "formattedRelativeTime");
        if !age.is_empty() {
            facts.push(age);
        }
        let mut entry = format!("\n## {titl}\n{}\n", facts.join(" · "));
        // The snippet is an HTML `<ul>` — render it, then keep one line of it.
        let snip = crate::web::fragment_to_markdown(&s(j, "snippet"));
        let snip = snip.split_whitespace().collect::<Vec<_>>().join(" ");
        if !snip.is_empty() {
            let short: String = snip.chars().take(220).collect();
            entry.push_str(&format!("{short}\n"));
        }
        let key = s(j, "jobkey");
        if !key.is_empty() {
            entry.push_str(&format!("{origin}/viewjob?jk={key}\n"));
        }
        len += entry.chars().count();
        out.push_str(&entry);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n({dropped} further offer(s) beyond the size cap not shown)\n"
        ));
    }
    Some(out.trim_end().to_string())
}

/// Plan that re-fetches the article's own URL and renders it with `spec`.
/// Query and fragment are dropped: they are tracking/anchor noise on every one
/// of these sites, and keeping them would split the session fetch cache.
pub(super) fn article_plan(host: &str, path: &str, spec: &'static ArticleSpec) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    Some(Plan::get(
        format!("https://{host}{clean}"),
        Kind::Article(spec),
        spec.via,
    ))
}

/// Render an article page: title, byline, date, then the prose the spec points
/// at. Shared by every per-site article reader.
pub(super) fn article_page(spec: &ArticleSpec, html: &str) -> Option<String> {
    let ld = json_ld_object(html, ARTICLE_TYPES);
    let ld_str = |key: &str| -> Option<String> {
        let v = ld.as_ref()?.get(key)?;
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            // `author` is `{"@type":"Person","name":…}`, or an array of them.
            serde_json::Value::Object(o) => o.get("name")?.as_str()?.to_string(),
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|x| match x {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Object(o) => {
                        o.get("name").and_then(|n| n.as_str()).map(str::to_string)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => return None,
        };
        let s = crate::web::decode_entities(s.trim());
        (!s.is_empty()).then_some(s)
    };

    let dom_text = |sel: Option<&str>| -> Option<String> {
        let md = crate::web::fragment_to_markdown(&crate::web::extract_by_selector(html, sel?)?);
        let md = md.trim().to_string();
        (!md.is_empty()).then_some(md)
    };
    // Title: the page's own <h1> first where the spec says so, then the
    // structured headline, then og:title LAST. og:title is the least
    // trustworthy of the three — IGN ships an SEO rewrite there ("From a
    // $1,000 Golden Mega Darkrai…") that is not the headline on the page, and
    // the generic path prints both, one after the other. Britannica's JSON-LD
    // headline is keyword-stuffed the same way ("… | Biography, Family,
    // Presidency, Assassination, & Facts").
    let title = dom_text(spec.title_sel)
        .or_else(|| ld_str("headline"))
        .or_else(|| crate::web::meta_content(html, "og:title"))?;
    let mut out = format!("# {title}\n");

    let author = ld_str("author")
        .or_else(|| crate::web::meta_content(html, "article:author"))
        .or_else(|| dom_text(spec.byline_sel));
    // PUBLISHED, not modified. Both cbsnews and IGN put `dateModified` in the
    // DOM `<time>` element while the real publication time lives in the
    // structured data — a reader keying off `<time>` reports the wrong date on
    // every updated story.
    let date = if spec.prefer_modified {
        ld_str("dateModified").or_else(|| ld_str("datePublished"))
    } else {
        ld_str("datePublished").or_else(|| crate::web::meta_content(html, "article:published_time"))
    }
    .map(|d| d.split('T').next().unwrap_or(&d).to_string());
    let meta: Vec<String> = [author, date].into_iter().flatten().collect();
    if !meta.is_empty() {
        out.push_str(&format!("> {} — {}\n", meta.join(" • "), spec.via));
    }

    let body = match &spec.body {
        ArticleBody::JsonLdField(field) => ld_str(field)?,
        ArticleBody::Container { selector, cut_at } => {
            let inner = crate::web::extract_by_selector(html, selector)?;
            let inner = match cut_at.and_then(|m| inner.find(m)) {
                Some(i) => &inner[..i],
                None => inner.as_str(),
            };
            crate::web::fragment_to_markdown(inner)
        }
        ArticleBody::Between { start, end } => {
            let from = html.find(start)? + start.len();
            let to = html[from..]
                .find(end)
                .map(|i| from + i)
                .unwrap_or(html.len());
            crate::web::fragment_to_markdown(&html[from..to])
        }
    };
    if body.trim().is_empty() {
        return None;
    }
    out.push_str(&format!("\n{}\n", body.trim()));
    // Say it when this URL is only a slice of the article.
    if spec.check_wordcount {
        let whole = ld.as_ref().and_then(|v| v.get("wordcount")).and_then(|w| {
            w.as_u64()
                .or_else(|| w.as_str().and_then(|s| s.parse::<u64>().ok()))
        });
        let here = body.split_whitespace().count() as u64;
        // 15% slack: `wordcount` counts the publisher's own way, and a reader
        // that cried "partial" on every complete page would be noise.
        if let Some(whole) = whole.filter(|w| here * 115 / 100 < *w) {
            out.push_str(&format!(
                "\n[partial article: this page holds ~{here} of {whole} words. \
                 The rest is on the sibling section URLs listed in this page's \
                 table of contents.]\n"
            ));
        }
    }
    if let Some(note) = spec.note {
        out.push_str(&format!("\n{note}\n"));
    }
    Some(out.trim_end().to_string())
}

pub(super) fn britannica_plan(host: &str, path: &str) -> Option<Plan> {
    article_plan(host, path, &BRITANNICA)
}

pub(super) fn masterclass_plan(host: &str, path: &str) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    Some(Plan::get(
        format!("https://{host}{clean}"),
        Kind::MasterclassArticle,
        "masterclass.com (article)",
    ))
}

pub(super) fn cbsnews_plan(host: &str, path: &str) -> Option<Plan> {
    article_plan(host, path, &CBSNEWS)
}

pub(super) fn ign_plan(host: &str, path: &str) -> Option<Plan> {
    article_plan(host, path, &IGN)
}

pub(super) fn academia_plan(host: &str, path: &str) -> Option<Plan> {
    article_plan(host, path, &ACADEMIA)
}

/// Article-page recipes, one per site. A site earns a row only where the
/// generic distillation MEASURABLY loses — chrome that dominates the output,
/// or metadata it gets wrong. Sites whose generic output is already clean are
/// deliberately absent: indeed.com career-advice (8.6% chrome), PMC article
/// pages (5.3%, and the rest is real author/affiliation metadata). Adding a
/// reader there would be churn with no measured win.
///
/// quora.com is absent for a different reason: its answers are never in the
/// HTML. The relay payload (`window.ansFrontendGlobals.data.inlineQueryResults`)
/// ships as empty stubs to logged-out clients, so there is nothing for ANY
/// reader to parse — the dashboard's "write a reader" verdict is unachievable
/// there, not merely unimplemented.
pub(crate) const BRITANNICA: ArticleSpec = ArticleSpec {
    via: "britannica.com (article)",
    title_sel: Some("h1.tw:inline"),
    byline_sel: None,
    prefer_modified: true,
    // The article is a RUN of `<section data-level=N>` siblings; the quiz,
    // "Top Questions" FAQ, related-questions and news blocks sit outside the
    // comment fence, which is why the fence beats any wrapper class.
    body: ArticleBody::Between {
        start: "<!--[BEFORE-ARTICLE]-->",
        end: "<!--[AFTER-ARTICLE]-->",
    },
    check_wordcount: true,
    note: None,
};

pub(crate) const CBSNEWS: ArticleSpec = ArticleSpec {
    via: "cbsnews.com (article)",
    title_sel: None,
    byline_sel: None,
    prefer_modified: false,
    // `articleBody` is the complete prose (verified against the container tail
    // on three stories); it omits only photo captions and agency credits.
    body: ArticleBody::JsonLdField("articleBody"),
    check_wordcount: false,
    note: None,
};

pub(crate) const IGN: ArticleSpec = ArticleSpec {
    via: "ign.com (article)",
    title_sel: None,
    byline_sel: None,
    prefer_modified: false,
    // No `articleBody` in IGN's JSON-LD — the prose only exists in the DOM.
    // The container is `<div data-cy="article-content" class="jsx-2870106660
    // article-content page-0">`; anchor on the `article-content` CLASS TOKEN
    // and never on the `jsx-…` sibling token, which is a build artifact that
    // rotates on every deploy. (`data-cy` would be the other stable anchor,
    // but attribute selectors are not something the extractor needs to grow
    // for a single site.)
    body: ArticleBody::Container {
        selector: "div.article-content",
        cut_at: None,
    },
    check_wordcount: false,
    note: None,
};

pub(crate) const ACADEMIA: ArticleSpec = ArticleSpec {
    via: "academia.edu (abstract)",
    title_sel: None,
    byline_sel: None,
    prefer_modified: false,
    // Prefer the JSON-LD abstract over the DOM one: the page ships the full
    // text in `ScholarlyArticle.abstract` while the visible container is
    // truncated by `js-truncate-component--abstract`.
    body: ArticleBody::JsonLdField("abstract"),
    check_wordcount: false,
    note: Some(
        "[abstract only — academia.edu keeps the full text in a PDF behind \
         a download/registration wall, so it is not in this page.]",
    ),
};

/// schema.org types that denote "this page is an article".
pub(crate) const ARTICLE_TYPES: &[&str] = &[
    "Article",
    "NewsArticle",
    "ScholarlyArticle",
    "TechArticle",
    "BlogPosting",
    "ReportageNewsArticle",
];
