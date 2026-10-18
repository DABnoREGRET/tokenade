//! old.reddit HTML: threads, listings and search results.
//!
//! The modern frontend answers a non-browser fetch with a "Please wait for
//! verification" interstitial that distils to zero characters; old.reddit
//! serves the same content server-rendered. These parse that markup.

use super::*;

/// Reddit paths that are a LISTING of submissions rather than a thread.
///
/// Deliberately narrow: only shapes old.reddit still serves. `/about/…`,
/// `/wiki/…`, `/settings` and the like carry no submission list, so they keep
/// falling through to the generic pipeline rather than rendering as an empty
/// listing.
pub(super) fn is_reddit_listing(path: &str) -> bool {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    re(r"(?x)
        ^/r/[^/]+/?$
      | ^/r/[^/]+/(hot|new|top|best|rising|controversial|gilded)/?$
      | ^/r/[^/]+/search/?$
      | ^/user/[^/]+/?$
      | ^/user/[^/]+/(submitted|posts|comments|overview|saved|upvoted)/?$
      | ^/search/?$
      | ^/(hot|new|top|rising|best|controversial)/?$
      | ^/domain/[^/]+/?$
    ")
    .is_match(clean)
}

/// Parse an old.reddit thread page: title, OP selftext, flat comment list
/// (author + score + body, in the server's ranking order).
pub(super) fn reddit_thread(html: &str) -> Option<String> {
    // The Atom feed first: it is what the plan requests since 2026-09-11.
    if is_a_feed(html) {
        return super::feed::feed_document(html);
    }
    let title = crate::web::meta_content(html, "og:title").or_else(|| {
        re(r#"(?is)<a class="title[^"]*"[^>]*>(.*?)</a>"#)
            .captures(html)
            .map(|c| crate::web::fragment_to_markdown(c.get(1).unwrap().as_str()))
    })?;
    let mut out = format!("# {}\n", title.trim());
    let mut meta: Vec<String> = Vec::new();
    if let Some(c) = re(r#"data-subreddit-prefixed="([^"]+)""#).captures(html) {
        meta.push(c[1].to_string());
    }
    // The submission (`data-type="link"`) carries author + score as attrs.
    if let Some(link_tag) = re(r#"(?s)<div[^>]*?data-type="link"[^>]*?>"#).find(html) {
        for (label, attr) in [("by u/", "data-author"), ("", "data-score")] {
            if let Some(c) = re(&format!(r#"{attr}="([^"]+)""#)).captures(link_tag.as_str()) {
                meta.push(if attr == "data-score" {
                    format!("{} pts", &c[1])
                } else {
                    format!("{label}{}", &c[1])
                });
            }
        }
    }
    // Comment count from the submission's own `comments` anchor — a bare
    // `N comments <` scan can hit sidebar/related links first.
    if let Some(c) =
        re(r#"(?is)<a[^>]*class="[^"]*\bcomments\b[^"]*"[^>]*>\s*(\d[\d,]*)\s+comments"#)
            .captures(html)
    {
        meta.push(format!("{} comments", &c[1]));
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {}\n", meta.join(" • ")));
    }

    let comment_open = re(r#"(?s)<div[^>]*?data-type="comment"[^>]*?>"#);
    let starts: Vec<usize> = comment_open.find_iter(html).map(|m| m.start()).collect();
    let md_div = re(r#"(?is)<div class="md"[^>]*>(.*?)</div>"#);

    // OP selftext: the `.md` body inside the submission's `expando`, before
    // the first comment (absent on link posts). Scoped to the expando so the
    // sidebar's rules/description `.md` blocks can't masquerade as the OP.
    // `class="expando"` was an EXACT match until old.reddit started shipping
    // `class="expando expando-uninitialized"`, at which point the OP selftext
    // stopped being captured on every self-post. Threads with comments hid it
    // — the comments alone carry the render past the thin-parse floor — so it
    // only surfaced as "reader broke" on a thread with none, where title+meta
    // is under the floor and the whole reader fails open. Match the class as a
    // TOKEN in the list, the way the comments-anchor regex above already does.
    let head_end = starts.first().copied().unwrap_or(html.len());
    let expando_md =
        re(r#"(?is)<div class="[^"]*\bexpando\b[^"]*"[^>]*>.*?<div class="md"[^>]*>(.*?)</div>"#);
    if let Some(c) = expando_md.captures(&html[..head_end]) {
        let op = crate::web::fragment_to_markdown(c.get(1).unwrap().as_str());
        if !op.is_empty() {
            out.push_str(&format!("\n{op}\n"));
        }
    }

    if !starts.is_empty() {
        out.push_str("\n## Comments\n\n");
    }
    let author_re = re(r#"data-author="([^"]*)""#);
    let score_re = re(r#"(?is)<span class="score[^"]*"[^>]*title="([^"]+)""#);
    let mut shown = 0usize;
    let mut dropped = 0usize;
    let mut comments_len = 0usize;
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(html.len());
        let slice = &html[start..end];
        let Some(body_c) = md_div.captures(slice) else {
            continue; // deleted/collapsed stub
        };
        let body = crate::web::fragment_to_markdown(body_c.get(1).unwrap().as_str());
        if body.is_empty() {
            continue;
        }
        if comments_len > REDDIT_COMMENTS_CAP_CHARS {
            dropped += 1;
            continue;
        }
        let author = author_re
            .captures(slice)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| "unknown".into());
        let score = score_re
            .captures(slice)
            .map(|c| format!(" ({} pts)", &c[1]))
            .unwrap_or_default();
        let entry = format!("**u/{author}**{score}: {body}\n\n");
        comments_len += entry.chars().count();
        out.push_str(&entry);
        shown += 1;
    }
    if dropped > 0 {
        out.push_str(&format!(
            "({dropped} more comment(s) beyond the size cap not shown; {shown} shown)\n"
        ));
    }
    Some(out.trim_end().to_string())
}

/// One attribute of an old.reddit `<div class="thing">` opening tag. The name
/// is matched with its `="` so `data-author` cannot capture
/// `data-author-fullname`, and `data-subreddit` cannot capture
/// `data-subreddit-prefixed`.
pub(super) fn thing_attr(tag: &str, name: &str) -> Option<String> {
    let v = re(&format!(r#"{name}="([^"]*)""#))
        .captures(tag)?
        .get(1)?
        .as_str();
    let v = crate::web::decode_entities(v.trim());
    (!v.is_empty()).then_some(v)
}

/// Render an old.reddit LISTING: subreddit front page, sorted view, user page
/// or search results.
///
/// Three different markups sit behind that one word. Submissions and comments
/// are `thing` divs carrying `data-*` attributes; search results are their own
/// `search-result` blocks with no attributes at all; and a user's overview page
/// mixes the first two, so both are collected rather than the first that hits.
pub(super) fn reddit_listing(html: &str) -> Option<String> {
    if is_a_feed(html) {
        return super::feed::feed_document(html);
    }
    let heading = re(r"(?is)<title[^>]*>(.*?)</title>")
        .captures(html)
        .map(|c| crate::web::decode_entities(c[1].trim()))
        .filter(|t| !t.is_empty())
        .or_else(|| crate::web::meta_content(html, "og:title"))
        .unwrap_or_else(|| "reddit listing".to_string());

    // A user's overview page interleaves both, so neither may shadow the
    // other: rendering submissions alone dropped 21 of the 25 things on
    // /user/spez.
    let mut sections: Vec<(&str, Vec<String>)> = Vec::new();
    for (label, entries) in [
        ("submissions", reddit_submission_entries(html)),
        ("comments", reddit_comment_entries(html)),
    ] {
        if !entries.is_empty() {
            sections.push((label, entries));
        }
    }
    if sections.is_empty() {
        // Search is the third shape: old.reddit renders it with its own
        // `search-result` markup and no `thing` at all.
        let hits = reddit_search_entries(html);
        if hits.is_empty() {
            // Nothing we recognise: an interstitial, a private subreddit or a
            // layout drift. Fail open rather than serve a header as a listing.
            return None;
        }
        sections.push(("results", hits));
    }

    let counts: Vec<String> = sections
        .iter()
        .map(|(l, e)| format!("{} {l}", e.len()))
        .collect();
    let mut out = format!("# {heading}\n> {} — old.reddit.com\n", counts.join(" • "));
    let mut len = 0usize;
    let mut dropped = 0usize;
    for (_, entries) in &sections {
        for e in entries {
            if len > REDDIT_LISTING_CAP_CHARS {
                dropped += 1;
                continue;
            }
            len += e.chars().count();
            out.push_str(e);
        }
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n({dropped} further entr(y/ies) beyond the size cap not shown)\n"
        ));
    }
    // The listing's own paging link, so an agent can walk further without
    // having to reverse-engineer reddit's cursor scheme.
    if let Some(c) = re(r#"(?is)<a[^>]+href="([^"]+)"[^>]*rel="[^"]*\bnext\b[^"]*""#).captures(html)
    {
        let next = crate::web::decode_entities(c.get(1).map_or("", |m| m.as_str()));
        if next.starts_with("http") {
            out.push_str(&format!("\n[next page: {next}]\n"));
        }
    }
    Some(out.trim_end().to_string())
}

/// Bounds of each `thing` of `data_type`, as `(open-tag start, open-tag end,
/// slice end)`. The slice runs to the next thing of the SAME type, which is
/// how old.reddit's flat listing markup delimits them.
pub(super) fn reddit_things(html: &str, data_type: &str) -> Vec<(usize, usize, usize)> {
    let open = re(&format!(
        r#"(?s)<div[^>]*?\sdata-type="{data_type}"[^>]*?>"#
    ));
    let found: Vec<(usize, usize)> = open.find_iter(html).map(|m| (m.start(), m.end())).collect();
    found
        .iter()
        .enumerate()
        .map(|(i, &(s, e))| (s, e, found.get(i + 1).map(|n| n.0).unwrap_or(html.len())))
        .collect()
}

/// One entry per submission on a subreddit front page, sorted view or user
/// page: the title plus the facts a listing exists to convey — who posted it,
/// how it scored, how much discussion it drew, and where it points.
pub(super) fn reddit_submission_entries(html: &str) -> Vec<String> {
    let title_a = re(r#"(?is)<a[^>]*\bclass="[^"]*\btitle\b[^"]*"[^>]*>(.*?)</a>"#);
    let mut entries = Vec::new();
    for (start, tag_end, end) in reddit_things(html, "link") {
        let tag = &html[start..tag_end];
        // Promoted submissions are advertising, not the listing the agent
        // asked for; old.reddit flags them on the thing itself.
        if thing_attr(tag, "data-promoted").as_deref() == Some("true") {
            continue;
        }
        let slice = &html[start..end];
        let Some(title) = title_a
            .captures(slice)
            .map(|c| crate::web::fragment_to_markdown(c.get(1).map_or("", |m| m.as_str())))
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let author = thing_attr(tag, "data-author");
        let mut facts: Vec<String> = Vec::new();
        // A post on someone's own profile is filed under the pseudo-subreddit
        // `u/<name>`, which would print the author's name a second time.
        facts.extend(
            thing_attr(tag, "data-subreddit-prefixed")
                .filter(|s| Some(s.trim_start_matches("u/")) != author.as_deref()),
        );
        if let Some(a) = &author {
            facts.push(format!("u/{a}"));
        }
        if let Some(v) = thing_attr(tag, "data-score") {
            facts.push(format!("{v} pts"));
        }
        if let Some(v) = thing_attr(tag, "data-comments-count") {
            facts.push(format!("{v} comments"));
        }
        // `self.<sub>` is reddit's own marker for a text post; printing it as
        // a "domain" would read as an outbound link that does not exist.
        let self_post = thing_attr(tag, "data-domain")
            .map(|d| {
                let is_self = d.starts_with("self.");
                if !is_self {
                    facts.push(d);
                }
                is_self
            })
            .unwrap_or(false);
        let mut entry = format!("\n## {title}\n{}\n", facts.join(" • "));
        if let Some(p) = thing_attr(tag, "data-permalink") {
            entry.push_str(&format!("https://www.reddit.com{p}\n"));
        }
        if !self_post {
            if let Some(u) = thing_attr(tag, "data-url").filter(|u| u.starts_with("http")) {
                entry.push_str(&format!("{u}\n"));
            }
        }
        entries.push(entry);
    }
    entries
}

/// One entry per reply on `/user/<name>/comments/` (and the comment half of an
/// overview page). What identifies a reply is the thread it was posted IN,
/// which old.reddit renders as a `<p class="parent">` line above the body —
/// without it the page is a wall of unattributed paragraphs.
pub(super) fn reddit_comment_entries(html: &str) -> Vec<String> {
    let md_div = re(r#"(?is)<div class="md"[^>]*>(.*?)</div>"#);
    // Anchored on the parent paragraph so the comment's own body links, which
    // also carry classes, cannot be mistaken for the thread title.
    let parent_title =
        re(r#"(?is)<p class="parent">.*?<a[^>]*\bclass="[^"]*\btitle\b[^"]*"[^>]*>(.*?)</a>"#);
    let score_re = re(r#"(?is)<span class="score[^"]*"[^>]*title="([^"]+)""#);
    let mut entries = Vec::new();
    for (start, tag_end, end) in reddit_things(html, "comment") {
        let tag = &html[start..tag_end];
        let slice = &html[start..end];
        let Some(body) = md_div
            .captures(slice)
            .map(|c| crate::web::fragment_to_markdown(c.get(1).map_or("", |m| m.as_str())))
            .filter(|b| !b.trim().is_empty())
        else {
            continue; // deleted/removed stub
        };
        let thread = parent_title
            .captures(slice)
            .map(|c| crate::web::fragment_to_markdown(c.get(1).map_or("", |m| m.as_str())))
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "(thread unknown)".into());
        let author = thing_attr(tag, "data-author");
        let mut facts: Vec<String> = Vec::new();
        if let Some(a) = &author {
            facts.push(format!("u/{a}"));
        }
        if let Some(s) = score_re.captures(slice) {
            facts.push(format!("{} pts", &s[1]));
        }
        facts.extend(
            thing_attr(tag, "data-subreddit-prefixed")
                .filter(|s| Some(s.trim_start_matches("u/")) != author.as_deref()),
        );
        let mut entry = format!(
            "\n## in: {thread}\n{}\n\n{}\n",
            facts.join(" • "),
            body.trim()
        );
        if let Some(p) = thing_attr(tag, "data-permalink") {
            entry.push_str(&format!("https://www.reddit.com{p}\n"));
        }
        entries.push(entry);
    }
    entries
}

/// One entry per hit on `/search` and `/r/<sub>/search`.
///
/// Search is the one listing old.reddit does NOT build out of `thing` divs —
/// it has its own `search-result` markup with no `data-*` attributes at all,
/// so the submission parser found nothing and the whole reader failed open on
/// every search URL it claimed.
pub(super) fn reddit_search_entries(html: &str) -> Vec<String> {
    // The class token must END here: `\b` also matched `search-result-meta`
    // and `search-result-body`, the hit's own CHILDREN, so every entry was cut
    // short before its score, author and date.
    let open = re(r#"(?is)<div[^>]*\bclass="[^"]*\bsearch-result[ "][^>]*>"#);
    let found: Vec<(usize, usize)> = open.find_iter(html).map(|m| (m.start(), m.end())).collect();
    let title_a = re(r#"(?is)<a[^>]+class="[^"]*\bsearch-title\b[^"]*"[^>]*>(.*?)</a>"#);
    let href_a = re(r#"(?is)<a[^>]+href="([^"]+)"[^>]*class="[^"]*\bsearch-title\b[^"]*""#);
    let score = re(r#"(?is)<span class="search-score"[^>]*>(.*?)</span>"#);
    let comments = re(r#"(?is)<a[^>]+class="[^"]*\bsearch-comments\b[^"]*"[^>]*>(.*?)</a>"#);
    let author = re(r#"(?is)<span class="search-author">.*?<a[^>]*>(.*?)</a>"#);
    let sub = re(r#"(?is)<a[^>]+class="[^"]*\bsearch-subreddit-link\b[^"]*"[^>]*>(.*?)</a>"#);
    let time = re(r#"(?is)<time[^>]+datetime="([^"T]+)"#);

    let mut entries = Vec::new();
    for (i, &(start, _)) in found.iter().enumerate() {
        let slice = &html[start..found.get(i + 1).map(|n| n.0).unwrap_or(html.len())];
        let text = |r: &Regex| -> Option<String> {
            let t = crate::web::fragment_to_markdown(r.captures(slice)?.get(1)?.as_str());
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            (!t.is_empty()).then_some(t)
        };
        let Some(title) = text(title_a) else { continue };
        let facts: Vec<String> = [
            text(sub),
            text(author).map(|a| format!("u/{a}")),
            text(score),
            text(comments),
            text(time),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut entry = format!("\n## {title}\n{}\n", facts.join(" • "));
        if let Some(c) = href_a.captures(slice) {
            entry.push_str(&format!(
                "{}\n",
                crate::web::decode_entities(c.get(1).map_or("", |m| m.as_str()))
            ));
        }
        entries.push(entry);
    }
    entries
}

/// **`old.reddit.com` no longer serves anyone without an account.** MEASURED
/// 2026-09-11, anonymously: `/r/rust/`, `/r/rust/wiki/index` and a thread all
/// answer **302 to `/login/?reason=lor2`**, and `www.reddit.com` serves
/// instead the 8 KB "Please wait for verification" interstitial — from which
/// the delivered page extracted **0 characters**. The public `.json`
/// endpoints answer 403.
///
/// **The Atom feed, however, answers 200 without an account**: `/r/rust/.rss`
/// returns 49 KB, 25 entries with author, date, link and the post's full
/// body. It is the only anonymous source left, and the repository's feed
/// reader already renders it.
///
/// The `.rss` slips in BEFORE the query: `/r/rust/top/.rss?t=week` is what
/// reddit serves, `/r/rust/top/?t=week/.rss` does not exist.
fn atom_url(path: &str, keep_query: bool) -> String {
    let no_fragment = path.split('#').next().unwrap_or(path);
    let (clean, query) = match no_fragment.split_once('?') {
        Some((c, q)) if keep_query => (c, format!("?{q}")),
        Some((c, _)) => (c, String::new()),
        None => (no_fragment, String::new()),
    };
    let base = clean.trim_end_matches('/');
    format!("https://www.reddit.com{base}/.rss{query}")
}

pub(super) fn reddit_thread_plan(path: &str) -> Option<Plan> {
    // Strip query/fragment: sort params change the layout, and the
    // canonical thread is what the agent wants.
    Some(Plan::get(
        atom_url(path, false),
        Kind::RedditThread,
        "reddit .rss",
    ))
}

pub(super) fn reddit_listing_plan(path: &str) -> Option<Plan> {
    // The query IS the request on a listing (`?t=week`, `?q=…`), so it stays.
    Some(Plan::get(
        atom_url(path, true),
        Kind::RedditListing,
        "reddit .rss",
    ))
}

/// Is this body the Atom feed both plans now request?
///
/// Both readers keep their `old.reddit` HTML path: it is dead for an anonymous
/// client, it is not for whoever serves that page another way (a mirror, a
/// session). The SHAPE of the body decides, not a guess about who served it.
pub(super) fn is_a_feed(body: &str) -> bool {
    let head: String = body
        .chars()
        .take(600)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("<feed") || head.contains("<rss") || head.contains("<rdf:rdf")
}

/// **A THREAD's feed carries its comments**, and it took a measurement to
/// know it: the first version of this reader added a line saying the
/// opposite — "reddit serves comments to no anonymous client" — because a
/// `grep -c "<entry>"` had answered 1. The feed fits on ONE line, so
/// `grep -c` was counting lines, not entries. Properly measured 2026-09-11 on
/// `/r/rust/comments/1wc8sbm/…/.rss`: **5 entries**, the submission then four
/// comments, each titled `/u/<author> on <thread>`.
///
/// A false sentence in the delivered page is worse than no sentence at all:
/// an agent would have stopped asking for threads. It has been removed, and
/// this comment is what remains in its place.
const _THE_FEED_CARRIES_COMMENTS: () = ();

/// Comment sections are capped so a 2000-comment megathread doesn't blow the
/// inline budget; the cut is annotated with how many comments were dropped.
pub(crate) const REDDIT_COMMENTS_CAP_CHARS: usize = 24_000;

/// Listings are capped like threads are: `/r/all` with `?limit=100` would
/// otherwise hand back a hundred submissions nobody asked for.
pub(crate) const REDDIT_LISTING_CAP_CHARS: usize = 20_000;
